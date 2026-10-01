//! Collateral withdrawal, open_position bounds and rounding, draw_funds, and
//! settlement payouts to wallets near the rent-exempt minimum.

mod common;

use {common::*, proof_of_agent::instructions::required_collateral};

// ---------- withdraw ----------

#[test]
fn locked_collateral_cannot_be_withdrawn_but_the_free_part_can_to_the_lamport() {
    let mut env = Env::launched();
    env.open(0, SOL, 3_600).unwrap(); // locks 0.3
    env.open(1, 1_000_000_001, 3_600).unwrap(); // locks 300_000_001 (rounded up)
    let a = env.agent_state();
    assert_eq!(a.locked_collateral, 600_000_001);
    assert_eq!(a.free_collateral(), 399_999_999);

    assert_err(env.withdraw(SOL), E_INSUFFICIENT);
    assert_err(env.withdraw(400_000_000), E_INSUFFICIENT);
    let op_before = env.balance(&env.operator.pubkey());
    env.withdraw(399_999_999).unwrap();
    assert_eq!(env.balance(&env.operator.pubkey()) - op_before + TX_FEE, 399_999_999);
    assert_err(env.withdraw(1), E_INSUFFICIENT);

    let a = env.agent_state();
    assert_eq!((a.total_collateral, a.locked_collateral, a.free_collateral()), (600_000_001, 600_000_001, 0));
    assert_eq!(env.balance(&env.agent_vault), 600_000_001 + env.rent_floor());
    env.check_invariants();

    // Releasing a lock frees exactly that amount.
    env.cancel(0).unwrap();
    assert_err(env.withdraw(300_000_001), E_INSUFFICIENT);
    env.withdraw(300_000_000).unwrap();
    env.check_invariants();
}

#[test]
fn withdrawing_all_collateral_keeps_the_vault_rent_floor() {
    let mut env = Env::launched();
    let rent = env.rent_floor();
    assert_eq!(env.balance(&env.agent_vault), SOL + rent);
    env.withdraw(SOL).unwrap();
    assert_eq!(env.agent_state().total_collateral, 0);
    // The vault survives with exactly its rent floor, so it can be refilled.
    assert_eq!(env.balance(&env.agent_vault), rent);
    assert!(env.svm.get_account(&env.agent_vault).is_some());
    assert_err(env.withdraw(1), E_INSUFFICIENT);
    env.deposit(5).unwrap();
    assert_eq!(env.balance(&env.agent_vault), rent + 5);
    env.withdraw(5).unwrap();
    assert_eq!(env.balance(&env.agent_vault), rent);
    env.check_invariants();
}

#[test]
fn free_collateral_is_correct_after_a_partial_slash_and_a_new_open() {
    let mut env = Env::launched();
    let op = env.op();
    env.open(0, SOL, 3_600).unwrap(); // lock 0.3
    env.open(1, SOL, 3_600).unwrap(); // lock 0.3
    env.draw(0, &op).unwrap();
    // Loss 0.3, tolerance 0.2 -> slash 0.1 of the 0.3 lock.
    env.settle(0, 700_000_000, &op).unwrap();
    assert_eq!(env.position_state(0).slashed, 100_000_000);
    let a = env.agent_state();
    assert_eq!(a.total_collateral, 900_000_000);
    assert_eq!(a.locked_collateral, 300_000_000); // only position 1's lock remains
    assert_eq!(a.free_collateral(), 600_000_000);
    env.check_invariants();

    // New open uses the reduced free collateral.
    assert_err(env.open(2, 2_000_000_001, 3_600), E_INSUFFICIENT); // needs 600_000_001
    env.open(2, 2 * SOL, 3_600).unwrap(); // needs exactly 0.6
    let a = env.agent_state();
    assert_eq!((a.total_collateral, a.locked_collateral, a.free_collateral()), (900_000_000, 900_000_000, 0));
    assert_err(env.withdraw(1), E_INSUFFICIENT);
    assert_err(env.open(3, 1, 3_600), E_INSUFFICIENT);
    assert_eq!(env.balance(&env.agent_vault), 900_000_000 + env.rent_floor());

    // Cancel position 1 and the freed 0.3 is withdrawable, not a lamport more.
    env.cancel(1).unwrap();
    assert_err(env.withdraw(300_000_001), E_INSUFFICIENT);
    env.withdraw(300_000_000).unwrap();
    let a = env.agent_state();
    assert_eq!((a.total_collateral, a.locked_collateral), (600_000_000, 600_000_000));
    env.check_invariants();
}

