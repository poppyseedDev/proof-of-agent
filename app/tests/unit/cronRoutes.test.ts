import { setEnv } from "./helpers/stubs";
import { store } from "./helpers/mockStore";
import { blob } from "./helpers/fakeBlob";
import { beforeEach, describe, it, mock } from "node:test";
import assert from "node:assert/strict";
import * as heartbeat from "@/app/api/heartbeat/route";
import * as backup from "@/app/api/waitlist/backup/route";

const req = (method: string, authorization?: string, body?: unknown) =>
  new Request("https://app.test/api/x", {
    method,
    headers: authorization ? { authorization } : {},
    body: body === undefined ? undefined : JSON.stringify(body),
  });

beforeEach(() => {
  blob.reset();
  store.reset();
});

describe("POST /api/heartbeat auth", () => {
  beforeEach(() => setEnv({ HEARTBEAT_SECRET: "hb-secret", BLOB_READ_WRITE_TOKEN: undefined }));

  it("is 404 without, or with a wrong, bearer token", async () => {
    for (const auth of [undefined, "Bearer wrong", "hb-secret", "Basic hb-secret"]) {
      const res = await heartbeat.POST(req("POST", auth, { agents: ["a"] }));
      assert.equal(res.status, 404, String(auth));
    }
  });

  it("is 404 for everyone when HEARTBEAT_SECRET is unset", async () => {
    setEnv({ HEARTBEAT_SECRET: undefined });
    assert.equal((await heartbeat.POST(req("POST", "Bearer ", {}))).status, 404);
    assert.equal((await heartbeat.POST(req("POST", "Bearer undefined", {}))).status, 404);
  });

  it("accepts the right token and records the beat", async () => {
    const res = await heartbeat.POST(req("POST", "Bearer hb-secret", { agents: ["alpha", "beta"] }));
    assert.equal(res.status, 200);
    const status = await (await heartbeat.GET()).json();
    assert.equal(status.online, true);
    assert.deepEqual(status.agents, ["alpha", "beta"]);
  });

  it("writes to Blob only when it is configured", async () => {
    await heartbeat.POST(req("POST", "Bearer hb-secret", {}));
    assert.equal(blob.calls.put.length, 0);
    setEnv({ BLOB_READ_WRITE_TOKEN: "test-token" });
    await heartbeat.POST(req("POST", "Bearer hb-secret", {}));
    assert.deepEqual(blob.calls.put, ["status/heartbeat.json"]);
  });
});

describe("POST /api/heartbeat body", () => {
  beforeEach(() => setEnv({ HEARTBEAT_SECRET: "hb-secret", BLOB_READ_WRITE_TOKEN: undefined }));
  const beat = (body: unknown) => heartbeat.POST(req("POST", "Bearer hb-secret", body));

  it("rejects wrong field types with 400 instead of throwing", async () => {
    for (const body of [{ note: 42 }, { note: { x: 1 } }, { agents: "alpha" }, { agents: [1, 2] }, { agents: ["a", null] }, [], "text", 7]) {
      const res = await beat(body);
      assert.equal(res.status, 400, JSON.stringify(body));
      assert.match((await res.json()).error, /array of strings/);
    }
  });

  it("accepts a null or empty body as a beat with no agents", async () => {
    for (const body of [null, {}]) {
      assert.equal((await beat(body)).status, 200, JSON.stringify(body));
      assert.deepEqual((await (await heartbeat.GET()).json()).agents, []);
    }
  });

  it("records the runner's program interface hash and rejects anything that is not one", async () => {
    assert.equal((await beat({ agents: ["a"], idl: "a5b3a6ec25d5" })).status, 200);
    assert.equal((await (await heartbeat.GET()).json()).idl, "a5b3a6ec25d5");
    for (const idl of [12, "short", "A5B3A6EC25D5", "a5b3a6ec25d5<script>"]) {
      assert.equal((await beat({ agents: ["a"], idl })).status, 400, String(idl));
    }
    // An older runner sends none.
    assert.equal((await beat({ agents: ["a"] })).status, 200);
    assert.equal((await (await heartbeat.GET()).json()).idl, null);
  });

  it("accepts a string or missing note, and caps agents at 20", async () => {
    assert.equal((await beat({ agents: ["a"], note: "degraded" })).status, 200);
    assert.equal((await beat({ agents: ["a"], note: null })).status, 200);
    assert.equal((await beat({ agents: Array.from({ length: 25 }, (_, i) => `a${i}`) })).status, 200);
    assert.equal((await (await heartbeat.GET()).json()).agents.length, 20);
  });
});

describe("GET /api/waitlist/backup auth", () => {
  beforeEach(() => setEnv({ CRON_SECRET: "cron-secret" }));
  const quiet = () => [mock.method(console, "error", () => {}), mock.method(console, "log", () => {})];

  it("is 404 without, or with a wrong, bearer token, and writes nothing", async () => {
    for (const auth of [undefined, "Bearer wrong", "cron-secret"]) {
      assert.equal((await backup.GET(req("GET", auth))).status, 404, String(auth));
    }
    assert.equal(store.snapshots, 0);
  });

  it("is 404 for everyone when CRON_SECRET is unset", async () => {
    setEnv({ CRON_SECRET: undefined });
    assert.equal((await backup.GET(req("GET", "Bearer "))).status, 404);
    assert.equal(store.snapshots, 0);
  });

  it("writes a snapshot with the right token", async () => {
    const q = quiet();
    store.entries = [{} as never, {} as never];
    const res = await backup.GET(req("GET", "Bearer cron-secret"));
    q.forEach((m) => m.mock.restore());
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), { ok: true, count: 2, pathname: "waitlist-snapshots/test.enc" });
    assert.equal(store.snapshots, 1);
  });

  it("fails loudly (non-200 JSON) when storage isn't configured or the snapshot fails", async () => {
    const q = quiet();
    store.configured = false;
    const unconfigured = await backup.GET(req("GET", "Bearer cron-secret"));
    store.configured = true;
    store.failLoad = true;
    const failed = await backup.GET(req("GET", "Bearer cron-secret"));
    q.forEach((m) => m.mock.restore());
    assert.equal(unconfigured.status, 503);
    assert.deepEqual(await unconfigured.json(), { error: "storage not configured" });
    assert.equal(failed.status, 500);
    assert.equal((await failed.json()).ok, false);
  });
});
