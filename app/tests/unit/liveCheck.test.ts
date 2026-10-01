import { freshRequire, setEnv, stubFile } from "./helpers/stubs";
import { beforeEach, describe, it } from "node:test";
import assert from "node:assert/strict";
import { Keypair, PublicKey, VersionedTransaction, type Connection } from "@solana/web3.js";
import idl from "@/lib/idl.json";
import deployed from "@/lib/deployed/devnet.json";
import { DEVNET_CHECK_PAYER, MIN_PAYER_LAMPORTS, buildLifecycle, checkLive, classify, idlHash, type IdlLike, type LiveCheck } from "@/lib/liveCheck";
import { BN } from "@coral-xyz/anchor";

const IDL = idl as unknown as IdlLike;
const payer = Keypair.generate().publicKey;
const errorCode = (name: string) => IDL.errors!.find((e) => e.name === name)!.code;

/** A cluster that answers each simulation from `results`, in order. */
function fakeCluster(results: { err: unknown; logs?: string[] }[], opts: { balance?: number; deployed?: boolean; down?: boolean } = {}) {
  const simulated: VersionedTransaction[] = [];
  const connection = {
    getAccountInfo: async () => {
      if (opts.down) throw new Error("fetch failed");
      return opts.deployed === false ? null : { executable: true };
    },
    getBalance: async () => opts.balance ?? 2e9,
    getLatestBlockhash: async () => ({ blockhash: Keypair.generate().publicKey.toBase58(), lastValidBlockHeight: 1 }),
    simulateTransaction: async (tx: VersionedTransaction, config: { sigVerify: boolean }) => {
      assert.equal(config.sigVerify, false, "the check never needs a signature");
      simulated.push(tx);
      const r = results[simulated.length - 1];
      return { value: { err: r.err, logs: r.logs ?? [] } };
    },
  } as unknown as Connection;
  return { connection, simulated };
}

const lifecycle = () => buildLifecycle(IDL, payer, new BN(7));
const claimIndex = () => lifecycle().claim.findIndex((s) => s.name === "claim_default");
const tooEarly = () => ({ err: { InstructionError: [claimIndex(), { Custom: errorCode("UseSettle") }] } });

describe("buildLifecycle", () => {
  it("covers every instruction the site, the runner and the SDK send", () => {
    const { happy, claim } = lifecycle();
    const covered = new Set([...happy, ...claim].map((s) => s.name));
    const admin = new Set(["init_config", "set_paused", "set_caps", "set_trading_config"]); // sent by `npm run config` only
    admin.add("execute_swap"); // needs a DEX and oracle prices on the cluster; covered by the program's own tests
    admin.add("draw_funds"); // disabled since vault custody; kept so old clients get a clear error
    const missing = IDL.instructions.map((i) => i.name).filter((n) => !covered.has(n) && !admin.has(n));
    assert.deepEqual(missing, [], "add new instructions to buildLifecycle in lib/liveCheck.ts");
  });

  it("puts accounts on the wire in the IDL's order, with its flags", () => {
    const { happy } = lifecycle();
    for (const step of happy) {
      const def = IDL.instructions.find((i) => i.name === step.name)!;
      assert.equal(step.ix.keys.length, def.accounts.length, step.name);
      def.accounts.forEach((a, i) => {
        assert.equal(step.ix.keys[i].isSigner, !!a.signer, `${step.name}.${a.name}`);
        assert.equal(step.ix.keys[i].isWritable, !!a.writable, `${step.name}.${a.name}`);
      });
    }
    const open = happy.find((s) => s.name === "open_position")!;
    const names = IDL.instructions.find((i) => i.name === "open_position")!.accounts.map((a) => a.name);
    const config = PublicKey.findProgramAddressSync([Buffer.from("config")], new PublicKey(IDL.address))[0];
    assert.ok(open.ix.keys[names.indexOf("config")].pubkey.equals(config));
    assert.ok(open.ix.keys[names.indexOf("trader")].pubkey.equals(payer));
  });

  it("fits in one transaction", () => {
    const { connection, simulated } = fakeCluster([{ err: null }, tooEarly()]);
    return checkLive(connection, payer).then(() => {
      for (const tx of simulated) assert.ok(tx.serialize().length <= 1232, `${tx.serialize().length} bytes`);
    });
  });

  it("refuses to guess an account it does not know", () => {
    const next: IdlLike = JSON.parse(JSON.stringify(IDL));
    next.instructions.find((i) => i.name === "begin_trading")!.accounts.push({ name: "oracle" });
    assert.throws(() => buildLifecycle(next, payer, new BN(1)), /begin_trading needs account "oracle"/);
  });

  it("also builds from the IDL of the deployed program", () => {
    // The deployed program predates custody: it draws instead of beginning trading.
    const old = buildLifecycle(deployed as unknown as IdlLike, payer, new BN(1));
    assert.ok(old.happy.some((s) => s.name === "draw_funds"));
    assert.ok(!old.happy.some((s) => s.name === "begin_trading"));
    assert.ok(lifecycle().happy.some((s) => s.name === "begin_trading"));
  });

  it("reads a new instruction the deployed program lacks as blocked, not broken", async () => {
    const at = lifecycle().happy.findIndex((s) => s.name === "begin_trading");
    const r = await checkLive(fakeCluster([{ err: { InstructionError: [at, "InvalidInstructionData"] }, logs: [] }]).connection, payer);
    assert.deepEqual([r.status, r.failedAt], ["blocked", "begin_trading"]);
    assert.match(r.reason, /not in the deployed program yet/);
    // Against a program that has it, the same failure is a real break.
    const same = await checkLive(fakeCluster([{ err: { InstructionError: [at, "InvalidInstructionData"] }, logs: [] }]).connection, payer, IDL, IDL);
    assert.equal(same.status, "broken");
  });
});

