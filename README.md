# Proof of Agent

An over-collateralized marketplace for AI trading agents on Solana.
Public one-pager and waitlist at [proofofagent.dev](https://proofofagent.dev); the app runs at [dev.proofofagent.dev](https://dev.proofofagent.dev). Both come from the same Next.js deployment; `app/proxy.ts` routes by host.

**The rule:** an agent must post its own SOL as collateral before it can manage
anyone's capital. The more it guarantees per unit managed, the higher the fee it
may charge. If it misbehaves, the collateral is paid to the trader by the program.

## How it works

| Step | Who | What happens on-chain |
|------|-----|-----------------------|
| Create | Operator | Creates a draft agent with its terms: collateral ratio (10–100%), fee (capped at ratio / 2), max drawdown (≤50%, and ratio + drawdown ≤ 100%), trading window, allowed assets, and plain-language rules. Terms are editable while it is a draft. |
| Deposit | Operator | Sends SOL into the agent's collateral vault (a program PDA). |
| Bind key | Operator | Optionally binds a trading key that may draw and settle, and nothing else. |
| Publish | Operator | Requires collateral. Terms become permanent and traders can allocate. |
| Open position | Trader | Deposits `P` SOL with a deadline inside the agent's window. `P × ratio` of the agent's free collateral is locked to this position. Fails if the agent cannot back it. |
| Draw | Trading key | Pulls the principal to trade with. The clock is now running. |
| Settle | Trading key | Returns `R` SOL. Profit: the operator earns `fee` of the profit. Loss within drawdown: trader takes it. Loss beyond drawdown: a breach; the shortfall is paid from locked collateral to the trader. Settling at or after the deadline is still allowed until the trader claims the default, but it is recorded as a missed-deadline breach. Settling a position that was never drawn declines it: the trader is refunded and it does not count as a settled position. |
| Claim default | Trader | If the agent never settled by the deadline, the trader takes the entire locked guarantee and a breach is recorded. |
| Cancel | Trader | While the position is still open (not drawn), the trader can pull out with no fee. Cancel and draw race: whichever transaction lands first wins. |

Example: you allocate 1,000 to a 30% agent. The protocol reserves 300 of the
agent's collateral as your guarantee. The collateral stays in the protocol's
vault and is paid to you only if the agent breaks its mandate or misses the
deadline.

See [docs/settlement.md](docs/settlement.md) for the exact settlement and
slashing rules, worked examples, and planned extensions. The same material is
on the site under **How it works**.

## Layout

```
programs/proof_of_agent   Anchor program (Rust) + LiteSVM tests
app/                             Next.js frontend (wallet adapter + Anchor client)
agent/                           Agent runner: devnet agents trading on Orca
sdk/                             Operator SDK and CLI for external agents
sim/                             Operator risk simulation on real price history
docs/                            Protocol docs
```

Program id (devnet & localnet): `49aHwbzdT1iN8WYWdUZxrGoZpjSryyugMm4q9VTjXgSr`

The program is deployed on devnet, and three agents run there from
[agent/](agent/README.md). They trade traders' SOL on Orca's devnet pools and
settle through the program.

## Local development

```bash
# 1. validator (leave running)
solana-test-validator --reset --quiet --ledger test-ledger

# 2. build, test, deploy the program to it
anchor build
cargo test --manifest-path programs/proof_of_agent/Cargo.toml
solana airdrop 100 -u localhost
anchor deploy --provider.cluster localnet

# 3. frontend against localnet (app/.env.local already points at 127.0.0.1:8899)
cd app && npm install
npm run seed                          # creates the protocol config, registers 4 demo agents with collateral
npm run fund -- <your-wallet-address> # 10 SOL to your browser wallet
npm run dev
npm run test:localnet                 # integration tests through the RPC (~90s)
```

Two test suites cover the program: `cargo test` runs in-process LiteSVM tests
with a controllable clock, and `npm run test:localnet` drives the deployed
program through the same TypeScript client the frontend uses (real RPC,
airdrops, PDAs, IDL decoding). The last localnet test waits out a real
60-second deadline to exercise the default claim.

On devnet and localnet the wallet picker also offers **Test wallet**, a keypair
kept in the browser's localStorage (one per cluster, never on mainnet), so
testers need no wallet extension and keep the same wallet across reloads.
Clearing site data loses it. On localnet the header shows an **Airdrop 10 SOL**
button.

To use Phantom instead: Settings → Developer settings → enable Testnet mode and
pick **Localnet**, otherwise Phantom simulates against devnet and reports
"not enough SOL". Then connect on http://localhost:3000.

After changing the program: `anchor build && anchor deploy --provider.cluster localnet`
and copy the IDL: `cp target/idl/proof_of_agent.json app/lib/idl.json` and to
`sdk/idl/`. Then run `npm test` in `app/`: it fails if the new IDL would not work
against the program on devnet.

## Devnet

The program is deployed on devnet with room for upgrades up to 320 KB. The
upgrade authority is the deploy wallet
`SGzzPobm6doLkxhub7tFKEFiC8JPEZjYarA6cxWbb8w`. To upgrade, first make sure the
clients are ready (see "Deploying without breaking trading"):

```bash
anchor build
npm run check:upgrade      # must say "Safe to upgrade"
solana program deploy target/deploy/proof_of_agent.so \
  --program-id target/deploy/proof_of_agent-keypair.json -u devnet
npm run check:live         # must say OK
cp target/idl/proof_of_agent.json app/lib/deployed/devnet.json
```

After deploying a program version that has the protocol config for the first
time, create the config straight away, because no position can open, draw or
take a deposit until it exists:

```bash
NEXT_PUBLIC_RPC_URL=https://api.devnet.solana.com npm run config -- init none none
```

The Vercel deployment reads `NEXT_PUBLIC_RPC_URL` / `NEXT_PUBLIC_CLUSTER`
and defaults to public devnet.

Agents on devnet: `npm run agents:setup`, then `npm run agents:start`. See
[agent/README.md](agent/README.md).

## Devnet testing

`/start` on the app is the tester guide. `/api/faucet` sends 0.2 devnet SOL once
per wallet from a dedicated faucet key (`FAUCET_KEYPAIR` on Vercel; the wallet
is separate from the program authority). `/api/heartbeat` reports whether the
agent runner has checked in within five minutes; the header shows "Agents
online/offline" from it. The runner posts the heartbeat with `HEARTBEAT_SECRET`
and runs on the operator's Mac under launchd (`dev.proofofagent.agent-runner`,
restarts on crash).

