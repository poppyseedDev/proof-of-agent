"use client";

import { AnchorProvider, BN, Idl, Program } from "@coral-xyz/anchor";
import { Connection, PublicKey, LAMPORTS_PER_SOL } from "@solana/web3.js";
import type { AnchorWallet } from "@solana/wallet-adapter-react";
import idl from "./idl.json";

export const PROGRAM_ID = new PublicKey(idl.address);
export const RPC_URL =
  process.env.NEXT_PUBLIC_RPC_URL ?? "https://api.devnet.solana.com";
export const CLUSTER = process.env.NEXT_PUBLIC_CLUSTER ?? "devnet";
export { SITE_URL, APP_URL, WAITLIST_URL } from "./urls";

export const BPS = 10_000;
export const MIN_RATIO_BPS = 1_000;
export const MAX_RATIO_BPS = 10_000;
export const MAX_DRAWDOWN_BPS = 5_000;
export const FEE_CAP_DIVISOR = 2;
export const MIN_DURATION_SECS = 60;
export const MAX_DURATION_SECS = 90 * 24 * 3600;
export const MAX_NAME_LEN = 32;
export const MAX_DESCRIPTION_LEN = 128;
export const MAX_RULES_LEN = 512;
export const MAX_ALLOWED_ASSETS = 8;

const enc = (s: string) => Buffer.from(s);

export function agentPda(operator: PublicKey, agentId: BN) {
  return PublicKey.findProgramAddressSync(
    [enc("agent"), operator.toBuffer(), agentId.toArrayLike(Buffer, "le", 8)],
    PROGRAM_ID,
  )[0];
}
export function agentVaultPda(agent: PublicKey) {
  return PublicKey.findProgramAddressSync(
    [enc("agent_vault"), agent.toBuffer()],
    PROGRAM_ID,
  )[0];
}
export function positionPda(agent: PublicKey, trader: PublicKey, nonce: BN) {
  return PublicKey.findProgramAddressSync(
    [
      enc("position"),
      agent.toBuffer(),
      trader.toBuffer(),
      nonce.toArrayLike(Buffer, "le", 8),
    ],
    PROGRAM_ID,
  )[0];
}
export function positionVaultPda(position: PublicKey) {
  return PublicKey.findProgramAddressSync(
    [enc("position_vault"), position.toBuffer()],
    PROGRAM_ID,
  )[0];
}
/** Exists while a position trades under vault custody. */
export function custodyPda(position: PublicKey) {
  return PublicKey.findProgramAddressSync(
    [enc("custody"), position.toBuffer()],
    PROGRAM_ID,
  )[0];
}
export function configPda() {
  return PublicKey.findProgramAddressSync([enc("config")], PROGRAM_ID)[0];
}

export const WSOL_MINT = new PublicKey("So11111111111111111111111111111111111111112");
export const TOKEN_PROGRAM_ID = new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
export const ASSOCIATED_TOKEN_PROGRAM_ID = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
/** The associated token account of `owner` for `mint` under the classic token program. */
export function ataOf(owner: PublicKey, mint: PublicKey) {
  return PublicKey.findProgramAddressSync(
    [owner.toBuffer(), TOKEN_PROGRAM_ID.toBuffer(), mint.toBuffer()],
    ASSOCIATED_TOKEN_PROGRAM_ID,
  )[0];
}

export type CustodyAccount = {
  position: PublicKey;
  rentPayer: PublicKey;
  heldMask: number;
  swaps: number;
};

/** Read-only program (no wallet) for listing accounts. */
export function readonlyProgram(connection: Connection) {
  const provider = new AnchorProvider(
    connection,
    // Dummy wallet: never signs.
    {
      publicKey: PublicKey.default,
      signTransaction: async (tx) => tx,
      signAllTransactions: async (txs) => txs,
    },
    { commitment: "confirmed" },
  );
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return new Program(idl as Idl, provider) as any;
}

export function walletProgram(connection: Connection, wallet: AnchorWallet) {
  const provider = new AnchorProvider(connection, wallet, {
    commitment: "confirmed",
  });
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return new Program(idl as Idl, provider) as any;
}

// ---------- decoded shapes ----------

export type AgentTerms = {
  collateralRatioBps: number;
  feeBps: number;
  maxDrawdownBps: number;
  minDurationSecs: BN;
  maxDurationSecs: BN;
  allowedAssets: PublicKey[];
  rules: string;
};

export type AgentStatus = "draft" | "active" | "paused";

export type AgentAccount = {
  publicKey: PublicKey;
  operator: PublicKey;
  executor: PublicKey;
  agentId: BN;
  status: AgentStatus;
  name: string;
  description: string;
  terms: AgentTerms;
  createdAt: BN;
  publishedAt: BN;
  totalCollateral: BN;
  lockedCollateral: BN;
  capitalManaged: BN;
  openPositions: number;
  settledPositions: number;
  defaultedPositions: number;
  breachCount: number;
  slashedTotal: BN;
  feesEarned: BN;
};

export type PositionStatus =
  | "open"
  | "trading"
  | "settled"
  | "defaulted"
  | "cancelled";

export type BreachKind = "none" | "drawdown" | "missedDeadline";

