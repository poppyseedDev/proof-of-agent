"use client";

import { useRef, useState } from "react";
import Link from "next/link";
import { useAgents, useActions } from "@/lib/useProtocol";
import { WAITLIST_URL, assetLabel, capacity, fmtDuration, feePct, freeCollateral, pct, short, sol } from "@/lib/program";
import { Certificate } from "@/components/Certificate";
import { TxNotice } from "@/components/TxNotice";
import { Avatar } from "@/components/Avatar";
import { IconRefresh } from "@/components/Icons";

function tierOf(ratioBps: number) {
  if (ratioBps >= 7_500) return { label: "Fully bonded", cls: "gold" };
  if (ratioBps >= 3_000) return { label: "Bonded", cls: "bond" };
  return { label: "Light bond", cls: "ink" };
}

const STEPS = [
  ["Operator publishes terms", "Before launch, the operator publishes the rules, fee, trading window, drawdown limit and assets, and deposits collateral into the protocol’s vault."],
  ["You allocate", "Deposit 1,000 with a 30% agent and the protocol reserves 300 of its bond as your guarantee. The bond stays in the vault."],
  ["Agent trades", "The agent draws your principal and must return it before your deadline."],
  ["Settle or slash", "Follow the mandate, earn the fee. Break the mandate, risk the bond. Market losses don’t count."],
];

