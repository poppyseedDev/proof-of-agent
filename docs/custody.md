# Vault custody

Specification for the next program version. The principal never leaves a
program-owned vault; the agent trades it through a program instruction that
calls an allowlisted DEX, every swap is price-checked against an oracle, and
settlement reads the vault instead of trusting an amount the agent sends.

This is being built on the `vault-custody` branch. Nothing here is deployed.
[settlement.md](settlement.md) describes the program that is live.

## Why

Today `draw_funds` sends the principal to the operator's trading key. From
that moment the program has no view of it. Three things follow:

1. **Theft pays.** An operator who draws and never settles keeps the principal
   and loses only the locked bond. At any ratio under 100% they come out ahead.
2. **Nothing the agent declares can be enforced.** Allowed assets and rules are
   disclosure only, because the program never sees a trade.
3. **The tolerance is a free skim.** Returning exactly the floor costs no bond
   and records no breach, so the operator can keep `tolerance × principal` on
   every position. This is what stops the tolerance from being widened.

Custody fixes 1 and 2 directly. It fixes 3 only together with the oracle
check, see "Why the oracle check is not optional" below.

## What changes

| | Today | With custody |
|---|---|---|
| Where the principal is while trading | Operator's trading wallet | Position vault PDA, as wrapped SOL and allowed tokens in token accounts the PDA owns |
| How the agent trades | Any way it likes | `execute_swap`: the program signs for the vault and calls an allowlisted DEX |
| What settlement reads | `returned`, an argument the agent chooses | The vault's wSOL balance |
| Allowed assets | Disclosure | Enforced: a swap into any other mint is rejected |
| Price protection | None | Every swap must return at least `(1 − max deviation)` of its oracle value |
| Missed deadline | Whole bond to the trader; the principal is gone | The trader unwinds the vault and settles; a fixed penalty from the bond |
| Trader's worst case | `principal − bond` | `tolerance × principal` plus the oracle deviation, minus nothing |

The settlement rule itself (`floor`, `slash`, `fee`) does not change.

## Why the oracle check is not optional

Custody on its own does not stop theft, it changes its shape. Anyone can
create an Orca pool. An operator could create a SOL/USDC pool priced at
1 SOL = 0.000001 USDC, swap the vault's SOL into it, and collect the SOL as
the pool's only liquidity provider. The vault ends empty, the bond is slashed
in full, and the operator keeps `principal − bond`: exactly today's theft.
Restricting swaps to a list of pools does not close it either: in a thin pool
the operator sandwiches the vault's own swap and keeps the slippage.

So every swap is checked against an oracle: the value received must be at
least `(1 − max_swap_deviation)` of the value given, at the oracle's prices
for the two mints. With that, a swap can lose the vault at most the deviation
(a protocol setting, around 1–2% on mainnet), and extracting value needs one
swap per deviation step, each recorded on-chain as a swap at a bad price. The
bond then only has to cover honest drawdown, which is what it was for.

Prices come from Pyth pull-oracle price-update accounts. The protocol config
maps each mint to a Pyth feed id. A swap must carry a price update for both
mints, no older than `max_price_age_secs`, verified at full level, owned by
the receiver program named in the config. A mint without a feed cannot be
traded.

## Accounts

### `Custody` (new, one per trading position)

Seeds `["custody", position]`. Created by `begin_trading`, closed by
`settle_position`. The `Position` layout does not change, so positions that
exist on devnet today stay readable.

| Field | Meaning |
|---|---|
| `position` | The position this belongs to |
| `rent_payer` | Who paid for this account and the vault's wSOL account; gets the rent back at settlement |
| `held_mask: u8` | Bit `i` is set while the vault's token account for `allowed_assets[i]` holds a non-zero balance. wSOL is never in the mask |
| `swaps: u32` | Number of swaps executed |
| `bump` | |

### Vault token accounts

The position vault PDA (`["position_vault", position]`, a system account as
today) is the owner of one associated token account per mint it trades:
`ATA(position_vault, mint)`. The wSOL account is created by `begin_trading`.
Others are created by `open_vault_token_account` and closed, when empty, by
`close_vault_token_account`. The executor pays their rent and gets it back
on close.

