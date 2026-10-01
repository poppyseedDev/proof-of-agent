# Settlement and slashing

This page explains how a position closes, when an agent's collateral is used,
and how much of it goes to the trader. The first part describes what the
program does today. The second part describes planned extensions and is marked
as such.

## Terms

| Term | Meaning | On-chain field |
|------|---------|----------------|
| Operator | Wallet that creates and manages the agent, publishes its terms, and owns its collateral. | `Agent.operator` |
| Trading key | Wallet the operator binds to draw and settle positions. Defaults to the operator. | `Agent.executor` |
| Principal | SOL the trader deposited into the position. | `Position.principal` |
| Collateral ratio | Share of the principal the agent must lock as a guarantee. 10% to 100%. | `Agent.terms.collateral_ratio_bps` |
| Locked bond | `principal × collateral ratio`, rounded up. Reserved from the agent's collateral vault for this position only. | `Position.locked_collateral` |
| Tolerance | How much worse than holding SOL the agent may do before its bond pays the trader. Called *max drawdown* in the code. Up to 50%, and collateral ratio + tolerance may not exceed 100%. | `Agent.terms.max_drawdown_bps` |
| Returned | SOL the agent sends back when it settles. | `Position.returned` |
| Performance fee | Share of profit paid to the operator. Chosen by the operator, capped at half the collateral ratio. | `Agent.terms.fee_bps` |
| Trading window | Shortest and longest deadline a trader may choose. | `Agent.terms.min_duration_secs`, `max_duration_secs` |
| Breach | A settlement below the floor, a settlement at or after the deadline, or a claimed default. | `Position.breach`, `Agent.breach_count` |

The position snapshots the fee and tolerance when it opens. Published terms
cannot change in any case.

## Publishing an agent (live)

An agent is created as a **draft**. While it is a draft, the operator can edit
its name, description and every term: collateral ratio, fee, maximum drawdown,
trading window, allowed assets (up to eight mints) and plain-language rules
(up to 512 characters, stored on-chain). Traders cannot allocate to a draft.

`create_agent`, `update_agent` and `publish_agent` all validate the terms. On
top of the per-field limits, the collateral ratio plus the tolerance may not
exceed 100% (error `RatioPlusDrawdownTooHigh`). `open_position` checks the same
bound against the agent's stored terms, so an agent published before the check
existed cannot open new positions with terms that break it.

`publish_agent` requires a collateral deposit, validates the terms again, and
moves the agent to **active**. From then on the terms are permanent. The
operator can pause and resume new positions, deposit and withdraw free
collateral, and bind a different trading key. To change terms, the operator
creates a new agent; one operator can run many, each with its own `agent_id`.

Allowed assets and rules are disclosure: the program stores and shows them,
but does not check which trades an agent makes.

## The rule (live)

```
floor = principal × (1 − tolerance)
slash = min(locked_bond, max(0, floor − returned))
fee   = profit × fee_bps          only when returned > principal
```

- **Profit.** The operator receives its fee on the profit. The trader receives the rest. The bond is untouched.
- **Loss within tolerance.** The trader absorbs the loss. The agent earns no fee. The bond is untouched.
- **Loss beyond tolerance.** A breach. The part below the floor is paid from the agent's locked bond to the trader, up to the full locked amount.
- **Loss beyond the bond.** Anything below `floor − locked_bond` is the trader's loss. A higher collateral ratio shrinks this gap, which is why it earns a higher fee.

In every case the locked bond is released back to the agent's free collateral, minus any slash.

### Why ratio + tolerance may not exceed 100%

If the agent returns nothing, the shortfall below the floor is
`principal × (1 − tolerance)`. That has to be at least the locked bond, or
settling with `returned = 0` would cost the agent less than letting the trader
claim the default. With `ratio + tolerance ≤ 100%` it always is: the bond is
rounded up and the tolerated loss is rounded down, so `settle(0)` slashes the
whole locked bond, exactly what `claim_default` takes. A 100% ratio therefore
requires a 0% tolerance.

### Why market drops are not slashed

Positions are denominated in SOL. A fall in SOL's dollar price does not reduce
the number of lamports in a position. An agent that returns the same SOL it
received has matched the market, even if SOL lost half its dollar value.
The only way to return fewer lamports is to trade worse than simply holding
SOL.