describe("classify", () => {
  const steps = lifecycle().happy;
  const at = (name: string) => steps.findIndex((s) => s.name === name);

  it("passes a clean simulation", () => {
    assert.equal(classify(IDL, steps, null, []).kind, "passed");
  });

  it("calls a framework error wiring, with the program's own message", () => {
    const logs = ["Program log: AnchorError caused by account: system_program. Error Code: InvalidProgramId. Error Number: 3008. Error Message: Program ID was not as expected."];
    const r = classify(IDL, steps, { InstructionError: [at("deposit_collateral"), { Custom: 3008 }] }, logs);
    assert.deepEqual([r.kind, r.at], ["wiring", "deposit_collateral"]);
    assert.match((r as { detail: string }).detail, /InvalidProgramId/);
  });

  it("calls missing accounts and unknown instructions wiring", () => {
    assert.equal(classify(IDL, steps, { InstructionError: [at("draw_funds"), "NotEnoughAccountKeys"] }, []).kind, "wiring");
    assert.equal(classify(IDL, steps, { InstructionError: [0, { Custom: 101 }] }, []).kind, "wiring");
    assert.equal(classify(IDL, steps, "BlockhashNotFound", []).kind, "wiring");
  });

  it("calls a program error a refusal, by name", () => {
    const r = classify(IDL, steps, { InstructionError: [at("open_position"), { Custom: errorCode("ProtocolPaused") }] }, []);
    assert.deepEqual([r.kind, r.at, (r as { error: string }).error], ["refused", "open_position", "ProtocolPaused"]);
  });
});

