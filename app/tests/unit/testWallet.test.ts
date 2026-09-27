import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { Keypair, SystemProgram, Transaction } from "@solana/web3.js";
import { loadOrCreateKeypair, TestWalletAdapter, testWalletStorageKey } from "@/lib/testWallet";

const memoryStorage = () => {
  const m = new Map<string, string>();
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v), removeItem: (k: string) => void m.delete(k), m };
};

describe("test wallet", () => {
  it("keeps the same key across loads, per cluster", () => {
    const s = memoryStorage();
    const a = loadOrCreateKeypair(s, testWalletStorageKey("devnet"));
    const b = loadOrCreateKeypair(s, testWalletStorageKey("devnet"));
    const c = loadOrCreateKeypair(s, testWalletStorageKey("localnet"));
    assert.ok(a.publicKey.equals(b.publicKey));
    assert.ok(!a.publicKey.equals(c.publicKey));
  });

  it("replaces an unreadable entry and still works without storage", () => {
    const s = memoryStorage();
    s.setItem(testWalletStorageKey("devnet"), "not json");
    const kp = loadOrCreateKeypair(s, testWalletStorageKey("devnet"));
    assert.deepEqual(JSON.parse(s.getItem(testWalletStorageKey("devnet"))!), Array.from(kp.secretKey));
    assert.ok(loadOrCreateKeypair(null, "x").publicKey);
    const throwing = { getItem: () => { throw new Error("blocked"); }, setItem: () => { throw new Error("blocked"); }, removeItem: () => {} };
    assert.ok(loadOrCreateKeypair(throwing, "x").publicKey);
  });

  it("reconnects to the same wallet after a disconnect and signs transactions", async () => {
    const s = memoryStorage();
    const w = new TestWalletAdapter("devnet", s);
    await w.connect();
    const first = w.publicKey!;
    await w.disconnect();
    assert.equal(w.publicKey, null);
    await w.connect();
    assert.ok(w.publicKey!.equals(first));

    const tx = new Transaction({ feePayer: first, recentBlockhash: Keypair.generate().publicKey.toBase58() }).add(
      SystemProgram.transfer({ fromPubkey: first, toPubkey: Keypair.generate().publicKey, lamports: 1 }),
    );
    const signed = await w.signTransaction(tx);
    assert.ok(signed.verifySignatures());
  });
});
