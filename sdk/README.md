# @proofofagent/operator

Run a trading bot as a Proof of Agent operator. This package handles what the
protocol needs from you: publishing terms, posting collateral, starting each
position, swapping through the program, and settling before the deadline.

> **Vault custody.** The principal never reaches your wallet. `beginTrading`
> wraps it into the position's vault; every trade goes through
> `executeSwap`, which forwards a DEX instruction (Orca Whirlpools) signed by
> the vault and checks it against Pyth prices; `settlePosition` pays out
> whatever the vault holds. The `poa run` hook model, where a bot traded from
> the trading wallet and the runner settled by wallet balance, no longer
> trades anything: a hook that does not route its trades through
> `executeSwap` just holds SOL in the vault until settlement.

It talks to the on-chain program through its public interface only
(`idl/proof_of_agent.json`, program `49aHwbzdT1iN8WYWdUZxrGoZpjSryyugMm4q9VTjXgSr`
on devnet). No account with us is needed.

## How an agent works

1. You publish an agent: collateral ratio, fee, max drawdown, deadline window,
   allowed assets, and your rules in plain language. Terms are permanent once
   published.
2. You deposit SOL collateral. Capacity is collateral ÷ ratio.
3. Traders allocate SOL to your agent for a deadline inside your window. The
   program reserves `principal × ratio` of your collateral for each position.
4. Your bot draws the SOL, trades, and returns SOL before the deadline.
5. Settlement: profit pays your fee; a loss inside your max drawdown is the
   trader's; a loss beyond it, or a missed deadline, is a **breach** paid from
   your reserved collateral.

Everything is in SOL. A fall in SOL's price is not a loss; returning fewer SOL
than the floor is.

## Two keys

- **Operator key**: creates the agent, owns the collateral, receives fees.
  Keep it offline.
- **Trading key**: bound to the agent, can only draw and settle. Give this one
  to your bot. Keep only working balances in it.

## Quick start (devnet)

```bash
npm install
npm run poa -- keygen --out operator.json
npm run poa -- keygen --out trading.json
# fund both on devnet, then:
export POA_RPC_URL=https://api.devnet.solana.com
npm run poa -- agent create --key operator.json --name "My Bot" --ratio 30 --fee 10 --drawdown 15 \
  --min-hours 1 --max-days 7 --assets SOL,DEVUSDC --rules "What the bot does, in plain words."
npm run poa -- agent deposit --key operator.json --agent <AGENT> --sol 1
npm run poa -- agent bind    --key operator.json --agent <AGENT> --trading-key <TRADING_PUBKEY>
npm run poa -- agent publish --key operator.json --agent <AGENT>
npm run poa -- agent status  --agent <AGENT>
```

Then run the runner next to your bot:

```bash
npm run poa -- run --key trading.json --agent <AGENT> --hook ./my-bot.sh --hold-min 60 --buffer-min 10
```

## The runner

`poa run` polls the chain. When a trader opens a position it draws the SOL
into the trading wallet and starts your **hook** command with:

| Variable | Meaning |
|---|---|
| `POA_POSITION` | position address |
| `POA_PRINCIPAL_SOL`, `POA_PRINCIPAL_LAMPORTS` | what was drawn |
| `POA_DEADLINE` | unix time the trader may claim your collateral |
| `POA_SETTLE_BY` | unix time the runner will settle, hook or no hook |
| `POA_WALLET` | trading wallet address |
| `POA_RPC_URL` | RPC in use |
| `POA_PAPER` | `1` in paper mode (nothing was drawn; do not trade) |

It settles when the hook exits, or at `--hold-min` after drawing, or
`--buffer-min` before the deadline, whichever comes first. If the hook is
still running, its whole process group (the hook and everything it started)
gets `SIGTERM`, then up to `--grace-sec` (default 60) to unwind, then
`SIGKILL`. The grace period is cut short so the hook never runs past the
deadline less 30 seconds for the settle transaction. The wallet balance is read
only after every process in the group has exited. **A missed deadline costs
you the whole reserved collateral, so the runner never waits past that.**

The hook's process group id is kept in the state file. If the runner crashes
while the hook runs, the restarted runner stops that group the same way
(`SIGTERM`, grace, `SIGKILL`) before it reads the wallet.

`--buffer-min` must be at least `--poll` plus 30 seconds; the runner refuses to
start otherwise. It wakes early when a settle comes due rather than waiting out
the poll, and compares settle times and deadlines on the cluster's clock (the
one the program enforces), re-measured every 5 minutes.

The settle transaction carries a priority fee: `--priority-fee` micro-lamports
per compute unit (default 1000, about 200 lamports), or `--priority-fee-urgent`
(default 50000, about 10,000 lamports) within 2 minutes of the deadline. The
same signed transaction is resent every 2 seconds until it confirms; if its
blockhash expires it is signed again with a fresh one.

Settlement amount = principal + change in the trading wallet's SOL balance
since the draw (the balance just before the draw, plus the principal, less the
draw's 5000-lamport fee). So:

- Use a **dedicated trading wallet**. Anything else that lands in it during a
  position is counted as that position's result (we learned this the hard way:
  leftover USDC from a failed cycle got swept into the next position's profit).
- **End in SOL.** Tokens still held at settlement are not counted, and the
  position looks like a loss. The runner sends a `warning` event if the wallet
  holds any non-SOL token balance at settle time.
- Network fees and token-account rent come out of the result.

One position trades at a time; others wait in the open state.

`--paper` is a dry run: nothing is drawn or settled and the wallet is never
touched. For each position it would draw, the runner logs the draw and the
settle time, runs the hook with `POA_PAPER=1` until then, and logs the would-be
settle. Positions stay open for a real runner (or the trader) to act on.

The runner re-reads the agent every 2 minutes. If the operator has bound a
different trading key, every settle would fail, so it sends an `unbound` alert
(repeated at every check while a position is at stake: rebind the key or settle
with the operator key before the deadline) and stops drawing until the key is
bound again.

`--notify "cmd"` runs a command on `settled`, `settle-soon`, `adopted`,
`low-balance`, `unbound`, `rebound`, `warning` and `error` events with
`POA_EVENT` and `POA_MESSAGE`. State lives in
`.poa/state.json`, written atomically with the previous version kept as
`.poa/state.json.bak`; a restart resumes an open position. If the state file
does not parse, it is moved aside to `state.json.corrupt-<time>` and the backup
is used.

`poa keygen` writes the key with mode 600 and refuses to overwrite an existing
file. Add key files to `.gitignore`. Loading a key file that other users can
read prints a warning.

## Example: Hummingbot

`hooks/hummingbot-orca.sh` drives an unmodified Hummingbot Gateway: it sells
part of the principal into USDC on Orca, holds, and sells the USDC back before
the runner settles. Point `GATEWAY_URL`, `GATEWAY_NETWORK`, `POOL` and the two
mints at your setup, and add the trading key to the Gateway with
`POST /wallet/add`.

Tested on devnet with Gateway 2.17: a first cycle broke its terms (the buy-back
used the wrong side and left half the position in USDC), was recorded as a
breach, and paid the trader from collateral; the corrected cycle settled at a
profit with the fee paid to the operator.

## Limits of this version

- Positions and collateral are SOL only.
- The protocol caps the size of each position and the total one agent may
  manage, and can be paused. While it is paused, draws and collateral deposits
  fail with `ProtocolPaused`; settling keeps working.
- The program does not see your trades. Only the draw, the settlement amount
  and any breach are on-chain.
- Devnet has no real prices; the only liquid pools are Orca's test pools.
