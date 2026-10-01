/**
 * Minimal client for the Proof of Agent program. Uses only the public IDL and
 * program id, so it works for any operator without access to our app code.
 */
import { randomBytes } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import anchor, { AnchorProvider, Program, Wallet, type BN as AnchorBN, type Idl } from "@coral-xyz/anchor";
import { ComputeBudgetProgram, Connection, Keypair, PublicKey, SystemProgram, Transaction, type TransactionInstruction } from "@solana/web3.js";

// Node before 26 cannot see BN as a named export of anchor's CommonJS build.
export const BN = anchor.BN;
export type BN = AnchorBN;

export const IDL = JSON.parse(readFileSync(fileURLToPath(new URL("../idl/proof_of_agent.json", import.meta.url)), "utf8")) as Idl & { address: string };
export const PROGRAM_ID = new PublicKey(IDL.address);
export const SYSTEM = SystemProgram.programId;
export const LAMPORTS = 1_000_000_000;

/** Solana's base fee per signature, in lamports. Every transaction here has one signature. */
export const SIGNATURE_FEE = 5_000;
/** Compute units a settle requests: the default for one instruction, so the limit is what `.rpc()` had. The priority fee is price × this. */
export const SETTLE_COMPUTE_UNITS = 200_000;
/** Default priority fee for a settle, in micro-lamports per compute unit (200 lamports at SETTLE_COMPUTE_UNITS). */
export const DEFAULT_PRIORITY_MICROLAMPORTS = 1_000;
/** Default priority fee for a settle close to the deadline (10,000 lamports at SETTLE_COMPUTE_UNITS). */
export const DEFAULT_URGENT_PRIORITY_MICROLAMPORTS = 50_000;

/** The priority fee in lamports for `microLamports` per compute unit, rounded up as the runtime does. */
export const priorityFeeLamports = (microLamports: number, units = SETTLE_COMPUTE_UNITS) => Math.ceil((microLamports * units) / 1_000_000);

const seed = (s: string) => Buffer.from(s);
export const agentPda = (operator: PublicKey, agentId: number | BN) =>
  PublicKey.findProgramAddressSync([seed("agent"), operator.toBuffer(), new BN(agentId).toArrayLike(Buffer, "le", 8)], PROGRAM_ID)[0];
export const agentVaultPda = (agent: PublicKey) => PublicKey.findProgramAddressSync([seed("agent_vault"), agent.toBuffer()], PROGRAM_ID)[0];
/** Protocol config: the pause switch and the position and agent caps. */
export const configPda = () => PublicKey.findProgramAddressSync([seed("config")], PROGRAM_ID)[0];
export const positionVaultPda = (position: PublicKey) => PublicKey.findProgramAddressSync([seed("position_vault"), position.toBuffer()], PROGRAM_ID)[0];
/** Exists while a position trades under vault custody; closed when it settles. */
export const custodyPda = (position: PublicKey) => PublicKey.findProgramAddressSync([seed("custody"), position.toBuffer()], PROGRAM_ID)[0];

export const WSOL_MINT = new PublicKey("So11111111111111111111111111111111111111112");
export const TOKEN_PROGRAM_ID = new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
export const TOKEN_2022_PROGRAM_ID = new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
export const ASSOCIATED_TOKEN_PROGRAM_ID = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
/** The associated token account of `owner` for `mint`. */
export const ataOf = (owner: PublicKey, mint: PublicKey, tokenProgram: PublicKey = TOKEN_PROGRAM_ID) =>
  PublicKey.findProgramAddressSync([owner.toBuffer(), tokenProgram.toBuffer(), mint.toBuffer()], ASSOCIATED_TOKEN_PROGRAM_ID)[0];
export const positionPda = (agent: PublicKey, trader: PublicKey, nonce: BN) =>
  PublicKey.findProgramAddressSync([seed("position"), agent.toBuffer(), trader.toBuffer(), nonce.toArrayLike(Buffer, "le", 8)], PROGRAM_ID)[0];