## Waitlist

The one-pager at proofofagent.dev (`app/(landing)`) collects retail sign-ups. Submissions are stored in the Vercel Blob
store `proof-of-agent-waitlist`, encrypted with a key derived from the
`WAITLIST_ADMIN_KEY` environment variable. Export them as CSV, sending the key in a header
(a `?key=` query parameter is not accepted, so the key never ends up in URL logs):

```
curl -H "Authorization: Bearer <WAITLIST_ADMIN_KEY>" https://proofofagent.dev/api/waitlist
```

Emails aren't verified, so the first submission for an email is kept as its row. A later submission
from the same email is stored beside it, never over it, and the `laterSubmissions` column lists what it
changed, for a human to accept or ignore.

The key is set on Vercel, in `app/.env.local`, and in the macOS Keychain
(account `proofofagent`, service `WAITLIST_ADMIN_KEY`). Anyone with it can read
the list, so treat it like a password. Entries are encrypted with it, so never
rotate it without re-encrypting.

Backups:

- **Daily snapshot on Vercel.** A cron job (`vercel.json`, 03:00 UTC) calls
  `/api/waitlist/backup`, which writes every entry into one encrypted file
  under `waitlist-snapshots/`. Recover from the newest one with
  `/api/waitlist?snapshot=latest` (same header).
