/**
 * Seed a local validator with demo agents so the ledger has something to show.
 *   npm run seed                     -> 4 published agents
 *   npm run fund -- <wallet-address> -> airdrop 10 SOL to your browser wallet
 */
import { AnchorProvider, BN, Idl, Program, Wallet } from "@coral-xyz/anchor";
import { Connection, Keypair, LAMPORTS_PER_SOL, PublicKey, SystemProgram } from "@solana/web3.js";
import idl from "../lib/idl.json";

const RPC = process.env.NEXT_PUBLIC_RPC_URL ?? "http://127.0.0.1:8899";
const PROGRAM_ID = new PublicKey(idl.address);
const SOL_MINT = new PublicKey("So11111111111111111111111111111111111111112");
const USDC_MINT = new PublicKey("BRjpCHtyQLNCo8gqRUr8jtdAj5AjPYQaoqbvcZiHok1k");
const JUP_MINT = new PublicKey("JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN");
const HOUR = 3600;
const DAY = 24 * HOUR;

const AGENTS = [
  { name: "Momentum Bot", description: "SOL/USDC 4h momentum, trend-following", ratio: 3000, fee: 1500, drawdown: 2000, window: [HOUR, 7 * DAY], assets: [SOL_MINT, USDC_MINT], bond: 20,
    rules: "Trades SOL/USDC only.\nEnters SOL when the 4h trend is up, exits to USDC when it turns down.\nNever more than 80% of a position in one asset.\nNo leverage. Settles before the deadline." },
  { name: "Basis Harvester", description: "Perp funding-rate carry, delta neutral", ratio: 7500, fee: 2500, drawdown: 1000, window: [DAY, 30 * DAY], assets: [SOL_MINT, USDC_MINT], bond: 45,
    rules: "Holds spot SOL against a short perp of equal size to earn funding.\nCloses both legs if funding turns negative for 24h.\nNo directional exposure beyond 5%." },
  { name: "Degen Sniper", description: "New-launch sniping, high variance", ratio: 1000, fee: 500, drawdown: 5000, window: [HOUR, DAY], assets: [SOL_MINT, USDC_MINT, JUP_MINT], bond: 4,
    rules: "Buys newly listed tokens in the first minutes of trading.\nExits within 6 hours.\nExpect large swings; losses up to 50% are within terms." },
  { name: "Vault Keeper", description: "Fully bonded LST yield rotation", ratio: 10000, fee: 3000, drawdown: 0, window: [7 * DAY, 90 * DAY], assets: [SOL_MINT], bond: 60,
    rules: "Rotates between liquid staking tokens for the best yield.\nPrincipal stays in SOL terms.\nFully collateralised: every SOL deposited is backed by a SOL of bond." },
];

async function main() {
  const connection = new Connection(RPC, "confirmed");
  const [, , cmd, arg] = process.argv;

  if (cmd === "fund") {
    const to = new PublicKey(arg);
    const sig = await connection.requestAirdrop(to, 10 * LAMPORTS_PER_SOL);
    await connection.confirmTransaction(sig, "confirmed");
    console.log(`funded ${to.toBase58()} with 10 SOL`);
    return;
  }

  for (const a of AGENTS) {
    const operator = Keypair.generate();
    const sig = await connection.requestAirdrop(operator.publicKey, (a.bond + 2) * LAMPORTS_PER_SOL);
    await connection.confirmTransaction(sig, "confirmed");

    const provider = new AnchorProvider(connection, new Wallet(operator), { commitment: "confirmed" });
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const program = new Program(idl as Idl, provider) as any;
    const agentId = new BN(1);
    const agent = PublicKey.findProgramAddressSync(
      [Buffer.from("agent"), operator.publicKey.toBuffer(), agentId.toArrayLike(Buffer, "le", 8)],
      PROGRAM_ID,
    )[0];
    const agentVault = PublicKey.findProgramAddressSync([Buffer.from("agent_vault"), agent.toBuffer()], PROGRAM_ID)[0];
    const terms = {
      collateralRatioBps: a.ratio,
      feeBps: a.fee,
      maxDrawdownBps: a.drawdown,
      minDurationSecs: new BN(a.window[0]),
      maxDurationSecs: new BN(a.window[1]),
      allowedAssets: a.assets,
      rules: a.rules,
    };
    const accounts = { operator: operator.publicKey, agent, agentVault, systemProgram: SystemProgram.programId };
    await program.methods.createAgent(agentId, a.name, a.description, terms).accounts(accounts).rpc();
    await program.methods.depositCollateral(new BN(a.bond * LAMPORTS_PER_SOL)).accounts(accounts).rpc();
    await program.methods.publishAgent().accounts({ operator: operator.publicKey, agent }).rpc();
    console.log(`${a.name.padEnd(16)} ratio ${a.ratio / 100}%  fee ${a.fee / 100}%  bond ${a.bond} SOL  operator ${operator.publicKey.toBase58()}`);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
