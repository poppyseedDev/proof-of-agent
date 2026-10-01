/**
 * Protocol config (pause switch and caps), shared by the admin CLI, the localnet
 * seed and the localnet tests. Every call must be signed by the program's upgrade
 * authority; the program checks it against the ProgramData account.
 */
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { AnchorProvider, BN, Idl, Program, Wallet } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import idl from "../lib/idl.json";

const PROGRAM_ID = new PublicKey(idl.address);
/** "No cap", as the program stores it. */
export const U64_MAX = new BN("18446744073709551615");

export const configPda = () => PublicKey.findProgramAddressSync([Buffer.from("config")], PROGRAM_ID)[0];

/** The upgrade authority's key: ADMIN_KEYPAIR, else the Solana CLI default wallet. */
export function loadAdmin(path = process.env.ADMIN_KEYPAIR ?? `${homedir()}/.config/solana/id.json`): Keypair {
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(path, "utf8"))));
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function programFor(connection: Connection, admin: Keypair): any {
  return new Program(idl as Idl, new AnchorProvider(connection, new Wallet(admin), { commitment: "confirmed" }));
}

export type FeedMapping = { mint: PublicKey; feedId: number[] };
export type TradingConfig = {
  allowedDexPrograms: PublicKey[];
  oracleProgram: PublicKey;
  maxPriceAgeSecs: BN;
  maxSwapDeviationBps: number;
  latePenaltyBps: number;
  feeds: FeedMapping[];
};
export type ConfigState = { paused: boolean; maxPosition: BN; maxAgentCapital: BN } & TradingConfig;

/** Well-known programs and Pyth feeds, the same ids on devnet and mainnet. */
export const ORCA_WHIRLPOOL_PROGRAM = new PublicKey("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc");
export const PYTH_RECEIVER_PROGRAM = new PublicKey("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ");
export const PYTH_FEEDS: Record<string, string> = {
  "SOL/USD": "ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d",
  "USDC/USD": "eaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a",
};
export const MINTS = {
  SOL: new PublicKey("So11111111111111111111111111111111111111112"),
  USDC: new PublicKey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"),
  DEVUSDC: new PublicKey("BRjpCHtyQLNCo8gqRUr8jtdAj5AjPYQaoqbvcZiHok1k"),
};
export const feedId = (hex: string) => Array.from(Buffer.from(hex.replace(/^0x/, ""), "hex"));
/** The trading config this deployment starts with: Orca, Pyth, SOL and USDC (real and devnet) priced by their USD feeds. */
export function defaultTradingConfig(maxSwapDeviationBps: number, latePenaltyBps: number, maxPriceAgeSecs = 60): TradingConfig {
  return {
    allowedDexPrograms: [ORCA_WHIRLPOOL_PROGRAM],
    oracleProgram: PYTH_RECEIVER_PROGRAM,
    maxPriceAgeSecs: new BN(maxPriceAgeSecs),
    maxSwapDeviationBps,
    latePenaltyBps,
    feeds: [
      { mint: MINTS.SOL, feedId: feedId(PYTH_FEEDS["SOL/USD"]) },
      { mint: MINTS.USDC, feedId: feedId(PYTH_FEEDS["USDC/USD"]) },
      { mint: MINTS.DEVUSDC, feedId: feedId(PYTH_FEEDS["USDC/USD"]) },
    ],
  };
}

/** The current config, or null if the admin has not created it yet. */
export async function readConfig(connection: Connection): Promise<ConfigState | null> {
  const info = await connection.getAccountInfo(configPda(), "confirmed");
  if (!info) return null;
  // The connection is only used to build the coder; decoding is local.
  const program = programFor(connection, Keypair.generate());
  return program.coder.accounts.decode("config", info.data) as ConfigState;
}

export async function initConfig(connection: Connection, admin: Keypair, maxPosition: BN, maxAgentCapital: BN) {
  return programFor(connection, admin).methods.initConfig(maxPosition, maxAgentCapital).accounts({ admin: admin.publicKey }).rpc();
}

export async function setPaused(connection: Connection, admin: Keypair, paused: boolean) {
  return programFor(connection, admin).methods.setPaused(paused).accounts({ admin: admin.publicKey }).rpc();
}

export async function setCaps(connection: Connection, admin: Keypair, maxPosition: BN, maxAgentCapital: BN) {
  return programFor(connection, admin).methods.setCaps(maxPosition, maxAgentCapital).accounts({ admin: admin.publicKey }).rpc();
}

export async function setTradingConfig(connection: Connection, admin: Keypair, t: TradingConfig) {
  return programFor(connection, admin).methods
    .setTradingConfig(t.allowedDexPrograms, t.oracleProgram, t.maxPriceAgeSecs, t.maxSwapDeviationBps, t.latePenaltyBps, t.feeds)
    .accounts({ admin: admin.publicKey })
    .rpc();
}

/** Creates the config with no caps if it does not exist yet. For localnet. */
export async function ensureConfig(connection: Connection, admin: Keypair) {
  if (await readConfig(connection)) return;
  await initConfig(connection, admin, U64_MAX, U64_MAX);
}