export type AgentStatus = "draft" | "active" | "paused";
export type PositionStatus = "open" | "trading" | "settled" | "defaulted" | "cancelled";
export type Breach = "none" | "drawdown" | "missedDeadline";

export type Agent = {
  publicKey: PublicKey;
  operator: PublicKey;
  executor: PublicKey;
  agentId: BN;
  status: AgentStatus;
  name: string;
  description: string;
  terms: {
    collateralRatioBps: number;
    feeBps: number;
    maxDrawdownBps: number;
    minDurationSecs: BN;
    maxDurationSecs: BN;
    allowedAssets: PublicKey[];
    rules: string;
  };
  totalCollateral: BN;
  lockedCollateral: BN;
  capitalManaged: BN;
  openPositions: number;
  settledPositions: number;
  defaultedPositions: number;
  breachCount: number;
  slashedTotal: BN;
  feesEarned: BN;
  publishedAt: BN;
};

export type Custody = {
  position: PublicKey;
  /** Paid the custody and wSOL account rent; gets it back at settlement. */
  rentPayer: PublicKey;
  /** Bit i set while the vault holds some of the agent's allowed asset i. */
  heldMask: number;
  swaps: number;
};

/** What `executeSwap` needs: the DEX instruction as the DEX's client built it, plus the two legs and their price updates. */
export type SwapRequest = {
  position: PublicKey;
  mintIn: PublicKey;
  mintOut: PublicKey;
  /** Token program of each mint (classic unless the mint is Token-2022). */
  tokenProgramIn?: PublicKey;
  tokenProgramOut?: PublicKey;
  /** Pyth price update accounts for the two mints. */
  priceIn: PublicKey;
  priceOut: PublicKey;
  /** The DEX program and its instruction, with the vault PDA as the token authority and the vault's ATAs as its token accounts. */
  dex: TransactionInstruction;
  /** The program refuses the swap if the DEX takes more than this, or returns less than minOut. */
  amountInMax: Amount;
  minOut: Amount;
};

export type Position = {
  publicKey: PublicKey;
  trader: PublicKey;
  agent: PublicKey;
  nonce: BN;
  principal: BN;
  lockedCollateral: BN;
  status: PositionStatus;
  breach: Breach;
  openedAt: BN;
  deadline: BN;
  drawnAt: BN;
  closedAt: BN;
  returned: BN;
  slashed: BN;
  feePaid: BN;
};

const enumKey = <T extends string>(v: object) => Object.keys(v)[0] as T;

/** Lamports or seconds: a safe-integer number, a bigint, or a BN. */
export type Amount = number | bigint | BN;

export function toBN(x: Amount, what = "amount"): BN {
  if (BN.isBN(x)) {
    // borsh encodes a negative BN as its absolute value, so -5 would silently become 5
    if ((x as BN).isNeg()) throw new Error(`${what} must not be negative (got ${x.toString()})`);
    return x as BN;
  }
  if (typeof x === "number" && !Number.isSafeInteger(x)) throw new Error(`${what} must be a whole number below 2^53 (got ${x}); pass a bigint or BN`);
  if (x < 0) throw new Error(`${what} must not be negative (got ${x})`);
  return new BN(x.toString());
}

/** Loads a JSON keypair file, warning if other users can read it. */
export function loadKeypair(path: string) {
  try {
    const mode = statSync(path).mode & 0o777;
    if (mode & 0o077) console.warn(`warning: key file ${path} is readable by other users (mode ${mode.toString(8)}); run: chmod 600 ${path}`);
  } catch {
    // readFileSync below reports a missing file
  }
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(path, "utf8"))));
}

/** A confirmed transaction: its signature and the slot it landed in. */
export type Sent = { sig: string; slot: number };