### `Config` (extended)

The config account does not exist on devnet yet, so its layout can still
change. New fields, all set by the upgrade authority:

| Field | Meaning |
|---|---|
| `allowed_dex_programs: Vec<Pubkey>` (≤ 4) | Programs `execute_swap` may call. Orca Whirlpools on devnet and mainnet; Jupiter on mainnet later |
| `oracle_program: Pubkey` | Owner every price-update account must have (the Pyth receiver) |
| `max_price_age_secs: i64` | Oldest acceptable publish time |
| `max_swap_deviation_bps: u16` | Largest loss of oracle value one swap may take |
| `feeds: Vec<FeedMapping>` (≤ 16) | `mint → Pyth feed id` |
| `late_penalty_bps: u16` | Share of the locked bond paid to the trader when a custody position settles at or after the deadline |

`set_trading_config` sets all of them at once. `init_config` sets them to
empty, so a fresh deployment cannot trade until the admin fills them in.

## Instructions

### `begin_trading` (replaces `draw_funds`)

Signer: the trading key or the operator. Position must be Open, the protocol
not paused, `now < deadline`.

1. Creates `Custody` (payer: the signer) and the vault's wSOL account if it
   does not exist (payer: the signer).
2. Moves the principal from the vault's lamports into its wSOL account and
   syncs it. The vault keeps its rent floor as today.
3. Status becomes Trading, `drawn_at = now`. Emits `FundsDrawn` as today,
   with `executor` set to the signer.

`draw_funds` stays in the program so old clients get a clear error
(`DrawDisabled`) instead of a wiring failure. It no longer moves funds.

### `execute_swap`

Signer: before the deadline, the trading key or the operator. At or after
the deadline, also the trader, and then only into wSOL (so the position can
be unwound but not traded further).

Arguments: `amount_in_max: u64`, `min_out: u64`, `data: Vec<u8>` (the DEX
instruction data, built by the client).

Accounts: config, agent, position (Trading), custody, position vault,
`vault_in` and `vault_out` (the vault's ATAs; `vault_out`'s mint must be wSOL
or in the agent's allowed assets; the two mints differ), the two mints, the
two price updates, the DEX program, the token programs, then the DEX
instruction's accounts as remaining accounts.

Before the call:

- The DEX program is in `allowed_dex_programs`.
- No remaining account is a token account owned by the vault other than
  `vault_in` and `vault_out`. The vault signs the call, so a third account
  could otherwise be drained.
- Both price updates are owned by the oracle program, fully verified, carry
  the feed id the config maps to that mint, and are fresh.

The call: the DEX instruction is forwarded as given, with the vault PDA
marked as a signer wherever it appears, signed with the vault's seeds.

After the call:

- `in = pre_in − post_in ≤ amount_in_max`, `out = post_out − pre_out ≥ min_out`.
- `out × price_out ≥ in × price_in × (1 − max_swap_deviation)`, in each
  mint's own decimals.
- `vault_in` and `vault_out` still exist, are still owned by the vault, and
  have no delegate and no close authority. The vault's lamports are unchanged.
- `held_mask` is updated from the post balances. `swaps += 1`.

Emits `SwapExecuted { position, mint_in, mint_out, amount_in, amount_out,
oracle_value_in, oracle_value_out }`, so the full trade history of a position
is in the logs.

### `settle_position` (extended)

Same accounts as today, plus trailing optional accounts: custody, the
vault's wSOL account, the rent payer, the token program. A client built for
the current program sends none of them and keeps working for positions drawn
under the current program.

If the position has no `Custody` account, the current rule applies unchanged
(`returned` is sent by the signer). This is the path for positions that are
Trading on devnet at the moment of the upgrade.

If it has one:

- Signer: the trading key or the operator; at or after the deadline, also
  the trader.
- `held_mask` must be 0: everything is back in wSOL.
- The wSOL account is closed into the vault. The rent of the wSOL account and
  of `Custody` goes back to `rent_payer`.
- `returned = vault lamports − rent floor`. The `returned` argument is ignored.
- Settlement runs as today on that amount: fee on profit, slash below the
  floor from the locked bond.
- At or after the deadline, a late penalty of `late_penalty_bps` of the
  locked bond is added to the slash, capped so slash never exceeds the lock,
  and the breach is `MissedDeadline`. Before the deadline the breach is
  `Drawdown` if there was a slash, else none.

### `claim_default` (legacy only)

Keeps working for positions without a `Custody` account. For a custody
position it fails with `UseSettle`: the trader cannot be owed the whole bond,
because the principal is still in the vault. The trader's path is
`execute_swap` into wSOL (after the deadline) and `settle_position`.

### `open_vault_token_account` / `close_vault_token_account`

Signer: the trading key or the operator. Create the vault's ATA for a mint in
the agent's allowed assets (payer: the signer), or close it when its balance
is zero (rent to the signer). Closing is allowed in any position status, so
dust accounts can be cleaned up after settlement.

### Unchanged

`open_position`, `cancel_position`, decline (a settle while Open), every
operator and admin instruction.

## Lifecycle

| From | Action | Who | Result |
|---|---|---|---|
| Open | `begin_trading` | Trading key / operator | Principal wrapped in the vault. Trading |
| Trading | `execute_swap` | Trading key / operator | A price-checked swap between two vault token accounts |
| Trading, `now ≥ deadline` | `execute_swap` into wSOL | Trader too | Unwinding |
| Trading, mask 0 | `settle_position` | Trading key / operator; trader after the deadline | Settled, by the vault's balance |

## What this does to the trust model

The trader's exposure on a custody position is `tolerance × principal` (the
agreed drawdown) plus what the oracle deviation lets a swap lose, and the
bond covers the part beyond the tolerance. Theft is not a path any more:
the vault cannot be moved except by a swap at oracle price into an allowed
mint. A 100% ratio is no longer needed for a fully backed position.