The rule therefore means *"slash only when the strategy underperformed holding
SOL by more than the tolerance"*. It needs no price oracle.

### Worked example

10 SOL principal, 30% collateral ratio (3 SOL locked, 15% fee), 10% tolerance
(floor 9 SOL).

| Agent returns | Slash | Agent fee | Trader receives | What happened |
|---------------|-------|-----------|-----------------|---------------|
| 12 SOL | 0 | 0.3 SOL | 11.7 SOL | Profit of 2 SOL, 15% to the agent. |
| 10 SOL, SOL's dollar price fell 40% | 0 | 0 | 10 SOL | The market fell; the strategy did not lose. |
| 9.5 SOL | 0 | 0 | 9.5 SOL | Small loss inside the tolerance. |
| 8 SOL | 1 SOL | 0 | 9 SOL | 1 SOL below the floor, paid from the bond. |
| 5 SOL | 3 SOL | 0 | 8 SOL | 4 SOL below the floor, capped at the 3 SOL bond. |

The trader also gets back the position vault's rent deposit (about
0.00089 SOL) when the position closes.

Solana rejects a transaction that leaves an account holding some lamports but
less than its rent-exempt minimum (about 0.00089 SOL for a plain wallet). If
the operator wallet is empty and the fee is smaller than that, paying it would
make every settle attempt fail. In that case the trader keeps the fee, and the
position records `fee_paid = 0`. An operator whose trading key is a separate
wallet should keep at least the rent minimum in the operator wallet. Trader
payouts do not need this rule: they always include the position vault's rent
deposit, so they reach the minimum even in an empty wallet.

## Position lifecycle (live)

| From | Action | Who | Condition | Result |
|------|--------|-----|-----------|--------|
| — | `open_position` | Trader | Protocol not paused, agent is active, deadline inside the published window, enough free collateral, principal within the protocol's position cap and the agent's total within the per-agent cap | Principal moves into the position vault. Bond is reserved. |
| Open | `draw_funds` | Trading key or operator | Protocol not paused, before the deadline | Principal moves to the trading key. Status becomes Trading. |
| Open | `cancel_position` | Trader | While still Open, before or after the deadline | Full refund. Bond released. No fee. Only `open_positions` goes down. |
| Open | `settle_position` | Trading key or operator | While still Open, before or after the deadline | Declines the position. Treated as returning the full principal, so no fee and no slash. The position is marked Settled with `drawn_at = 0`, `settled_positions` is **not** incremented, and a `PositionDeclined` event is emitted alongside `PositionClosed`. |
| Trading | `settle_position` | Trading key or operator | Until the trader claims the default | The rule above applies to the SOL sent. Before the deadline, a slash records a drawdown breach. At or after the deadline, a missed-deadline breach is recorded instead (see below). `settled_positions` is incremented. |
| Trading | `claim_default` | Trader | At or after the deadline | The whole locked bond goes to the trader. Status becomes Defaulted and a missed-deadline breach is recorded. `defaulted_positions` is incremented. |

Cancel, settle and claim default work whether or not the protocol is paused
(see "Pause switch and caps" in the README).

Deadlines are exact to the second: `draw_funds` works while
`now < deadline`, and `claim_default` works once `now >= deadline`.

### Cancel is first come, first served

A trader can cancel only while the position is Open. The trading key can draw
at any moment while the position is Open and before the deadline. There is no
grace period and no ordering between the two: if the agent's `draw_funds`
lands before the trader's `cancel_position`, the cancel fails with
`InvalidStatus` and the position is now Trading. An agent can therefore
front-run a cancel it sees pending. A trader who wants out after a draw has to
wait for the agent to settle or for the deadline to claim the default.

### Missed deadline

While a position is Trading, the agent holds the principal. If it has not
settled by the deadline, the trader can call `claim_default` and receive the
entire locked bond, whatever the market did.

An agent that is late can still call `settle_position` until the trader
claims the default. Whichever transaction lands first decides the outcome.
A late settle pays out by the same rule as an on-time one (fee on profit,
slash below the floor), but it is not a clean settle: the position records a
`MissedDeadline` breach, `breach_count` goes up by one, and a `BreachRecorded`
event is emitted. Because ratio + tolerance ≤ 100%, a late settle that returns
nothing still costs the whole locked bond, so settling late is never cheaper
for the agent than a default.

### What the bond does not cover

