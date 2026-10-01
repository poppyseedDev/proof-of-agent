import { join } from "node:path";
import { Keypair, PublicKey } from "@solana/web3.js";
import type { Agent, Position } from "../src/client.js";
import { Runner, type RunnerOptions } from "../src/runner.js";
import { DEAD_RPC, tempDir } from "./helpers.js";

export const TOKEN_PROGRAM = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
export const TOKEN_2022_PROGRAM = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";

export type TokenAccount = { mint: string; amount: string; ui: string };

/** A stand-in for the chain: the runner's client is swapped for this, so nothing leaves the process. */
export class FakeChain {
  positions: Position[] = [];
  balance = 5_000_000_000;
  tokens: Record<string, TokenAccount[]> = {};
  tokenError: Error | null = null;
  drawn: PublicKey[] = [];
  settled: { position: PublicKey; returned: bigint; microLamports: number }[] = [];
  /** The agent account the runner re-reads; set by makeRunner. */
  agentAcc: Agent | null = null;
  agentReads = 0;
  /** When set, settle throws this (as an on-chain rejection would). */
  settleError: Error | null = null;
  /** Slot the fake settle reports it landed in. */
  settleSlot = 1000;
  /** Cluster time getBlockTime reports; null follows the local clock. */
  blockTime: number | null = null;
  blockTimeError: Error | null = null;
  blockTimeReads = 0;
  /** The commitment or config of every balance read. */
  balanceConfigs: unknown[] = [];
  tokenLookups = 0;
  /** Rent-exempt minimum for a 0-byte account, as mainnet reports it. */
  rentExempt = 890_880;
  /** When set, the program-account scan throws this. */
  scanError: Error | null = null;
  /** Position keys (base58) the direct fetch reports as missing, as a lagging node would. */
  missing = new Set<string>();
  /** Status the direct fetch reports instead of the real one, as a lagging node would. */
  lagStatus: Position["status"] | null = null;
  scans = 0;
  /** Called on every balance read, before it returns. */
  onBalance: (() => void) | null = null;

  client() {
    return {
      positions: async () => {
        this.scans++;
        if (this.scanError) throw this.scanError;
        return this.positions;
      },
      positionNullable: async (key: PublicKey) => {
        if (this.missing.has(key.toBase58())) return null;
        const p = this.positions.find((x) => x.publicKey.equals(key));
        return p && this.lagStatus ? { ...p, status: this.lagStatus } : p ?? null;
      },
      beginTrading: async (_agent: PublicKey, position: PublicKey) => {
        this.drawn.push(position);
        const p = this.positions.find((x) => x.publicKey.equals(position));
        if (p) p.status = "trading";
        return "drawsig1111111111111";
      },
      agent: async (_key: PublicKey) => {
        this.agentReads++;
        return this.agentAcc;
      },
      settle: async (_a: Agent, p: Position, returned: bigint, fee: { microLamports: number | (() => number) }) => {
        if (this.settleError) throw this.settleError;
        const microLamports = typeof fee.microLamports === "function" ? fee.microLamports() : fee.microLamports;
        this.settled.push({ position: p.publicKey, returned: BigInt(returned), microLamports });
        return { sig: "settlesig11111111111", slot: this.settleSlot };
      },
      connection: {
        getSlot: async () => 999,
        getBlockTime: async (_slot: number) => {
          this.blockTimeReads++;
          if (this.blockTimeError) throw this.blockTimeError;
          return this.blockTime ?? Math.floor(Date.now() / 1000);
        },
        getMinimumBalanceForRentExemption: async (_bytes: number) => this.rentExempt,
        getBalance: async (_key: PublicKey, config?: unknown) => {
          this.balanceConfigs.push(config);
          this.onBalance?.();
          return this.balance;
        },
        getParsedTokenAccountsByOwner: async (_owner: PublicKey, { programId }: { programId: PublicKey }) => {
          this.tokenLookups++;
          if (this.tokenError) throw this.tokenError;
          return {
            value: (this.tokens[programId.toBase58()] ?? []).map((a) => ({
              account: { data: { parsed: { info: { mint: a.mint, tokenAmount: { amount: a.amount, uiAmountString: a.ui } } } } },
            })),
          };
        },
      },
    };
  }
}

/** The runner's private members, for driving it one step at a time. */
export type RunnerInternals = {
  tick(): Promise<void>;
  settle(p: Position, book: Book): Promise<void>;
  startHook(book: Book): void;
  stopHook(deadline: number): Promise<void>;
  stop(): void;
  hook: { pid?: number } | null;
  hookExited: boolean;
  skew: number;
  agentAt: number;
  paperBook: Book | null;
  state: { active: Book | null; history: { position: string; principal: string; returned: string; at: number }[]; hookPgid?: number | null; minSlot?: number };
};
export type Book = { position: string; principal: string; balanceAtDraw: string; drawnAt: number; settleAt: number; deadline: number };

export function makeRunner(t: { after: (fn: () => void) => void }, opts: Partial<RunnerOptions> = {}) {
  const dir = tempDir(t);
  const logs: string[] = [];
  const tradingKey = Keypair.generate();
  const agentKey = PublicKey.unique();
  const chain = new FakeChain();
  const runner = new Runner({
    rpcUrl: DEAD_RPC, agent: agentKey, tradingKey, holdSecs: 600, bufferSecs: 60, graceSecs: 5, paper: false,
    pollMs: 15_000, stateFile: join(dir, "state", "state.json"), log: (m) => logs.push(m), ...opts,
  });
  const agent = { publicKey: agentKey, operator: PublicKey.unique(), executor: tradingKey.publicKey } as Agent;
  chain.agentAcc = agent;
  Object.assign(runner, { client: chain.client(), agentAcc: agent });
  const r = runner as unknown as RunnerInternals;
  // never leave a hook running if a test fails half way
  t.after(() => {
    for (const pgid of [r.hook?.pid, r.state.hookPgid]) if (pgid) try { process.kill(-pgid, "SIGKILL"); } catch { /* gone */ }
  });
  return { runner, r, chain, logs, dir, tradingKey, agent };
}

export function book(p: Position, fields: Partial<Book> = {}): Book {
  const t = Math.floor(Date.now() / 1000);
  return {
    position: p.publicKey.toBase58(), principal: p.principal.toString(), balanceAtDraw: "5000000000",
    drawnAt: t, settleAt: t + 600, deadline: t + 3600, ...fields,
  };
}
