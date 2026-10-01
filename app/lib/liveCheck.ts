/**
 * Checks that instructions built from this build's IDL work against the program
 * that is deployed on the cluster right now.
 *
 * It simulates a whole agent and position lifecycle in one transaction, paid by an
 * account that already holds SOL. Nothing is signed or sent, so it needs no key and
 * changes nothing on-chain. Instructions are built straight from the IDL (account
 * order, flags, argument encoding), which is exactly what the site, the runner and
 * the SDK put on the wire.
 *
 * Used by the deploy scripts (before and after a deploy), by /api/health, and by
 * the monitors that read it.
 */
import { createHash } from "node:crypto";
import { BN, BorshCoder, type Idl } from "@coral-xyz/anchor";
import {
  Connection,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import idl from "./idl.json";
import deployedIdl from "./deployed/devnet.json";

type IdlAccount = { name: string; writable?: boolean; signer?: boolean; address?: string };
type IdlIx = { name: string; accounts: IdlAccount[] };
type IdlError = { code: number; name: string; msg?: string };
export type IdlLike = { address: string; instructions: IdlIx[]; errors?: IdlError[] };

export type LiveCheck = {
  /**
   * ok: every instruction works. blocked: the wiring is right, but the program refused
   * on purpose (paused, a cap). broken: clients built from this IDL cannot talk to the
   * deployed program. unknown: the check could not run (RPC down, payer has no SOL).
   */
  status: "ok" | "blocked" | "broken" | "unknown";
  /** One sentence for a person. */
  reason: string;
  /** The instruction that failed, if any. */
  failedAt?: string;
  idlHash: string;
  checked: string[];
};

/** Short fingerprint of an IDL, to tell which program interface a client was built with. */
export function idlHash(i: unknown = idl): string {
  return createHash("sha256").update(JSON.stringify(i)).digest("hex").slice(0, 12);
}

const SOL_MINT = new PublicKey("So11111111111111111111111111111111111111112");
const TOKEN_PROGRAM = new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const ATA_PROGRAM = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
/** The associated token account of `owner` for `mint` under the classic token program. */
export const ataOf = (owner: PublicKey, mint: PublicKey) =>
  PublicKey.findProgramAddressSync([owner.toBuffer(), TOKEN_PROGRAM.toBuffer(), mint.toBuffer()], ATA_PROGRAM)[0];
const BOND = 20_000_000; // 0.02 SOL
const PRINCIPAL = 10_000_000; // 0.01 SOL
/** Enough for the bond, two positions and the rent of the accounts the simulation creates. */
export const MIN_PAYER_LAMPORTS = 100_000_000;
/** The devnet faucet wallet: always funded, and the faucet check warns when it runs low. */
export const DEVNET_CHECK_PAYER = "CXBGKyrkwWFjVaEGgAinSFcbSKnfcuoUcZ3xQjCiTm59";

const u64le = (n: BN) => n.toArrayLike(Buffer, "le", 8);

/** One instruction, with accounts in the IDL's order and with the IDL's flags. */
export function buildIx(
  i: IdlLike,
  coder: BorshCoder,
  name: string,
  args: Record<string, unknown>,
  keys: Record<string, PublicKey>,
): TransactionInstruction {
  const def = i.instructions.find((x) => x.name === name);
  if (!def) throw new Error(`the IDL has no instruction ${name}`);
  return new TransactionInstruction({
    programId: new PublicKey(i.address),
    keys: def.accounts.map((a) => {
      const pubkey = a.address ? new PublicKey(a.address) : keys[a.name];
      if (!pubkey) throw new Error(`${name} needs account "${a.name}", which the live check does not know. Add it to app/lib/liveCheck.ts.`);
      return { pubkey, isSigner: !!a.signer, isWritable: !!a.writable };
    }),
    data: coder.instruction.encode(name, args),
  });
}

type Step = { name: string; ix: TransactionInstruction };

/** The lifecycle, as two instruction lists. The second ends in a claim that must be refused as too early. */
export function buildLifecycle(i: IdlLike, payer: PublicKey, agentId: BN): { happy: Step[]; claim: Step[] } {
  const programId = new PublicKey(i.address);
  const coder = new BorshCoder(i as unknown as Idl);
  const pda = (...seeds: Buffer[]) => PublicKey.findProgramAddressSync(seeds, programId)[0];
  const agent = pda(Buffer.from("agent"), payer.toBuffer(), u64le(agentId));
  const position = (nonce: BN) => pda(Buffer.from("position"), agent.toBuffer(), payer.toBuffer(), u64le(nonce));
  const base = {
    operator: payer,
    executor: payer,
    trader: payer,
    admin: payer,
    agent,
    agent_vault: pda(Buffer.from("agent_vault"), agent.toBuffer()),
    config: pda(Buffer.from("config")),
    system_program: SystemProgram.programId,
  };
  const terms = {
    collateral_ratio_bps: 3_000,
    fee_bps: 1_000,
    max_drawdown_bps: 2_000,
    min_duration_secs: new BN(60),
    max_duration_secs: new BN(3_600),
    allowed_assets: [SOL_MINT],
    rules: "Live check. Simulated only.",
  };
  const step = (name: string, args: Record<string, unknown> = {}, nonce?: BN): Step => {
    const p = nonce ? position(nonce) : undefined;
    const vault = p ? pda(Buffer.from("position_vault"), p.toBuffer()) : undefined;
    const keys = p && vault
      ? {
          ...base,
          position: p,
          position_vault: vault,
          custody: pda(Buffer.from("custody"), p.toBuffer()),
          wsol_mint: SOL_MINT,
          mint: SOL_MINT,
          vault_wsol: ataOf(vault, SOL_MINT),
          vault_ata: ataOf(vault, SOL_MINT),
          rent_payer: payer,
          token_program: TOKEN_PROGRAM,
          associated_token_program: ATA_PROGRAM,
        }
      : base;
    return { name, ix: buildIx(i, coder, name, args, keys) };
  };
  const [n1, n2] = [new BN(1), new BN(2)];
  const open = (nonce: BN) => step("open_position", { nonce, amount: new BN(PRINCIPAL), duration_secs: new BN(600) }, nonce);
  const setup = [
    step("create_agent", { agent_id: agentId, name: "Live check", description: "", terms }),
    step("update_agent", { name: "Live check", description: "simulated", terms }),
    step("deposit_collateral", { amount: new BN(BOND) }),
    step("set_executor", { executor: payer }),
    step("publish_agent"),
  ];
  // Positions trade under vault custody since begin_trading exists; before it, the
  // program moved the principal to the trading key with draw_funds.
  const custody = i.instructions.some((x) => x.name === "begin_trading");
  const start = (nonce: BN) => step(custody ? "begin_trading" : "draw_funds", {}, nonce);
  return {
    happy: [
      ...setup,
      open(n1),
      start(n1),
      // Under custody `returned` is ignored: the vault's wSOL (the untouched principal) settles.
      step("settle_position", { returned: new BN(PRINCIPAL) }, n1),
      open(n2),
      ...(custody ? [step("open_vault_token_account", {}, n2), step("close_vault_token_account", {}, n2)] : []),
      step("cancel_position", {}, n2),
      step("set_accepting", { accepting: false }),
      step("set_accepting", { accepting: true }),
      step("withdraw_collateral", { amount: new BN(BOND) }),
    ],
    claim: [...setup, open(n1), start(n1), step("claim_default", {}, n1)],
  };
}

/** The refusal that proves claim_default is wired: a custody position cannot default; a legacy one is simply too early. */
export const EXPECTED_CLAIM_REFUSAL = (i: IdlLike) => (i.instructions.some((x) => x.name === "begin_trading") ? "UseSettle" : "DeadlineNotReached");

type SimError = null | string | { InstructionError?: [number, string | { Custom?: number }] };

/** Where a simulated transaction stopped and why. Program errors (6000 and up) come from the handler, after every account was accepted. */
export function classify(i: IdlLike, steps: Step[], err: SimError, logs: string[]) {
  if (err === null) return { kind: "passed" as const };
  const ie = typeof err === "object" ? err.InstructionError : undefined;
  const detail = logs.find((l) => l.includes("Error Message")) ?? logs.filter((l) => /failed|error/i.test(l)).pop() ?? JSON.stringify(err);
  if (!ie) return { kind: "wiring" as const, at: "the transaction", detail };
  const [index, what] = ie;
  const at = steps[index]?.name ?? `instruction ${index}`;
  const code = typeof what === "object" ? what.Custom : undefined;
  const known = (i.errors ?? []).find((e) => e.code === code);
  if (code !== undefined && code >= 6000) return { kind: "refused" as const, at, error: known?.name ?? `error ${code}`, msg: known?.msg ?? detail };
  return { kind: "wiring" as const, at, detail };
}

async function simulate(connection: Connection, payer: PublicKey, steps: Step[]) {
  const { blockhash } = await connection.getLatestBlockhash("confirmed");
  const message = new TransactionMessage({ payerKey: payer, recentBlockhash: blockhash, instructions: steps.map((s) => s.ix) }).compileToV0Message();
  const res = await connection.simulateTransaction(new VersionedTransaction(message), {
    sigVerify: false,
    replaceRecentBlockhash: true,
    commitment: "confirmed",
  });
  return { err: res.value.err as SimError, logs: res.value.logs ?? [] };
}

/**
 * Runs the check. `payer` must hold at least MIN_PAYER_LAMPORTS on the cluster; its key is
 * not needed. Never throws: problems with the RPC come back as `unknown`.
 */
export async function checkLive(
  connection: Connection,
  payer: PublicKey,
  i: IdlLike = idl as unknown as IdlLike,
  /** The IDL of the program that is deployed, so an instruction it lacks reads as "needs the upgrade" rather than broken. */
  deployed: IdlLike | null = deployedIdl as unknown as IdlLike,
): Promise<LiveCheck> {
  const hash = idlHash(i);
  const done = (status: LiveCheck["status"], reason: string, failedAt?: string, checked: string[] = []): LiveCheck => ({
    status,
    reason,
    failedAt,
    idlHash: hash,
    checked,
  });
  let steps: ReturnType<typeof buildLifecycle>;
  try {
    // A fresh agent id, so the simulated agent never collides with a real one.
    steps = buildLifecycle(i, payer, new BN(Date.now()).mul(new BN(1000)).add(new BN(Math.floor(Math.random() * 1000))));
  } catch (e) {
    return done("broken", (e as Error).message);
  }
  try {
    const program = await connection.getAccountInfo(new PublicKey(i.address), "confirmed");
    if (!program?.executable) return done("broken", `program ${i.address} is not deployed on this cluster`);
    const balance = await connection.getBalance(payer, "confirmed");
    if (balance < MIN_PAYER_LAMPORTS) {
      return done("unknown", `the check's payer ${payer.toBase58()} holds ${balance / 1e9} SOL; it needs ${MIN_PAYER_LAMPORTS / 1e9} to simulate`);
    }

    const happy = await simulate(connection, payer, steps.happy);
    const h = classify(i, steps.happy, happy.err, happy.logs);
    if (h.kind === "wiring" && deployed && !deployed.instructions.some((x) => x.name === h.at)) {
      return done("blocked", `${h.at} is not in the deployed program yet: this build trades once the program is upgraded`, h.at);
    }
    if (h.kind === "wiring") {
      const hint = /AccountNotInitialized/.test(h.detail) && /config/.test(h.detail) ? " The protocol config does not exist yet: run `npm run config -- init`." : "";
      return done("broken", `${h.at} does not work against the deployed program: ${h.detail}.${hint}`, h.at);
    }
    if (h.kind === "refused") return done("blocked", `${h.at} was refused by the program: ${h.error} (${h.msg})`, h.at);

    const claim = await simulate(connection, payer, steps.claim);
    const c = classify(i, steps.claim, claim.err, claim.logs);
    const names = [...new Set([...steps.happy, ...steps.claim].map((s) => s.name))];
    // The only right answer: the position is in custody and cannot default (or,
    // for a program without custody, the deadline ten minutes away is not reached).
    if (c.kind === "refused" && c.at === "claim_default" && c.error === EXPECTED_CLAIM_REFUSAL(i)) {
      return done("ok", `all ${names.length} instructions work against the deployed program`, undefined, names);
    }
    if (c.kind === "passed") return done("broken", "claim_default succeeded on a position that cannot default", "claim_default");
    if (c.kind === "refused") return done("blocked", `${c.at} was refused by the program: ${c.error} (${c.msg})`, c.at);
    return done("broken", `${c.at} does not work against the deployed program: ${c.detail}`, c.at);
  } catch (e) {
    return done("unknown", `could not reach the cluster: ${(e as Error).message}`);
  }
}