In the current version the agent takes custody of the principal. If an agent
never returns it, the trader recovers only the locked bond. The same holds if
the agent settles with nothing returned, because the ratio + tolerance bound
guarantees that slash equals the whole bond. So on every path the trader's
maximum loss is `principal − locked_bond` (plus nothing else: no fee is
charged on a loss). A 100% collateral ratio, which forces a 0% tolerance,
makes a position fully backed.

## Planned extensions

None of the following is implemented yet.

### Vault custody

Specified in [custody.md](custody.md) and being built on the `vault-custody`
branch. Funds stay in the position vault, the agent trades them through a
program instruction that calls an allowlisted DEX, every swap is checked
against an oracle price, and settlement reads the vault's balance instead of
trusting the amount the agent sends. A missed deadline then costs a fixed
penalty instead of the whole bond, because the trader can unwind the vault
themselves.

### USDC deposits and a benchmark

Once positions can hold USDC, a fall in SOL's price would show up as a loss.
Each agent would declare a benchmark at registration, such as SOL for a SOL
trading strategy or none for a stablecoin strategy. The rule gains one term:

```
benchmark_return = price_at_settlement / price_at_open − 1
floor            = principal × (1 + benchmark_return) × (1 − tolerance)
slash            = min(locked_bond, max(0, floor − vault_value))
```

With no benchmark, `benchmark_return` is zero and the rule is the same as
today. Prices would come from an oracle such as Pyth, using its moving-average
price at both ends so a single trade cannot move the settlement price.

Example: 1,000 USDC, SOL benchmark, 5% tolerance, 300 USDC locked.

| SOL move | Vault at settlement | Floor | Slash |
|----------|---------------------|-------|-------|
| −20% | 820 | 760 | 0 |
| −20% | 700 | 760 | 60 |
| +20% | 1,050 | 1,140 | 90 |
| +10% | 1,250 | 1,045 | 0 |

The fee would only be paid when the vault ends above the principal and there
is no slash.

### Declared rules

Agents would publish their trading rules in a machine-readable on-chain
account: allowed tokens, maximum slippage, maximum share of the vault in one
token, stop-loss level, and maximum trades per day.

- **Rules the program can check before a trade** are enforced directly. A trade that breaks them is rejected, so no dispute is needed.
- **Rules that can only be checked afterwards** go through a challenge. Anyone can point to the offending transactions and post a small deposit. The agent has a window to dispute. An undisputed claim is executed by the program. A disputed one goes to an independent judge. A false claim forfeits the deposit.

A proven violation suspends the agent and slashes its bond whether or not the
position lost money, so an agent cannot break rules and keep the upside.
Affected traders are compensated up to their loss before any remainder goes
to the treasury. Rules that cannot be checked, such as "uses AI", are left to
reputation.

### Smoothing out bad luck

A single short position is noisy. SOL moves roughly 10% in a typical week.
Instead of slashing each position on its own, the program could accumulate an
agent's shortfalls across positions and slash only when the total over several
positions crosses a threshold. That separates a bad strategy from a bad week.

### Reputation

Each settlement or default could post a score to the Solana Agent Registry,
so an agent's track record is public and portable beyond this app.

## Does the bond actually cost an honest operator?

`sim/` answers this empirically. It replays ordinary strategies over six years
of real SOL/USD daily closes, opens a position on every start date, and settles
each one through a port of `compute_settlement`.

The short version:

* Holding SOL and returning it breaches **0%** of positions at every tolerance
  and every length. Because settlement is SOL-denominated, market direction can
  never cost the operator its bond.
* A trend-following rule at a 30% ratio and a 15% tolerance breaches 10.8% of
  30-day positions and still returns about 17% a year on a fully deployed bond.
* What costs an operator money is publishing a floor tighter than the strategy
  can hold, not being mediocre. Each strategy has a tolerance below which it
  loses money and above which it does not.
* Higher collateral improves operator economics. Fee income per SOL of bond is
  constant across ratios because the fee cap scales with the ratio, while slash
  risk per SOL of bond falls as the ratio rises.
* Theft still pays. An operator who draws and never settles loses the whole
  reserved bond but keeps the principal, so at any ratio below 100% they come
  out ahead. The bond makes bad trading expensive; it does not make theft
  unprofitable.

Results are published in the How it works section of the app, under
"What operators risk". See `sim/README.md` for the method and the full tables.
