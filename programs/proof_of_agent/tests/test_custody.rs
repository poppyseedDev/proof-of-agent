//! Vault custody: begin_trading wraps the principal in the vault, execute_swap
//! trades it through an allowlisted DEX at oracle-checked prices, and
//! settlement reads the vault.

mod common;

use {
    common::*,
    proof_of_agent::events::SwapExecuted,
};

/// A published agent with a bound trading key, one open 1 SOL position, and
/// the mock market allowed at the given deviation and late penalty.
fn setup(deviation_bps: u16, late_penalty_bps: u16) -> (Env, Market, Keypair) {
    let mut env = Env::launched();
    let op = env.op();
    let key = env.executor.insecure_clone();
    env.bind_executor(key.pubkey(), &op).unwrap();
    let m = env.market(150);
    env.allow_market(&m, deviation_bps, late_penalty_bps);
    // The agent's allowed assets are the market's USDC and an unrelated mint.
    let mut a = env.agent_state();
    a.terms.allowed_assets = vec![m.usdc, Pubkey::new_unique()];
    env.write_agent(&a);
    env.open(0, SOL, 3_600).unwrap();
    (env, m, key)
}

#[test]
fn begin_trading_wraps_the_principal_and_the_executor_pays_the_rent() {
    let (mut env, _m, key) = setup(100, 0);
    let (position, vault) = env.position_pda(0);
    let before = env.balance(&key.pubkey());
    env.begin_trading(0, &key).unwrap();
    assert_eq!(before - env.balance(&key.pubkey()), TX_FEE + env.custody_rents());
    // The vault holds nothing while trading; its rent floor is parked in the custody account.
    assert_eq!(env.balance(&vault), 0);
    assert_eq!(env.balance(&env.custody_pda(&position)), env.custody_rent() + env.rent_floor());
    assert_eq!(env.vault_wsol(0), SOL);
    let c = env.custody_state(&position).unwrap();
    assert_eq!((c.position, c.rent_payer, c.held_mask, c.swaps), (position, key.pubkey(), 0, 0));
    let p = env.position_state(0);
    assert_eq!(p.status, PositionStatus::Trading);
    // Not twice.
    assert!(env.begin_trading(0, &key).is_err());
    // The old draw is disabled.
    env.open(1, SOL, 3_600).unwrap();
    assert_err(env.draw_funds_legacy(1, &key), E_DRAW_DISABLED);
}

#[test]
fn settle_reads_the_vault_and_refunds_the_rent() {
    let (mut env, _m, key) = setup(100, 0);
    env.begin_trading(0, &key).unwrap();
    let (trader_before, key_before, op_before) =
        (env.balance(&env.trader.pubkey()), env.balance(&key.pubkey()), env.balance(&env.operator.pubkey()));
    // The agent made 20%: the vault holds 1.2 SOL of wSOL.
    env.set_vault_wsol(&env.trader.pubkey(), 0, 1_200_000_000);
    env.custody_settle(0, &key).unwrap();
    assert_eq!(env.balance(&env.trader.pubkey()) - trader_before, 1_170_000_000 + env.rent_floor());
    assert_eq!(env.balance(&env.operator.pubkey()) - op_before, 30_000_000);
    assert_eq!(env.balance(&key.pubkey()) + TX_FEE - key_before, env.custody_rents());
    let p = env.position_state(0);
    assert_eq!((p.status, p.breach, p.returned, p.fee_paid, p.slashed), (PositionStatus::Settled, Breach::None, 1_200_000_000, 30_000_000, 0));
    let (position, vault) = env.position_pda(0);
    assert!(env.custody_state(&position).is_none());
    assert!(env.token_account(&ata(&vault, &WSOL)).is_none());
    env.check_invariants();
}