export type PositionAccount = {
  publicKey: PublicKey;
  trader: PublicKey;
  agent: PublicKey;
  nonce: BN;
  principal: BN;
  lockedCollateral: BN;
  feeBps: number;
  maxDrawdownBps: number;
  status: PositionStatus;
  breach: BreachKind;
  openedAt: BN;
  deadline: BN;
  drawnAt: BN;
  closedAt: BN;
  returned: BN;
  slashed: BN;
  feePaid: BN;
};

const enumKey = <T extends string>(v: object) => Object.keys(v)[0] as T;

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function decodeAgent(raw: { publicKey: PublicKey; account: any }): AgentAccount {
  return { publicKey: raw.publicKey, ...raw.account, status: enumKey<AgentStatus>(raw.account.status) };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function decodePosition(raw: { publicKey: PublicKey; account: any }): PositionAccount {
  return {
    publicKey: raw.publicKey,
    ...raw.account,
    status: enumKey<PositionStatus>(raw.account.status),
    breach: enumKey<BreachKind>(raw.account.breach),
  };
}

export const isPublished = (a: AgentAccount) => a.status !== "draft";

// ---------- math mirrors of the on-chain formulas ----------

export function maxFeeForRatio(ratioBps: number) {
  return Math.floor(ratioBps / FEE_CAP_DIVISOR);
}
export function requiredCollateral(principalLamports: number, ratioBps: number) {
  return Math.ceil((principalLamports * ratioBps) / BPS);
}
export function freeCollateral(a: AgentAccount) {
  return a.totalCollateral.sub(a.lockedCollateral);
}
/** Protocol-wide limits set by the program's upgrade authority, in lamports. */
export type ProtocolConfig = { paused: boolean; maxPosition: number; maxAgentCapital: number };

// "No cap" is stored as u64::MAX, which does not fit in a JS number.
const toSafeNumber = (b: BN) => (b.gt(new BN(Number.MAX_SAFE_INTEGER)) ? Number.MAX_SAFE_INTEGER : b.toNumber());

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function decodeConfig(raw: any): ProtocolConfig {
  return { paused: raw.paused, maxPosition: toSafeNumber(raw.maxPosition), maxAgentCapital: toSafeNumber(raw.maxAgentCapital) };
}

/**
 * Max principal a new position on this agent may hold: what its free collateral
 * can back, within the protocol's position and per-agent caps when `config` is given.
 */
export function capacity(a: AgentAccount, config: ProtocolConfig | null = null) {
  const free = freeCollateral(a).toNumber();
  const backed = Math.floor((free * BPS) / a.terms.collateralRatioBps);
  if (!config) return backed;
  const underAgentCap = Math.max(0, config.maxAgentCapital - a.capitalManaged.toNumber());
  return Math.min(backed, config.maxPosition, underAgentCap);
}
/** Max principal the agent's whole bond could back. */
export function maxCapacity(a: AgentAccount) {
  return Math.floor((a.totalCollateral.toNumber() * BPS) / a.terms.collateralRatioBps);
}

// ---------- assets ----------

export type AssetInfo = { mint: string; symbol: string; name: string };

const DEVNET_USDC = "BRjpCHtyQLNCo8gqRUr8jtdAj5AjPYQaoqbvcZiHok1k";
const MAINNET_USDC = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

/** Assets offered in the operator form. Operators can also paste any mint. */
export const KNOWN_ASSETS: AssetInfo[] = [
  { mint: "So11111111111111111111111111111111111111112", symbol: "SOL", name: "Solana" },
  { mint: CLUSTER === "mainnet-beta" ? MAINNET_USDC : DEVNET_USDC, symbol: "USDC", name: "USD Coin" },
  { mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", symbol: "USDT", name: "Tether" },
  { mint: "JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN", symbol: "JUP", name: "Jupiter" },
  { mint: "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263", symbol: "BONK", name: "Bonk" },
];

export function assetLabel(mint: PublicKey | string) {
  const m = mint.toString();
  return KNOWN_ASSETS.find((a) => a.mint === m)?.symbol ?? short(m);
}

// ---------- durations ----------

export function fmtDuration(secs: number) {
  if (secs % 86_400 === 0) return `${secs / 86_400} day${secs === 86_400 ? "" : "s"}`;
  if (secs % 3_600 === 0) return `${secs / 3_600} hour${secs === 3_600 ? "" : "s"}`;
  if (secs % 60 === 0) return `${secs / 60} min`;
  return `${secs}s`;
}

// ---------- formatting ----------

export const sol = (lamports: BN | number, digits = 2) =>
  (Number(lamports.toString()) / LAMPORTS_PER_SOL).toLocaleString(undefined, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
export const pct = (bps: number, digits = 0) =>
  (bps / 100).toLocaleString(undefined, { maximumFractionDigits: digits }) + "%";
/** Fees move in 50 bps steps, so they get one decimal when needed: "15%", "15.5%". */
export const feePct = (bps: number) => pct(bps, 1);
export const short = (k: PublicKey | string) => {
  const s = k.toString();
  return s.slice(0, 4) + "…" + s.slice(-4);
};
export { toLamports } from "./amounts";
export const explorer = (sig: string) =>
  CLUSTER === "localnet"
    ? `https://explorer.solana.com/tx/${sig}?cluster=custom&customUrl=${encodeURIComponent(RPC_URL)}`
    : `https://explorer.solana.com/tx/${sig}?cluster=${CLUSTER}`;
