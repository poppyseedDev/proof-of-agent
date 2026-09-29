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

export type ConfigState = { paused: boolean; maxPosition: BN; maxAgentCapital: BN };

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

/** Creates the config with no caps if it does not exist yet. For localnet. */
export async function ensureConfig(connection: Connection, admin: Keypair) {
  if (await readConfig(connection)) return;
  await initConfig(connection, admin, U64_MAX, U64_MAX);
}
