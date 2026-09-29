/**
 * Integration tests against a running local validator with the program deployed.
 *
 *   npm run validator        (in another terminal)
 *   npm run deploy:local
 *   npm run test:localnet
 *
 * Unlike the LiteSVM tests in programs/, these go through the real RPC and the
 * TypeScript client the frontend uses, so they also cover the IDL + PDA plumbing.
 */
import { describe, it, before } from "node:test";
import assert from "node:assert/strict";
import { AnchorProvider, BN, Idl, Program, Wallet } from "@coral-xyz/anchor";
import {
  Connection, Keypair, LAMPORTS_PER_SOL, PublicKey, SystemProgram, Transaction, sendAndConfirmTransaction,
} from "@solana/web3.js";
import idl from "../lib/idl.json";
import { agentPda, agentVaultPda, positionPda, positionVaultPda } from "../lib/program";
import { U64_MAX, ensureConfig, loadAdmin, setCaps, setPaused } from "../scripts/protocolConfig";

const RPC = process.env.NEXT_PUBLIC_RPC_URL ?? "http://127.0.0.1:8899";
const SOL = LAMPORTS_PER_SOL;
const PROGRAM_ID = new PublicKey(idl.address);
const connection = new Connection(RPC, "confirmed");

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Prog = any;

function programFor(kp: Keypair): Prog {
  const provider = new AnchorProvider(connection, new Wallet(kp), { commitment: "confirmed" });
  return new Program(idl as Idl, provider);
}

async function fund(kp: Keypair, sol = 20) {
  const sig = await connection.requestAirdrop(kp.publicKey, sol * SOL);
  await connection.confirmTransaction(sig, "confirmed");
}

async function expectAnchorError(p: Promise<unknown>, code: string) {
  try {
    await p;
  } catch (e) {
    const err = e as { error?: { errorCode?: { code?: string } }; message?: string };
    const got = err?.error?.errorCode?.code ?? err?.message ?? String(e);
    assert.ok(got.includes(code), `expected error ${code}, got: ${got}`);
    return;
  }
  assert.fail(`expected error ${code}, but the transaction succeeded`);
}

const balance = (k: PublicKey) => connection.getBalance(k, "confirmed");

