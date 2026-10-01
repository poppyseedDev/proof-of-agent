"use client";

import { useEffect, useMemo, useState } from "react";
import { useConnection, useWallet } from "@solana/wallet-adapter-react";
import { LAMPORTS_PER_SOL, PublicKey } from "@solana/web3.js";
import { BN } from "@coral-xyz/anchor";
import Link from "next/link";
import { termsToInput, useActions, useBalance, useOperatorAgents } from "@/lib/useProtocol";
import {
  AgentAccount,
  PositionAccount,
  agentPda,
  capacity,
  feePct,
  freeCollateral,
  maxCapacity,
  pct,
  short,
  sol,
} from "@/lib/program";

import { lamportsToInput, parseNonNegativeSolInput, parseSolInput } from "@/lib/amounts";
import { TxNotice } from "@/components/TxNotice";
import { Avatar } from "@/components/Avatar";
import { AgentForm, AgentDraft, DEFAULT_DRAFT, draftProblems } from "@/components/operator/AgentForm";
import { TermsView } from "@/components/operator/TermsView";

type Actions = ReturnType<typeof useActions>;

const STATUS_LABEL = { draft: "Draft", active: "Live", paused: "Paused" } as const;
const STATUS_CLASS = { draft: "open", active: "accepting", paused: "paused" } as const;

