import { createHash } from "node:crypto";
import { mkdirSync } from "node:fs";
import { AGENTS, POLL_MS, WATCHDOG_SECS } from "./config.js";
import { initOrca, mainPrice } from "./orca.js";
import { AgentRunner, glog, heartbeatNote, msg, refreshClock, tickAll } from "./agent.js";
import { IDL } from "./chain.js";
import { acquireLock } from "./lock.js";
import { stateDir } from "./state.js";

const HEARTBEAT_URL = process.env.HEARTBEAT_URL ?? "https://dev.proofofagent.dev/api/heartbeat";
const HEARTBEAT_SECRET = process.env.HEARTBEAT_SECRET;
/** Fingerprint of the program interface this runner was built with; the same formula as idlHash in app/lib/liveCheck.ts. */
const IDL_HASH = createHash("sha256").update(JSON.stringify(IDL)).digest("hex").slice(0, 12);

/** Tells the site the runner is alive so testers can see whether agents are online. */
async function heartbeat(agents: string[], note?: string) {
  if (!HEARTBEAT_SECRET) return;
  try {
    await fetch(HEARTBEAT_URL, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${HEARTBEAT_SECRET}` },
      body: JSON.stringify({ agents, note, idl: IDL_HASH }),
      signal: AbortSignal.timeout(8000),
    });
  } catch (e) {
    glog("heartbeat failed:", (e as Error).message);
  }
}

async function main() {
  // Two runners on one STATE_DIR would trade the same books.
  mkdirSync(stateDir, { recursive: true });
  const release = acquireLock(`${stateDir}runner.lock`, undefined, glog);
  process.on("exit", release); // clean stop, watchdog exit, or a fatal error

  glog(`program interface ${IDL_HASH}`);
  await initOrca();
  const only = process.argv.slice(2);
  const runners = AGENTS.filter((a) => !only.length || only.includes(a.id)).map((a) => new AgentRunner(a));
  for (const r of runners) await r.init();

  let stopping = false;
  const stop = () => {
    stopping = true;
    console.log("stopping after this tick…");
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);

  // Runs on its own timer, so it also fires when a tick hangs.
  let lastRan = Date.now();
  setInterval(() => {
    const idle = Math.round((Date.now() - lastRan) / 1000);
    if (idle < WATCHDOG_SECS) return;
    glog(`WATCHDOG no agent has completed a tick in ${idle}s; exiting so the supervisor restarts the runner`);
    for (const r of runners) r.save();
    process.exit(2);
  }, 15_000).unref();

  while (!stopping) {
    await refreshClock();
    let price: number | null = null;
    try {
      price = await mainPrice();
    } catch (e) {
      glog("price unavailable:", msg(e));
    }
    const { ran, degraded } = await tickAll(runners, price);
    // Only report agents whose tick actually ran.
    if (ran.length) {
      lastRan = Date.now();
      await heartbeat(ran, heartbeatNote(degraded));
    }
    await new Promise((res) => setTimeout(res, POLL_MS));
  }
  for (const r of runners) r.save();
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
