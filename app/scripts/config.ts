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
 *   npm run config -- trading <maxSwapDeviation%> <latePenalty%>
 *
 * Pausing stops new positions, draws and collateral deposits. Cancels, settles,
 * default claims and withdrawals keep working.
 *
 * `trading` sets what custody positions may do: swaps through Orca Whirlpools,
 * priced by Pyth (SOL and USDC feeds, including devnet USDC), losing at most
 * maxSwapDeviation% of oracle value per swap, with latePenalty% of the locked
 * bond charged on a late settlement. No position can swap until it is set.
 */
import { BN } from "@coral-xyz/anchor";
import { Connection, LAMPORTS_PER_SOL } from "@solana/web3.js";
import { U64_MAX, defaultTradingConfig, initConfig, loadAdmin, readConfig, setCaps, setPaused, setTradingConfig } from "./protocolConfig";

const RPC = process.env.NEXT_PUBLIC_RPC_URL ?? "http://127.0.0.1:8899";

function capArg(s: string | undefined, name: string): BN {
  if (s === "none") return U64_MAX;
  const sol = Number(s);
  if (!s || !Number.isFinite(sol) || sol <= 0) throw new Error(`${name}: give a positive amount in SOL, or "none"`);
  return new BN(Math.round(sol * LAMPORTS_PER_SOL).toString());
}

const fmtCap = (b: BN) => (b.eq(U64_MAX) ? "none" : `${Number(b.toString()) / LAMPORTS_PER_SOL} SOL`);

/** A percentage with up to two decimals, as bps. */
function pctArg(s: string | undefined, name: string): number {
  const pct = Number(s);
  if (!s || !Number.isFinite(pct) || pct < 0 || pct > 50) throw new Error(`${name}: give a percentage from 0 to 50`);
  return Math.round(pct * 100);
}

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
    else if (cmd === "trading") sig = await setTradingConfig(connection, admin, defaultTradingConfig(pctArg(a, "maxSwapDeviation"), pctArg(b, "latePenalty")));
    else throw new Error(`unknown command "${cmd}": use init, caps, pause, resume or trading`);
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
  console.log(`dex programs:      ${c.allowedDexPrograms.length ? c.allowedDexPrograms.map((k) => k.toBase58()).join(", ") : "none (no position can swap)"}`);
  console.log(`oracle program:    ${c.oracleProgram.toBase58()}`);
  console.log(`max price age:     ${c.maxPriceAgeSecs.toString()} s`);
  console.log(`max swap deviation ${c.maxSwapDeviationBps / 100}%`);
  console.log(`late penalty:      ${c.latePenaltyBps / 100}% of the locked bond`);
  console.log(`priced mints:      ${c.feeds.length ? c.feeds.map((f) => f.mint.toBase58()).join(", ") : "none"}`);
}

main().catch((e) => {
  console.error(e?.error?.errorMessage ?? e?.message ?? e);
  process.exit(1);
});
