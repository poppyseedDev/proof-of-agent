"use client";

import { useEffect, useState } from "react";
import { useConnection, useWallet } from "@solana/wallet-adapter-react";
import { LAMPORTS_PER_SOL } from "@solana/web3.js";
import { CLUSTER } from "@/lib/program";
import { BALANCE_EVENT } from "@/lib/useProtocol";
import { IconDrop } from "./Icons";

/**
 * Test SOL for the connected wallet: an airdrop on localnet, our faucet on devnet.
 * Hidden on mainnet.
 */
export function Faucet({ compact = false }: { compact?: boolean }) {
  const { connection } = useConnection();
  const { publicKey } = useWallet();
  const [state, setState] = useState<"idle" | "busy" | "done" | "err">("idle");
  const [msg, setMsg] = useState("");
  const [enabled, setEnabled] = useState(CLUSTER === "localnet");
  // Devnet only: our faucet is off, unreachable or out of SOL, so point to the public one instead.
  const [empty, setEmpty] = useState(false);

  useEffect(() => {
    if (CLUSTER !== "devnet") return;
    fetch("/api/faucet")
      .then((r) => r.json())
      .then((d) => {
        const on = !!d.enabled && d.remainingDrops !== 0;
        setEnabled(on);
        setEmpty(!on);
      })
      .catch(() => {
        setEnabled(false);
        setEmpty(true);
      });
  }, []);

  if (!publicKey) return null;
  if (!enabled) {
    if (!empty) return null;
    return (
      <a className={"btn ghost sm faucet" + (compact ? "" : " wide")} href="https://faucet.solana.com" target="_blank" rel="noreferrer"
        title={`Our faucet is empty. Paste ${publicKey.toBase58()} at faucet.solana.com and pick Devnet.`}>
        <IconDrop width={14} height={14} />
        <span className="faucet-label">Get test SOL at faucet.solana.com</span>
      </a>
    );
  }

  const drop = async () => {
    setState("busy");
    setMsg("");
    try {
      if (CLUSTER === "localnet") {
        const sig = await connection.requestAirdrop(publicKey, 10 * LAMPORTS_PER_SOL);
        await connection.confirmTransaction(sig, "confirmed");
        setMsg("+10 SOL");
      } else {
        const res = await fetch("/api/faucet", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ address: publicKey.toBase58() }) });
        const d = await res.json();
        if (!res.ok) throw new Error(d.error ?? "Faucet failed");
        setMsg(`+${d.amountSol} SOL`);
      }
      window.dispatchEvent(new Event(BALANCE_EVENT));
      setState("done");
      setTimeout(() => setState("idle"), 3000);
    } catch (e) {
      setMsg((e as Error).message);
      setState("err");
      setTimeout(() => setState("idle"), 6000);
    }
  };

  const label = state === "busy" ? "Sending…" : state === "done" ? msg : state === "err" ? msg : CLUSTER === "localnet" ? "Airdrop 10 SOL" : "Get test SOL";
  return (
    <button className={"btn ghost sm faucet" + (compact ? "" : " wide")} onClick={drop} disabled={state === "busy"} title={state === "err" ? msg : `Send test SOL to ${publicKey.toBase58()}`}>
      <IconDrop width={14} height={14} />
      <span className="faucet-label">{label}</span>
    </button>
  );
}