#[test]
fn the_operator_can_settle_when_it_also_paid_the_rent() {
    // Operator, executor and rent payer are one wallet, so the same account
    // appears three times in the instruction.
    let mut env = Env::launched();
    let op = env.op();
    let m = env.market(150);
    env.allow_market(&m, 100, 0);
    env.open(0, SOL, 3_600).unwrap();
    env.begin_trading(0, &op).unwrap();
    let (trader_before, op_before) = (env.balance(&env.trader.pubkey()), env.balance(&op.pubkey()));
    env.set_vault_wsol(&env.trader.pubkey(), 0, 1_200_000_000);
    env.custody_settle(0, &op).unwrap();
    assert_eq!(env.balance(&env.trader.pubkey()) - trader_before, 1_170_000_000 + env.rent_floor());
    assert_eq!(env.balance(&op.pubkey()) + TX_FEE - op_before, 30_000_000 + env.custody_rents());
    env.check_invariants();
}

// ---------- swaps ----------

/// 1 SOL = 150 USDC at the market's oracle prices.
const USDC_PER_SOL: u64 = 150_000_000;

/// Setup, trading begun by the bound key, with the vault's USDC account open.
fn trading(deviation_bps: u16, late_penalty_bps: u16) -> (Env, Market, Keypair) {
    let (mut env, m, key) = setup(deviation_bps, late_penalty_bps);
    env.begin_trading(0, &key).unwrap();
    env.open_vault_ata(0, &m.usdc, &key).unwrap();
    (env, m, key)
}

#[test]
fn an_honest_round_trip_moves_tokens_and_tracks_what_the_vault_holds() {
    let (mut env, m, key) = trading(100, 0);
    let (position, vault) = env.position_pda(0);
    let usdc_ata = ata(&vault, &m.usdc);

    // Half the SOL into USDC at the oracle price.
    let logs = env.swap(&m, 0, &key, WSOL, m.usdc, SOL / 2, USDC_PER_SOL / 2).unwrap();
    assert_eq!(env.vault_wsol(0), SOL / 2);
    assert_eq!(env.token_balance(&usdc_ata), USDC_PER_SOL / 2);
    let c = env.custody_state(&position).unwrap();
    assert_eq!((c.held_mask, c.swaps), (0b01, 1));
    assert!(has_event_prefix(&logs, SwapExecuted::DISCRIMINATOR));
    let ev: SwapExecuted = one_event(&logs);
    assert_eq!((ev.position, ev.mint_in, ev.mint_out), (position, WSOL, m.usdc));
    assert_eq!((ev.amount_in, ev.amount_out, ev.held_mask), (SOL / 2, USDC_PER_SOL / 2, 0b01));
    assert_eq!(ev.value_in, ev.value_out);

    // Holding USDC, the position cannot settle.
    assert_err(env.custody_settle(0, &key).map(|_| ()), E_NOT_UNWOUND);

    // Back into SOL, 1% better than the oracle.
    env.swap(&m, 0, &key, m.usdc, WSOL, USDC_PER_SOL / 2, SOL / 2 + SOL / 200).unwrap();
    assert_eq!(env.vault_wsol(0), SOL + SOL / 200);
    assert_eq!(env.token_balance(&usdc_ata), 0);
    let c = env.custody_state(&position).unwrap();
    assert_eq!((c.held_mask, c.swaps), (0, 2));

    // Settles on the vault's balance: a 0.5% profit, 15% of it to the operator.
    let trader_before = env.balance(&env.trader.pubkey());
    env.custody_settle(0, &key).unwrap();
    let p = env.position_state(0);
    assert_eq!((p.returned, p.fee_paid, p.slashed, p.breach), (SOL + SOL / 200, SOL / 200 * 15 / 100, 0, Breach::None));
    assert_eq!(env.balance(&env.trader.pubkey()) - trader_before, p.returned - p.fee_paid + env.rent_floor());
    // The empty USDC account can be closed afterwards, rent back to the key.
    let before = env.balance(&key.pubkey());
    env.close_vault_ata(0, &m.usdc, &key).unwrap();
    assert!(env.balance(&key.pubkey()) > before);
    env.check_invariants();
}

