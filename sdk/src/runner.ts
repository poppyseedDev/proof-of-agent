/**
 * Drives an external trading bot through a Proof of Agent position:
 *   1. a trader opens a position on the agent
 *   2. the runner draws the SOL into the trading wallet
 *   3. it starts the operator's hook command (the bot) and waits
 *   4. it settles before the deadline with whatever the wallet gained or lost,
 *      killing the hook if it is still running
 *
 * One position trades at a time; others queue. Accounting is by wallet
 * balance, so anything the bot does with the wallet counts for the position.
 * State is persisted so a restart resumes open positions.
 *
 * The hook runs in its own process group. At settle time the whole group gets
 * SIGTERM, then up to `graceSecs` to unwind (never past the deadline), then
 * SIGKILL. The balance is read only once nothing in the group is running.
 * The group id is saved in the state file, so a hook left behind by a crashed
 * runner is stopped the same way before the restarted runner reads the wallet.
 *
 * Times are compared on the cluster's clock, which is what the program checks
 * the deadline against; the offset from the local clock is re-measured every few minutes.
 */
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { closeSync, copyFileSync, existsSync, fsyncSync, mkdirSync, openSync, readFileSync, renameSync, writeSync } from "node:fs";
import { dirname } from "node:path";
import { PublicKey, type Keypair } from "@solana/web3.js";
import {
  DEFAULT_PRIORITY_MICROLAMPORTS, DEFAULT_URGENT_PRIORITY_MICROLAMPORTS, LAMPORTS, PoaClient, SIGNATURE_FEE, priorityFeeLamports,
  type Agent, type Position,
} from "./client.js";

export type RunnerOptions = {
  rpcUrl: string;
  agent: PublicKey;
  tradingKey: Keypair;
  /** Shell command started after each draw. Env: POA_POSITION, POA_PRINCIPAL_SOL, POA_DEADLINE, POA_SETTLE_BY, POA_WALLET, POA_RPC_URL, POA_PAPER. */
  hook?: string;
  /** Shell command run on notable events, with POA_EVENT and POA_MESSAGE. */
  notify?: string;
  /** Settle this many seconds after drawing, even if the hook is still running. */
  holdSecs: number;
  /** Settle at least this many seconds before the deadline. */
  bufferSecs: number;
  /**
   * After SIGTERM at settle time, how long the hook may take to unwind before SIGKILL.
   * Default 60. Capped so the hook never runs past the deadline minus the time needed to settle.
   */
  graceSecs?: number;
  /**
   * Dry run: never draw or settle. Each position the runner would draw is logged,
   * the hook runs with POA_PAPER=1 for the would-be hold, and the would-be settle is logged.
   */
  paper: boolean;
  /** Longest sleep between ticks; the runner wakes sooner when a settle comes due. */
  pollMs: number;
  /** Settle priority fee in micro-lamports per compute unit. Default DEFAULT_PRIORITY_MICROLAMPORTS. */
  priorityMicroLamports?: number;
  /** Settle priority fee within URGENT_SECS of the deadline. Default DEFAULT_URGENT_PRIORITY_MICROLAMPORTS. */
  urgentPriorityMicroLamports?: number;
  stateFile: string;
  log?: (msg: string) => void;
};

type Book = { position: string; principal: string; balanceAtDraw: string; drawnAt: number; settleAt: number; deadline: number };
type State = {
  active: Book | null;
  history: { position: string; principal: string; returned: string; at: number }[];
  /** Process group of a hook that may still be running, cleared once the group is gone. */
  hookPgid?: number | null;
  /** Slot of our last confirmed settle: balance reads come from a node at least this far along. */
  minSlot?: number;
};

const localNow = () => Math.floor(Date.now() / 1000);
const sol = (n: bigint | number | string) => (Number(n) / LAMPORTS).toFixed(4);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
/** Seconds kept free before the deadline for the settle transaction itself. */
export const SETTLE_TX_SECS = 30;
/** Lamports kept on top of the rent-exempt minimum to pay the settle transaction's fee (base fee plus priority fee). */
export const SETTLE_FEE_MARGIN = 20_000n;
/**
 * What the draw transaction costs the trading wallet: one signature, no priority
 * fee. balanceAtDraw is the balance read before the draw plus the principal less
 * this, so it never depends on a node having caught up with the draw.
 */