export default function Marketplace() {
  const { agents, stats, config, loading, error, refresh } = useAgents();
  const actions = useActions();
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const widgetRef = useRef<HTMLDivElement>(null);
  const selected = agents.find((a) => a.publicKey.toBase58() === selectedKey) ?? null;

  const totalCollateral = agents.reduce((n, a) => n + a.totalCollateral.toNumber(), 0);
  const totalManaged = agents.reduce((n, a) => n + a.capitalManaged.toNumber(), 0);

  return (
    <>
      <section className="hero rise">
        <div>
          <div className="eyebrow">
            <span className="dot" /> Over-collateralized AI agents on Solana
          </div>
          <h1>
            Trade with agents that <em>put up collateral</em>
          </h1>
          <p>
            Every agent locks its own SOL against the capital you allocate. More collateral earns a higher fee. If
            the agent misbehaves, the program pays the collateral to you.
          </p>
          <div className="hero-ctas">
            <a href={WAITLIST_URL} className="btn">Join the waitlist</a>
            <Link href="/how-it-works" className="hero-link">How it works →</Link>
          </div>
        </div>
        <div className="stats">
          <div className="stat">
            <span className="k">Agents</span>
            <span className="v">{agents.length}</span>
          </div>
          <div className="stat">
            <span className="k">Collateral posted</span>
            <span className="v">
              {sol(totalCollateral)}
              <small>SOL</small>
            </span>
          </div>
          <div className="stat">
            <span className="k">Capital managed</span>
            <span className="v">
              {sol(totalManaged)}
              <small>SOL</small>
            </span>
          </div>
        </div>
      </section>

      <div className="split">
        <div className="panel rise d1">
          <div className="sec-head">
            <h2>Agents</h2>
            <span className="meta">
              {loading ? "Syncing…" : `${agents.length} listed`}
              <button className="btn ghost sm icon-btn" onClick={refresh} aria-label="Refresh">
                <IconRefresh />
              </button>
            </span>
          </div>

          {error && <div className="notice">RPC error: {error}</div>}
          <TxNotice tx={actions.tx} />

          {loading && agents.length === 0 ? (
            <>
              <div className="skeleton" />
              <div className="skeleton" />
              <div className="skeleton" />
            </>
          ) : agents.length === 0 ? (
            <div className="empty">No agents have posted collateral yet. Register one from the agent console.</div>
          ) : (
            <div className="table-wrap">
              <table className="ledger">
                <thead>
                  <tr>
                    <th>Agent</th>
                    <th className="num">Collateral</th>
                    <th className="num">Fee</th>
                    <th className="num hide-sm">Available bond</th>
                  </tr>
                </thead>
                <tbody>
                  {agents.map((a) => {
                    const tier = tierOf(a.terms.collateralRatioBps);
                    const free = freeCollateral(a).toNumber();
                    const total = a.totalCollateral.toNumber();
                    const used = total ? 1 - free / total : 0;
                    const st = stats[a.publicKey.toBase58()];
                    const ret = st && st.principal > 0 ? (st.traderPnl / st.principal) * 100 : null;
                    const retText = ret === null ? null : `${ret >= 0 ? "+" : ""}${ret.toFixed(1)}%`;
                    const isSel = selected?.publicKey.equals(a.publicKey);
                    return (
                      <tr
                        key={a.publicKey.toBase58()}
                        className={"row" + (isSel ? " selected" : "")}
                        onClick={() => {
                          setSelectedKey(a.publicKey.toBase58());
                          if (window.matchMedia("(max-width: 1100px)").matches) {
                            setTimeout(() => widgetRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }), 60);
                          }
                        }}
                      >
                        <td>
                          <div className="agent-cell">
                            <Avatar seed={a.publicKey.toBase58()} name={a.name} />
                            <div>
                              <div className="agent-name">
                                <Link href={`/agents/${a.publicKey.toBase58()}`} className="name-link" onClick={(e) => e.stopPropagation()}>
                                  {a.name}
                                </Link>
                                {a.status === "paused" && <span className="pill paused">Paused</span>}
                              </div>
                              <div className="agent-strategy">{a.description || short(a.operator)}</div>
                              <div className="tiny">
                                {a.terms.allowedAssets.map((m) => assetLabel(m)).join(" · ")} ·{" "}
                                {fmtDuration(a.terms.minDurationSecs.toNumber())}–{fmtDuration(a.terms.maxDurationSecs.toNumber())}
                              </div>
                              <div className="tiny">
                                {retText ? (
                                  <span className={"ret " + (ret! >= 0 ? "pos" : "neg")}>{retText} to traders</span>
                                ) : (
                                  <span>No history yet</span>
                                )}
                                {" · "}
                                {a.settledPositions} settled · {a.openPositions} open
                                {a.breachCount > 0 && (
                                  <span className="neg">
                                    {" "}· {a.breachCount} breach{a.breachCount === 1 ? "" : "es"}
                                  </span>
                                )}
                              </div>
                            </div>
                          </div>
                        </td>
                        <td className="num">
                          <div className="big">{pct(a.terms.collateralRatioBps)}</div>
                          <span className={"tier " + tier.cls}>{tier.label}</span>
                        </td>
                        <td className="num">
                          <div className="big pos">{feePct(a.terms.feeBps)}</div>
                          <div className="tiny">of profit</div>
                        </td>
                        <td className="num hide-sm">
                          <div>
                            {sol(free)} <span className="tiny">/ {sol(total)} SOL</span>
                          </div>
                          <div className="bar">
                            <i className={used > 0.9 ? "warn" : ""} style={{ width: `${Math.max(100 - used * 100, 0)}%` }} />
                          </div>
                          <div className="tiny">Capacity {sol(capacity(a, config))} SOL</div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </div>

        <div className="sticky rise d2" ref={widgetRef} style={{ scrollMarginTop: 120 }}>
          <Certificate
            agent={selected}
            config={config}
            connected={actions.connected}
            busy={actions.tx.kind === "pending"}
            tx={actions.tx}
            onOpen={async (lamports, secs) => {
              if (!selected) return;
              try {
                await actions.openPosition(selected, lamports, secs);
                await refresh();
              } catch {}
            }}
          />
        </div>
      </div>

      <div className="steps">
        {STEPS.map(([title, body], i) => (
          <div key={title} className={`step rise d${i + 1}`}>
            <div className="n">{i + 1}</div>
            <h4>{title}</h4>
            <p>{body}</p>
          </div>
        ))}
      </div>
    </>
  );
}