/** The connection calls sendUntilConfirmed needs, so tests can stand in for the cluster. */
export type SendConnection = Pick<Connection, "getLatestBlockhash" | "sendRawTransaction" | "getSignatureStatuses" | "getBlockHeight">;

export type SendOptions = {
  /** How often to check the status and resend the same signed bytes. Default 2000. */
  rebroadcastMs?: number;
  /** Fresh blockhashes to try before giving up. Default 3. */
  maxBuilds?: number;
  /** Status checks per blockhash before rebuilding even if the block height never showed it expired. Default: 90s worth. */
  maxPolls?: number;
  sleep?: (ms: number) => Promise<void>;
  log?: (msg: string) => void;
};

const landed = (s: { confirmationStatus?: string } | null | undefined) => s?.confirmationStatus === "confirmed" || s?.confirmationStatus === "finalized";

/**
 * Sends a transaction and keeps resending the same signed bytes until it
 * confirms or its blockhash expires, then signs a fresh one with a new blockhash
 * (`build` is called again, so it can raise the priority fee). The first send of
 * each build is simulated, so a program error such as UnauthorizedExecutor
 * throws at once with its logs; a transaction that lands with an error throws too.
 */
export async function sendUntilConfirmed(conn: SendConnection, build: (blockhash: string) => Transaction, opts: SendOptions = {}): Promise<Sent> {
  const every = opts.rebroadcastMs ?? 2000;
  const maxBuilds = opts.maxBuilds ?? 3;
  const maxPolls = opts.maxPolls ?? Math.ceil(90_000 / every);
  const sleep = opts.sleep ?? ((ms: number) => new Promise((r) => setTimeout(r, ms)));
  const log = opts.log ?? (() => {});
  const check = async (sig: string, history = false) => {
    const s = (await conn.getSignatureStatuses([sig], { searchTransactionHistory: history })).value[0];
    if (s?.err) throw new Error(`transaction ${sig} failed: ${JSON.stringify(s.err)}`);
    return landed(s) ? { sig, slot: s!.slot } : null;
  };
  for (let attempt = 1; attempt <= maxBuilds; attempt++) {
    const { blockhash, lastValidBlockHeight } = await conn.getLatestBlockhash("confirmed");
    const tx = build(blockhash);
    const raw = tx.serialize();
    const sig = anchor.utils.bytes.bs58.encode(tx.signature!);
    await conn.sendRawTransaction(raw, { skipPreflight: false, preflightCommitment: "confirmed", maxRetries: 0 });
    for (let poll = 0; poll < maxPolls; poll++) {
      await sleep(every);
      try {
        const done = await check(sig);
        if (done) return done;
        if ((await conn.getBlockHeight("confirmed")) > lastValidBlockHeight) break;
        await conn.sendRawTransaction(raw, { skipPreflight: true, maxRetries: 0 });
      } catch (e) {
        if ((e as Error).message.startsWith(`transaction ${sig} failed`)) throw e;
        log(`resending ${sig.slice(0, 12)}…: ${(e as Error).message}`); // a flaky RPC call; keep going
      }
    }
    // it may have landed in the last valid blocks; once expired it never can, so a rebuild cannot double-send
    const done = await check(sig, true).catch((e) => {
      if ((e as Error).message.startsWith(`transaction ${sig} failed`)) throw e;
      return null;
    });
    if (done) return done;
    log(`transaction ${sig.slice(0, 12)}… expired unconfirmed${attempt < maxBuilds ? "; signing a fresh one" : ""}`);
  }
  throw new Error(`transaction not confirmed after ${maxBuilds} blockhashes`);
}

export class PoaClient {
  readonly connection: Connection;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  readonly program: any;
  constructor(rpcUrl: string, readonly signer: Keypair) {
    this.connection = new Connection(rpcUrl, "confirmed");
    const provider = new AnchorProvider(this.connection, new Wallet(signer), { commitment: "confirmed" });
    this.program = new Program(IDL, provider);
  }

