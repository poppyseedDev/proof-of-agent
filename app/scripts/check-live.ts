/**
 * Does this checkout's IDL work against the program deployed on the cluster?
 * Simulates the whole lifecycle; signs and sends nothing.
 *
 *   npm run check:live                       -> devnet
 *   npm run check:live -- --idl <file.json>  -> check another IDL file
 *   npm run check:live -- --hash             -> print this checkout's IDL fingerprint
 *
 * Env: CHECK_RPC_URL (default public devnet), CHECK_PAYER (an address that holds SOL
 * there; default the devnet faucet wallet). Exit code 0 only when every instruction works.
 */
import { readFileSync } from "node:fs";
import { Connection, PublicKey } from "@solana/web3.js";
import { DEVNET_CHECK_PAYER, checkLive, idlHash, type IdlLike } from "../lib/liveCheck";

async function main() {
  if (process.argv.includes("--hash")) {
    console.log(idlHash());
    return;
  }
  const rpc = process.env.CHECK_RPC_URL ?? "https://api.devnet.solana.com";
  const payer = new PublicKey(process.env.CHECK_PAYER ?? DEVNET_CHECK_PAYER);
  const at = process.argv.indexOf("--idl");
  const idl = at > 0 ? (JSON.parse(readFileSync(process.argv[at + 1], "utf8")) as IdlLike) : undefined;
  const r = await checkLive(new Connection(rpc, "confirmed"), payer, idl);
  console.log(`${r.status.toUpperCase()}: ${r.reason}`);
  console.log(`IDL ${r.idlHash} against ${rpc}`);
  process.exit(r.status === "ok" ? 0 : r.status === "broken" ? 1 : 2);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
