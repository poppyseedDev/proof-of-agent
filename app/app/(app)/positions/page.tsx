"use client";

import Link from "next/link";
import { useWallet } from "@solana/wallet-adapter-react";
import { useAgents, useActions, usePositions } from "@/lib/useProtocol";
import { PositionAccount, feePct, pct, short, sol } from "@/lib/program";
import { TxNotice } from "@/components/TxNotice";
import { Avatar } from "@/components/Avatar";
import { IconRefresh } from "@/components/Icons";

function fmtTime(ts: number) {
  if (!ts) return "—";
  return new Date(ts * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

export default function Positions() {
  const { publicKey } = useWallet();
  const { positions, loading, error, refresh } = usePositions(publicKey ? { trader: publicKey } : null);
  const { agents } = useAgents();
  const actions = useActions();
  const now = Math.floor(Date.now() / 1000);
  /** Past the deadline: a custody position is settled from its vault; one drawn before custody claims the bond. */
  const settleOrClaim = async (p: PositionAccount) => {
    const agent = agents.find((a) => a.publicKey.equals(p.agent));
    if (!agent) throw new Error("agent not loaded yet");
    const custody = await actions.custodyOf(p);
    return custody ? actions.settleLate(agent, p) : actions.claimDefault(p);
  };
  const agentName = (k: PositionAccount["agent"]) =>
    agents.find((a) => a.publicKey.equals(k))?.name ?? short(k);

  if (!publicKey) return <div className="empty">Connect your wallet to see your positions.</div>;

  const live = positions.filter((p) => p.status === "open" || p.status === "trading");
  const closed = positions.filter((p) => p.status === "settled" || p.status === "defaulted");
  const sum = (list: PositionAccount[], f: (p: PositionAccount) => number) => list.reduce((n, p) => n + f(p), 0);
  const atWork = sum(live, (p) => p.principal.toNumber());
  const guaranteed = sum(live, (p) => p.lockedCollateral.toNumber());
  const closedPrincipal = sum(closed, (p) => p.principal.toNumber());
  const paidOut = sum(closed, (p) => p.returned.toNumber() - p.feePaid.toNumber() + p.slashed.toNumber());
  const collateralReceived = sum(closed, (p) => p.slashed.toNumber());
  const feesPaid = sum(closed, (p) => p.feePaid.toNumber());
  const profit = paidOut - closedPrincipal;
  const profitPct = closedPrincipal > 0 ? (profit / closedPrincipal) * 100 : null;
  const signed = (lamports: number) => `${lamports >= 0 ? "+" : "−"}${sol(Math.abs(lamports), 3)}`;

  return (
    <>
      <div className="stats rise">
        <div className="stat">
          <span className="k">Net profit</span>
          <span className={"v " + (closed.length === 0 ? "" : profit >= 0 ? "pos" : "neg")}>
            {closed.length === 0 ? "—" : signed(profit)}
            {closed.length > 0 && <small>SOL</small>}
          </span>
          <span className="sub">
            {profitPct === null
              ? "Appears once a position closes"
              : `${profitPct >= 0 ? "+" : ""}${profitPct.toFixed(1)}% on ${sol(closedPrincipal)} SOL closed`}
          </span>
        </div>
        <div className="stat">
          <span className="k">Paid out to you</span>
          <span className="v">{sol(paidOut, 3)}<small>SOL</small></span>
          <span className="sub">
            {closed.length} closed · {sol(feesPaid, 3)} SOL in agent fees
          </span>
        </div>
        <div className="stat">
          <span className="k">Capital at work</span>
          <span className="v">{sol(atWork)}<small>SOL</small></span>
          <span className="sub">
            {live.length} active · <span className="cy">{sol(guaranteed)} SOL guaranteed</span>
          </span>
        </div>
        <div className="stat">
          <span className="k">Collateral received</span>
          <span className={"v " + (collateralReceived > 0 ? "cy" : "")}>{sol(collateralReceived, 3)}<small>SOL</small></span>
          <span className="sub">Paid from agents' bonds when they fell short</span>
        </div>
      </div>
      <div className="panel rise d1">
      <div className="sec-head">
        <h2>My positions</h2>
        <span className="meta">
          {loading ? "Syncing…" : error ? "Couldn't load" : `${positions.length} total`}
          <button className="btn ghost sm icon-btn" onClick={refresh} aria-label="Refresh"><IconRefresh /></button>
        </span>
      </div>
      <TxNotice tx={actions.tx} />
      {error && positions.length === 0 && !loading ? (
        <div className="empty">
          Couldn&apos;t load your positions. <button type="button" className="rules-toggle" style={{ padding: 0 }} onClick={refresh}>Retry</button>
        </div>
      ) : positions.length === 0 && !loading ? (
        <div className="empty">You have not allocated capital to any agent yet.</div>
      ) : (
        <div className="table-wrap">
        <table className="ledger">
          <thead>
            <tr>
              <th>Agent</th>
              <th>Status</th>
              <th className="num">Principal</th>
              <th className="num">Guaranteed</th>
              <th className="num hide-sm">Outcome</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {positions.map((p) => {
              const deadline = p.deadline.toNumber();
              const expired = now >= deadline;
              const payout = p.returned.toNumber() - p.feePaid.toNumber() + p.slashed.toNumber();
              return (
                <tr key={p.publicKey.toBase58()}>
                  <td>
                    <div className="agent-cell">
                      <Avatar seed={p.agent.toBase58()} name={agentName(p.agent)} />
                      <div>
                        <div className="agent-name">
                          <Link href={`/agents/${p.agent.toBase58()}`} className="name-link">{agentName(p.agent)}</Link>
                        </div>
                        <div className="tiny">
                          {p.status === "open" || p.status === "trading" ? (
                            <span className={expired && p.status === "trading" ? "neg" : undefined}>
                              {expired && p.status === "trading" ? "Overdue since" : "Due"} {fmtTime(deadline)}
                            </span>
                          ) : p.closedAt.toNumber() > 0 ? (
                            <>Closed {fmtTime(p.closedAt.toNumber())}</>
                          ) : (
                            <>Opened {fmtTime(p.openedAt.toNumber())}</>
                          )}
                        </div>
                      </div>
                    </div>
                  </td>
                  <td>
                    <span className={"pill " + p.status} style={{ textTransform: "capitalize" }}>{p.status}</span>
                  </td>
                  <td className="num">{sol(p.principal, 3)} SOL</td>
                  <td className="num cy">
                    {sol(p.lockedCollateral, 3)} SOL
                    <div className="tiny">fee {feePct(p.feeBps)} · tolerance {pct(p.maxDrawdownBps)}</div>
                  </td>
                  <td className="num hide-sm">
                    {p.status === "settled" || p.status === "defaulted" ? (
                      <>
                        <div>{sol(payout, 3)} SOL to you</div>
                        <div className="tiny">
                          {p.breach === "missedDeadline"
                            ? "agent missed the deadline"
                            : `returned ${sol(p.returned, 3)} · fee ${sol(p.feePaid, 3)}`}
                          {p.slashed.gtn(0) && <span className="cy"> · {sol(p.slashed, 3)} from bond</span>}
                        </div>
                        {p.breach !== "none" && (
                          <span className="pill defaulted" style={{ marginTop: 4 }}>
                            {p.breach === "drawdown" ? "Breach: drawdown exceeded" : "Breach: missed deadline"}
                          </span>
                        )}
                      </>
                    ) : (
                      "—"
                    )}
                  </td>
                  <td className="num">
                    {p.status === "open" && (
                      <button className="btn danger sm" onClick={() => actions.cancelPosition(p).then(refresh).catch(() => {})}>
                        Cancel
                      </button>
                    )}
                    {p.status === "trading" && (
                      <button
                        className="btn seal sm"
                        disabled={!expired}
                        title={expired ? "Deadline passed: settle the position from its vault, with the late penalty from the agent's bond" : "Available after the deadline"}
                        onClick={() => settleOrClaim(p).then(refresh).catch(() => {})}
                      >
                        {expired ? "Settle late" : "Awaiting settlement"}
                      </button>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
        </div>
      )}
      </div>
    </>
  );
}
