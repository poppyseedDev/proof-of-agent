"use client";

import { useState } from "react";
import Link from "next/link";
import { useWallet } from "@solana/wallet-adapter-react";
import { useBalance, type TxState } from "@/lib/useProtocol";
import {
  AgentAccount,
  assetLabel,
  capacity,
  feePct,
  fmtDuration,
  freeCollateral,
  pct,
  requiredCollateral,
  sol,
} from "@/lib/program";
import { lamportsToInput, parseSolInput } from "@/lib/amounts";
import { Avatar } from "./Avatar";
import { IconArrowDown, IconShield } from "./Icons";
import { Faucet } from "./Faucet";

const PRESETS = [
  { label: "1H", secs: 3_600 },
  { label: "1D", secs: 86_400 },
  { label: "7D", secs: 7 * 86_400 },
  { label: "30D", secs: 30 * 86_400 },
];

/** Deadline choices inside the agent's published trading window. */
function durationsFor(agent: AgentAccount) {
  const min = agent.terms.minDurationSecs.toNumber();
  const max = agent.terms.maxDurationSecs.toNumber();
  const inside = PRESETS.filter((d) => d.secs >= min && d.secs <= max);
  if (inside.length) return inside;
  const short = (s: number) => fmtDuration(s).replace(/ hours?/, "H").replace(/ days?/, "D").replace(" min", "M");
  return min === max ? [{ label: short(min), secs: min }] : [
    { label: short(min), secs: min },
    { label: short(max), secs: max },
  ];
}

/** Swap-style widget: what a trader gets when allocating capital to an agent. */
export function Certificate({
  agent,
  connected,
  busy,
  tx,
  onOpen,
}: {
  agent: AgentAccount | null;
  connected: boolean;
  busy: boolean;
  /** Latest transaction state; a confirmed one refreshes the balance. */
  tx?: TxState;
  onOpen: (lamports: number, durationSecs: number) => void;
}) {
  // Half of what the devnet faucet sends, so a new tester can allocate straight away.
  const [amount, setAmount] = useState("0.1");
  const [picked, setPicked] = useState<number | null>(null);
  const [showRules, setShowRules] = useState(false);
  const { publicKey } = useWallet();
  const balance = useBalance(publicKey ?? null, tx);
  // Leave a little SOL for fees and the position vault's rent deposit.
  const maxLamports = balance === null ? 0 : Math.max(0, balance - 0.01 * 1e9);

  const head = (
    <div className="cert-head">
      <div className="cert-tabs">
        <span className="cert-tab active">Allocate</span>
        {agent && (
          <Link href={`/agents/${agent.publicKey.toBase58()}`} className="cert-agent box-link">
            to {agent.name}
          </Link>
        )}
      </div>
      <span className="stamp">
        <IconShield width={12} height={12} /> Bonded
      </span>
    </div>
  );

  if (!agent) {
    return (
      <div className="cert">
        {head}
        <div className="cert-empty">
          <div className="big-icon">
            <IconShield />
          </div>
          <div style={{ fontWeight: 600, color: "var(--text)", marginBottom: 4 }}>Select an agent</div>
          <div className="tiny">Pick an agent from the list to see how much collateral it guarantees for your deposit.</div>
        </div>
      </div>
    );
  }

  const durations = durationsFor(agent);
  const duration = durations.some((d) => d.secs === picked) ? picked! : durations[Math.min(1, durations.length - 1)].secs;
  const lamports = parseSolInput(amount);
  const guaranteed = requiredCollateral(lamports, agent.terms.collateralRatioBps);
  const cap = capacity(agent);
  const overCap = lamports > cap;
  const overBalance = balance !== null && lamports > maxLamports;
  const tolerance = Math.floor((lamports * agent.terms.maxDrawdownBps) / 10_000);

  const label = !connected
    ? "Connect wallet"
    : agent.status !== "active"
      ? "Agent paused"
      : !(lamports > 0)
        ? "Enter an amount"
        : overCap
          ? "Exceeds agent capacity"
          : overBalance
            ? "Not enough SOL"
          : busy
            ? "Confirming…"
            : "Allocate";

  return (
    <div className="cert rise">
      {head}

      <div className="box">
        <div className="box-label">
          <span>You allocate</span>
          {balance !== null && (
            <span className="box-balance">
              Balance {sol(balance)} SOL
              <button type="button" className="max-btn" onClick={() => setAmount(lamportsToInput(maxLamports))}>Max</button>
            </span>
          )}
        </div>
        <div className="box-row">
          <input
            className="amount"
            type="number"
            inputMode="decimal"
            min={0}
            step={0.1}
            placeholder="0.00"
            value={amount}
            onChange={(e) => setAmount(e.target.value)}
          />
          <span className="token">
            <span className="token-icon" />
            SOL
          </span>
        </div>
      </div>

      <div className="flip">
        <IconArrowDown width={16} height={16} />
      </div>

      <div className="box">
        <div className="box-label">
          <span>Guaranteed to you</span>
          <span>{pct(agent.terms.collateralRatioBps)} of deposit</span>
        </div>
        <div className="box-row">
          <span className="amount-static">{sol(guaranteed, 3)}</span>
          <span className="token">
            <Avatar seed={agent.publicKey.toBase58()} name={agent.name} size={26} />
            Bond
          </span>
        </div>
      </div>

      <div className="details">
        <div className="cert-row">
          <span className="k">Settlement deadline</span>
          <div className="segmented">
            {durations.map((d) => (
              <button key={d.secs} className={duration === d.secs ? "on" : ""} onClick={() => setPicked(d.secs)}>
                {d.label}
              </button>
            ))}
          </div>
        </div>
        <div className="cert-row">
          <span className="k">Performance fee</span>
          <span className="v seal">{feePct(agent.terms.feeBps)} of profit</span>
        </div>
        <div className="cert-row">
          <span className="k">Loss tolerance</span>
          <span className="v">
            {pct(agent.terms.maxDrawdownBps)} · {sol(tolerance, 3)} SOL
          </span>
        </div>
        <div className="cert-row">
          <span className="k">Allowed assets</span>
          <span className="v">{agent.terms.allowedAssets.map((m) => assetLabel(m)).join(", ")}</span>
        </div>
        <div className="cert-row">
          <span className="k">Breaches</span>
          <span className={"v " + (agent.breachCount > 0 ? "neg" : "")}>
            {agent.breachCount} of {agent.settledPositions + agent.defaultedPositions} closed
          </span>
        </div>
        <div className="cert-row">
          <span className="k">Agent capacity</span>
          <span className={"v " + (overCap ? "neg" : "")}>{sol(cap)} SOL</span>
        </div>
      </div>

      <button className="rules-toggle" onClick={() => setShowRules((v) => !v)}>
        {showRules ? "Hide" : "Read"} the published rules
      </button>
      {showRules && <div className="rules-box">{agent.terms.rules}</div>}

      <p className="note">
        If the agent returns less than {pct(10_000 - agent.terms.maxDrawdownBps)} of your SOL, or misses the deadline, up to{" "}
        <b>{sol(guaranteed, 3)} SOL</b> of its bond is paid to you. Free bond now: {sol(freeCollateral(agent))} SOL.
      </p>

      {connected && overBalance && (
        <div className="actions" style={{ marginBottom: 10, justifyContent: "center" }}>
          <Faucet compact={false} />
        </div>
      )}
      <button
        className="btn lg"
        disabled={!connected || busy || overCap || overBalance || agent.status !== "active" || !(lamports > 0)}
        onClick={() => onOpen(lamports, duration)}
      >
        {label}
      </button>
    </div>
  );
}