#[test]
fn a_swap_may_lose_at_most_the_deviation_against_the_oracle() {
    let (mut env, m, key) = trading(200, 0);
    let fair = USDC_PER_SOL / 2;
    // 2% below fair passes; one unit below that fails.
    let floor = fair * 98 / 100;
    let ix = env.swap_ix(&m, &env.trader.pubkey(), 0, &key.pubkey(), WSOL, m.usdc, SOL / 2, floor - 1, SOL / 2, 0, mock_dex::mode::HONEST, &[]);
    assert_err(env.send(ix, &key), E_BELOW_ORACLE);
    env.swap(&m, 0, &key, WSOL, m.usdc, SOL / 2, floor).unwrap();
    // The same bound applies on the way back.
    let fair_back = SOL / 2 * 98 / 100;
    let ix = env.swap_ix(&m, &env.trader.pubkey(), 0, &key.pubkey(), m.usdc, WSOL, floor, fair_back * 98 / 100 - 1, floor, 0, mock_dex::mode::HONEST, &[]);
    assert_err(env.send(ix, &key), E_BELOW_ORACLE);
    // Zero deviation: only the oracle price or better.
    env.allow_market(&m, 0, 0);
    let ix = env.swap_ix(&m, &env.trader.pubkey(), 0, &key.pubkey(), m.usdc, WSOL, floor, floor * SOL / USDC_PER_SOL - 1, floor, 0, mock_dex::mode::HONEST, &[]);
    assert_err(env.send(ix, &key), E_BELOW_ORACLE);
    env.swap(&m, 0, &key, m.usdc, WSOL, floor, floor * SOL / USDC_PER_SOL).unwrap();
}

#[test]
fn the_executors_own_bounds_are_enforced_too() {
    let (mut env, m, key) = trading(100, 0);
    let t = env.trader.pubkey();
    // The DEX takes more than amount_in_max.
    let ix = env.swap_ix(&m, &t, 0, &key.pubkey(), WSOL, m.usdc, SOL / 2, USDC_PER_SOL / 2, SOL / 2 - 1, 0, mock_dex::mode::HONEST, &[]);
    assert_err(env.send(ix, &key), E_TOO_MUCH_IN);
    // The DEX returns less than min_out.
    let ix = env.swap_ix(&m, &t, 0, &key.pubkey(), WSOL, m.usdc, SOL / 2, USDC_PER_SOL / 2, SOL / 2, USDC_PER_SOL / 2 + 1, mock_dex::mode::HONEST, &[]);
    assert_err(env.send(ix, &key), E_TOO_LITTLE_OUT);
    // A DEX that keeps the input fails the oracle check even with min_out = 0.
    let ix = env.swap_ix(&m, &t, 0, &key.pubkey(), WSOL, m.usdc, SOL / 2, 0, SOL / 2, 0, mock_dex::mode::KEEP_INPUT, &[]);
    assert_err(env.send(ix, &key), E_BELOW_ORACLE);
    assert_eq!(env.vault_wsol(0), SOL);
}

#[test]
fn only_allowed_dex_programs_and_assets_can_be_used() {
    let (mut env, m, key) = trading(100, 0);
    let t = env.trader.pubkey();
    // A mint the agent did not publish cannot even get a vault account.
    let stranger_mint = Pubkey::new_unique();
    env.create_mint(stranger_mint, 6, None);
    assert_err(env.open_vault_ata(0, &stranger_mint, &key), E_MINT_NOT_ALLOWED);
    // An allowed mint without a price feed cannot be traded.
    let other = env.agent_state().terms.allowed_assets[1];
    env.create_mint(other, 6, None);
    env.open_vault_ata(0, &other, &key).unwrap();
    let ix = env.swap_ix(&m, &t, 0, &key.pubkey(), WSOL, other, SOL / 2, 1, SOL / 2, 0, mock_dex::mode::HONEST, &[]);
    assert_err(env.send(ix, &key), E_NO_FEED);
    // Same mint on both legs: the two legs resolve to one account and the
    // account constraints refuse it before the handler's own check.
    let ix = env.swap_ix(&m, &t, 0, &key.pubkey(), WSOL, WSOL, 1, 1, 1, 0, mock_dex::mode::HONEST, &[]);
    assert!(env.send(ix, &key).is_err());
    // The DEX is taken off the allowlist.
    let admin = env.admin.insecure_clone();
    env.set_trading_config_as(vec![], m.oracle_program, 60, 100, 0, vec![], &admin).unwrap();
    let ix = env.swap_ix(&m, &t, 0, &key.pubkey(), WSOL, m.usdc, SOL / 2, USDC_PER_SOL / 2, SOL / 2, 0, mock_dex::mode::HONEST, &[]);
    assert_err(env.send(ix, &key), E_DEX_NOT_ALLOWED);
    env.allow_market(&m, 100, 0);
    env.swap(&m, 0, &key, WSOL, m.usdc, SOL / 2, USDC_PER_SOL / 2).unwrap();
}