  async agent(key: PublicKey): Promise<Agent | null> {
    const a = await this.program.account.agent.fetchNullable(key);
    return a ? { publicKey: key, ...a, status: enumKey<AgentStatus>(a.status) } : null;
  }

  /** Positions on one agent, via a filtered program-account scan. */
  async positions(agent: PublicKey): Promise<Position[]> {
    const raw = await this.program.account.position.all([{ memcmp: { offset: 40, bytes: agent.toBase58() } }]);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    return raw.map((r: any) => ({ publicKey: r.publicKey, ...r.account, status: enumKey<PositionStatus>(r.account.status), breach: enumKey<Breach>(r.account.breach) }));
  }

  async position(key: PublicKey): Promise<Position> {
    const p = await this.program.account.position.fetch(key);
    return { publicKey: key, ...p, status: enumKey<PositionStatus>(p.status), breach: enumKey<Breach>(p.breach) };
  }

  /** One position by direct account fetch, or null if the RPC node does not have it. */
  async positionNullable(key: PublicKey): Promise<Position | null> {
    const p = await this.program.account.position.fetchNullable(key);
    return p ? { publicKey: key, ...p, status: enumKey<PositionStatus>(p.status), breach: enumKey<Breach>(p.breach) } : null;
  }

  // ---- operator ----
  createAgent(agentId: number, name: string, description: string, terms: Agent["terms"]) {
    const agent = agentPda(this.signer.publicKey, agentId);
    return this.program.methods.createAgent(new BN(agentId), name, description, terms)
      .accounts({ operator: this.signer.publicKey, agent, agentVault: agentVaultPda(agent), systemProgram: SYSTEM }).rpc() as Promise<string>;
  }
  depositCollateral(agent: PublicKey, lamports: Amount) {
    return this.program.methods.depositCollateral(toBN(lamports, "lamports"))
      .accounts({ operator: this.signer.publicKey, agent, agentVault: agentVaultPda(agent), systemProgram: SYSTEM }).rpc() as Promise<string>;
  }
  withdrawCollateral(agent: PublicKey, lamports: Amount) {
    return this.program.methods.withdrawCollateral(toBN(lamports, "lamports"))
      .accounts({ operator: this.signer.publicKey, agent, agentVault: agentVaultPda(agent), systemProgram: SYSTEM }).rpc() as Promise<string>;
  }
  setExecutor(agent: PublicKey, executor: PublicKey) {
    return this.program.methods.setExecutor(executor).accounts({ operator: this.signer.publicKey, agent }).rpc() as Promise<string>;
  }
  publishAgent(agent: PublicKey) {
    return this.program.methods.publishAgent().accounts({ operator: this.signer.publicKey, agent }).rpc() as Promise<string>;
  }
  setAccepting(agent: PublicKey, accepting: boolean) {
    return this.program.methods.setAccepting(accepting).accounts({ operator: this.signer.publicKey, agent }).rpc() as Promise<string>;
  }

