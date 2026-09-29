import { Connection, PublicKey } from "@solana/web3.js";
import { DEVNET_CHECK_PAYER, checkLive, type LiveCheck } from "@/lib/liveCheck";

/**
 * Can this deployment trade? Simulates the whole position lifecycle, built from the
 * IDL this deployment was built with, against the program on the cluster. Nothing is
 * sent. Monitors and the deploy script read it.
 *
 * 503 only when the site's instructions do not work against the program ("broken").
 * A paused protocol ("blocked") or an unreachable RPC ("unknown") answers 200, with
 * the status in the body.
 */
export const dynamic = "force-dynamic";
export const runtime = "nodejs";

const RPC = process.env.RPC_URL ?? process.env.NEXT_PUBLIC_RPC_URL ?? "https://api.devnet.solana.com";
const PAYER = process.env.CHECK_PAYER ?? DEVNET_CHECK_PAYER;
const TTL_MS = 60_000;
/** An unknown result is retried sooner, so one RPC hiccup does not hide the real state for a minute. */
const UNKNOWN_TTL_MS = 10_000;

type Health = LiveCheck & { checkedAt: string };
let cache: { at: number; health: Health } | null = null;
let inflight: Promise<Health> | null = null;

async function run(): Promise<Health> {
  const check = await checkLive(new Connection(RPC, "confirmed"), new PublicKey(PAYER));
  const health = { ...check, checkedAt: new Date().toISOString() };
  cache = { at: Date.now(), health };
  return health;
}

export async function GET() {
  const fresh = cache && Date.now() - cache.at < (cache.health.status === "unknown" ? UNKNOWN_TTL_MS : TTL_MS);
  const health = fresh ? cache!.health : await (inflight ??= run().finally(() => (inflight = null)));
  return Response.json(health, { status: health.status === "broken" ? 503 : 200, headers: { "Cache-Control": "no-store" } });
}