#[test]
fn price_updates_must_be_fresh_verified_and_for_the_right_feed() {
    let (mut env, m, key) = trading(100, 0);
    let t = env.trader.pubkey();
    let good = |env: &Env, key: &Keypair| env.swap_ix(&m, &t, 0, &key.pubkey(), WSOL, m.usdc, SOL / 2, USDC_PER_SOL / 2, SOL / 2, 0, mock_dex::mode::HONEST, &[]);
    // Stale: published 61 s ago with a 60 s limit.
    let now = env.now();
    env.set_price(m.price_sol, m.oracle_program, SOL_FEED, 150 * 100_000_000, -8, now - 61);
    assert_err(env.send(good(&env, &key), &key), E_ORACLE_STALE);
    env.set_price(m.price_sol, m.oracle_program, SOL_FEED, 150 * 100_000_000, -8, now - 60);
    env.send(good(&env, &key), &key).unwrap();
    // Wrong feed in the account.
    env.set_price(m.price_sol, m.oracle_program, OTHER_FEED, 150 * 100_000_000, -8, now);
    assert_err(env.send(good(&env, &key), &key), E_ORACLE_FEED);
    // Not owned by the oracle program.
    env.set_price(m.price_sol, Pubkey::new_unique(), SOL_FEED, 150 * 100_000_000, -8, now);
    assert_err(env.send(good(&env, &key), &key), E_ORACLE_INVALID);
    // Partially verified.
    env.set_price(m.price_sol, m.oracle_program, SOL_FEED, 150 * 100_000_000, -8, now);
    let mut acc = env.svm.get_account(&m.price_sol).unwrap();
    acc.data[8 + 32] = 0; // Partial { num_signatures }
    acc.data.insert(8 + 32 + 1, 5);
    env.svm.set_account(m.price_sol, acc).unwrap();
    assert_err(env.send(good(&env, &key), &key), E_ORACLE_UNVERIFIED);
    // A non-positive price.
    env.set_price(m.price_sol, m.oracle_program, SOL_FEED, 0, -8, now);
    assert_err(env.send(good(&env, &key), &key), E_ORACLE_PRICE);
    env.refresh_prices(&m, 150);
    env.send(good(&env, &key), &key).unwrap();
}