/** Move every lamport out of `kp`, with `payer` paying the fee, so the wallet ends at 0. */
async function emptyWallet(kp: Keypair, payer: Keypair) {
  const lamports = await balance(kp.publicKey);
  const tx = new Transaction().add(
    SystemProgram.transfer({ fromPubkey: kp.publicKey, toPubkey: payer.publicKey, lamports }),
  );
  tx.feePayer = payer.publicKey;
  await sendAndConfirmTransaction(connection, tx, [payer, kp], { commitment: "confirmed" });
  assert.equal(await balance(kp.publicKey), 0);
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const SOL_MINT = new PublicKey("So11111111111111111111111111111111111111112");
const USDC_MINT = new PublicKey("BRjpCHtyQLNCo8gqRUr8jtdAj5AjPYQaoqbvcZiHok1k");

function termsArg(ratioBps: number, feeBps: number, drawdownBps: number) {
  return {
    collateralRatioBps: ratioBps,
    feeBps,
    maxDrawdownBps: drawdownBps,
    minDurationSecs: new BN(60),
    maxDurationSecs: new BN(7 * 24 * 3600),
    allowedAssets: [SOL_MINT, USDC_MINT],
    rules: "Trade SOL/USDC only.",
  };
}

/**
 * A fresh operator + trader pair. By default the agent is created, bonded and
 * published with a 30% ratio, 15% fee and 20% tolerance.
 */
async function setup(ratioBps = 3000, drawdownBps = 2000, bondSol = 1, publish = true) {
  const agentKp = Keypair.generate();
  const traderKp = Keypair.generate();
  await Promise.all([fund(agentKp), fund(traderKp)]);
  const agentId = new BN(1);
  const agent = agentPda(agentKp.publicKey, agentId);
  const agentVault = agentVaultPda(agent);
  const ap = programFor(agentKp);
  const tp = programFor(traderKp);
  const opAccounts = { operator: agentKp.publicKey, agent, agentVault, systemProgram: SystemProgram.programId };

  await ap.methods
    .createAgent(agentId, "Test Agent", "integration", termsArg(ratioBps, Math.floor(ratioBps / 2), drawdownBps))
    .accounts(opAccounts)
    .rpc();
  if (bondSol > 0) {
    await ap.methods.depositCollateral(new BN(bondSol * SOL)).accounts(opAccounts).rpc();
  }
  if (publish && bondSol > 0) {
    await ap.methods.publishAgent().accounts({ operator: agentKp.publicKey, agent }).rpc();
  }

  let nonceCounter = 0;
  const env = {
    agentKp, traderKp, agent, agentVault, ap, tp,
    agentState: () => ap.account.agent.fetch(agent),
    positionState: (position: PublicKey) => tp.account.position.fetch(position),
    open: async (lamports: number, durationSecs: number, program: Prog = tp, trader: PublicKey = traderKp.publicKey) => {
      const nonce = new BN(Date.now() + nonceCounter++);
      const position = positionPda(agent, trader, nonce);
      await program.methods
        .openPosition(nonce, new BN(lamports), new BN(durationSecs))
        .accounts({ trader, agent, position, positionVault: positionVaultPda(position), systemProgram: SystemProgram.programId })
        .rpc();
      return position;
    },
    draw: (position: PublicKey, program: Prog = ap, executor: PublicKey = agentKp.publicKey) =>
      program.methods
        .drawFunds()
        .accounts({ executor, agent, position, positionVault: positionVaultPda(position), systemProgram: SystemProgram.programId })
        .rpc(),
    settle: (position: PublicKey, returned: number, program: Prog = ap, executor: PublicKey = agentKp.publicKey) =>
      program.methods
        .settlePosition(new BN(returned))
        .accounts({
          executor, operator: agentKp.publicKey, agent, agentVault, position,
          positionVault: positionVaultPda(position), trader: traderKp.publicKey, systemProgram: SystemProgram.programId,
        })
        .rpc(),
    publish: () => ap.methods.publishAgent().accounts({ operator: agentKp.publicKey, agent }).rpc(),
    bindExecutor: (executor: PublicKey) =>
      ap.methods.setExecutor(executor).accounts({ operator: agentKp.publicKey, agent }).rpc(),
    cancel: (position: PublicKey) =>
      tp.methods
        .cancelPosition()
        .accounts({ trader: traderKp.publicKey, agent, position, positionVault: positionVaultPda(position), systemProgram: SystemProgram.programId })
        .rpc(),
    claimDefault: (position: PublicKey) =>
      tp.methods
        .claimDefault()
        .accounts({
          trader: traderKp.publicKey, agent, agentVault, position,
          positionVault: positionVaultPda(position), systemProgram: SystemProgram.programId,
        })
        .rpc(),
    withdraw: (lamports: number) =>
      ap.methods.withdrawCollateral(new BN(lamports)).accounts(opAccounts).rpc(),
  };
  return env;
}

describe("proof_of_agent on localnet", () => {
  let rentFloor: number;

  before(async () => {
    const info = await connection.getAccountInfo(PROGRAM_ID);
    assert.ok(info?.executable, `program ${PROGRAM_ID} is not deployed at ${RPC}. Run: npm run deploy:local`);
    rentFloor = await connection.getMinimumBalanceForRentExemption(0);
    // The CLI wallet deployed the program, so it is the upgrade authority and admin.
    await ensureConfig(connection, loadAdmin());
  });

  it("creates a draft with published terms, then publishes once bonded", async () => {
    const env = await setup(3000, 2000, 0, false);
    let a = await env.agentState();
    assert.deepEqual(a.status, { draft: {} });
    assert.equal(a.terms.collateralRatioBps, 3000);
    assert.equal(a.terms.feeBps, 1500);
    assert.equal(a.terms.allowedAssets.length, 2);
    assert.ok(a.operator.equals(env.agentKp.publicKey));
    await expectAnchorError(env.open(0.1 * SOL, 3600), "AgentNotAccepting");
    await expectAnchorError(env.publish(), "NoCollateral");
    await env.ap.methods
      .depositCollateral(new BN(SOL))
      .accounts({ operator: env.agentKp.publicKey, agent: env.agent, agentVault: env.agentVault, systemProgram: SystemProgram.programId })
      .rpc();
    await env.publish();
    a = await env.agentState();
    assert.deepEqual(a.status, { active: {} });
    assert.ok(a.publishedAt.toNumber() > 0);
  });

  it("rejects a fee above the collateral cap", async () => {
    const kp = Keypair.generate();
    await fund(kp, 2);
    const agentId = new BN(1);
    const agent = agentPda(kp.publicKey, agentId);
    await expectAnchorError(
      programFor(kp).methods
        .createAgent(agentId, "Bad", "", termsArg(3000, 1600, 1000))
        .accounts({ operator: kp.publicKey, agent, agentVault: agentVaultPda(agent), systemProgram: SystemProgram.programId })
        .rpc(),
      "FeeTooHigh",
    );
  });

  it("locks principal × ratio and refuses positions the agent cannot back", async () => {
    const env = await setup(3000, 2000, 1);
    // 1 SOL bond at 30% backs at most 3.33 SOL
    await expectAnchorError(env.open(4 * SOL, 3600), "InsufficientFreeCollateral");

    const position = await env.open(1 * SOL, 3600);
    const a = await env.agentState();
    assert.equal(a.lockedCollateral.toNumber(), 0.3 * SOL);
    assert.equal(a.capitalManaged.toNumber(), 1 * SOL);
    assert.equal(a.openPositions, 1);

    const p = await env.positionState(position);
    assert.equal(p.principal.toNumber(), 1 * SOL);
    assert.equal(p.lockedCollateral.toNumber(), 0.3 * SOL);
    assert.deepEqual(p.status, { open: {} });
    assert.equal(await balance(positionVaultPda(position)), 1 * SOL + rentFloor);

    // locked collateral cannot leave the vault
    await expectAnchorError(env.withdraw(0.8 * SOL), "InsufficientFreeCollateral");
    await env.withdraw(0.7 * SOL);
    assert.equal((await env.agentState()).totalCollateral.toNumber(), 0.3 * SOL);
  });

  it("pays a performance fee on profit and returns the rest to the trader", async () => {
    const env = await setup(3000, 2000, 1);
    const position = await env.open(1 * SOL, 3600);
    await env.draw(position);
    assert.deepEqual((await env.positionState(position)).status, { trading: {} });
    assert.equal(await balance(positionVaultPda(position)), rentFloor);

    const traderBefore = await balance(env.traderKp.publicKey);
    await env.settle(position, 1.2 * SOL); // +20%: fee = 15% of 0.2 = 0.03

    const p = await env.positionState(position);
    assert.deepEqual(p.status, { settled: {} });
    assert.equal(p.feePaid.toNumber(), 0.03 * SOL);
    assert.equal(p.slashed.toNumber(), 0);
    assert.equal((await balance(env.traderKp.publicKey)) - traderBefore, 1.17 * SOL + rentFloor);
    assert.equal(await balance(positionVaultPda(position)), 0); // vault fully drained

    const a = await env.agentState();
    assert.equal(a.lockedCollateral.toNumber(), 0);
    assert.equal(a.totalCollateral.toNumber(), 1 * SOL);
    assert.equal(a.settledPositions, 1);
    assert.equal(a.feesEarned.toNumber(), 0.03 * SOL);
  });

  it("does not slash a loss inside the declared drawdown", async () => {
    const env = await setup(3000, 2000, 1);
    const position = await env.open(1 * SOL, 3600);
    await env.draw(position);
    await env.settle(position, 0.85 * SOL); // -15%, allowed -20%
    const p = await env.positionState(position);
    assert.equal(p.slashed.toNumber(), 0);
    assert.equal(p.feePaid.toNumber(), 0);
    assert.equal((await env.agentState()).totalCollateral.toNumber(), 1 * SOL);
  });

  it("slashes the shortfall beyond the drawdown, capped at the locked guarantee", async () => {
    const env = await setup(3000, 2000, 1);
    const position = await env.open(1 * SOL, 3600);
    await env.draw(position);
    const traderBefore = await balance(env.traderKp.publicKey);

    await env.settle(position, 0); // lost everything: shortfall 0.8, guarantee 0.3
    const p = await env.positionState(position);
    assert.equal(p.slashed.toNumber(), 0.3 * SOL);
    assert.deepEqual(p.breach, { drawdown: {} });
    assert.equal((await balance(env.traderKp.publicKey)) - traderBefore, 0.3 * SOL + rentFloor);

    const a = await env.agentState();
    assert.equal(a.totalCollateral.toNumber(), 0.7 * SOL);
    assert.equal(a.slashedTotal.toNumber(), 0.3 * SOL);
    assert.equal(a.lockedCollateral.toNumber(), 0);
    assert.equal(await balance(env.agentVault), 0.7 * SOL + rentFloor);
  });

  it("lets the trader cancel before the agent draws, with a full refund", async () => {
    const env = await setup(3000, 2000, 1);
    const position = await env.open(1 * SOL, 3600);
    const before = await balance(env.traderKp.publicKey);
    await env.cancel(position);
    // trader paid the 5000-lamport tx fee for the cancel itself
    assert.equal((await balance(env.traderKp.publicKey)) - before + 5000, 1 * SOL + rentFloor);
    assert.deepEqual((await env.positionState(position)).status, { cancelled: {} });
    assert.equal((await env.agentState()).lockedCollateral.toNumber(), 0);
    await expectAnchorError(env.cancel(position), "InvalidStatus");
    await expectAnchorError(env.draw(position), "InvalidStatus");
  });

  it("lets a bound trading key draw and settle, and refuses anyone else", async () => {
    const env = await setup(3000, 2000, 1);
    const position = await env.open(1 * SOL, 3600);
    const impostor = Keypair.generate();
    const executor = Keypair.generate();
    await Promise.all([fund(impostor, 2), fund(executor, 3)]);
    await expectAnchorError(env.draw(position, programFor(impostor), impostor.publicKey), "UnauthorizedExecutor");
    await env.bindExecutor(executor.publicKey);
    const ex = programFor(executor);
    await env.draw(position, ex, executor.publicKey);
    const opBefore = await balance(env.agentKp.publicKey);
    await env.settle(position, 1.2 * SOL, ex, executor.publicKey);
    // 15% of the 0.2 profit goes to the operator wallet.
    assert.equal((await balance(env.agentKp.publicKey)) - opBefore, 0.03 * SOL);
  });

  it("refuses new positions while the agent is paused", async () => {
    const env = await setup(3000, 2000, 1);
    await env.ap.methods.setAccepting(false).accounts({ operator: env.agentKp.publicKey, agent: env.agent }).rpc();
    await expectAnchorError(env.open(0.1 * SOL, 3600), "AgentNotAccepting");
    await env.ap.methods.setAccepting(true).accounts({ operator: env.agentKp.publicKey, agent: env.agent }).rpc();
    await env.open(0.1 * SOL, 3600);
  });

  it("enforces the protocol caps and pause switch set by the upgrade authority", async () => {
    const admin = loadAdmin();
    const env = await setup(3000, 2000, 1);
    try {
      await setCaps(connection, admin, new BN(0.2 * SOL), new BN(0.3 * SOL));
      await expectAnchorError(env.open(0.2 * SOL + 1, 3600), "PositionTooLarge");
      await env.open(0.2 * SOL, 3600);
      await expectAnchorError(env.open(0.2 * SOL, 3600), "AgentCapReached");
      await setPaused(connection, admin, true);
      await expectAnchorError(env.open(0.1 * SOL, 3600), "ProtocolPaused");
      // A stranger cannot resume.
      const stranger = Keypair.generate();
      await fund(stranger, 1);
      await expectAnchorError(setPaused(connection, stranger, false), "UnauthorizedAdmin");
    } finally {
      await setPaused(connection, admin, false);
      await setCaps(connection, admin, U64_MAX, U64_MAX);
    }
    await env.open(0.1 * SOL, 3600);
  });

  it("pays the whole guarantee to the trader when the agent misses the deadline (waits ~65s)", { timeout: 120_000 }, async () => {
    const env = await setup(3000, 2000, 1);
    const position = await env.open(1 * SOL, 60); // minimum duration
    await env.draw(position);
    await expectAnchorError(env.claimDefault(position), "DeadlineNotReached");

    const deadline = (await env.positionState(position)).deadline.toNumber();
    const wait = deadline * 1000 - Date.now() + 5_000;
    await sleep(Math.max(wait, 0));

    const before = await balance(env.traderKp.publicKey);
    await env.claimDefault(position);
    assert.equal((await balance(env.traderKp.publicKey)) - before + 5000, 0.3 * SOL + rentFloor);

    const p = await env.positionState(position);
    assert.deepEqual(p.status, { defaulted: {} });
    assert.deepEqual(p.breach, { missedDeadline: {} });
    assert.equal(p.slashed.toNumber(), 0.3 * SOL);
    const a = await env.agentState();
    assert.equal(a.defaultedPositions, 1);
    assert.equal(a.breachCount, 1);
    assert.equal(a.totalCollateral.toNumber(), 0.7 * SOL);
    assert.equal(a.lockedCollateral.toNumber(), 0);
    // the agent cannot settle a defaulted position afterwards
    await expectAnchorError(env.settle(position, 1 * SOL), "InvalidStatus");
  });

  // The runtime refuses a transaction that leaves a 0-data account holding
  // lamports below the rent-exempt minimum. LiteSVM skips that check for
  // accounts without data, so these cases only run against a real validator.
  it("settles a dust fee when the operator wallet is empty: the trader keeps the fee", async () => {
    const env = await setup(3000, 2000, 1);
    const executor = Keypair.generate();
    await fund(executor, 3);
    await env.bindExecutor(executor.publicKey);
    const ex = programFor(executor);
    const position = await env.open(1 * SOL, 3600);
    await env.draw(position, ex, executor.publicKey);
    await emptyWallet(env.agentKp, executor);

    const traderBefore = await balance(env.traderKp.publicKey);
    await env.settle(position, 1 * SOL + 1_000_000, ex, executor.publicKey); // fee would be 150_000
    assert.equal(await balance(env.agentKp.publicKey), 0);
    assert.equal((await balance(env.traderKp.publicKey)) - traderBefore, 1 * SOL + 1_000_000 + rentFloor);
    const p = await env.positionState(position);
    assert.deepEqual(p.status, { settled: {} });
    assert.equal(p.feePaid.toNumber(), 0);
    assert.equal((await env.agentState()).feesEarned.toNumber(), 0);
  });

  it("pays a fee above the rent floor to an empty operator wallet", async () => {
    const env = await setup(3000, 2000, 1);
    const executor = Keypair.generate();
    await fund(executor, 3);
    await env.bindExecutor(executor.publicKey);
    const ex = programFor(executor);
    const position = await env.open(1 * SOL, 3600);
    await env.draw(position, ex, executor.publicKey);
    await emptyWallet(env.agentKp, executor);

    await env.settle(position, 1.2 * SOL, ex, executor.publicKey);
    assert.equal(await balance(env.agentKp.publicKey), 0.03 * SOL);
    assert.equal((await env.positionState(position)).feePaid.toNumber(), 0.03 * SOL);
  });

  it("pays a dust slash to an empty trader wallet", async () => {
    const env = await setup(3000, 2000, 1);
    const position = await env.open(1 * SOL, 3600);
    await env.draw(position);
    await emptyWallet(env.traderKp, env.agentKp);

    await env.settle(position, 0.8 * SOL - 1); // 1 lamport below the floor
    const p = await env.positionState(position);
    assert.equal(p.slashed.toNumber(), 1);
    assert.equal(await balance(env.traderKp.publicKey), 0.8 * SOL - 1 + 1 + rentFloor);
  });

  it("pays a default to an empty trader wallet (waits ~65s)", { timeout: 120_000 }, async () => {
    const env = await setup(3000, 2000, 1);
    const position = await env.open(1 * SOL, 60);
    await env.draw(position);
    const payer = Keypair.generate();
    await fund(payer, 1);
    await emptyWallet(env.traderKp, payer);

    const deadline = (await env.positionState(position)).deadline.toNumber();
    await sleep(Math.max(deadline * 1000 - Date.now() + 5_000, 0));
    const tx = await env.tp.methods
      .claimDefault()
      .accounts({
        trader: env.traderKp.publicKey, agent: env.agent, agentVault: env.agentVault, position,
        positionVault: positionVaultPda(position), systemProgram: SystemProgram.programId,
      })
      .transaction();
    tx.feePayer = payer.publicKey;
    await sendAndConfirmTransaction(connection, tx, [payer, env.traderKp], { commitment: "confirmed" });
    assert.equal(await balance(env.traderKp.publicKey), 0.3 * SOL + rentFloor);
  });
});