const fmtTime = (ts: number) =>
  ts ? new Date(ts * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "—";

export default function OperatorConsole() {
  const { publicKey } = useWallet();
  const actions = useActions();
  const { agents, positions, loading, loaded, error, refresh } = useOperatorAgents(publicKey ?? null);
  const [selected, setSelected] = useState<string | "new" | null>(null);

  useEffect(() => {
    if (selected === null && loaded) setSelected(agents[0]?.publicKey.toBase58() ?? "new");
  }, [agents, loaded, selected]);

  if (!publicKey) {
    return (
      <div className="empty">
        Connect the wallet you want to operate agents with. That wallet becomes the operator: it publishes terms,
        holds control of the collateral, and can bind a separate trading key.
      </div>
    );
  }

  const agent = agents.find((a) => a.publicKey.toBase58() === selected) ?? null;
  const nextId = agents.reduce((m, a) => BN.max(m, a.agentId.addn(1)), new BN(1));

  return (
    <>
      <div className="sec-head">
        <h2>Operator console</h2>
        <span className="meta">
          Operator {short(publicKey)}
          <button className="btn sm" onClick={() => setSelected("new")}>New agent</button>
        </span>
      </div>
      <TxNotice tx={actions.tx} />

      <div className="operator-layout">
        <aside className="agent-picker">
          {error && agents.length === 0 && !loading && (
            <div className="tiny neg" style={{ padding: 8 }}>
              Couldn&apos;t load your agents. <button type="button" className="rules-toggle" style={{ padding: 0 }} onClick={() => refresh()}>Retry</button>
            </div>
          )}
          {agents.length === 0 && !loading && !error && <div className="tiny" style={{ padding: 8 }}>No agents yet.</div>}
          {agents.map((a) => (
            <button
              key={a.publicKey.toBase58()}
              className={"picker-item" + (selected === a.publicKey.toBase58() ? " on" : "")}
              onClick={() => setSelected(a.publicKey.toBase58())}
            >
              <Avatar seed={a.publicKey.toBase58()} name={a.name} size={32} />
              <span className="picker-text">
                <span className="agent-name">{a.name}</span>
                <span className="tiny">
                  {sol(a.totalCollateral)} SOL bond · {a.openPositions} open
                </span>
              </span>
              <span className={"pill " + STATUS_CLASS[a.status]}>{STATUS_LABEL[a.status]}</span>
            </button>
          ))}
          <button className={"picker-item new" + (selected === "new" ? " on" : "")} onClick={() => setSelected("new")}>
            + Create an agent
          </button>
        </aside>

        <div className="operator-main">
          {selected === "new" ? (
            <CreateAgent
              actions={actions}
              nextId={nextId}
              onCreated={async (id) => {
                await refresh();
                setSelected(id);
              }}
            />
          ) : agent ? (
            <AgentManager
              key={agent.publicKey.toBase58()}
              agent={agent}
              positions={positions.filter((p) => p.agent.equals(agent.publicKey))}
              me={publicKey}
              actions={actions}
              refresh={refresh}
            />
          ) : (
            <div className="empty">{loading ? "Loading…" : error ? `Couldn't load your agents: ${error}` : "Select an agent."}</div>
          )}
        </div>
      </div>
    </>
  );
}

function CreateAgent({ actions, nextId, onCreated }: { actions: Actions; nextId: BN; onCreated: (key: string) => void }) {
  const { publicKey } = useWallet();
  const { connection } = useConnection();
  const [draft, setDraft] = useState<AgentDraft>(DEFAULT_DRAFT);
  const problems = draftProblems(draft);
  const busy = actions.tx.kind === "pending";

  return (
    <div className="card rise">
      <h3>Create an agent</h3>
      <p className="tiny" style={{ marginTop: -6 }}>
        Agents start as drafts. You can edit every term until you publish. Publishing requires a collateral deposit and
        makes the terms permanent.
      </p>
      <AgentForm value={draft} onChange={setDraft} />
      {problems.length > 0 && (
        <ul className="problems">{problems.map((p) => <li key={p}>{p}</li>)}</ul>
      )}
      <div className="actions">
        <button
          className="btn"
          disabled={busy || problems.length > 0 || !publicKey}
          onClick={async () => {
            try {
              // The listed agents can miss accounts the decoder skips (e.g. older layouts), so the
              // computed id may already be taken. Probe upward until the address is free.
              let id = nextId;
              for (let i = 0; i < 64 && (await connection.getAccountInfo(agentPda(publicKey!, id), "confirmed")); i++) {
                id = id.addn(1);
              }
              await actions.createAgent(id, draft.name.trim(), draft.description.trim(), draft.terms);
              onCreated(agentPda(publicKey!, id).toBase58());
            } catch {}
          }}
        >
          Create draft
        </button>
      </div>
    </div>
  );
}

function AgentManager({
  agent,
  positions,
  me,
  actions,
  refresh,
}: {
  agent: AgentAccount;
  positions: PositionAccount[];
  me: PublicKey;
  actions: Actions;
  refresh: () => Promise<void>;
}) {
  const busy = actions.tx.kind === "pending";
  const isDraft = agent.status === "draft";
  const done = () => refresh();
  const live = positions.filter((p) => p.status === "open" || p.status === "trading");
  const closed = positions
    .filter((p) => p.status === "settled" || p.status === "defaulted")
    .sort((a, b) => b.closedAt.cmp(a.closedAt));
  const breaches = closed.filter((p) => p.breach !== "none");
  const [tab, setTab] = useState<"overview" | "terms" | "positions" | "history" | "breaches">(isDraft ? "terms" : "overview");

  return (
    <div className="rise">
      <div className="card">
        <h3>
          <Avatar seed={agent.publicKey.toBase58()} name={agent.name} size={34} />
          <span>
            {agent.name}
            <span className="tiny" style={{ display: "block", fontWeight: 400 }}>{agent.description}</span>
          </span>
          <span className={"pill " + STATUS_CLASS[agent.status]} style={{ marginLeft: "auto" }}>
            {STATUS_LABEL[agent.status]}
          </span>
        </h3>
        {agent.status !== "draft" && (
          <p className="tiny" style={{ marginTop: -8 }}>
            <Link href={`/agents/${agent.publicKey.toBase58()}`} className="box-link">View the public agent page →</Link>
          </p>
        )}
        <div className="stats">
          <Stat k="Collateral" v={sol(agent.totalCollateral)} unit="SOL" />
          <Stat k="Reserved" v={sol(agent.lockedCollateral)} unit="SOL" />
          <Stat k="Max manageable capital" v={sol(maxCapacity(agent))} unit="SOL" tone="cy" sub={`${sol(capacity(agent))} SOL still available`} />
          <Stat k="Capital managed" v={sol(agent.capitalManaged)} unit="SOL" sub={`${agent.openPositions} open positions`} />
          <Stat k="Fees earned" v={sol(agent.feesEarned, 3)} unit="SOL" tone="pos" />
          <Stat k="Breaches" v={String(agent.breachCount)} tone={agent.breachCount ? "neg" : undefined} sub={`${sol(agent.slashedTotal, 3)} SOL paid to traders`} />
        </div>
        <div className="tabs">
          {(["overview", "terms", "positions", "history", "breaches"] as const).map((t) => (
            <button key={t} className={"tab" + (tab === t ? " on" : "")} onClick={() => setTab(t)}>
              {{ overview: "Collateral & keys", terms: isDraft ? "Terms & publish" : "Published terms", positions: `Positions (${live.length})`, history: `Settlements (${closed.length})`, breaches: `Breaches (${breaches.length})` }[t]}
            </button>
          ))}
        </div>
      </div>

      {tab === "overview" && <Overview agent={agent} me={me} actions={actions} busy={busy} done={done} />}
      {tab === "terms" && (isDraft ? <DraftTerms agent={agent} actions={actions} busy={busy} done={done} /> : (
        <div className="card">
          <h3>Published terms</h3>
          <p className="tiny" style={{ marginTop: -6 }}>Published {fmtTime(agent.publishedAt.toNumber())}. These terms can no longer change.</p>
          <TermsView agent={agent} />
        </div>
      ))}
      {tab === "positions" && <LivePositions agent={agent} positions={live} me={me} actions={actions} busy={busy} done={done} />}
      {tab === "history" && <History rows={closed} />}
      {tab === "breaches" && <Breaches rows={breaches} />}
    </div>
  );
}

function Stat({ k, v, unit, sub, tone }: { k: string; v: string; unit?: string; sub?: string; tone?: string }) {
  return (
    <div className="stat">
      <span className="k">{k}</span>
      <span className={"v " + (tone ?? "")}>
        {v}
        {unit && <small>{unit}</small>}
      </span>
      {sub && <span className="sub">{sub}</span>}
    </div>
  );
}

function Overview({ agent, me, actions, busy, done }: { agent: AgentAccount; me: PublicKey; actions: Actions; busy: boolean; done: () => Promise<void> }) {
  const [amt, setAmt] = useState("1");
  const balance = useBalance(me, actions.tx);
  const maxDeposit = balance === null ? 0 : Math.max(0, balance - 0.01 * 1e9);
  const [key, setKey] = useState("");
  const lamports = parseSolInput(amt);
  const keyValid = (() => {
    try {
      return key.length > 30 && !!new PublicKey(key);
    } catch {
      return false;
    }
  })();
  const executorIsOperator = agent.executor.equals(agent.operator);
  const isOperator = agent.operator.equals(me);

  return (
    <>
      <div className="card">
        <h3>Collateral</h3>
        <p className="tiny" style={{ marginTop: -6 }}>
          Held in the protocol&apos;s vault. Only free collateral can be withdrawn; reserved collateral backs open
          positions. Free now: <b>{sol(freeCollateral(agent))} SOL</b>.
        </p>
        {balance !== null && (
          <p className="tiny">
            Wallet balance {sol(balance)} SOL ·{" "}
            <button type="button" className="rules-toggle" style={{ padding: 0 }} onClick={() => setAmt(lamportsToInput(maxDeposit))}>Deposit max</button>
            {" · "}
            <button type="button" className="rules-toggle" style={{ padding: 0 }} onClick={() => setAmt(lamportsToInput(freeCollateral(agent).toNumber()))}>Withdraw all free</button>
          </p>
        )}
        <div className="actions">
          <input type="number" min={0} step={0.1} value={amt} onChange={(e) => setAmt(e.target.value)} style={{ width: 140 }} />
          <span className="tiny">SOL</span>
          <button className="btn" disabled={busy || !isOperator || !(lamports > 0) || (balance !== null && lamports > maxDeposit)}
            onClick={() => actions.depositCollateral(agent, lamports).then(done).catch(() => {})}>Deposit</button>
          <button className="btn ghost" disabled={busy || !isOperator || !(lamports > 0)}
            onClick={() => actions.withdrawCollateral(agent, lamports).then(done).catch(() => {})}>Withdraw free</button>
        </div>
        {lamports > 0 && (
          <p className="tiny">
            Depositing {amt} SOL raises max manageable capital by {sol(Math.floor((lamports * 10_000) / agent.terms.collateralRatioBps))} SOL.
          </p>
        )}
      </div>

      <div className="card">
        <h3>Trading key</h3>
        <p className="tiny" style={{ marginTop: -6 }}>
          The trading key can draw and settle positions, and nothing else. Bind your AI&apos;s hot wallet here and keep
          this operator wallet offline. The operator can always act too.
        </p>
        <div className="cert-row">
          <span className="k">Bound key</span>
          <span className="v mono wrap">{executorIsOperator ? "Operator wallet" : agent.executor.toBase58()}</span>
        </div>
        <div className="actions">
          <input type="text" placeholder="Trading key address" value={key} onChange={(e) => setKey(e.target.value.trim())} style={{ flex: 1, minWidth: 0 }} />
          <button className="btn" disabled={busy || !isOperator || !keyValid}
            onClick={() => actions.setExecutor(agent, new PublicKey(key)).then(done).then(() => setKey("")).catch(() => {})}>Bind</button>
          {!executorIsOperator && (
            <button className="btn ghost" disabled={busy || !isOperator}
              onClick={() => actions.setExecutor(agent, agent.operator).then(done).catch(() => {})}>Unbind</button>
          )}
        </div>
      </div>

      {agent.status !== "draft" && (
        <div className="card">
          <h3>New positions</h3>
          <p className="tiny" style={{ marginTop: -6 }}>
            {agent.status === "active"
              ? "Traders can allocate to this agent. Pausing stops new positions; open ones are unaffected."
              : "Paused. Traders cannot open new positions until you resume."}
          </p>
          <button className={"btn " + (agent.status === "active" ? "danger" : "")} disabled={busy || !isOperator}
            onClick={() => actions.setAccepting(agent, agent.status !== "active").then(done).catch(() => {})}>
            {agent.status === "active" ? "Pause agent" : "Resume agent"}
          </button>
        </div>
      )}
    </>
  );
}

function DraftTerms({ agent, actions, busy, done }: { agent: AgentAccount; actions: Actions; busy: boolean; done: () => Promise<void> }) {
  const saved: AgentDraft = useMemo(
    () => ({ name: agent.name, description: agent.description, terms: termsToInput(agent.terms) }),
    [agent],
  );
  const [draft, setDraft] = useState<AgentDraft>(saved);
  const [ack, setAck] = useState(false);
  const dirty = JSON.stringify(draft) !== JSON.stringify(saved);
  const problems = draftProblems(draft);

  const checks = [
    { ok: !!saved.name.trim(), label: "Name set" },
    { ok: saved.terms.allowedAssets.length > 0, label: "Allowed assets chosen" },
    { ok: !!saved.terms.rules.trim(), label: "Trading rules written" },
    { ok: agent.totalCollateral.gtn(0), label: "Collateral deposited (Collateral & keys tab)" },
    { ok: !dirty, label: "All edits saved" },
  ];
  const ready = checks.every((c) => c.ok) && ack;

  return (
    <>
      <div className="card">
        <h3>Draft terms</h3>
        <AgentForm value={draft} onChange={setDraft} bondSol={agent.totalCollateral.toNumber() / LAMPORTS_PER_SOL} />
        {problems.length > 0 && <ul className="problems">{problems.map((p) => <li key={p}>{p}</li>)}</ul>}
        <div className="actions">
          <button className="btn" disabled={busy || !dirty || problems.length > 0}
            onClick={() => actions.updateAgent(agent, draft.name.trim(), draft.description.trim(), draft.terms).then(done).catch(() => {})}>
            Save draft
          </button>
          {dirty && <button className="btn ghost" onClick={() => setDraft(saved)}>Discard changes</button>}
        </div>
      </div>

      <div className="card publish-card">
        <h3>Publish</h3>
        <p className="tiny" style={{ marginTop: -6 }}>
          Publishing makes these rules and collateral terms permanent and visible to every trader. Traders can allocate
          as soon as it confirms.
        </p>
        <ul className="checklist">
          {checks.map((c) => (
            <li key={c.label} className={c.ok ? "ok" : ""}>{c.ok ? "✓" : "○"} {c.label}</li>
          ))}
        </ul>
        <label className="ack">
          <input type="checkbox" checked={ack} onChange={(e) => setAck(e.target.checked)} />
          I have reviewed the terms: {pct(saved.terms.collateralRatioBps)} collateral, {feePct(saved.terms.feeBps)} fee,{" "}
          {pct(saved.terms.maxDrawdownBps)} max drawdown. I understand they cannot be changed after publishing.
        </label>
        <button className="btn lg" disabled={busy || !ready}
          onClick={() => actions.publishAgent(agent).then(done).catch(() => {})}>
          Publish agent
        </button>
      </div>
    </>
  );
}

function LivePositions({ agent, positions, me, actions, busy, done }: { agent: AgentAccount; positions: PositionAccount[]; me: PublicKey; actions: Actions; busy: boolean; done: () => Promise<void> }) {
  const [ret, setRet] = useState<Record<string, string>>({});
  const canExecute = agent.executor.equals(me) || agent.operator.equals(me);
  const now = Math.floor(Date.now() / 1000);
  if (positions.length === 0) return <div className="card"><div className="empty">No open positions.</div></div>;
  return (
    <div className="card">
      <h3>Open positions</h3>
      {!canExecute && <p className="tiny">Connect the operator wallet or the bound trading key to draw and settle.</p>}
      <div className="table-wrap">
        <table className="ledger">
          <thead>
            <tr><th>Trader</th><th>Status</th><th className="num">Principal</th><th className="num">Reserved</th><th></th></tr>
          </thead>
          <tbody>
            {positions.map((p) => {
              const key = p.publicKey.toBase58();
              const deadline = p.deadline.toNumber();
              const late = now >= deadline;
              return (
                <tr key={key}>
                  <td>
                    <div className="mono">{short(p.trader)}</div>
                    <div className={"tiny " + (late ? "neg" : "")}>
                      {late ? "Overdue since" : "Due"} {fmtTime(deadline)}
                      {late && p.status === "open" && " · too late to draw"}
                    </div>
                  </td>
                  <td><span className={"pill " + p.status} style={{ textTransform: "capitalize" }}>{p.status}</span></td>
                  <td className="num">{sol(p.principal, 3)} SOL</td>
                  <td className="num cy">{sol(p.lockedCollateral, 3)} SOL</td>
                  <td className="num">
                    {p.status === "open" && (
                      <div className="actions" style={{ marginTop: 0, justifyContent: "flex-end" }}>
                        <button className="btn sm" disabled={busy || late || !canExecute}
                          title={late ? "The deadline has passed. Decline to refund the trader." : "Wrap the principal in the position's vault and start trading it through the program"}
                          onClick={() => actions.beginTrading(p).then(done).catch(() => {})}>Start trading</button>
                        <button className="btn ghost sm" disabled={busy || !canExecute}
                          title="Refund the trader in full. No fee, no breach."
                          onClick={() => actions.settlePosition(agent, p, 0).then(done).catch(() => {})}>Decline &amp; refund</button>
                      </div>
                    )}
                    {p.status === "trading" && (
                      <div className="actions" style={{ marginTop: 0, justifyContent: "flex-end" }}>
                        <input type="number" min={0} step={0.01} placeholder="SOL to return (legacy only)" value={ret[key] ?? ""}
                          title="Only for a position drawn before vault custody. A custody position settles on what its vault holds."
                          onChange={(e) => setRet({ ...ret, [key]: e.target.value })} style={{ width: 170 }} />
                        <button className="btn sm" disabled={busy || !canExecute}
                          title="Settles on the vault's wSOL balance. Everything else in the vault must be swapped back to SOL first."
                          onClick={() => {
                            const returned = parseNonNegativeSolInput(ret[key] ?? "") ?? 0;
                            actions.settlePosition(agent, p, returned).then(done).catch(() => {});
                          }}>Settle</button>
                      </div>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function History({ rows }: { rows: PositionAccount[] }) {
  if (rows.length === 0) return <div className="card"><div className="empty">No settlements yet.</div></div>;
  return (
    <div className="card">
      <h3>Settlement history</h3>
      <div className="table-wrap">
        <table className="ledger">
          <thead>
            <tr><th>Closed</th><th className="num">Principal</th><th className="num">Returned</th><th className="num">Result</th><th className="num">Fee</th><th className="num">Paid from bond</th></tr>
          </thead>
          <tbody>
            {rows.map((p) => {
              const principal = p.principal.toNumber();
              const r = p.returned.toNumber() - principal;
              return (
                <tr key={p.publicKey.toBase58()}>
                  <td>
                    <div>{fmtTime(p.closedAt.toNumber())}</div>
                    <div className="tiny mono">{short(p.trader)}</div>
                  </td>
                  <td className="num">{sol(principal, 3)}</td>
                  <td className="num">{p.status === "defaulted" ? "—" : sol(p.returned, 3)}</td>
                  <td className={"num " + (p.status === "defaulted" ? "neg" : r >= 0 ? "pos" : "neg")}>
                    {p.status === "defaulted" ? "Missed deadline" : `${r >= 0 ? "+" : ""}${((r / principal) * 100).toFixed(1)}%`}
                  </td>
                  <td className="num pos">{sol(p.feePaid, 3)}</td>
                  <td className={"num " + (p.slashed.gtn(0) ? "neg" : "")}>{sol(p.slashed, 3)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function Breaches({ rows }: { rows: PositionAccount[] }) {
  if (rows.length === 0) {
    return <div className="card"><div className="empty">No breaches. Every closed position stayed within the published terms.</div></div>;
  }
  return (
    <div className="card">
      <h3>Breach records</h3>
      <div className="table-wrap">
        <table className="ledger">
          <thead>
            <tr><th>When</th><th>Breach</th><th className="num">Principal</th><th className="num">Floor</th><th className="num">Returned</th><th className="num">Paid to trader</th></tr>
          </thead>
          <tbody>
            {rows.map((p) => {
              const principal = p.principal.toNumber();
              const floor = principal - Math.floor((principal * p.maxDrawdownBps) / 10_000);
              return (
                <tr key={p.publicKey.toBase58()}>
                  <td>
                    <div>{fmtTime(p.closedAt.toNumber())}</div>
                    <div className="tiny mono">{short(p.trader)}</div>
                  </td>
                  <td><span className="pill defaulted">{p.breach === "drawdown" ? "Drawdown exceeded" : "Missed deadline"}</span></td>
                  <td className="num">{sol(principal, 3)}</td>
                  <td className="num">{sol(floor, 3)}</td>
                  <td className="num">{p.breach === "missedDeadline" ? "—" : sol(p.returned, 3)}</td>
                  <td className="num neg">{sol(p.slashed, 3)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
