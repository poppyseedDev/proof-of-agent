/**
 * Protocol admin: pause switch and caps. Signed by the program's upgrade authority
 * (ADMIN_KEYPAIR, default ~/.config/solana/id.json) on NEXT_PUBLIC_RPC_URL
 * (default localnet).
 *
 *   npm run config                          -> show the current config
 *   npm run config -- init <maxPositionSol> <maxAgentSol>
 *   npm run config -- caps <maxPositionSol> <maxAgentSol>   ("none" for no cap)
 *   npm run config -- pause
 *   npm run config -- resume
 *
 * Pausing stops new positions, draws and collateral deposits. Cancels, settles,
 * default claims and withdrawals keep working.
 */
import { BN } from "@coral-xyz/anchor";
import { Connection, LAMPORTS_PER_SOL } from "@solana/web3.js";
import { U64_MAX, initConfig, loadAdmin, readConfig, setCaps, setPaused } from "./protocolConfig";

const RPC = process.env.NEXT_PUBLIC_RPC_URL ?? "http://127.0.0.1:8899";

function capArg(s: string | undefined, name: string): BN {
  if (s === "none") return U64_MAX;
  const sol = Number(s);
  if (!s || !Number.isFinite(sol) || sol <= 0) throw new Error(`${name}: give a positive amount in SOL, or "none"`);
  return new BN(Math.round(sol * LAMPORTS_PER_SOL).toString());
}

const fmtCap = (b: BN) => (b.eq(U64_MAX) ? "none" : `${Number(b.toString()) / LAMPORTS_PER_SOL} SOL`);

async function main() {
  const connection = new Connection(RPC, "confirmed");
  const [, , cmd, a, b] = process.argv;
  if (cmd) {
    const admin = loadAdmin();
    console.log(`admin ${admin.publicKey.toBase58()} on ${RPC}`);
    let sig: string;
    if (cmd === "init") sig = await initConfig(connection, admin, capArg(a, "maxPositionSol"), capArg(b, "maxAgentSol"));
    else if (cmd === "caps") sig = await setCaps(connection, admin, capArg(a, "maxPositionSol"), capArg(b, "maxAgentSol"));
    else if (cmd === "pause") sig = await setPaused(connection, admin, true);
    else if (cmd === "resume") sig = await setPaused(connection, admin, false);
    else throw new Error(`unknown command "${cmd}": use init, caps, pause or resume`);
    console.log(`sent ${sig}`);
  }
  const c = await readConfig(connection);
  if (!c) {
    console.log("No config yet: no position can open until `npm run config -- init` runs.");
    return;
  }
  console.log(`paused:            ${c.paused}`);
  console.log(`max per position:  ${fmtCap(c.maxPosition)}`);
  console.log(`max per agent:     ${fmtCap(c.maxAgentCapital)}`);
}

main().catch((e) => {
  console.error(e?.error?.errorMessage ?? e?.message ?? e);
  process.exit(1);
});