  // ---- trading key ----
  /** The custody account of a position, or null for a position that is not trading under custody. */
  async custody(position: PublicKey): Promise<Custody | null> {
    return (await this.program.account.custody.fetchNullable(custodyPda(position))) as Custody | null;
  }
  /**
   * Starts trading a position under vault custody: the principal is wrapped into
   * the vault's wSOL account and can only move through executeSwap. The signer
   * pays the rent of the custody and wSOL accounts and gets it back at settlement.
   */
  beginTrading(agent: PublicKey, position: PublicKey) {
    const vault = positionVaultPda(position);
    return this.program.methods.beginTrading()
      .accounts({
        executor: this.signer.publicKey, agent, position, positionVault: vault, custody: custodyPda(position),
        wsolMint: WSOL_MINT, vaultWsol: ataOf(vault, WSOL_MINT), tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID, systemProgram: SYSTEM, config: configPda(),
      }).rpc() as Promise<string>;
  }
  /** Disabled by the program since vault custody; use beginTrading. Kept so old scripts fail with the program's own error. */
  drawFunds(agent: PublicKey, position: PublicKey) {
    return this.program.methods.drawFunds()
      .accounts({ executor: this.signer.publicKey, agent, position, positionVault: positionVaultPda(position), systemProgram: SYSTEM }).rpc() as Promise<string>;
  }
  /** Creates the vault's token account for one of the agent's allowed assets, so a swap can receive it. The signer pays its rent. */
  openVaultTokenAccount(agent: PublicKey, position: PublicKey, mint: PublicKey, tokenProgram: PublicKey = TOKEN_PROGRAM_ID) {
    const vault = positionVaultPda(position);
    return this.program.methods.openVaultTokenAccount()
      .accounts({
        executor: this.signer.publicKey, agent, position, positionVault: vault, mint, vaultAta: ataOf(vault, mint, tokenProgram),
        tokenProgram, associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID, systemProgram: SYSTEM,
      }).rpc() as Promise<string>;
  }
  /** Closes an empty vault token account; the rent goes to the signer. Not the wSOL account while the position trades. */
  closeVaultTokenAccount(agent: PublicKey, position: PublicKey, mint: PublicKey, tokenProgram: PublicKey = TOKEN_PROGRAM_ID) {
    const vault = positionVaultPda(position);
    return this.program.methods.closeVaultTokenAccount()
      .accounts({ executor: this.signer.publicKey, agent, position, positionVault: vault, mint, vaultAta: ataOf(vault, mint, tokenProgram), tokenProgram })
      .rpc() as Promise<string>;
  }
  /**
   * The execute_swap instruction: the DEX instruction is forwarded by the program,
   * signed by the vault. The DEX's accounts go in as remaining accounts with the
   * vault not marked as a signer (the program marks it inside the CPI).
   */
  async executeSwapInstruction(agent: PublicKey, r: SwapRequest): Promise<TransactionInstruction> {
    const vault = positionVaultPda(r.position);
    const tokenProgramIn = r.tokenProgramIn ?? TOKEN_PROGRAM_ID;
    const tokenProgramOut = r.tokenProgramOut ?? TOKEN_PROGRAM_ID;
    const remaining = r.dex.keys.map((k) => ({ ...k, isSigner: k.pubkey.equals(vault) ? false : k.isSigner }));
    return this.program.methods.executeSwap(toBN(r.amountInMax, "amountInMax"), toBN(r.minOut, "minOut"), Buffer.from(r.dex.data))
      .accounts({
        executor: this.signer.publicKey, agent, position: r.position, custody: custodyPda(r.position), positionVault: vault,
        mintIn: r.mintIn, mintOut: r.mintOut, vaultIn: ataOf(vault, r.mintIn, tokenProgramIn), vaultOut: ataOf(vault, r.mintOut, tokenProgramOut),
        priceIn: r.priceIn, priceOut: r.priceOut, dexProgram: r.dex.programId, tokenProgramIn, tokenProgramOut, config: configPda(),
      })
      .remainingAccounts(remaining)
      .instruction();
  }
  /** Sends one execute_swap, with `before` (price updates, compute budget) in front of it. */
  async executeSwap(agent: PublicKey, r: SwapRequest, before: TransactionInstruction[] = []): Promise<string> {
    const ix = await this.executeSwapInstruction(agent, r);
    const tx = new Transaction().add(...before, ix);
    return this.program.provider.sendAndConfirm(tx, [this.signer]) as Promise<string>;
  }
  /**
   * Settle accounts for a position: with the custody set when the position trades
   * under custody (the vault's balance settles; `returned` is ignored), the legacy
   * set otherwise (the signer sends `returned`).
   */
  private async settleMethod(a: Agent, p: Position, returnedLamports: Amount, custodyState?: Custody | null) {
    const custody = custodyPda(p.publicKey);
    const c = custodyState !== undefined ? custodyState : p.status === "trading" ? await this.custody(p.publicKey) : null;
    const vault = positionVaultPda(p.publicKey);
    return this.program.methods.settlePosition(toBN(c ? 0 : returnedLamports, "returnedLamports"))
      .accounts({
        executor: this.signer.publicKey, operator: a.operator, agent: a.publicKey, agentVault: agentVaultPda(a.publicKey),
        position: p.publicKey, positionVault: vault, trader: p.trader, systemProgram: SYSTEM,
        custody, vaultWsol: c ? ataOf(vault, WSOL_MINT) : null, rentPayer: c ? c.rentPayer : null,
        tokenProgram: c ? TOKEN_PROGRAM_ID : null, config: c ? configPda() : null,
      });
  }
  /**
   * Settles with one plain send, no priority fee. The runner uses settle() instead.
   * `custody` skips the lookup: pass the position's custody account, or null for
   * a position drawn before vault custody.
   */
  async settlePosition(a: Agent, p: Position, returnedLamports: Amount, custody?: Custody | null) {
    return (await this.settleMethod(a, p, returnedLamports, custody)).rpc() as Promise<string>;
  }
  /**
   * Settles with a compute-budget priority fee, resending until confirmed (see
   * sendUntilConfirmed). `microLamports` is read at each fresh signing, so a
   * caller can raise it as the deadline nears.
   */
  async settle(a: Agent, p: Position, returnedLamports: Amount, fee: { microLamports: number | (() => number) }, opts: SendOptions = {}, custody?: Custody | null): Promise<Sent> {
    const ix: TransactionInstruction = await (await this.settleMethod(a, p, returnedLamports, custody)).instruction();
    return sendUntilConfirmed(this.connection, (blockhash) => {
      const microLamports = typeof fee.microLamports === "function" ? fee.microLamports() : fee.microLamports;
      const tx = new Transaction({ feePayer: this.signer.publicKey, recentBlockhash: blockhash }).add(
        ComputeBudgetProgram.setComputeUnitLimit({ units: SETTLE_COMPUTE_UNITS }),
        ComputeBudgetProgram.setComputeUnitPrice({ microLamports }),
        ix,
      );
      tx.sign(this.signer);
      return tx;
    }, opts);
  }

