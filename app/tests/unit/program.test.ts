import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { BN } from "@coral-xyz/anchor";
import { Keypair, PublicKey } from "@solana/web3.js";
import idl from "@/lib/idl.json";
import {
  PROGRAM_ID,
  agentPda,
  agentVaultPda,
  capacity,
  decodeConfig,
  fmtDuration,
  maxFeeForRatio,
  feePct,
  pct,
  positionPda,
  positionVaultPda,
  requiredCollateral,
  short,
  sol,
  toLamports,
  type AgentAccount,
} from "@/lib/program";
import { withLocale } from "./helpers/locale";

const key = () => Keypair.generate().publicKey;
const pda = (seeds: (Buffer | Uint8Array)[]) => PublicKey.findProgramAddressSync(seeds, PROGRAM_ID)[0];
/** A decimal u64 as 8 little-endian bytes. */
const le64 = (n: string) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(n));
  return b;
};

/** The constant seed prefix the IDL declares for an account, as text. */
function idlSeed(account: string): string {
  for (const ix of idl.instructions) {
    for (const a of ix.accounts as { name: string; pda?: { seeds: { kind: string; value?: number[] }[] } }[]) {
      if (a.name === account && a.pda) return Buffer.from(a.pda.seeds[0].value!).toString();
    }
  }
  throw new Error(`no pda for ${account}`);
}

describe("PDA helpers", () => {
  it("use the program id from the IDL", () => {
    assert.equal(PROGRAM_ID.toBase58(), idl.address);
  });

  it("use the seed prefixes the IDL declares", () => {
    assert.equal(idlSeed("agent"), "agent");
    assert.equal(idlSeed("agent_vault"), "agent_vault");
    assert.equal(idlSeed("position"), "position");
    assert.equal(idlSeed("position_vault"), "position_vault");
  });

  it("agentPda = [\"agent\", operator, agent_id as u64 LE]", () => {
    const op = key();
    for (const id of ["0", "1", "258", "1099511627783" /* 2^40 + 7 */, "18446744073709551615" /* u64 max */]) {
      const expected = pda([Buffer.from("agent"), op.toBuffer(), le64(id)]);
      assert.ok(agentPda(op, new BN(id)).equals(expected), `id ${id}`);
    }
    assert.ok(!agentPda(op, new BN(1)).equals(agentPda(key(), new BN(1))), "operator is part of the seed");
  });

  it("agentVaultPda = [\"agent_vault\", agent]", () => {
    const agent = key();
    assert.ok(agentVaultPda(agent).equals(pda([Buffer.from("agent_vault"), agent.toBuffer()])));
  });

  it("positionPda = [\"position\", agent, trader, nonce as u64 LE]", () => {
    const agent = key();
    const trader = key();
    for (const nonce of ["0", "5", "8589934592" /* 2^33 */]) {
      const expected = pda([Buffer.from("position"), agent.toBuffer(), trader.toBuffer(), le64(nonce)]);
      assert.ok(positionPda(agent, trader, new BN(nonce)).equals(expected), `nonce ${nonce}`);
    }
    assert.ok(!positionPda(agent, trader, new BN(1)).equals(positionPda(trader, agent, new BN(1))), "order matters");
  });

  it("positionVaultPda = [\"position_vault\", position]", () => {
    const position = key();
    assert.ok(positionVaultPda(position).equals(pda([Buffer.from("position_vault"), position.toBuffer()])));
  });
});

describe("sol()", () => {
  it("formats lamports as SOL with 2 decimals by default", () => {
    withLocale("en-US", () => {
      assert.equal(sol(1_500_000_000), "1.50");
      assert.equal(sol(0), "0.00");
      assert.equal(sol(1), "0.00");
      assert.equal(sol(1_234_567_890_000), "1,234.57");
    });
  });
  it("takes a BN", () => {
    withLocale("en-US", () => {
      assert.equal(sol(new BN("2500000000")), "2.50");
      assert.equal(sol(new BN("123456789012345")), "123,456.79");
    });
  });
  it("honours the digits argument", () => {
    withLocale("en-US", () => {
      assert.equal(sol(1_234_567_890, 3), "1.235");
      assert.equal(sol(1_000_000_000, 0), "1");
      assert.equal(sol(1_999_999_999, 2), "2.00");
    });
  });
  it("follows the viewer's locale for display", () => {
    withLocale("de-DE", () => {
      assert.equal(sol(1_500_000_000), "1,50");
      assert.equal(sol(1_234_567_890_000), "1.234,57");
    });
  });
});