#[test]
fn a_dex_that_touches_the_vault_beyond_the_swap_is_rejected() {
    let (mut env, m, key) = trading(100, 0);
    let t = env.trader.pubkey();
    let (_, vault) = env.position_pda(0);
    let attacker = Pubkey::new_unique();
    let swap = |env: &Env, mode: u8, extra: &[Pubkey]| env.swap_ix(&m, &t, 0, &key.pubkey(), WSOL, m.usdc, SOL / 2, USDC_PER_SOL / 2, SOL / 2, 0, mode, extra);
    assert_err(env.send(swap(&env, mock_dex::mode::APPROVE_DELEGATE, &[attacker]), &key), E_TAMPERED);
    assert_err(env.send(swap(&env, mock_dex::mode::SET_CLOSE_AUTHORITY, &[attacker]), &key), E_TAMPERED);
    // The executor builds the DEX call with the vault writable, as a thief would.
    // The vault holds no lamports while trading, so there is nothing to take:
    // the system transfer inside the DEX fails before the program's own check.
    let mut ix = swap(&env, mock_dex::mode::TAKE_LAMPORTS, &[attacker, system_program::ID]);
    for a in ix.accounts.iter_mut().filter(|a| a.pubkey == vault) {
        a.is_writable = true;
    }
    assert_err(env.send(ix, &key), E_SYSTEM_INSUFFICIENT);
    // A third vault token account in the DEX's accounts is refused before the call.
    let other = env.agent_state().terms.allowed_assets[1];
    env.create_mint(other, 6, None);
    env.open_vault_ata(0, &other, &key).unwrap();
    let third = ata(&vault, &other);
    assert_err(env.send(swap(&env, mock_dex::mode::DRAIN_THIRD_ACCOUNT, &[third]), &key), E_EXTRA_VAULT_ACCOUNT);
    // Nothing moved.
    assert_eq!(env.vault_wsol(0), SOL);
    assert_eq!(env.balance(&vault), 0);
    let acc = env.token_account(&ata(&vault, &WSOL)).unwrap();
    assert!(acc.delegate.is_none() && acc.close_authority.is_none());
    env.send(swap(&env, mock_dex::mode::HONEST, &[]), &key).unwrap();
}

// ---------- who may swap, and when ----------

#[test]
fn before_the_deadline_only_the_agent_swaps_and_after_it_only_unwinds() {
    let (mut env, m, key) = trading(100, 0);
    let t = env.trader.pubkey();
    let trader = env.tr();
    let stranger = env.funded(SOL);
    let to_usdc = |env: &Env, who: &Pubkey| env.swap_ix(&m, &t, 0, who, WSOL, m.usdc, SOL / 4, USDC_PER_SOL / 4, SOL / 4, 0, mock_dex::mode::HONEST, &[]);
    let to_sol = |env: &Env, who: &Pubkey| env.swap_ix(&m, &t, 0, who, m.usdc, WSOL, USDC_PER_SOL / 4, SOL / 4, USDC_PER_SOL / 4, 0, mock_dex::mode::HONEST, &[]);

    assert_err(env.send(to_usdc(&env, &stranger.pubkey()), &stranger), E_EXECUTOR);
    assert_err(env.send(to_usdc(&env, &trader.pubkey()), &trader), E_EXECUTOR);
    env.send(to_usdc(&env, &key.pubkey()), &key).unwrap();
    env.send(to_usdc(&env, &key.pubkey()), &key).unwrap();

    // Paused: no new exposure, but unwinding always works.
    env.set_paused(true).unwrap();
    assert_err(env.send(to_usdc(&env, &key.pubkey()), &key), E_PAUSED);
    env.send(to_sol(&env, &key.pubkey()), &key).unwrap();
    env.set_paused(false).unwrap();

    // At the deadline the agent may only unwind; the trader may unwind too.
    let deadline = env.position_state(0).deadline;
    env.set_time(deadline);
    env.refresh_prices(&m, 150);
    assert_err(env.send(to_usdc(&env, &key.pubkey()), &key), E_ONLY_UNWIND);
    assert_err(env.send(to_usdc(&env, &trader.pubkey()), &trader), E_ONLY_UNWIND);
    assert_err(env.send(to_sol(&env, &stranger.pubkey()), &stranger), E_EXECUTOR);
    env.send(to_sol(&env, &trader.pubkey()), &trader).unwrap();
    assert_eq!(env.vault_wsol(0), SOL);
    assert_eq!(env.custody_state(&env.position_pda(0).0).unwrap().held_mask, 0);

    // And the trader settles it, late, with the rent going back to the key.
    let (trader_before, key_before) = (env.balance(&trader.pubkey()), env.balance(&key.pubkey()));
    env.custody_settle_for(&t, 0, &trader, Some(key.pubkey())).unwrap();
    let p = env.position_state(0);
    assert_eq!((p.status, p.breach, p.returned, p.slashed), (PositionStatus::Settled, Breach::MissedDeadline, SOL, 0));
    assert_eq!(env.balance(&trader.pubkey()) + TX_FEE - trader_before, SOL + env.rent_floor());
    assert_eq!(env.balance(&key.pubkey()) - key_before, env.custody_rents());
    assert_counters(&env.agent_state(), 0, 1, 0, 1);
}

