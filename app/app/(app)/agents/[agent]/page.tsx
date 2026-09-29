"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { useParams } from "next/navigation";
import { PublicKey } from "@solana/web3.js";
import { fetchProgramAccounts } from "@/lib/accounts";
import { useActions } from "@/lib/useProtocol";
import { AgentAccount, PositionAccount, ProtocolConfig, capacity, feePct, pct, short, sol } from "@/lib/program";
import { Avatar } from "@/components/Avatar";
import { Certificate } from "@/components/Certificate";
import { TxNotice } from "@/components/TxNotice";
import { TermsView } from "@/components/operator/TermsView";

const STATUS = { draft: ["Draft", "open"], active: ["Live", "accepting"], paused: ["Paused", "paused"] } as const;
const fmtTime = (ts: number) =>
  new Date(ts * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

export default function AgentPage() {
  const { agent: key } = useParams<{ agent: string }>();
  const actions = useActions();
  const [agent, setAgent] = useState<AgentAccount | null | undefined>(undefined);
  const [positions, setPositions] = useState<PositionAccount[]>([]);
  const [config, setConfig] = useState<ProtocolConfig | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const load = async (fresh = false) => {
    let pk: PublicKey;
    try {
      pk = new PublicKey(key);
    } catch {
      setAgent(null); // not a valid address: nothing to find
      return;
    }
    try {
      const snap = await fetchProgramAccounts(fresh);
      const a = snap.agents.find((x) => x.publicKey.equals(pk)) ?? null;
      setAgent(a);
      setConfig(snap.config);
      setPositions(a ? snap.positions.filter((p) => p.agent.equals(pk)) : []);
      setLoadError(null);
    } catch (e) {
      // Keep whatever was shown before; only an empty page turns into the error state.
      setLoadError((e as Error).message || "request failed");
    }
  };
  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  useEffect(() => {
    if (agent) document.title = `${agent.name} · Proof of Agent`;
  }, [agent]);

  if (agent === undefined && loadError) {
    return (
      <div className="empty">
        Couldn&apos;t load this agent. The network or RPC may be down.{" "}
        <button type="button" className="rules-toggle" style={{ padding: 0 }} onClick={() => load(true)}>Retry</button>
      </div>
    );
  }
  if (agent === undefined) return <div className="empty">Loading…</div>;
  if (agent === null || agent.status === "draft") {
    return <div className="empty">This agent doesn&apos;t exist or hasn&apos;t been published yet. <Link href="/" className="box-link">Browse agents</Link></div>;
  }

  const closed = positions
    .filter((p) => p.status === "settled" || p.status === "defaulted")
    .sort((a, b) => b.closedAt.cmp(a.closedAt));
  const principal = closed.reduce((n, p) => n + p.principal.toNumber(), 0);
  const traderPnl = closed.reduce((n, p) => n + p.returned.toNumber() - p.feePaid.toNumber() + p.slashed.toNumber() - p.principal.toNumber(), 0);
  const ret = principal > 0 ? (traderPnl / principal) * 100 : null;
  const [statusLabel, statusClass] = STATUS[agent.status];

  return (
    <>
      <p className="tiny" style={{ marginBottom: 12 }}>
        <Link href="/" className="box-link">← All agents</Link>
      </p>
      <div className="split">
        <div>
          <div className="card rise">
            <h3>
              <Avatar seed={agent.publicKey.toBase58()} name={agent.name} size={40} />
              <span>
                {agent.name}
                <span className="tiny" style={{ display: "block", fontWeight: 400 }}>
                  {agent.description} · operator {short(agent.operator)}
                </span>
              </span>
              <span className={"pill " + statusClass} style={{ marginLeft: "auto" }}>{statusLabel}</span>
            </h3>
            <div className="stats">
              <div className="stat"><span className="k">Return to traders</span>
                <span className={"v " + (ret === null ? "" : ret >= 0 ? "pos" : "neg")}>{ret === null ? "—" : `${ret >= 0 ? "+" : ""}${ret.toFixed(1)}%`}</span>
                <span className="sub">{closed.length} closed · after fees and collateral</span></div>
              <div className="stat"><span className="k">Collateral</span><span className="v">{sol(agent.totalCollateral)}<small>SOL</small></span>
                <span className="sub">{sol(agent.lockedCollateral)} SOL reserved</span></div>
              <div className="stat"><span className="k">Capacity left</span><span className="v cy">{sol(capacity(agent, config))}<small>SOL</small></span>
                <span className="sub">{sol(agent.capitalManaged)} SOL managed now</span></div>
              <div className="stat"><span className="k">Breaches</span>
                <span className={"v " + (agent.breachCount ? "neg" : "")}>{agent.breachCount}</span>
                <span className="sub">{sol(agent.slashedTotal, 3)} SOL paid to traders</span></div>
            </div>
          </div>

          <div className="card rise d1">
            <h3>Published terms</h3>
            <p className="tiny" style={{ marginTop: -6 }}>Published {fmtTime(agent.publishedAt.toNumber())}. These terms cannot change.</p>
            <TermsView agent={agent} />
          </div>

          <div className="card rise d2">
            <h3>Track record</h3>
            {closed.length === 0 ? (
              <div className="empty">No closed positions yet.</div>
            ) : (
              <div className="table-wrap">
                <table className="ledger">
                  <thead><tr><th>Closed</th><th className="num">Principal</th><th className="num">Result for trader</th><th className="num hide-sm">Fee</th><th>Outcome</th></tr></thead>
                  <tbody>
                    {closed.map((p) => {
                      const pr = p.principal.toNumber();
                      const net = p.returned.toNumber() - p.feePaid.toNumber() + p.slashed.toNumber() - pr;
                      return (
                        <tr key={p.publicKey.toBase58()}>
                          <td><div>{fmtTime(p.closedAt.toNumber())}</div><div className="tiny mono">{short(p.trader)}</div></td>
                          <td className="num">{sol(pr, 3)} SOL</td>
                          <td className={"num " + (net >= 0 ? "pos" : "neg")}>{net >= 0 ? "+" : ""}{((net / pr) * 100).toFixed(1)}%</td>
                          <td className="num hide-sm">{sol(p.feePaid, 3)}</td>
                          <td>
                            {p.breach === "none" ? <span className="pill settled">Within terms</span>
                              : <span className="pill defaulted">{p.breach === "drawdown" ? "Breach: drawdown" : "Breach: missed deadline"}</span>}
                            {p.slashed.gtn(0) && <div className="tiny cy">{sol(p.slashed, 3)} SOL from collateral</div>}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </div>

        <div className="sticky rise d2">
          <TxNotice tx={actions.tx} />
          <Certificate
            agent={agent}
            config={config}
            connected={actions.connected}
            busy={actions.tx.kind === "pending"}
            tx={actions.tx}
            onOpen={async (lamports, secs) => {
              try {
                await actions.openPosition(agent, lamports, secs);
                await load(true);
              } catch {}
            }}
          />
          <p className="tiny" style={{ marginTop: 10 }}>Fee {feePct(agent.terms.feeBps)} of profit · collateral {pct(agent.terms.collateralRatioBps)} of your deposit.</p>
        </div>
      </div>
    </>
  );
}