describe("pct()", () => {
  it("formats bps as a percentage", () => {
    withLocale("en-US", () => {
      assert.equal(pct(2_500), "25%");
      assert.equal(pct(10_000), "100%");
      assert.equal(pct(0), "0%");
      assert.equal(pct(1_234), "12%");
      assert.equal(pct(1_234, 1), "12.3%");
      assert.equal(pct(1, 2), "0.01%");
    });
    withLocale("de-DE", () => assert.equal(pct(1_250, 1), "12,5%"));
  });
});

describe("feePct()", () => {
  it("shows the half-percent steps fees move in, without a trailing .0", () => {
    withLocale("en-US", () => {
      assert.equal(feePct(1_550), "15.5%");
      assert.equal(feePct(1_500), "15%");
      assert.equal(feePct(50), "0.5%");
      assert.equal(feePct(0), "0%");
    });
  });
});

describe("on-chain math mirrors", () => {
  it("maxFeeForRatio floors half the ratio", () => {
    assert.equal(maxFeeForRatio(3_000), 1_500);
    assert.equal(maxFeeForRatio(3_001), 1_500);
  });
  it("requiredCollateral rounds up", () => {
    assert.equal(requiredCollateral(1_000_000_000, 3_000), 300_000_000);
    assert.equal(requiredCollateral(1, 3_000), 1);
    assert.equal(requiredCollateral(0, 3_000), 0);
  });

  // 3 SOL bond, 1 SOL reserved, at 50%: free collateral backs 4 SOL.
  const agent = {
    totalCollateral: new BN(3e9),
    lockedCollateral: new BN(1e9),
    capitalManaged: new BN(2e9),
    terms: { collateralRatioBps: 5_000 },
  } as unknown as AgentAccount;
  const open = { paused: false, maxPosition: Number.MAX_SAFE_INTEGER, maxAgentCapital: Number.MAX_SAFE_INTEGER };

  it("capacity is what free collateral backs when there are no caps", () => {
    assert.equal(capacity(agent), 4e9);
    assert.equal(capacity(agent, open), 4e9);
  });
  it("capacity respects the position cap and what is left under the agent cap", () => {
    assert.equal(capacity(agent, { ...open, maxPosition: 1e9 }), 1e9);
    assert.equal(capacity(agent, { ...open, maxAgentCapital: 5e9 }), 3e9);
    // An agent already above a lowered cap has no room, not negative room.
    assert.equal(capacity(agent, { ...open, maxAgentCapital: 1e9 }), 0);
  });
  it("decodeConfig turns u64::MAX (no cap) into the largest safe number", () => {
    const max = new BN("18446744073709551615");
    assert.deepEqual(decodeConfig({ paused: true, maxPosition: max, maxAgentCapital: new BN(5e9) }), {
      paused: true,
      maxPosition: Number.MAX_SAFE_INTEGER,
      maxAgentCapital: 5e9,
    });
  });
});

describe("misc formatting", () => {
  it("toLamports rounds to the nearest lamport", () => {
    assert.equal(toLamports(0.1 + 0.2), 300_000_000);
    assert.equal(toLamports(1.999), 1_999_000_000);
  });
  it("short()", () => {
    assert.equal(short("So11111111111111111111111111111111111111112"), "So11…1112");
  });
  it("fmtDuration()", () => {
    assert.equal(fmtDuration(86_400), "1 day");
    assert.equal(fmtDuration(7 * 86_400), "7 days");
    assert.equal(fmtDuration(3_600), "1 hour");
    assert.equal(fmtDuration(7_200), "2 hours");
    assert.equal(fmtDuration(120), "2 min");
    assert.equal(fmtDuration(61), "61s");
  });
});