#[test]
fn the_trader_cannot_settle_before_the_deadline_and_the_rent_payer_must_match() {
    let (mut env, _m, key) = trading(100, 0);
    let t = env.trader.pubkey();
    let trader = env.tr();
    assert_err(env.custody_settle_for(&t, 0, &trader, Some(key.pubkey())).map(|_| ()), E_EXECUTOR);
    // Wrong rent payer.
    assert_err(env.custody_settle_for(&t, 0, &key, Some(Pubkey::new_unique())).map(|_| ()), E_OPERATOR);
    // Custody accounts left out by an old client.
    let mut accounts = env.custody_settle_accounts_for(&t, 0, &key.pubkey(), &key.pubkey());
    accounts.vault_wsol = None;
    assert_err(env.settle_send(accounts, 0, &key).map(|_| ()), E_CUSTODY_MISSING);
    let mut accounts = env.custody_settle_accounts_for(&t, 0, &key.pubkey(), &key.pubkey());
    accounts.config = None;
    assert_err(env.settle_send(accounts, 0, &key).map(|_| ()), E_CUSTODY_MISSING);
    env.custody_settle(0, &key).unwrap();
}

#[test]
fn a_late_settle_costs_the_late_penalty_on_top_of_any_drawdown_slash() {
    // 30% ratio: 0.3 SOL locked. 10% late penalty: 0.03 SOL.
    let (mut env, _m, key) = trading(100, 1_000);
    let deadline = env.position_state(0).deadline;
    env.set_time(deadline + 1);
    let trader_before = env.balance(&env.trader.pubkey());
    // No drawdown: the penalty alone.
    env.set_vault_wsol(&env.trader.pubkey(), 0, SOL);
    env.custody_settle(0, &key).unwrap();
    let p = env.position_state(0);
    assert_eq!((p.breach, p.slashed), (Breach::MissedDeadline, 30_000_000));
    assert_eq!(env.balance(&env.trader.pubkey()) - trader_before, SOL + 30_000_000 + env.rent_floor());

    // A drawdown that already takes most of the bond: the penalty is capped at what is left.
    env.open(1, SOL, 60).unwrap();
    env.begin_trading(1, &key).unwrap();
    env.advance_time(61);
    env.set_vault_wsol(&env.trader.pubkey(), 1, 510_000_000); // loss 0.49, tolerance 0.2 -> slash 0.29
    env.custody_settle(1, &key).unwrap();
    let p = env.position_state(1);
    assert_eq!(p.slashed, 300_000_000);
    assert_eq!(env.agent_state().slashed_total, 330_000_000);
    env.check_invariants();
}

#[test]
fn vault_token_accounts_open_and_close_under_the_rules() {
    let (mut env, m, key) = trading(100, 0);
    let stranger = env.funded(SOL);
    assert_err(env.close_vault_ata(0, &m.usdc, &stranger), E_EXECUTOR);
    // The wSOL account stays while the position trades.
    assert_err(env.close_vault_ata(0, &WSOL, &key), E_WSOL_IN_USE);
    // A funded account cannot be closed.
    env.swap(&m, 0, &key, WSOL, m.usdc, SOL / 2, USDC_PER_SOL / 2).unwrap();
    assert_err(env.close_vault_ata(0, &m.usdc, &key), E_NOT_EMPTY);
    env.swap(&m, 0, &key, m.usdc, WSOL, USDC_PER_SOL / 2, SOL / 2).unwrap();
    env.close_vault_ata(0, &m.usdc, &key).unwrap();
    // Opening it again works; begin_trading cannot run on a settled position.
    env.open_vault_ata(0, &m.usdc, &key).unwrap();
    env.custody_settle(0, &key).unwrap();
    assert_err(env.begin_trading(0, &key), E_STATUS);
    env.close_vault_ata(0, &m.usdc, &key).unwrap();
}