export const DRAW_TX_FEE = BigInt(SIGNATURE_FEE);
/** Within this many seconds of the deadline, settles pay the urgent priority fee. */
export const URGENT_SECS = 120;
/** How often the offset between the local clock and the cluster's is re-measured. */
export const CLOCK_REFRESH_MS = 5 * 60_000;
/** How often the agent account is re-read to check the trading key is still bound. */
export const AGENT_REFRESH_MS = 2 * 60_000;
/** Shortest sleep between ticks, so a failing settle is retried without hammering the RPC. */
export const MIN_SLEEP_MS = 1000;
/** Statuses after which a position can never be settled again. */
const TERMINAL = new Set(["settled", "defaulted", "cancelled"]);

/**
 * What the trading wallet must keep after settling: the rent-exempt minimum for
 * a 0-byte account plus the fee. Solana rejects a transaction that leaves a
 * wallet non-zero but below rent exemption, so a smaller reserve fails every retry.
 */
export function walletReserve(rentExemptLamports: number | bigint, urgentMicroLamports = DEFAULT_URGENT_PRIORITY_MICROLAMPORTS) {
  const fee = DRAW_TX_FEE + BigInt(priorityFeeLamports(urgentMicroLamports));
  return BigInt(rentExemptLamports) + (fee > SETTLE_FEE_MARGIN ? fee : SETTLE_FEE_MARGIN);
}

/** How long to sleep before the next tick: `pollMs`, or less when the active book's settle time comes sooner. */
export function loopSleepMs(pollMs: number, settleAt: number | null, nowSecs: number) {
  if (settleAt === null) return pollMs;
  return Math.min(pollMs, Math.max(MIN_SLEEP_MS, (settleAt - nowSecs) * 1000));
}

/** The RPC URL's host only: the path or query often carries an API key. */
export function rpcHost(url: string) {
  try {
    return new URL(url).host || "(rpc)";
  } catch {
    return "(rpc)";
  }
}

/** How long the hook may unwind after SIGTERM: `graceSecs` (default 60), cut so it ends SETTLE_TX_SECS before the deadline, never negative. */
export function hookGraceSecs(graceSecs: number | undefined, deadline: number, nowSecs: number) {
  return Math.max(0, Math.min(graceSecs ?? 60, deadline - SETTLE_TX_SECS - nowSecs));
}
const TOKEN_PROGRAMS = [
  new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
  new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"),
];

/** True while any process in the group still exists. */
function groupAlive(pgid: number) {
  try {
    process.kill(-pgid, 0);
    return true;
  } catch (e) {
    return (e as NodeJS.ErrnoException).code === "EPERM";
  }
}

/** This process's own group id, or null if `ps` is unavailable. */
function ownPgid() {
  try {
    return Number(execFileSync("ps", ["-o", "pgid=", "-p", String(process.pid)], { encoding: "utf8" }).trim()) || null;
  } catch {
    return null;
  }
}

function signalGroup(pgid: number, sig: NodeJS.Signals) {
  try {
    process.kill(-pgid, sig);
  } catch {
    // group already gone
  }
}

