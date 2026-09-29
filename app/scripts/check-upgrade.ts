/**
 * Is it safe to upgrade the program on devnet now? Reads only; changes nothing.
 *
 *   npm run check:upgrade
 *
 * An upgrade that makes the program require something new breaks every client that
 * was built before the change, so the site and the runner have to be on the new IDL
 * first. This checks that they are, and prints the steps of the upgrade.
 */
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { Connection, PublicKey } from "@solana/web3.js";
import idl from "../lib/idl.json";
import deployed from "../lib/deployed/devnet.json";
import { compareIdl, type CompatIdl } from "../lib/idlCompat";
import { DEVNET_CHECK_PAYER, checkLive, idlHash } from "../lib/liveCheck";

const SITE = process.env.SITE_URL ?? "https://dev.proofofagent.dev";
const RPC = process.env.CHECK_RPC_URL ?? "https://api.devnet.solana.com";
const root = join(__dirname, "../..");

const problems: string[] = [];
const ok = (line: string) => console.log(`  ok       ${line}`);
const bad = (line: string) => {
  console.log(`  PROBLEM  ${line}`);
  problems.push(line);
};
/** The JSON a URL answers with; {} when it answers with something else (a build from before the route existed). */
const json = async (url: string) => {
  const text = await (await fetch(url, { signal: AbortSignal.timeout(20_000) })).text();
  try {
    return JSON.parse(text);
  } catch {
    return {};
  }
};

async function main() {
  const want = idlHash();
  console.log(`Upgrading the devnet program to the interface ${want}\n`);

  // 1. The program binary, the app and the SDK are from the same source.
  const built = join(root, "target/idl/proof_of_agent.json");
  const same = (path: string) => existsSync(path) && JSON.stringify(JSON.parse(readFileSync(path, "utf8"))) === JSON.stringify(idl);
  if (same(built)) ok("the built program matches app/lib/idl.json");
  else bad("target/idl/proof_of_agent.json differs from app/lib/idl.json: run `anchor build` and copy the IDL to app/lib and sdk/idl");
  if (same(join(root, "sdk/idl/proof_of_agent.json"))) ok("the SDK ships the same IDL");
  else bad("sdk/idl/proof_of_agent.json differs from app/lib/idl.json");

  // 2. What the upgrade changes for clients.
  const diff = compareIdl(deployed as unknown as CompatIdl, idl as unknown as CompatIdl);
  if (diff.breaking.length) for (const b of diff.breaking) bad(`incompatible with the deployed program: ${b}`);
  else ok("clients built from this IDL work before and after the upgrade");
  const clientsFirst = diff.clientsFirst.length > 0;
  if (clientsFirst) {
    console.log("\n  After the upgrade, clients built before this change fail:");
    for (const c of diff.clientsFirst) console.log(`    - ${c}`);
    console.log("  So every client has to run this IDL before the program is upgraded:\n");
  }

  // 3. The clients that are live.
  try {
    const health = await json(`${SITE}/api/health`);
    if (health.idlHash === want) ok(`the site runs ${want}`);
    else if (clientsFirst) bad(`the site runs ${health.idlHash ?? "an older build"}: run \`npm run deploy:site\` first`);
    else ok(`the site runs ${health.idlHash ?? "an older build"}, which keeps working`);
  } catch (e) {
    bad(`could not read ${SITE}/api/health: ${(e as Error).message}`);
  }
  try {
    const beat = await json(`${SITE}/api/heartbeat`);
    if (!beat.online) bad("the agent runner is offline");
    else if (beat.idl === want) ok(`the agent runner runs ${want}`);
    else if (clientsFirst) bad(`the agent runner runs ${beat.idl ?? "an older build"}: redeploy it first (fly deploy . --config agent/fly.toml --ha=false)`);
    else ok(`the agent runner runs ${beat.idl ?? "an older build"}, which keeps working`);
  } catch (e) {
    bad(`could not read ${SITE}/api/heartbeat: ${(e as Error).message}`);
  }
  if (clientsFirst) console.log("  note     operators on their own servers need the new SDK before the upgrade too");

  // 4. The cluster is healthy now, so a failure after the upgrade is the upgrade's.
  const live = await checkLive(new Connection(RPC, "confirmed"), new PublicKey(process.env.CHECK_PAYER ?? DEVNET_CHECK_PAYER));
  if (live.status === "ok") ok(`live check: ${live.reason}`);
  else bad(`live check is ${live.status}: ${live.reason}`);

  if (problems.length) {
    console.log(`\nNot safe to upgrade yet: ${problems.length} problem${problems.length > 1 ? "s" : ""} above.`);
    process.exit(1);
  }
  console.log(`
Safe to upgrade. In this order:
  1. solana program deploy target/deploy/proof_of_agent.so --program-id target/deploy/proof_of_agent-keypair.json -u devnet
  2. NEXT_PUBLIC_RPC_URL=${RPC} npm run config          (if it says "No config yet": npm run config -- init none none)
  3. npm run check:live                                  (must say OK)
  4. cp target/idl/proof_of_agent.json app/lib/deployed/devnet.json, then commit`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