#[test]
fn begin_trading_respects_the_pause_and_the_deadline() {
    let (mut env, _m, key) = setup(100, 0);
    env.set_paused(true).unwrap();
    assert_err(env.begin_trading(0, &key), E_PAUSED);
    env.set_paused(false).unwrap();
    env.open(1, SOL, 60).unwrap();
    env.advance_time(60);
    assert_err(env.begin_trading(1, &key), E_DEADLINE_PASSED);
    env.begin_trading(0, &key).unwrap();
}

#[test]
fn trading_config_is_validated_and_admin_only() {
    let mut env = Env::new();
    let m = env.market(150);
    let admin = env.admin.insecure_clone();
    let op = env.op();
    let feeds = |env: &Env| vec![proof_of_agent::state::FeedMapping { mint: WSOL, feed_id: SOL_FEED }];
    assert_err(env.set_trading_config_as(vec![mock_dex::ID], m.oracle_program, 60, 100, 0, feeds(&env), &op).map(|_| ()), E_ADMIN);
    assert_err(env.set_trading_config_as(vec![mock_dex::ID], m.oracle_program, 0, 100, 0, feeds(&env), &admin).map(|_| ()), E_TRADING_CONFIG);
    assert_err(env.set_trading_config_as(vec![mock_dex::ID], m.oracle_program, 60, 5_001, 0, feeds(&env), &admin).map(|_| ()), E_TRADING_CONFIG);
    assert_err(env.set_trading_config_as(vec![mock_dex::ID], m.oracle_program, 60, 100, 5_001, feeds(&env), &admin).map(|_| ()), E_TRADING_CONFIG);
    let dup = vec![feeds(&env)[0], feeds(&env)[0]];
    assert_err(env.set_trading_config_as(vec![mock_dex::ID], m.oracle_program, 60, 100, 0, dup, &admin).map(|_| ()), E_TRADING_CONFIG);
    assert_err(env.set_trading_config_as(vec![Pubkey::new_unique(); 5], m.oracle_program, 60, 100, 0, vec![], &admin).map(|_| ()), E_TRADING_CONFIG);
    env.set_trading_config_as(vec![mock_dex::ID], m.oracle_program, 60, 100, 1_000, feeds(&env), &admin).unwrap();
    let c = env.config_state();
    assert_eq!((c.allowed_dex_programs.clone(), c.max_price_age_secs, c.max_swap_deviation_bps, c.late_penalty_bps, c.feeds.len()), (vec![mock_dex::ID], 60, 100, 1_000, 1));
    assert_eq!(c.feed_for(&WSOL).unwrap(), SOL_FEED);
    assert!(c.feed_for(&m.usdc).is_err());
}

#[test]
fn legacy_positions_still_settle_and_default_the_old_way() {
    let mut env = Env::launched();
    let op = env.op();
    env.open(0, SOL, 3_600).unwrap();
    env.legacy_draw(0, &op.pubkey());
    assert!(env.custody_state(&env.position_pda(0).0).is_none());
    // No custody: the signer sends the returned SOL, as before.
    let (trader_before, op_before) = (env.balance(&env.trader.pubkey()), env.balance(&op.pubkey()));
    env.legacy_settle(0, 800_000_000, &op).unwrap();
    let p = env.position_state(0);
    assert_eq!((p.returned, p.slashed, p.breach), (800_000_000, 0, Breach::None));
    assert_eq!(env.balance(&env.trader.pubkey()) - trader_before, 800_000_000 + env.rent_floor());
    assert_eq!(op_before - env.balance(&op.pubkey()), 800_000_000 + TX_FEE);
    // A late legacy settle is a missed-deadline breach without a late penalty.
    env.open(1, SOL, 60).unwrap();
    env.legacy_draw(1, &op.pubkey());
    env.advance_time(60);
    env.legacy_settle(1, SOL, &op).unwrap();
    let p = env.position_state(1);
    assert_eq!((p.breach, p.slashed), (Breach::MissedDeadline, 0));
    env.check_invariants();
}
