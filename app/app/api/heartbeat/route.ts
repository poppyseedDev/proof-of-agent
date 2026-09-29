import { head, put } from "@vercel/blob";
import { hasBearer } from "@/lib/auth";

/**
 * Liveness of the agent runners. The runner POSTs every tick with the shared
 * secret; the site GETs it to show whether agents are online.
 */
export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const PATH = "status/heartbeat.json";
/** `idl` is the fingerprint of the program interface the runner was built with (see idlHash in lib/liveCheck). */
type Beat = { at: string; agents: string[]; note?: string; idl?: string };
let cached: Beat | null = null;

/** The runner's report, or null when a field has the wrong type. Every field is optional. */
function parseBeat(raw: unknown): { agents: string[]; note?: string; idl?: string } | null {
  const r = raw ?? {};
  if (typeof r !== "object" || Array.isArray(r)) return null;
  const { agents = [], note, idl } = r as { agents?: unknown; note?: unknown; idl?: unknown };
  if (!Array.isArray(agents) || !agents.every((a) => typeof a === "string")) return null;
  if (note !== undefined && note !== null && typeof note !== "string") return null;
  if (idl !== undefined && idl !== null && !(typeof idl === "string" && /^[0-9a-f]{12}$/.test(idl))) return null;
  return { agents, note: note ?? undefined, idl: idl ?? undefined };
}

export async function POST(req: Request) {
  if (!hasBearer(req, process.env.HEARTBEAT_SECRET)) return new Response("Not found", { status: 404 });
  const body = parseBeat(await req.json().catch(() => ({})));
  if (!body) return Response.json({ error: "agents must be an array of strings, note a string and idl a 12-character hash" }, { status: 400 });
  cached = { at: new Date().toISOString(), agents: body.agents.slice(0, 20), note: body.note?.slice(0, 200), idl: body.idl };
  if (process.env.BLOB_READ_WRITE_TOKEN) {
    await put(PATH, JSON.stringify(cached), { access: "public", contentType: "application/json", addRandomSuffix: false, allowOverwrite: true, cacheControlMaxAge: 60 }).catch((e) => console.error("[heartbeat] blob write failed:", e));
  }
  return Response.json({ ok: true });
}

export async function GET() {
  let beat = cached;
  if (!beat && process.env.BLOB_READ_WRITE_TOKEN) {
    const meta = await head(PATH).catch(() => null);
    if (meta) beat = (await fetch(meta.downloadUrl ?? meta.url, { cache: "no-store" }).then((r) => r.json()).catch(() => null)) as Beat | null;
  }
  const ageSec = beat ? Math.round((Date.now() - new Date(beat.at).getTime()) / 1000) : null;
  return Response.json({ online: ageSec !== null && ageSec < 300, ageSec, at: beat?.at ?? null, agents: beat?.agents ?? [], idl: beat?.idl ?? null }, { headers: { "Cache-Control": "no-store" } });
}