// ---------- open_position ----------

#[test]
fn open_rejects_zero_and_accepts_exact_capacity() {
    let mut env = Env::launched();
    assert_err(env.open(0, 0, 3_600), E_ZERO);
    // 1 SOL at 30%: ceil(p * 0.3) <= 1e9  <=>  p <= 3_333_333_333.
    assert_eq!(required_collateral(3_333_333_333, 3_000).unwrap(), SOL);
    assert_eq!(required_collateral(3_333_333_334, 3_000).unwrap(), SOL + 1);
    assert_err(env.open(0, 3_333_333_334, 3_600), E_INSUFFICIENT);
    assert_err(env.open(0, u64::MAX / 2, 3_600), E_INSUFFICIENT);
    env.open(0, 3_333_333_333, 3_600).unwrap();
    let a = env.agent_state();
    assert_eq!((a.locked_collateral, a.free_collateral()), (SOL, 0));
    assert_eq!(a.capital_managed, 3_333_333_333);
    assert_err(env.open(1, 1, 3_600), E_INSUFFICIENT);
    env.check_invariants();
}

#[test]
fn open_rejects_a_principal_the_trader_cannot_pay() {
    let mut env = Env::new();
    env.create(terms(1_000, 0, 0)).unwrap();
    env.deposit(50 * SOL).unwrap();
    env.publish().unwrap();
    // Trader has ~100 SOL; 200 SOL is backed by the bond but unaffordable.
    assert!(env.open(0, 200 * SOL, 3_600).is_err());
    let a = env.agent_state();
    assert_eq!((a.locked_collateral, a.capital_managed, a.open_positions), (0, 0, 0));
}

#[test]
fn open_duration_must_sit_inside_the_agent_window() {
    let mut env = Env::new();
    let mut t = terms(3_000, 0, 0);
    t.min_duration_secs = 3_600;
    t.max_duration_secs = 2 * 3_600;
    env.create(t).unwrap();
    env.deposit(SOL).unwrap();
    env.publish().unwrap();

    for (nonce, d) in [(0u64, 3_599i64), (1, 7_201), (2, 0), (3, -3_600), (4, i64::MAX), (5, i64::MIN), (6, 60)] {
        assert_err(env.open(nonce, SOL / 100, d), E_DURATION);
    }
    let t0 = env.now();
    env.open(10, SOL / 100, 3_600).unwrap();
    env.open(11, SOL / 100, 7_200).unwrap();
    assert_eq!(env.position_state(10).deadline, t0 + 3_600);
    assert_eq!(env.position_state(11).deadline, t0 + 7_200);
    assert_eq!(env.agent_state().open_positions, 2);
}

