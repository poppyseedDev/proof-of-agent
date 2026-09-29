# Agent runner

A small Node service that operates Proof of Agent on devnet. Each agent
has an operator key, which creates the agent, posts collateral, publishes the
terms and receives fees, and a separate trading key bound on-chain, which
draws, trades and settles. Agents trade traders' SOL on Orca's devnet SOL/USDC
pools.

## What each agent does

1. **Draws** every new position on its agent, as long as the deadline is at least seven minutes away.
2. **Trades** the position with its strategy, keeping a separate book of the SOL and USDC that belong to each position.
3. **Settles** before the deadline: sells any USDC back to SOL and returns the position's SOL through the program, so the usual fee and slash rules apply.

A position settles at whichever comes first: the agent's hold time after drawing, or five minutes before the deadline. Deadline decisions use the cluster's clock (block time of the latest slot, read once per tick), falling back to the local clock if it cannot be read. Unwinding USDC at settlement uses the main pool only.

Terms and rules published for each agent are in `src/config.ts`.

| Agent | Collateral | Fee | Tolerance | Strategy |
|-------|-----------|-----|-----------|----------|
| Orca Pool Arbitrage | 50% | 25% | 5% | Sells SOL on the pool that pays the most USDC and buys it back on the pool that pays the most SOL, when the round trip gains at least 0.5%. Up to three round trips per position, half the position each time. |
| Orca Momentum | 30% | 15% | 15% | Moves 60% of a position into USDC when the main pool's price drops 0.3% below its 10-sample average, and back when it rises 0.3% above. |
| Half-Stable Rotator | 10% | 5% | 30% | Holds half of each position in USDC until settlement. |

Devnet prices are set by whoever trades there, not by a real market. The
arbitrage agent profits from the gaps between Orca's devnet pools, and those
gaps shrink as it trades.

## Accounting

The agent's wallet mixes its own SOL with every trader's principal. Each
position's book starts at its principal. Selling SOL subtracts exactly the
amount sold. Buying SOL adds the amount received, measured on-chain, with the
network fee added back. Network fees and token account rent are paid by the
agent, never charged to a position.

## Commands

```bash
cd agent
npm install
npm run setup                          # create operator + trading keys, fund, create, bond, bind, publish
npm start                              # run all agents (Ctrl+C stops after the current tick)
npm start -- orca-arb                  # run selected agents only
npm run seed-positions -- 0.2 2 3600   # test trader opens 2 × 0.2 SOL positions per agent for 1 hour
```

Environment variables:

| Variable | Default | Purpose |
|----------|---------|---------|
| `RPC_URL` | `https://api.devnet.solana.com` | Cluster RPC. A private devnet RPC avoids rate limits. |
| `RPC_URL_FALLBACK` | none | Second RPC for reads (position scans, balances, clock, swap status) and for retrying a settle when the first RPC is unreachable. |
| `SLIPPAGE_BPS` | `50` | How far below the quote a trade was decided on its output may fall. |
| `POLL_MS` | `30000` | Time between ticks. |
| `STATE_DIR` | `state/` | Where books are kept. |
| `FUNDER_KEYPAIR` | `~/.config/solana/id.json` | Wallet that funds agents and the test trader. |

Keys are written to `keys/` (`<id>.json` is the operator, `<id>-executor.json`
the trading key) and per-agent state to `state/`. Both are
gitignored. Losing `keys/` means losing control of the agents and their
collateral; losing `state/` only resets the books, and open positions are
re-adopted on the next start.

## Running on a server

The runner is set up to run on Fly.io as the app `poa-agent-runner` (one machine
in `fra`, 512 MB, a 1 GB volume `runner_state` mounted at `/data` for the state
files). The server only holds the three trading keys; the operator keys, which
can withdraw collateral, stay on the laptop.

| Setting | Where |
|---|---|
| `EXECUTOR_KEYS` | Fly secret: `{"<id>": [secret key bytes]}` for each agent's `keys/<id>-executor.json` |
| `OPERATOR_PUBKEYS` | `fly.toml` `[env]`: public keys only |
| `HEARTBEAT_SECRET` | Fly secret, same value as the site's |
| `RPC_URL`, `RPC_URL_FALLBACK` | Fly secrets; use a paid RPC (e.g. Helius) as primary and the public endpoint as fallback |
| `WATCHDOG_SECS` | optional, default 600 |

Deploy from the repo root (the image also needs `app/lib/idl.json`):

```bash
fly deploy . --config agent/fly.toml
fly logs --app poa-agent-runner
```

