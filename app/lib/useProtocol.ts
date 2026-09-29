"use client";

import { useCallback, useEffect, useState } from "react";
import { useAnchorWallet, useConnection, useWallet } from "@solana/wallet-adapter-react";
import { PublicKey, SystemProgram } from "@solana/web3.js";
import { BN } from "@coral-xyz/anchor";
import { fetchProgramAccounts, noteConfirmedSlot } from "./accounts";
import { u64 } from "./amounts";
import {
  AgentAccount,
  PositionAccount,
  ProtocolConfig,
  agentPda,
  agentVaultPda,
  isPublished,
  positionPda,
  positionVaultPda,
  walletProgram,
} from "./program";

/** Realised results for traders on one agent, from its closed positions. */
export type AgentStats = {
  closed: number;
  principal: number;
  /** What traders got back minus what they put in, in lamports, after fees and slashing. */
  traderPnl: number;
};

export function useAgents() {
  const [agents, setAgents] = useState<AgentAccount[]>([]);
  const [stats, setStats] = useState<Record<string, AgentStats>>({});
  const [config, setConfig] = useState<ProtocolConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async (fresh = false) => {
    setLoading(true);
    try {
      const snap = await fetchProgramAccounts(fresh === true);
      const list = snap.agents.filter(isPublished);
      list.sort((a, b) => b.totalCollateral.cmp(a.totalCollateral));
      const next: Record<string, AgentStats> = {};
      for (const p of snap.positions) {
        if (p.status !== "settled" && p.status !== "defaulted") continue;
        const k = p.agent.toBase58();
        const s = (next[k] ??= { closed: 0, principal: 0, traderPnl: 0 });
        const principal = p.principal.toNumber();
        const payout = p.returned.toNumber() - p.feePaid.toNumber() + p.slashed.toNumber();
        s.closed += 1;
        s.principal += principal;
        s.traderPnl += payout - principal;
      }
      setAgents(list);
      setStats(next);
      setConfig(snap.config);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);
  return { agents, stats, config, loading, error, refresh: () => refresh(true) };
}

/** Positions belonging to a trader or to an agent. */
export function usePositions(filter: { trader?: PublicKey; agent?: PublicKey } | null) {
  const [positions, setPositions] = useState<PositionAccount[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const key = filter?.trader?.toBase58() ?? filter?.agent?.toBase58() ?? null;
  const kind = filter?.trader ? "trader" : "agent";

  const load = useCallback(
    async (fresh: boolean) => {
      if (!key) {
        setPositions([]);
        setError(null);
        return;
      }
      setLoading(true);
      try {
        const snap = await fetchProgramAccounts(fresh);
        const list = snap.positions.filter((p) => (kind === "trader" ? p.trader : p.agent).toBase58() === key);
        list.sort((a, b) => b.openedAt.cmp(a.openedAt));
        setPositions(list);
        setError(null);
      } catch (e) {
        setError((e as Error).message || "Could not load positions");
      } finally {
        setLoading(false);
      }
    },
    [key, kind],
  );

  useEffect(() => {
    load(false);
  }, [load]);
  return { positions, loading, error, refresh: () => load(true) };
}

export type TxState =
  | { kind: "idle" }
  | { kind: "pending"; label: string }
  | { kind: "ok"; sig: string; label: string }
  | { kind: "err"; message: string };

/** Every write instruction the UI needs, bound to the connected wallet. */
export function useActions() {
  const { connection } = useConnection();
  const wallet = useAnchorWallet();
  const { publicKey } = useWallet();
  const [tx, setTx] = useState<TxState>({ kind: "idle" });

  const run = useCallback(
    async (label: string, fn: () => Promise<string>) => {
      setTx({ kind: "pending", label });
      try {
        const sig = await fn();
        // The slot it confirmed in, so the refresh that follows doesn't show a snapshot from before it.
        const status = await connection.getSignatureStatus(sig).catch(() => null);
        if (status?.value?.slot) noteConfirmedSlot(status.value.slot);
        setTx({ kind: "ok", sig, label });
        return sig;
      } catch (e) {
        const msg = parseAnchorError(e);
        setTx({ kind: "err", message: msg });
        throw e;
      }
    },
    [connection],
  );

  const need = () => {
    if (!wallet || !publicKey) throw new Error("Connect a wallet first");
    return { program: walletProgram(connection, wallet), me: publicKey };
  };

  return {
    tx,
    reset: () => setTx({ kind: "idle" }),
    connected: !!publicKey,

    // ---- trader ----
    openPosition: (agent: AgentAccount, lamports: number, durationSecs: number) =>
      run("Open position", async () => {
        const { program, me } = need();
        const nonce = new BN(Date.now());
        const position = positionPda(agent.publicKey, me, nonce);
        return program.methods
          .openPosition(nonce, u64(lamports), new BN(durationSecs))
          .accounts({
            trader: me,
            agent: agent.publicKey,
            position,
            positionVault: positionVaultPda(position),
            systemProgram: SystemProgram.programId,
          })
          .rpc();
      }),
    cancelPosition: (p: PositionAccount) =>
      run("Cancel position", async () => {
        const { program, me } = need();
        return program.methods
          .cancelPosition()
          .accounts({
            trader: me,
            agent: p.agent,
            position: p.publicKey,
            positionVault: positionVaultPda(p.publicKey),
            systemProgram: SystemProgram.programId,
          })
          .rpc();
      }),
    claimDefault: (p: PositionAccount) =>
      run("Claim collateral", async () => {
        const { program, me } = need();
        return program.methods
          .claimDefault()
          .accounts({
            trader: me,
            agent: p.agent,
            agentVault: agentVaultPda(p.agent),
            position: p.publicKey,
            positionVault: positionVaultPda(p.publicKey),
            systemProgram: SystemProgram.programId,
          })
          .rpc();
      }),

    // ---- operator ----
    createAgent: (agentId: BN, name: string, description: string, terms: TermsInput) =>
      run("Create draft agent", async () => {
        const { program, me } = need();
        const agent = agentPda(me, agentId);
        return program.methods
          .createAgent(agentId, name, description, toTermsArg(terms))
          .accounts({ operator: me, agent, agentVault: agentVaultPda(agent), systemProgram: SystemProgram.programId })
          .rpc();
      }),
    updateAgent: (agent: AgentAccount, name: string, description: string, terms: TermsInput) =>
      run("Save draft", async () => {
        const { program, me } = need();
        return program.methods
          .updateAgent(name, description, toTermsArg(terms))
          .accounts({ operator: me, agent: agent.publicKey })
          .rpc();
      }),
    publishAgent: (agent: AgentAccount) =>
      run("Publish agent", async () => {
        const { program, me } = need();
        return program.methods.publishAgent().accounts({ operator: me, agent: agent.publicKey }).rpc();
      }),
    setExecutor: (agent: AgentAccount, executor: PublicKey) =>
      run("Bind trading key", async () => {
        const { program, me } = need();
        return program.methods.setExecutor(executor).accounts({ operator: me, agent: agent.publicKey }).rpc();
      }),
    setAccepting: (agent: AgentAccount, accepting: boolean) =>
      run(accepting ? "Resume agent" : "Pause agent", async () => {
        const { program, me } = need();
        return program.methods.setAccepting(accepting).accounts({ operator: me, agent: agent.publicKey }).rpc();
      }),
    depositCollateral: (agent: AgentAccount, lamports: number) =>
      run("Deposit collateral", async () => {
        const { program, me } = need();
        return program.methods
          .depositCollateral(u64(lamports))
          .accounts({
            operator: me,
            agent: agent.publicKey,
            agentVault: agentVaultPda(agent.publicKey),
            systemProgram: SystemProgram.programId,
          })
          .rpc();
      }),
    withdrawCollateral: (agent: AgentAccount, lamports: number) =>
      run("Withdraw collateral", async () => {
        const { program, me } = need();
        return program.methods
          .withdrawCollateral(u64(lamports))
          .accounts({
            operator: me,
            agent: agent.publicKey,
            agentVault: agentVaultPda(agent.publicKey),
            systemProgram: SystemProgram.programId,
          })
          .rpc();
      }),

    // ---- trading key (operator or bound executor) ----
    drawFunds: (p: PositionAccount) =>
      run("Draw funds", async () => {
        const { program, me } = need();
        return program.methods
          .drawFunds()
          .accounts({
            executor: me,
            agent: p.agent,
            position: p.publicKey,
            positionVault: positionVaultPda(p.publicKey),
            systemProgram: SystemProgram.programId,
          })
          .rpc();
      }),
    settlePosition: (agent: AgentAccount, p: PositionAccount, returnedLamports: number) =>
      run("Settle position", async () => {
        const { program, me } = need();
        return program.methods
          .settlePosition(u64(returnedLamports))
          .accounts({
            executor: me,
            operator: agent.operator,
            agent: p.agent,
            agentVault: agentVaultPda(p.agent),
            position: p.publicKey,
            positionVault: positionVaultPda(p.publicKey),
            trader: p.trader,
            systemProgram: SystemProgram.programId,
          })
          .rpc();
      }),
  };
}

/** Form-friendly terms: numbers and mint strings. */
export type TermsInput = {
  collateralRatioBps: number;
  feeBps: number;
  maxDrawdownBps: number;
  minDurationSecs: number;
  maxDurationSecs: number;
  allowedAssets: string[];
  rules: string;
};

function toTermsArg(t: TermsInput) {
  return {
    collateralRatioBps: t.collateralRatioBps,
    feeBps: t.feeBps,
    maxDrawdownBps: t.maxDrawdownBps,
    minDurationSecs: new BN(t.minDurationSecs),
    maxDurationSecs: new BN(t.maxDurationSecs),
    allowedAssets: t.allowedAssets.map((m) => new PublicKey(m)),
    rules: t.rules,
  };
}

export function termsToInput(t: AgentAccount["terms"]): TermsInput {
  return {
    collateralRatioBps: t.collateralRatioBps,
    feeBps: t.feeBps,
    maxDrawdownBps: t.maxDrawdownBps,
    minDurationSecs: t.minDurationSecs.toNumber(),
    maxDurationSecs: t.maxDurationSecs.toNumber(),
    allowedAssets: t.allowedAssets.map((m) => m.toBase58()),
    rules: t.rules,
  };
}

function parseAnchorError(e: unknown): string {
  const err = e as { error?: { errorMessage?: string }; message?: string; logs?: string[] };
  if (err?.error?.errorMessage) return err.error.errorMessage;
  const m = err?.message ?? String(e);
  if (m.includes("User rejected")) return "Transaction rejected in wallet.";
  return m.length > 240 ? m.slice(0, 240) + "…" : m;
}

/** Every agent (drafts included) operated by the given wallet, plus their positions. */
export function useOperatorAgents(operator: PublicKey | null) {
  const [agents, setAgents] = useState<AgentAccount[]>([]);
  const [positions, setPositions] = useState<PositionAccount[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  /** Which operator the current data belongs to; null until the first load finishes. */
  const [loadedFor, setLoadedFor] = useState<string | null>(null);
  const key = operator?.toBase58() ?? null;

  const load = useCallback(
    async (fresh: boolean) => {
      if (!key) {
        setAgents([]);
        setPositions([]);
        setError(null);
        setLoading(false);
        return;
      }
      setLoading(true);
      try {
        const snap = await fetchProgramAccounts(fresh);
        const mine = snap.agents.filter((a) => a.operator.toBase58() === key);
        mine.sort((a, b) => b.createdAt.cmp(a.createdAt));
        const ids = new Set(mine.map((a) => a.publicKey.toBase58()));
        setAgents(mine);
        setPositions(snap.positions.filter((p) => ids.has(p.agent.toBase58())));
        setLoadedFor(key);
        setError(null);
      } catch (e) {
        setError((e as Error).message || "Could not load agents");
      } finally {
        setLoading(false);
      }
    },
    [key],
  );

  useEffect(() => {
    load(false);
  }, [load]);
  return { agents, positions, loading, error, loaded: key !== null && loadedFor === key, refresh: () => load(true) };
}

/** SOL balance of a wallet, refreshed on demand and after each confirmed transaction. */
export const BALANCE_EVENT = "poa:balance-changed";

export function useBalance(owner: PublicKey | null, tx?: TxState) {
  const { connection } = useConnection();
  const [lamports, setLamports] = useState<number | null>(null);
  const [tick, setTick] = useState(0);
  const key = owner?.toBase58() ?? null;
  const okSig = tx?.kind === "ok" ? tx.sig : null;
  // Refresh when another part of the app changes the balance (e.g. the local faucet) or the tab regains focus.
  useEffect(() => {
    const bump = () => setTick((t) => t + 1);
    window.addEventListener(BALANCE_EVENT, bump);
    window.addEventListener("focus", bump);
    return () => {
      window.removeEventListener(BALANCE_EVENT, bump);
      window.removeEventListener("focus", bump);
    };
  }, []);
  useEffect(() => {
    if (!key) {
      setLamports(null);
      return;
    }
    let cancelled = false;
    connection
      .getBalance(new PublicKey(key), "confirmed")
      .then((n) => !cancelled && setLamports(n))
      .catch(() => !cancelled && setLamports(null));
    return () => {
      cancelled = true;
    };
  }, [connection, key, okSig, tick]);
  return lamports;
}