/** Writes to a temp file, fsyncs, keeps the previous version as `.bak`, then renames over the original. */
export function writeAtomic(path: string, data: string) {
  mkdirSync(dirname(path), { recursive: true });
  const tmp = `${path}.tmp-${process.pid}`;
  const fd = openSync(tmp, "w", 0o600);
  try {
    writeSync(fd, data);
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
  if (existsSync(path)) copyFileSync(path, `${path}.bak`);
  renameSync(tmp, path);
}

/**
 * Loads the runner state. A file that does not parse is moved aside to
 * `<file>.corrupt-<ts>` and the backup is used, so a bad write cannot crash-loop the runner.
 */
export function loadRunnerState(path: string, log: (m: string) => void): State {
  const empty: State = { active: null, history: [] };
  if (!existsSync(path)) return empty;
  try {
    return { ...empty, ...JSON.parse(readFileSync(path, "utf8")) };
  } catch (e) {
    const aside = `${path}.corrupt-${new Date().toISOString().replace(/[:.]/g, "-")}`;
    renameSync(path, aside);
    log(`ALERT state file ${path} did not parse (${(e as Error).message}); moved it to ${aside}`);
    const bak = `${path}.bak`;
    if (existsSync(bak)) {
      try {
        const s = { ...empty, ...JSON.parse(readFileSync(bak, "utf8")) };
        log(`ALERT resuming from backup ${bak}; check the wallet against it`);
        return s;
      } catch (e2) {
        log(`ALERT backup ${bak} did not parse either (${(e2 as Error).message})`);
      }
    }
    log("ALERT starting with empty state; a trading position will be re-adopted from chain");
    return empty;
  }
}

export class Runner {
  private client: PoaClient;
  private agentAcc!: Agent;
  private state: State;
  private hook: ChildProcess | null = null;
  /** Set when the hook's shell has exited; its children may still be running. */
  private hookExited = false;
  private stopping = false;
  /** Cached walletReserve(); the rent-exempt minimum does not change while running. */
  private reserve: bigint | null = null;
  /** Set once the low-balance refusal has been notified, so it is not repeated every poll. */
  private lowBalanceNotified = false;
  /** Cluster time minus local time, in seconds. */
  private skew = 0;
  /** Date.now() of the last successful clock measurement; 0 for never. */
  private clockAt = 0;
  private clockFailed = false;
  /** Date.now() of the last agent account read; 0 forces a read on the next tick. */
  private agentAt = 0;
  /** Set while the agent's executor and operator are both some other key. */
  private unbound = false;
  /** Paper mode's would-be position: in memory only, so a real run never settles it. */
  private paperBook: Book | null = null;
  /** Positions paper mode has already simulated, so each is logged once. */
  private paperSeen = new Set<string>();
  private log: (msg: string) => void;

  constructor(private opts: RunnerOptions) {
    this.client = new PoaClient(opts.rpcUrl, opts.tradingKey);
    this.log = opts.log ?? ((m) => console.log(new Date().toISOString(), m));
    this.state = loadRunnerState(opts.stateFile, this.log);
  }

  private save() {
    writeAtomic(this.opts.stateFile, JSON.stringify(this.state, null, 2));
  }

  private async notify(event: string, message: string) {
    this.log(`${event}: ${message}`);
    if (!this.opts.notify) return;
    const child = spawn(this.opts.notify, { shell: true, stdio: "inherit", env: { ...process.env, POA_EVENT: event, POA_MESSAGE: message } });
    // a failed spawn is an 'error' event; unhandled, it would crash the runner
    child.on("error", (e) => this.log(`notify command failed to start: ${e.message}`));
  }

  private async walletReserve() {
    this.reserve ??= walletReserve(await this.client.connection.getMinimumBalanceForRentExemption(0), this.urgentFee());
    return this.reserve;
  }

  private urgentFee() {
    return this.opts.urgentPriorityMicroLamports ?? DEFAULT_URGENT_PRIORITY_MICROLAMPORTS;
  }

  /** Seconds on the cluster's clock, as last measured; the local clock until then. */
  private now() {
    return localNow() + this.skew;
  }

  /** Re-measures the cluster clock offset at most every CLOCK_REFRESH_MS. On failure the last offset stands. */
  private async refreshClock() {
    if (this.clockAt && Date.now() - this.clockAt < CLOCK_REFRESH_MS) return;
    try {
      const t = await this.client.connection.getBlockTime(await this.client.connection.getSlot("confirmed"));
      if (t === null) throw new Error("no block time for the current slot");
      const skew = t - localNow();
      if (!this.clockAt || Math.abs(skew - this.skew) > 5) {
        this.log(`using chain time (local clock is ${Math.abs(skew)}s ${skew <= 0 ? "ahead" : "behind"})`);
      }
      this.skew = skew;
      this.clockAt = Date.now();
      this.clockFailed = false;
    } catch (e) {
      if (!this.clockFailed) this.log(`chain time unavailable (${(e as Error).message}); keeping the last offset of ${this.skew}s`);
      this.clockFailed = true;
    }
  }

  /** The trading wallet's balance, from a node that has seen our last confirmed settle. */
  private async balance() {
    const minContextSlot = this.state.minSlot;
    const key = this.opts.tradingKey.publicKey;
    return BigInt(await this.client.connection.getBalance(key, minContextSlot ? { commitment: "confirmed", minContextSlot } : "confirmed"));
  }

  private isBound(a: Agent) {
    const me = this.opts.tradingKey.publicKey;
    return a.executor.equals(me) || a.operator.equals(me);
  }

  async start() {
    const a = await this.client.agent(this.opts.agent);
    if (!a) throw new Error(`agent ${this.opts.agent.toBase58()} not found on ${rpcHost(this.opts.rpcUrl)}`);
    if (!this.isBound(a)) {
      throw new Error(`trading key ${this.opts.tradingKey.publicKey.toBase58()} is not bound to this agent (bound: ${a.executor.toBase58()})`);
    }
    this.agentAcc = a;
    this.agentAt = Date.now();
    this.log(`agent "${a.name}" ${a.publicKey.toBase58()} status=${a.status} bond=${sol(a.totalCollateral.toString())} SOL trading-key=${this.opts.tradingKey.publicKey.toBase58()}${this.opts.paper ? " [paper: nothing is drawn or settled]" : ""}`);
    process.on("SIGINT", () => this.stop());
    process.on("SIGTERM", () => this.stop());
    while (!this.stopping) {
      try {
        await this.tick();
      } catch (e) {
        await this.notify("error", (e as Error).message);
      }
      const book = this.state.active ?? this.paperBook;
      await sleep(loopSleepMs(this.opts.pollMs, book?.settleAt ?? null, this.now()));
    }
  }

  stop() {
    this.stopping = true;
    // the group id stays in the state file until the group is gone, so a restart finishes the job
    if (this.hook?.pid) signalGroup(this.hook.pid, "SIGTERM");
  }

  private settleTime(drawnAt: number, deadline: number) {
    return Math.min(deadline - this.opts.bufferSecs, drawnAt + this.opts.holdSecs);
  }

  /**
   * Re-reads the agent at most every AGENT_REFRESH_MS. If the operator has bound
   * another trading key, every settle will fail with UnauthorizedExecutor, so this
   * alerts on the change and again at every check while a position is at stake.
   */
  private async refreshAgent() {
    if (this.agentAt && Date.now() - this.agentAt < AGENT_REFRESH_MS) return;
    const a = await this.client.agent(this.opts.agent).catch((e) => {
      this.log(`could not re-read the agent account (${(e as Error).message}); keeping the last copy`);
      return undefined;
    });
    if (a === undefined) return;
    this.agentAt = Date.now();
    if (!a) {
      this.log(`agent ${this.opts.agent.toBase58()} not found on ${rpcHost(this.opts.rpcUrl)}; keeping the last copy`);
      return;
    }
    this.agentAcc = a;
    const bound = this.isBound(a);
    if (!bound) {
      const active = this.state.active;
      const stake = active ? ` Position ${active.position} must be settled by ${new Date(active.deadline * 1000).toISOString()}: rebind this key (poa agent bind) or settle it with the operator key.` : "";
      const msg = `ALERT trading key ${this.opts.tradingKey.publicKey.toBase58()} is no longer bound to agent ${a.publicKey.toBase58()} (trading key now ${a.executor.toBase58()}); every draw and settle will fail with UnauthorizedExecutor.${stake}`;
      if (!this.unbound || active) await this.notify("unbound", msg);
      else this.log(msg);
    } else if (this.unbound) {
      await this.notify("rebound", `trading key ${this.opts.tradingKey.publicKey.toBase58()} is bound to the agent again`);
    }
    this.unbound = !bound;
  }

  /**
   * A hook group saved in the state file but not started by this process was left
   * by a runner that crashed or was stopped. Stop it before anything reads the
   * wallet, or the balance would move under us while it keeps trading.
   */
  private async reapOrphanHook() {
    const pgid = this.state.hookPgid;
    if (!pgid || this.hook) return;
    if (pgid === process.pid || pgid === ownPgid()) {
      // the id was reused for this very process (a fresh container, say): never signal ourselves
      this.clearHookPgid();
      return;
    }
    if (groupAlive(pgid)) {
      await this.notify("warning", `hook process group ${pgid} from before a restart is still running; stopping it before reading the wallet`);
      await this.killGroup(pgid, this.state.active?.deadline ?? Infinity, "orphaned hook");
    }
    this.clearHookPgid();
  }

  private clearHookPgid() {
    if (this.state.hookPgid == null) return;
    this.state.hookPgid = null;
    this.save();
  }

  private async tick() {
    await this.refreshClock();
    await this.reapOrphanHook();
    await this.refreshAgent();
    if (this.opts.paper) return this.paperTick();

    const active = this.state.active;

    if (active) {
      // A direct fetch of the one account: the program-account scan is heavier,
      // more often rate-limited, and must never stand between us and settling.
      const p = await this.client.positionNullable(new PublicKey(active.position));
      if (!p) {
        this.log(`position ${active.position} not found on the RPC node; keeping the book and retrying`);
        return;
      }
      if (TERMINAL.has(p.status)) {
        this.log(`position ${active.position} is ${p.status}; dropping book`);
        await this.stopHook(active.deadline);
        this.state.active = null;
        this.save();
        return;
      }
      // "open" here is a node that has not caught up with our confirmed draw; the book stands.
      if (p.status !== "trading") this.log(`position ${active.position} reads as ${p.status} (RPC lag after the draw?); keeping the book`);
      const dueIn = active.settleAt - this.now();
      if (dueIn <= 0 || (this.hook && this.hookExited)) {
        await this.settle(p, active);
      } else if (dueIn <= 60 && dueIn > 60 - this.opts.pollMs / 1000) {
        await this.notify("settle-soon", `position ${p.publicKey.toBase58()} settles in ${dueIn}s`);
      }
      return;
    }

    const positions = await this.client.positions(this.opts.agent);

    // adopt a position we drew before a restart
    const orphan = positions.find((p) => p.status === "trading");
    if (orphan) {
      const drawnAt = orphan.drawnAt.toNumber();
      const deadline = orphan.deadline.toNumber();
      const balance = await this.balance();
      this.state.active = {
        position: orphan.publicKey.toBase58(), principal: orphan.principal.toString(), balanceAtDraw: String(balance),
        drawnAt, settleAt: Math.max(Math.min(this.settleTime(drawnAt, deadline), deadline - this.opts.bufferSecs), this.now() + 30), deadline,
      };
      this.save();
      await this.notify("adopted", `resumed trading position ${orphan.publicKey.toBase58()} without its book; settling with principal + balance change from now`);
      return;
    }

    if (this.unbound) {
      this.log("not drawing: the trading key is no longer bound to the agent");
      return;
    }
    const next = this.nextToDraw(positions);
    if (!next) return;
    // The wallet must already hold the settle reserve: after a loss it may return
    // everything it has left, and a settle that dips below rent exemption always fails.
    const reserve = await this.walletReserve();
    const balance = await this.balance();
    if (balance < reserve) {
      const msg = `not drawing ${next.publicKey.toBase58()}: trading wallet holds ${sol(balance)} SOL, below the ${sol(reserve)} SOL it must keep to settle; fund ${this.opts.tradingKey.publicKey.toBase58()}`;
      if (this.lowBalanceNotified) this.log(msg);
      else await this.notify("low-balance", msg);
      this.lowBalanceNotified = true;
      return;
    }
    this.lowBalanceNotified = false;
    await this.draw(next, balance);
  }

  /** The oldest open position with enough time left to trade and settle. */
  private nextToDraw(positions: Position[]) {
    return positions
      .filter((p) => p.status === "open" && p.deadline.toNumber() - this.now() > this.opts.bufferSecs + 120)
      .sort((a, b) => a.openedAt.cmp(b.openedAt))[0];
  }

  /**
   * `before` is the wallet balance read just before sending, while nothing else
   * moves it. The draw adds exactly the principal and costs one signature, so the
   * book does not depend on a lagging node reflecting the draw.
   */
  private async draw(p: Position, before: bigint) {
    const deadline = p.deadline.toNumber();
    const sig = await this.client.beginTrading(this.opts.agent, p.publicKey);
    const drawnAt = this.now();
    const balanceAtDraw = before + BigInt(p.principal.toString()) - DRAW_TX_FEE;
    const book: Book = { position: p.publicKey.toBase58(), principal: p.principal.toString(), balanceAtDraw: String(balanceAtDraw), drawnAt, settleAt: this.settleTime(drawnAt, deadline), deadline };
    this.state.active = book;
    this.save();
    this.log(`drew ${sol(p.principal.toString())} SOL from ${book.position} (${sig.slice(0, 12)}…); settle by ${new Date(book.settleAt * 1000).toISOString()}`);
    this.startHook(book);
  }

  /** Paper mode: nothing is sent. Logs what would be drawn and settled, running the hook with POA_PAPER=1 in between. */
  private async paperTick() {
    const b = this.paperBook;
    if (b) {
      if (b.settleAt - this.now() > 0 && !(this.hook && this.hookExited)) return;
      await this.stopHook(b.deadline);
      this.log(`[paper] would settle ${b.position} now, returning principal ${sol(b.principal)} SOL plus the wallet's change; nothing sent`);
      this.paperBook = null;
      return;
    }
    const positions = await this.client.positions(this.opts.agent);
    const next = this.nextToDraw(positions.filter((p) => !this.paperSeen.has(p.publicKey.toBase58())));
    if (!next) return;
    this.paperSeen.add(next.publicKey.toBase58());
    const drawnAt = this.now();
    const deadline = next.deadline.toNumber();
    const book: Book = { position: next.publicKey.toBase58(), principal: next.principal.toString(), balanceAtDraw: "0", drawnAt, settleAt: this.settleTime(drawnAt, deadline), deadline };
    this.paperBook = book;
    this.log(`[paper] would draw ${sol(book.principal)} SOL from ${book.position} and settle by ${new Date(book.settleAt * 1000).toISOString()} (deadline ${new Date(deadline * 1000).toISOString()}); nothing sent`);
    this.startHook(book);
  }

  private startHook(book: Book) {
    if (!this.opts.hook) return;
    const env = {
      ...process.env,
      POA_POSITION: book.position,
      POA_PRINCIPAL_SOL: sol(book.principal),
      POA_PRINCIPAL_LAMPORTS: book.principal,
      POA_DEADLINE: String(book.deadline),
      POA_SETTLE_BY: String(book.settleAt),
      POA_WALLET: this.opts.tradingKey.publicKey.toBase58(),
      POA_RPC_URL: this.opts.rpcUrl,
      POA_PAPER: this.opts.paper ? "1" : "0",
    };
    // Own process group, so settle can stop the hook and everything it started.
    const hook = spawn(this.opts.hook, { shell: true, stdio: "inherit", env, detached: true });
    this.hook = hook;
    this.hookExited = false;
    if (hook.pid) {
      // saved so a restarted runner can stop this group if we crash while it runs
      this.state.hookPgid = hook.pid;
      this.save();
    }
    this.log(`hook started (pid ${hook.pid}): ${this.opts.hook}`);
    // A spawn failure emits 'error' (and maybe no 'exit'). Treat it as the hook
    // having exited, so the position settles instead of the runner crashing.
    hook.on("error", (e) => {
      if (this.hook === hook) this.hookExited = true;
      this.log(`hook failed to start: ${e.message}`);
    });
    hook.on("exit", (code, signal) => {
      if (this.hook === hook) this.hookExited = true;
      this.log(`hook exited with ${signal ? `signal ${signal}` : `code ${code}`}`);
    });
  }

  /**
   * Stops the hook's process group: SIGTERM, up to the grace period for it to
   * unwind, then SIGKILL. Returns once no process in the group is left.
   */
  private async stopHook(deadline: number) {
    const hook = this.hook;
    this.hook = null;
    if (hook?.pid) await this.killGroup(hook.pid, deadline);
    this.clearHookPgid();
  }

  private async killGroup(pgid: number, deadline: number, why = "hook still running at settle time") {
    if (!groupAlive(pgid)) return;
    const grace = hookGraceSecs(this.opts.graceSecs, deadline, this.now());
    this.log(`${why}; sending SIGTERM to its process group, ${grace}s to finish`);
    signalGroup(pgid, "SIGTERM");
    const until = Date.now() + grace * 1000;
    while (groupAlive(pgid) && Date.now() < until) await sleep(250);
    if (groupAlive(pgid)) {
      this.log("hook did not stop in time; sending SIGKILL to its process group");
      signalGroup(pgid, "SIGKILL");
      const hard = Date.now() + 5000;
      while (groupAlive(pgid) && Date.now() < hard) await sleep(100);
      if (groupAlive(pgid)) this.log("warning: hook process group still present after SIGKILL");
    }
  }

  /** Token accounts with a non-zero balance: anything the hook left outside SOL is not counted. */
  private async strayTokens(): Promise<string[]> {
    const out: string[] = [];
    for (const programId of TOKEN_PROGRAMS) {
      const res = await this.client.connection.getParsedTokenAccountsByOwner(this.opts.tradingKey.publicKey, { programId });
      for (const a of res.value) {
        const info = a.account.data.parsed?.info;
        if (info && info.tokenAmount?.amount !== "0") out.push(`${info.tokenAmount.uiAmountString} of ${info.mint}`);
      }
    }
    return out;
  }

  /** The settle priority fee: the urgent one within URGENT_SECS of the deadline. */
  private priorityFee(deadline: number) {
    return deadline - this.now() <= URGENT_SECS ? this.urgentFee() : this.opts.priorityMicroLamports ?? DEFAULT_PRIORITY_MICROLAMPORTS;
  }

  private async settle(p: Position, book: Book) {
    // Read the balance only once nothing the hook started is still running.
    await this.stopHook(book.deadline);

    const principal = BigInt(book.principal);
    const stray = await this.strayTokens().catch((e) => [`(could not check token balances: ${(e as Error).message})`]);
    if (stray.length) {
      await this.notify("warning", `trading wallet holds non-SOL tokens at settle time, which are not counted for the position: ${stray.join(", ")}`);
    }
    const balance = await this.balance();
    const delta = balance - BigInt(book.balanceAtDraw);
    let returned = principal + delta;
    if (returned < 0n) returned = 0n;
    // keep the wallet rent-exempt and able to pay for the settle transaction itself
    const reserve = await this.walletReserve();
    if (balance - returned < reserve) returned = balance > reserve ? balance - reserve : 0n;
    let sent;
    try {
      sent = await this.client.settle(this.agentAcc, p, returned, { microLamports: () => this.priorityFee(book.deadline) }, { log: this.log });
    } catch (e) {
      this.agentAt = 0; // re-check the binding next tick: a rebound key fails every settle
      throw e;
    }
    const pnl = returned - principal;
    await this.notify("settled", `position ${book.position}: principal ${sol(principal)} returned ${sol(returned)} (${pnl >= 0n ? "+" : ""}${sol(pnl)}) ${sent.sig.slice(0, 12)}…`);
    this.state.history.push({ position: book.position, principal: book.principal, returned: returned.toString(), at: localNow() });
    this.state.minSlot = Math.max(this.state.minSlot ?? 0, sent.slot);
    this.state.active = null;
    this.save();
  }
}