#[test]
fn open_records_a_full_snapshot_and_funds_the_vault() {
    let mut env = Env::launched();
    let t0 = env.now();
    let trader_before = env.balance(&env.trader.pubkey());
    env.open(42, SOL / 3, 7_200).unwrap();
    let (_, vault) = env.position_pda(42);
    let rent = env.rent_floor();
    assert_eq!(env.balance(&vault), SOL / 3 + rent);
    let p = env.position_state(42);
    assert_eq!(p.trader, env.trader.pubkey());
    assert_eq!(p.agent, env.agent);
    assert_eq!(p.nonce, 42);
    assert_eq!(p.principal, SOL / 3);
    assert_eq!(p.locked_collateral, 100_000_000); // ceil(333_333_333 * 0.3)
    assert_eq!((p.fee_bps, p.max_drawdown_bps), (1_500, 2_000));
    assert_eq!((p.status, p.breach), (PositionStatus::Open, Breach::None));
    assert_eq!((p.opened_at, p.deadline), (t0, t0 + 7_200));
    assert_eq!((p.drawn_at, p.closed_at, p.returned, p.slashed, p.fee_paid), (0, 0, 0, 0, 0));
    // The trader paid principal + vault rent + position account rent + fee.
    let pos_rent = env.svm.get_account(&env.position_pda(42).0).unwrap().lamports;
    assert_eq!(trader_before - env.balance(&env.trader.pubkey()), SOL / 3 + rent + pos_rent + TX_FEE);

    // A nonce cannot be reused.
    assert!(env.open(42, SOL / 3, 7_200).is_err());
    assert_eq!(env.agent_state().open_positions, 1);
}

#[test]
fn required_collateral_rounds_up() {
    // Pure function.
    assert_eq!(required_collateral(0, 1_000).unwrap(), 0);
    assert_eq!(required_collateral(1, 1_000).unwrap(), 1);
    assert_eq!(required_collateral(1, 10_000).unwrap(), 1);
    assert_eq!(required_collateral(9, 1_000).unwrap(), 1);
    assert_eq!(required_collateral(10, 1_000).unwrap(), 1);
    assert_eq!(required_collateral(11, 1_000).unwrap(), 2);
    assert_eq!(required_collateral(10_000, 3_333).unwrap(), 3_333);
    assert_eq!(required_collateral(10_001, 3_333).unwrap(), 3_334);
    assert_eq!(required_collateral(u64::MAX, 10_000).unwrap(), u64::MAX);
    assert_eq!(required_collateral(u64::MAX, 1_000).unwrap(), u64::MAX / 10 + 1);
    let mut rng = Rng::new(0xC0FFEE);
    for _ in 0..10_000 {
        let p = rng.next_u64() >> rng.range(0, 63);
        let r = rng.range(MIN_COLLATERAL_RATIO_BPS as u64, MAX_COLLATERAL_RATIO_BPS as u64) as u16;
        let got = required_collateral(p, r).unwrap() as u128;
        let exact = p as u128 * r as u128;
        assert!(got * 10_000 >= exact, "rounded down: p={p} r={r}");
        assert!(got * 10_000 < exact + 10_000, "rounded up by more than one: p={p} r={r}");
    }

    // On chain: a 1 lamport principal at 10% still locks 1 lamport.
    let mut env = Env::new();
    env.create(terms(1_000, 0, 0)).unwrap();
    env.deposit(2).unwrap();
    env.publish().unwrap();
    env.open(0, 1, 3_600).unwrap();
    assert_eq!(env.position_state(0).locked_collateral, 1);
    assert_eq!(env.agent_state().locked_collateral, 1);
    // 11 lamports need 2 of collateral; only 1 is free.
    assert_err(env.open(1, 11, 3_600), E_INSUFFICIENT);
    env.open(1, 10, 3_600).unwrap();
    let a = env.agent_state();
    assert_eq!((a.locked_collateral, a.free_collateral(), a.capital_managed), (2, 0, 11));
    env.check_invariants();
}

#[test]
fn one_lamport_position_runs_end_to_end() {
    let mut env = Env::new();
    env.create(terms(1_000, 500, 0)).unwrap();
    env.deposit(1).unwrap();
    env.publish().unwrap();
    let op = env.op();
    env.open(0, 1, 3_600).unwrap();
    env.draw(0, &op).unwrap();
    // Returning nothing with zero tolerance slashes the whole 1-lamport lock.
    env.settle(0, 0, &op).unwrap();
    let p = env.position_state(0);
    assert_eq!((p.slashed, p.breach), (1, Breach::Drawdown));
    let a = env.agent_state();
    assert_eq!((a.total_collateral, a.locked_collateral, a.slashed_total), (0, 0, 1));
    assert_eq!(env.balance(&env.agent_vault), env.rent_floor());
    env.check_invariants();
}