  // ---- trader (for testing) ----
  /** Opens a position under a random u64 nonce, so two opens in the same millisecond cannot collide. */
  openPosition(agent: PublicKey, lamports: Amount, durationSecs: Amount) {
    const nonce = new BN(randomBytes(8), "le");
    const position = positionPda(agent, this.signer.publicKey, nonce);
    return this.program.methods.openPosition(nonce, toBN(lamports, "lamports"), toBN(durationSecs, "durationSecs"))
      .accounts({ trader: this.signer.publicKey, agent, position, positionVault: positionVaultPda(position), systemProgram: SYSTEM })
      .rpc().then((sig: string) => ({ position, nonce, sig }));
  }
  /** Withdraws a position's principal before the agent has drawn it. No fee, no slash. */
  cancelPosition(agent: PublicKey, position: PublicKey) {
    return this.program.methods.cancelPosition()
      .accounts({ trader: this.signer.publicKey, agent, position, positionVault: positionVaultPda(position), systemProgram: SYSTEM })
      .rpc() as Promise<string>;
  }
  /** Only for a position drawn before vault custody. A custody position past its deadline is settled by the trader instead (settlePosition). */
  claimDefault(a: Agent, p: Position) {
    return this.program.methods.claimDefault()
      .accounts({ trader: this.signer.publicKey, agent: a.publicKey, agentVault: agentVaultPda(a.publicKey), position: p.publicKey, positionVault: positionVaultPda(p.publicKey), systemProgram: SYSTEM, custody: custodyPda(p.publicKey) })
      .rpc() as Promise<string>;
  }
}
