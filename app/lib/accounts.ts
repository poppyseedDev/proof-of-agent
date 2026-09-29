"use client";

import { Connection, PublicKey } from "@solana/web3.js";
import {
  AgentAccount,
  PositionAccount,
  ProtocolConfig,
  RPC_URL,
  decodeAgent,
  decodeConfig,
  decodePosition,
  readonlyProgram,
} from "./program";

// The program client's coder uses the camelCase field names the rest of the app expects.
// The connection is never used for decoding.
const coder = readonlyProgram(new Connection(RPC_URL)).coder.accounts;
const AGENT_DISC: Buffer = coder.accountDiscriminator("agent");
const POSITION_DISC: Buffer = coder.accountDiscriminator("position");
const CONFIG_DISC: Buffer = coder.accountDiscriminator("config");

export type ProgramSnapshot = {
  /** When the server read the chain. */
  at: number;
  /** When this browser received it. */
  fetchedAt: number;
  stale: boolean;
  agents: AgentAccount[];
  positions: PositionAccount[];
  /** Null until the program's admin has created the config; no position can open before then. */
  config: ProtocolConfig | null;
};

let last: ProgramSnapshot | null = null;
let inflight: Promise<ProgramSnapshot> | null = null;
const CLIENT_TTL_MS = 5_000;
/** Slot of this browser's latest confirmed transaction; fresh loads ask the server for a snapshot at least this recent. */
let minSlot = 0;

/** Records the slot a transaction confirmed in, so the next fresh load includes it. */
export function noteConfirmedSlot(slot: number) {
  minSlot = Math.max(minSlot, slot);
}

/** The accounts route URL; a fresh load bypasses caches and, after a transaction, waits for its slot. */
export function accountsUrl(fresh: boolean, now = Date.now()): string {
  if (!fresh) return "/api/accounts";
  return `/api/accounts?fresh=${now}${minSlot ? `&minSlot=${minSlot}` : ""}`;
}

async function load(fresh: boolean): Promise<ProgramSnapshot> {
  const res = await fetch(accountsUrl(fresh));
  const body = await res.json();
  if (!res.ok) throw new Error(body.error ?? `accounts request failed (${res.status})`);
  // Responses can arrive out of order, and edge caches can serve older copies.
  // Never replace a newer snapshot with an older one.
  if (last && body.at < last.at) return last;

  const agents: AgentAccount[] = [];
  const positions: PositionAccount[] = [];
  let config: ProtocolConfig | null = null;
  for (const { pubkey, data } of body.accounts as { pubkey: string; data: string }[]) {
    const buf = Buffer.from(data, "base64");
    const disc = buf.subarray(0, 8);
    const publicKey = new PublicKey(pubkey);
    try {
      if (disc.equals(AGENT_DISC)) {
        agents.push(decodeAgent({ publicKey, account: coder.decode("agent", buf) }));
      } else if (disc.equals(POSITION_DISC)) {
        positions.push(decodePosition({ publicKey, account: coder.decode("position", buf) }));
      } else if (disc.equals(CONFIG_DISC)) {
        config = decodeConfig(coder.decode("config", buf));
      }
    } catch {
      // Skip accounts that don't match the current layout.
    }
  }
  last = { at: body.at, fetchedAt: Date.now(), stale: body.stale, agents, positions, config };
  return last;
}

/**
 * All agents and positions, via the cached server route. Concurrent callers
 * share one request; `fresh` skips caches after the user sends a transaction.
 */
export function fetchProgramAccounts(fresh = false): Promise<ProgramSnapshot> {
  if (!fresh && last && Date.now() - last.fetchedAt < CLIENT_TTL_MS) return Promise.resolve(last);
  if (!fresh && inflight) return inflight;
  const p = load(fresh).finally(() => {
    if (inflight === p) inflight = null;
  });
  inflight = p;
  return p;
}