describe("checkLive", () => {
  it("is ok when the lifecycle passes and an early claim is refused as too early", async () => {
    const { connection, simulated } = fakeCluster([{ err: null }, tooEarly()]);
    const r = await checkLive(connection, payer);
    assert.equal(r.status, "ok", r.reason);
    assert.equal(r.idlHash, idlHash());
    assert.equal(simulated.length, 2);
    assert.ok(r.checked.includes("claim_default") && r.checked.includes("open_position"));
  });

  it("is broken when the program rejects the accounts", async () => {
    const logs = ["Program log: AnchorError caused by account: system_program. Error Code: InvalidProgramId. Error Number: 3008. Error Message: Program ID was not as expected."];
    const r = await checkLive(fakeCluster([{ err: { InstructionError: [2, { Custom: 3008 }] }, logs }]).connection, payer);
    assert.deepEqual([r.status, r.failedAt], ["broken", "deposit_collateral"]);
    assert.match(r.reason, /InvalidProgramId/);
  });

  it("says how to fix a missing config", async () => {
    const logs = ["Program log: AnchorError caused by account: config. Error Code: AccountNotInitialized. Error Number: 3012. Error Message: The program expected this account to be already initialized."];
    const r = await checkLive(fakeCluster([{ err: { InstructionError: [2, { Custom: 3012 }] }, logs }]).connection, payer);
    assert.equal(r.status, "broken");
    assert.match(r.reason, /npm run config -- init/);
  });

  it("is blocked, not broken, when the protocol is paused", async () => {
    const r = await checkLive(fakeCluster([{ err: { InstructionError: [2, { Custom: errorCode("ProtocolPaused") }] } }]).connection, payer);
    assert.deepEqual([r.status, r.failedAt], ["blocked", "deposit_collateral"]);
    assert.match(r.reason, /ProtocolPaused/);
  });

  it("is broken when claim_default works before the deadline or fails on its accounts", async () => {
    assert.equal((await checkLive(fakeCluster([{ err: null }, { err: null }]).connection, payer)).status, "broken");
    const r = await checkLive(fakeCluster([{ err: null }, { err: { InstructionError: [claimIndex(), { Custom: 2006 }] } }]).connection, payer);
    assert.deepEqual([r.status, r.failedAt], ["broken", "claim_default"]);
  });

  it("is broken when the program is not deployed", async () => {
    assert.equal((await checkLive(fakeCluster([], { deployed: false }).connection, payer)).status, "broken");
  });

  it("is unknown, never broken, when the check itself cannot run", async () => {
    const poor = await checkLive(fakeCluster([], { balance: MIN_PAYER_LAMPORTS - 1 }).connection, payer);
    assert.equal(poor.status, "unknown");
    assert.match(poor.reason, /needs 0.1/);
    const down = await checkLive(fakeCluster([], { down: true }).connection, payer);
    assert.equal(down.status, "unknown");
    assert.match(down.reason, /fetch failed/);
  });
});

describe("GET /api/health", () => {
  type Route = typeof import("@/app/api/health/route");
  let answers: LiveCheck[] = [];
  let calls: { payer: string }[] = [];
  const result = (status: LiveCheck["status"]): LiveCheck => ({ status, reason: status, idlHash: idlHash(), checked: [] });
  const load = (env: Record<string, string | undefined> = {}) => {
    setEnv({ CHECK_PAYER: undefined, ...env });
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const real = require("@/lib/liveCheck") as typeof import("@/lib/liveCheck");
    stubFile("@/lib/liveCheck", {
      ...real,
      checkLive: async (_c: Connection, p: PublicKey) => {
        calls.push({ payer: p.toBase58() });
        return answers.length > 1 ? answers.shift()! : answers[0];
      },
    });
    return freshRequire<Route>("@/app/api/health/route");
  };
  beforeEach(() => {
    answers = [];
    calls = [];
  });

  it("answers 200 with the status and the IDL hash when trading works", async () => {
    answers = [result("ok")];
    const res = await load().GET();
    assert.equal(res.status, 200);
    assert.equal(res.headers.get("cache-control"), "no-store");
    const body = await res.json();
    assert.deepEqual([body.status, body.idlHash], ["ok", idlHash()]);
    assert.ok(!Number.isNaN(Date.parse(body.checkedAt)));
    assert.equal(calls[0].payer, DEVNET_CHECK_PAYER);
  });

  it("answers 503 only when broken", async () => {
    for (const [status, code] of [["broken", 503], ["blocked", 200], ["unknown", 200]] as const) {
      answers = [result(status)];
      assert.equal((await load().GET()).status, code, status);
    }
  });

  it("checks the cluster once a minute, however often it is asked", async () => {
    answers = [result("ok")];
    const route = load();
    await Promise.all([route.GET(), route.GET(), route.GET()]);
    await route.GET();
    assert.equal(calls.length, 1);
  });

  it("uses CHECK_PAYER when it is set", async () => {
    answers = [result("ok")];
    const other = Keypair.generate().publicKey.toBase58();
    await load({ CHECK_PAYER: other }).GET();
    assert.equal(calls[0].payer, other);
  });
});