- **Daily copy on the Mac.** `app/scripts/waitlist-backup.sh` is installed to
  `~/Library/Application Support/ProofOfAgent/` and run by the launchd job
  `dev.proofofagent.waitlist-backup` at 10:00. It keeps the newest 90 CSVs in
  `…/ProofOfAgent/backups/waitlist/` and warns in `backup.log` if the entry
  count ever drops. Run from a terminal, it also copies to `~/Documents` and
  iCloud Drive, which macOS hides from background jobs.

## Operator risk

`sim/` replays ordinary trading strategies over six years of real SOL/USD daily
closes and settles every position through a port of the on-chain maths, to
check whether posting a bond costs an honest operator money.

```bash
python3 sim/run.py          # no dependencies beyond the standard library
python3 sim/export_web.py   # refresh the figures shown in the app
```

Holding SOL and returning it never breaches, so market direction alone can
never cost an operator its bond. What costs money is publishing a floor tighter
than the strategy can hold. See [sim/README.md](sim/README.md) for the tables
and [docs/settlement.md](docs/settlement.md) for how that follows from the
settlement rule.

## Deploying without breaking trading

The site, the agent runner and the SDK are deployed separately from the program,
so for a while new clients talk to the old program. Four guards keep that safe:

| Guard | When it runs | What it catches |
|-------|--------------|-----------------|
| `npm test` in `app/` compares `app/lib/idl.json` with `app/lib/deployed/devnet.json`, the IDL of the program on devnet | Every test run | A change the deployed program cannot read: a reordered or removed account, changed arguments, a changed account layout |
| `npm run check:live` simulates a whole agent and position lifecycle against the program on devnet, built from this checkout's IDL. It signs and sends nothing and needs no key | Before every deploy | Anything that makes an instruction fail on the real program |
| `/api/health` on the site runs the same simulation from the IDL the site was built with | After every deploy, then every 5 minutes from the Mac's uptime check and the GitHub watchdog | A site that cannot trade, whatever the cause, including a program upgrade that left the site behind |
| The runner reports the fingerprint of its IDL in its heartbeat | Every tick | A runner that was not redeployed after the interface changed |

Rules for changing the program:

- Add new accounts at the **end** of an instruction's account list. A program ignores
  extra accounts after the ones it expects, so new clients keep working with the old
  program. Anywhere else, every instruction that has the account fails.
- Never change the layout of `Agent` or `Position`, the arguments of an
  instruction, or an error code. Add new ones instead.

Deploy in this order:

```bash
npm run deploy:site        # tests, live check, vercel --prod, then waits for /api/health to say ok
fly deploy . --config agent/fly.toml --ha=false
npm run check:upgrade      # says whether the site and the runner are ready for the new program
# then the program, with the steps check:upgrade prints
```

Deploy the site with `npm run deploy:site` only, never with `vercel --prod` directly.

## Pause switch and caps

The program's upgrade authority can pause the protocol and cap how much money
it holds. Admin rights come from the upgrade authority itself (read from the
program's ProgramData account), so moving that authority to a multisig moves
these controls with it, and making the program immutable removes them.

```bash
npm run config                     # show the current settings
npm run config -- caps 5 20        # at most 5 SOL per position, 20 SOL per agent ("none" for no cap)
npm run config -- pause            # stop new positions, draws and collateral deposits
npm run config -- resume
```

It signs with `ADMIN_KEYPAIR` (default `~/.config/solana/id.json`) on
`NEXT_PUBLIC_RPC_URL` (default localnet).

A pause never blocks money going back. Traders can still cancel and claim
defaults, agents can still settle and decline, and operators can still
withdraw free collateral. Draws stop because an undrawn position cannot
default: the trader can cancel it. New caps apply to new positions only.

## Trust model (v1)

In v1 the agent *borrows* the principal to trade off-chain, bonded by its
collateral. That means the trader's exposure is `principal − guarantee` if the
agent absconds, and the same bound holds if it settles with nothing returned:
terms must keep ratio + max drawdown ≤ 100%, so a slash on settlement can
always reach the whole bond. A ratio of 100% (which forces a max drawdown of
0) makes the position fully backed. v2 should keep
funds in the vault and restrict the agent to whitelisted DEX CPIs so the
guarantee only needs to cover drawdown, not the whole principal.
