"use client";

import Link from "next/link";
import { useWallet } from "@solana/wallet-adapter-react";
import { useBalance } from "@/lib/useProtocol";
import { CLUSTER, sol } from "@/lib/program";
import { Faucet } from "@/components/Faucet";
import { TestWalletName } from "@/lib/testWallet";

const STEPS = [
  {
    t: "Pick a wallet",
    d: "Fastest: use a test wallet, which lives in this browser and needs nothing installed. Or use your own: in Phantom, Settings → Developer settings → turn on Testnet mode and pick Devnet (Solflare and Backpack have the same switch). Devnet SOL has no value, so nothing here is at risk.",
  },
  {
    t: "Get test SOL",
    d: "Press Get test SOL. We send 0.2 devnet SOL once per wallet. If you need more, use faucet.solana.com.",
  },
  {
    t: "Pick an agent and allocate",
    d: "On the Agents page, choose one and read its terms and rules. Allocate a small amount (0.05 to 0.2 SOL) and pick a deadline. The agent's collateral is reserved for you on the spot.",
  },
  {
    t: "Watch it happen",
    d: "Our agents draw within a minute and settle about 10 to 20 minutes after drawing. Follow it under My positions. Returned SOL, the agent's fee and any collateral paid to you show there when it closes.",
  },
  {
    t: "Try the edge cases",
    d: "Cancel a position before it is drawn for a full refund. Open the Operator console and publish your own agent with your own terms. Look at the Breach Demo agent to see what a slashed operator looks like.",
  },
];

export default function Start() {
  const { publicKey, wallet, select } = useWallet();
  const balance = useBalance(publicKey ?? null);
  return (
    <div className="waitlist">
      <div className="rise">
        <div className="eyebrow"><span className="dot" /> Devnet test drive</div>
        <h1 className="waitlist-title">Start testing in five minutes</h1>
        <p className="waitlist-lead">
          Everything on this site runs on Solana devnet with test SOL. You can allocate to our agents, watch them
          trade and settle, and see collateral change hands, without spending anything real.
        </p>
      </div>

      <div className="card rise d1">
        <div className="checklist-steps">
          {STEPS.map((s, i) => (
            <div key={s.t} className="cstep">
              <span className="n">{i + 1}</span>
              <div>
                <b>{s.t}</b>
                <p>{s.d}</p>
                {i === 0 && CLUSTER !== "mainnet-beta" && (
                  <div className="actions" style={{ marginTop: 6 }}>
                    {wallet?.adapter.name === TestWalletName && publicKey ? (
                      <span className="tiny">Using the test wallet {publicKey.toBase58().slice(0, 4)}…{publicKey.toBase58().slice(-4)}. It stays in this browser; clearing site data loses it.</span>
                    ) : (
                      <button className="btn sm" onClick={() => select(TestWalletName)}>Use a test wallet</button>
                    )}
                  </div>
                )}
                {i === 1 && publicKey && (
                  <div className="actions" style={{ marginTop: 6 }}>
                    <Faucet />
                    {balance !== null && <span className="tiny">Balance {sol(balance)} SOL</span>}
                  </div>
                )}
                {i === 1 && !publicKey && <p className="tiny">Pick a wallet first to see the faucet button here.</p>}
              </div>
            </div>
          ))}
        </div>
        <div className="actions" style={{ marginTop: 18 }}>
          <Link href="/" className="btn">Browse agents</Link>
          <Link href="/how-it-works" className="btn ghost">How it works</Link>
        </div>
      </div>

      <div className="card rise d2">
        <h3>Good to know</h3>
        <ul className="bullets">
          <li><b>Prices are fake.</b> Devnet has test pools only, so profits and losses show that the mechanics work, not how a strategy would perform.</li>
          <li><b>Agents run from a machine we operate.</b> The header shows whether they are online. If they are offline, anything you allocate stays untouched and you can cancel it for a full refund.</li>
          <li><b>Capacity is limited.</b> Each agent can only manage what its collateral backs. If one is full, try another or allocate less.</li>
          <li><b>Found a problem?</b> Tell us on Telegram or email. Screenshots and the position address help.</li>
        </ul>
        {CLUSTER !== "devnet" && <p className="tiny">You are viewing the {CLUSTER} build; this guide is written for devnet.</p>}
      </div>
    </div>
  );
}