// ---------- draw_funds ----------

#[test]
fn draw_twice_fails_and_moves_the_principal_once() {
    let mut env = Env::launched();
    let op = env.op();
    env.open(0, SOL, 3_600).unwrap();
    let (_, vault) = env.position_pda(0);
    let t = env.now();
    env.draw(0, &op).unwrap();
    assert_eq!(env.balance(&vault), env.rent_floor());
    let p = env.position_state(0);
    assert_eq!((p.status, p.drawn_at), (PositionStatus::Trading, t));
    let op_bal = env.balance(&op.pubkey());
    // The custody account already exists, so the second draw fails before the handler.
    assert!(env.draw(0, &op).is_err());
    // Only the failed transaction's fee left the operator's wallet.
    assert_eq!(env.balance(&op.pubkey()), op_bal - TX_FEE);
    assert_eq!(env.balance(&vault), env.rent_floor());
    assert_eq!(env.vault_wsol(0), SOL);
    // Neither does a different valid executor succeed on a drawn position.
    let k = env.executor.insecure_clone();
    env.bind_executor(k.pubkey(), &op).unwrap();
    assert!(env.draw(0, &k).is_err());
    assert_eq!(env.position_state(0).status, PositionStatus::Trading);
}

#[test]
fn draw_rejects_signers_that_are_not_the_executor() {
    let mut env = Env::launched();
    let op = env.op();
    let k = env.executor.insecure_clone();
    env.bind_executor(k.pubkey(), &op).unwrap();
    env.open(0, SOL, 3_600).unwrap();
    let stranger = env.funded(SOL);
    let trader = env.tr();
    for s in [&stranger, &trader] {
        assert_err(env.draw(0, s), E_EXECUTOR);
    }
    // A stranger naming the real executor as the account but signing
    // themselves cannot sneak in: the executor must sign.
    let mut ix = env.draw_ix(&env.trader.pubkey(), 0, &k.pubkey());
    ix.accounts[0].is_signer = false;
    assert!(env.send(ix, &stranger).is_err());
    assert_eq!(env.position_state(0).status, PositionStatus::Open);
    env.draw(0, &k).unwrap();
}

#[test]
fn draw_after_cancel_settle_or_default_fails() {
    let mut env = Env::launched();
    let op = env.op();
    env.open(0, SOL / 10, 3_600).unwrap();
    env.cancel(0).unwrap();
    assert_err(env.draw(0, &op), E_STATUS);

    env.open(1, SOL / 10, 3_600).unwrap();
    env.settle(1, 0, &op).unwrap(); // decline
    assert_err(env.draw(1, &op), E_STATUS);

    env.open(2, SOL / 10, 3_600).unwrap();
    env.draw(2, &op).unwrap();
    env.settle(2, SOL / 10, &op).unwrap();
    assert_err(env.draw(2, &op), E_STATUS);

    env.open(3, SOL / 10, 60).unwrap();
    env.draw(3, &op).unwrap();
    env.advance_time(60);
    let tr = env.tr();
    env.settle(3, SOL / 10, &tr).unwrap(); // the trader settles it late
    assert_err(env.draw(3, &op), E_STATUS);

    let a = env.agent_state();
    assert_counters(&a, 0, 2, 0, 1);
    assert_eq!(a.locked_collateral, 0);
    env.check_invariants();
}

