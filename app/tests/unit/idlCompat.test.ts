import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import idl from "@/lib/idl.json";
import deployed from "@/lib/deployed/devnet.json";
import { compareIdl, type CompatIdl } from "@/lib/idlCompat";

const current = idl as unknown as CompatIdl;
const live = deployed as unknown as CompatIdl;
const copy = (i: CompatIdl): CompatIdl => JSON.parse(JSON.stringify(i));
const ix = (i: CompatIdl, name: string) => i.instructions.find((x) => x.name === name)!;

describe("the app's IDL against the program deployed on devnet", () => {
  it("works with the deployed program: deploying the site, runner or SDK is safe", () => {
    const { breaking } = compareIdl(live, current);
    assert.deepEqual(breaking, [], "clients built from app/lib/idl.json would fail on devnet. See app/lib/deployed/README.md.");
  });

  it("the SDK ships the same IDL as the app", () => {
    const sdk = JSON.parse(readFileSync(join(__dirname, "../../../sdk/idl/proof_of_agent.json"), "utf8"));
    assert.deepEqual(sdk, idl);
  });
});

describe("compareIdl", () => {
  it("finds nothing between identical IDLs", () => {
    assert.deepEqual(compareIdl(live, copy(live)), { breaking: [], needsUpgrade: [], clientsFirst: [] });
  });

  it("rejects an account added before an existing one (the 2026-09-29 outage)", () => {
    const next = copy(live);
    const accounts = ix(next, "open_position").accounts;
    accounts.splice(accounts.findIndex((a) => a.name === "system_program"), 0, { name: "config" });
    const { breaking } = compareIdl(live, next);
    assert.equal(breaking.length, 1);
    assert.match(breaking[0], /open_position: account 5 is now config, but the deployed program expects system_program/);
  });

  it("accepts an account added at the end, and says clients must be deployed before the program", () => {
    const next = copy(live);
    ix(next, "draw_funds").accounts.push({ name: "config" });
    const r = compareIdl(live, next);
    assert.deepEqual(r.breaking, []);
    assert.match(r.needsUpgrade[0], /draw_funds: sends config at the end/);
    assert.match(r.clientsFirst[0], /draw_funds: the upgraded program requires config/);
  });

  it("rejects removed or reordered accounts, changed flags, arguments and discriminators", () => {
    for (const change of [
      (n: CompatIdl) => ix(n, "settle_position").accounts.pop(),
      (n: CompatIdl) => ix(n, "settle_position").accounts.reverse(),
      (n: CompatIdl) => (ix(n, "cancel_position").accounts[0].signer = false),
      (n: CompatIdl) => (ix(n, "cancel_position").accounts[1].writable = false),
      (n: CompatIdl) => ix(n, "open_position").args.pop(),
      (n: CompatIdl) => (ix(n, "open_position").discriminator[0] ^= 1),
    ]) {
      const next = copy(live);
      change(next);
      assert.notEqual(compareIdl(live, next).breaking.length, 0, change.toString());
    }
  });

  it("rejects a changed account layout, account discriminator or error code", () => {
    const fields = (n: CompatIdl, name: string) => (n.types!.find((t) => t.name === name) as unknown as { type: { fields: unknown[] } }).type.fields;
    for (const change of [
      (n: CompatIdl) => fields(n, "Agent").splice(2, 0, { name: "extra", type: "u64" }),
      (n: CompatIdl) => fields(n, "Position").pop(),
      (n: CompatIdl) => (n.accounts![0].discriminator[0] ^= 1),
      (n: CompatIdl) => n.errors!.splice(3, 1),
    ]) {
      const next = copy(live);
      change(next);
      assert.notEqual(compareIdl(live, next).breaking.length, 0, change.toString());
    }
  });

  it("lists new instructions, types and errors as waiting for the upgrade, not as breaking", () => {
    const next = copy(live);
    next.instructions.push({ name: "set_paused", discriminator: [1, 2, 3, 4, 5, 6, 7, 8], accounts: [{ name: "admin", signer: true }], args: [] });
    next.types!.push({ name: "Config" });
    next.errors!.push({ code: 6999, name: "Brand new" });
    const r = compareIdl(live, next);
    assert.deepEqual(r.breaking, []);
    assert.match(r.needsUpgrade[0], /set_paused is new/);
  });
});