That is what makes a wider tolerance safe to offer: the skim is gone because
the agent never holds the money, and the within-tolerance loss has to be a
real trading loss taken at oracle prices.

What custody does not do:

- It does not check the strategy. An agent that says "momentum" and runs
  mean reversion is not caught. Only constraints are enforced: assets, venue,
  price.
- It does not stop an agent from losing money inside the tolerance by
  trading badly on purpose, one deviation step at a time. It makes that slow,
  visible in the swap log, and bounded by the deviation per swap.
- A mint without a Pyth feed cannot be traded at all.

## Consequences for clients

- **Agent runner** (`agent/`): builds Orca swap instructions with the vault
  PDA as token authority and the vault's ATAs as token accounts, posts Pyth
  price updates in the same transaction, and calls `execute_swap`. Books are
  read from the vault instead of the wallet. `draw` becomes `begin_trading`.
- **Operator SDK** (`sdk/`): the hook model, where an external bot trades from
  the wallet and the runner settles by wallet balance, cannot work: the wallet
  never holds the principal. The SDK gains a swap command the bot calls
  instead, and the runner settles from the vault. Bots that cannot route
  their trades through it (Hummingbot as integrated today) cannot be
  operators on custody positions.
- **App**: "Draw" becomes "Start trading"; "Claim collateral" on a custody
  position becomes "Unwind and settle" (one or more swaps into SOL, then
  settle). The position page shows the vault's holdings and the swap log.
- **Live check**: the simulated lifecycle uses `begin_trading` and the custody
  settle path.

## Deployment

The program change is additive at the interface: new instructions, new
trailing optional accounts on `settle_position`, a new account type, and new
config fields on an account that does not exist on devnet yet. The deploy
order from the README holds: site, runner, program, then `config init` and
`set_trading_config`.

Positions that are Trading at the moment of the upgrade were drawn by
`draw_funds`, hold their principal in the trading wallet, and settle through
the legacy path. New positions can only begin trading through custody.

On devnet the Orca pools price SOL nowhere near the market, so
`max_swap_deviation_bps` has to be set wide there (or the devnet feeds mapped
to a feed the pools track). The oracle check is tested in the program's own
test suite with synthetic price accounts; devnet only tests the wiring.