Run only one runner at a time. The runner takes `runner.lock` in `STATE_DIR` at
startup and refuses to start while another live runner holds it; a lock left by a
crashed runner (dead pid, earlier boot, or another machine) is taken over with a
log line. The lock only covers one `STATE_DIR`, so before the Fly machine starts, stop the launchd job
(`launchctl bootout gui/$(id -u)/dev.proofofagent.agent-runner`) and copy
`state/*.json` to the volume (`fly ssh sftp shell --app poa-agent-runner`, then
`put state/orca-arb.json /data/orca-arb.json` and so on).

Three layers keep settlement alive:

1. Fly restarts the process whenever it exits.
2. The runner's own watchdog exits when no agent has completed a tick for
   `WATCHDOG_SECS`, which covers a hung RPC call or a dead connection pool.
3. An uptime monitor checks the site heartbeat and emails you when the runner
   is offline. This is the only layer that notices the whole machine being down.
4. `scripts/uptime-check.sh`, run every 5 minutes on the Mac by launchd
   (`dev.proofofagent.uptime`, installed copy in
   `~/Library/Application Support/ProofOfAgent/`), checks the heartbeat, both
   sites, the faucet balance, and `/api/health`: whether the site can trade
   against the deployed program, and whether the runner and the site were built
   from the same program interface. It logs problems and recoveries to `uptime.log`
   there and shows a notification when the state changes. Only while the Mac is
   awake; re-copy the script after editing it.
5. `.github/workflows/runner-watchdog.yml` checks the heartbeat and `/api/health` as a backup.
   GitHub runs schedules best-effort (often late, sometimes skipped), so don't
   rely on it alone.

### Uptime monitor settings

UptimeRobot (free plan) or Better Stack (free plan), one monitor:

| Setting | Value |
|---|---|
| Monitor type | Keyword |
| URL | `https://dev.proofofagent.dev/api/heartbeat` |
| Keyword | `"online":true` (with the quotes) |
| Alert when | keyword does **not** exist |
| Interval | 5 minutes (UptimeRobot free) or 3 minutes (Better Stack free) |
| Timeout | 30 seconds |
| Alert contact | your email; add phone/push if you have it |

The heartbeat reports `"online":false` when the runner hasn't posted for 5
minutes, so an alert arrives within about 10 minutes of the runner stopping.
Positions settle at least 10 minutes before their deadline, and the shortest
window the agents offer is 30 minutes, so that leaves time to react. Check the
logs with `fly logs -a poa-agent-runner` and restart with
`fly machine restart -a poa-agent-runner`.

## Limits

- The runner has to stay online. If it is down past a position's deadline, the trader can claim the agent's collateral.
- The primary RPC is Helius (Fly secret `RPC_URL`) with the public devnet endpoint as fallback.
- One Fly machine, and its volume is tied to one physical host: if that host fails, the runner stays down until the volume is restored to a new machine (Fly keeps 5 daily snapshots). Mainnet needs a standby.
- Pool addresses are listed in `src/config.ts` to avoid a program-wide scan. Pools that lose liquidity are skipped.
- Every swap is signed and saved in the book as `pending` (signature and last valid block height) before it is sent. If its confirmation is lost, the next tick looks it up and books what actually happened, or drops it once its blockhash has expired. A swap still unresolved two minutes before the deadline is settled with the book as it stands, and logged as an `ALERT`.
- Each agent's positions are read with a scan filtered on that agent. If the scan fails, the runner fetches the positions already in its books by address so they still settle, but it does not draw new ones until the scan works again. The heartbeat lists only agents whose tick ran.
- Books settle at their hold time, and never later than ten minutes before the deadline. Each loop first settles every due book across all agents, then trades and draws, so one agent's slow quotes or swaps cannot push another agent's book past its deadline. No trade is opened inside those ten minutes, and nothing is drawn within twelve.
- A book still unsettled within fifteen minutes of its deadline, when it should already have settled, has a swap unresolved, or its tick could not run, is logged as an `ALERT` line on every tick. There is no other notification channel yet.
- State files are written atomically, keeping the previous version as `<id>.json.bak`. A file that does not parse is moved to `<id>.json.corrupt-<time>` and the backup is loaded. The backup is one save behind, so it can miss a swap that landed: the runner logs an `ALERT` at startup and lists the trading key's recent transactions that no book knows about. It does not book them; reconcile those by hand.
- The momentum signal reads the main pool's price, which only moves when someone trades it. On devnet it often stays flat, so the momentum agent may never trade. Samples older than twice the window (at least a minute per sample) are ignored, so after downtime the average starts over.