#[test]
fn cancel_after_draw_and_settle_or_claim_on_open_are_rejected() {
    let mut env = Env::launched();
    let op = env.op();
    env.open(0, SOL, 60).unwrap();
    // Claiming a default needs a drawn position, even past the deadline.
    env.advance_time(120);
    assert_err(env.claim_default(0), E_STATUS);
    env.cancel(0).unwrap();
    // A cancelled position cannot be settled.
    assert_err(env.settle(0, SOL, &op), E_STATUS);

    // Another trader cannot cancel someone else's position.
    env.open(1, SOL, 3_600).unwrap();
    let other = env.funded(SOL);
    let ix = {
        let mut ix = env.cancel_ix(&env.trader.pubkey(), 1);
        ix.accounts[0].pubkey = other.pubkey();
        ix
    };
    assert!(env.send(ix, &other).is_err());
    assert_eq!(env.position_state(1).status, PositionStatus::Open);
}

// ---------- payouts to empty wallets ----------
//
// The runtime rejects a transaction that leaves a 0-data account holding
// lamports below the rent-exempt minimum. LiteSVM only checks accounts with
// data, so these tests pin the program's side of it: a fee that would leave
// the operator wallet under the floor goes to the trader instead. The runtime
// side is covered by app/tests/localnet.test.ts against solana-test-validator.

/// Launched agent with a separate trading key, 1 SOL drawn, operator wallet
/// holding `operator_lamports`.
fn drawn_with_operator_balance(operator_lamports: u64) -> (Env, Keypair) {
    let mut env = Env::launched();
    let (op, ex) = (env.op(), env.executor.insecure_clone());
    env.bind_executor(ex.pubkey(), &op).unwrap();
    env.open(0, SOL, 3_600).unwrap();
    env.draw(0, &ex).unwrap();
    let mut acc = env.svm.get_account(&op.pubkey()).unwrap();
    acc.lamports = operator_lamports;
    env.svm.set_account(op.pubkey(), acc).unwrap();
    (env, ex)
}

#[test]
fn a_fee_below_the_rent_floor_goes_to_the_trader_when_the_operator_wallet_is_empty() {
    let (mut env, ex) = drawn_with_operator_balance(0);
    let trader_before = env.balance(&env.trader.pubkey());
    // 15% of a 1_000_000 profit = 150_000, well under the ~890_880 floor.
    env.settle(0, SOL + 1_000_000, &ex).unwrap();
    assert_eq!(env.balance(&env.operator.pubkey()), 0);
    assert_eq!(env.balance(&env.trader.pubkey()) - trader_before, SOL + 1_000_000 + env.rent_floor());
    let p = env.position_state(0);
    assert_eq!((p.status, p.breach, p.fee_paid), (PositionStatus::Settled, Breach::None, 0));
    assert_eq!(env.agent_state().fees_earned, 0);
    env.check_invariants();
}

#[test]
fn the_fee_is_paid_when_it_lifts_the_operator_wallet_to_the_rent_floor() {
    let floor = Env::new().rent_floor();
    // Exactly at the floor after the 150_000 fee.
    let (mut env, ex) = drawn_with_operator_balance(floor - 150_000);
    env.settle(0, SOL + 1_000_000, &ex).unwrap();
    assert_eq!(env.balance(&env.operator.pubkey()), floor);
    assert_eq!(env.position_state(0).fee_paid, 150_000);
    assert_eq!(env.agent_state().fees_earned, 150_000);

    // One lamport short: the trader keeps it.
    let (mut env, ex) = drawn_with_operator_balance(floor - 150_001);
    env.settle(0, SOL + 1_000_000, &ex).unwrap();
    assert_eq!(env.balance(&env.operator.pubkey()), floor - 150_001);
    assert_eq!(env.position_state(0).fee_paid, 0);

    // A fee above the floor reaches an empty wallet.
    let (mut env, ex) = drawn_with_operator_balance(0);
    env.settle(0, SOL + SOL / 5, &ex).unwrap();
    assert_eq!(env.balance(&env.operator.pubkey()), 3 * SOL / 100);
    assert_eq!(env.position_state(0).fee_paid, 3 * SOL / 100);
    env.check_invariants();
}
