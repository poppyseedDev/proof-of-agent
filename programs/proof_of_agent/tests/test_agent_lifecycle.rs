//! Agent lifecycle: create validation edges, operator-only checks, draft and
//! paused behaviour, trading-key rotation, collateral deposit and withdraw.

mod common;

use {
    common::*,
    proof_of_agent::events::{AgentCreated, AgentPublished, CollateralChanged, ExecutorBound},
};

/// Try `t` on a fresh agent id so that a success does not block later cases.
fn try_create(env: &mut Env, id: u64, t: AgentTerms) -> Result<(), String> {
    env.switch_agent(id);
    env.create(t)
}

// ---------- create_agent validation edges ----------

#[test]
fn collateral_ratio_bounds_are_inclusive() {
    let mut env = Env::new();
    assert_err(try_create(&mut env, 1, terms(MIN_COLLATERAL_RATIO_BPS - 1, 0, 0)), E_RATIO);
    assert_err(try_create(&mut env, 2, terms(MAX_COLLATERAL_RATIO_BPS + 1, 0, 0)), E_RATIO);
    assert_err(try_create(&mut env, 3, terms(0, 0, 0)), E_RATIO);
    assert_err(try_create(&mut env, 4, terms(u16::MAX, 0, 0)), E_RATIO);
    try_create(&mut env, 5, terms(MIN_COLLATERAL_RATIO_BPS, 0, 0)).unwrap();
    try_create(&mut env, 6, terms(MAX_COLLATERAL_RATIO_BPS, 0, 0)).unwrap();
    assert_eq!(env.agent_state().terms.collateral_ratio_bps, 10_000);
}

#[test]
fn fee_cap_is_half_the_ratio_rounded_down() {
    let mut env = Env::new();
    // (ratio, largest allowed fee)
    for (i, (ratio, cap)) in [(1_000u16, 500u16), (3_000, 1_500), (3_001, 1_500), (9_999, 4_999), (10_000, 5_000)]
        .into_iter()
        .enumerate()
    {
        assert_eq!(AgentTerms::max_fee_bps(ratio), cap);
        assert_eq!(cap, ratio / FEE_CAP_DIVISOR);
        let id = 10 * i as u64;
        assert_err(try_create(&mut env, id, terms(ratio, cap + 1, 0)), E_FEE);
        try_create(&mut env, id + 1, terms(ratio, cap, 0)).unwrap();
        try_create(&mut env, id + 2, terms(ratio, 0, 0)).unwrap();
    }
    // A fee larger than 100% is rejected by the cap, not an overflow.
    assert_err(try_create(&mut env, 99, terms(10_000, u16::MAX, 0)), E_FEE);
}

#[test]
fn drawdown_max_is_inclusive_and_checked_before_the_combined_bound() {
    let mut env = Env::new();
    try_create(&mut env, 1, terms(1_000, 0, MAX_DRAWDOWN_BPS)).unwrap();
    assert_err(try_create(&mut env, 2, terms(1_000, 0, MAX_DRAWDOWN_BPS + 1)), E_DRAWDOWN);
    // Would also break ratio + drawdown <= 100%, but the drawdown cap fires first.
    assert_err(try_create(&mut env, 3, terms(9_000, 0, MAX_DRAWDOWN_BPS + 1)), E_DRAWDOWN);
    assert_err(try_create(&mut env, 4, terms(1_000, 0, u16::MAX)), E_DRAWDOWN);
    try_create(&mut env, 5, terms(1_000, 0, 0)).unwrap();
}

#[test]
fn trading_window_bounds() {
    let mut env = Env::new();
    let window = |min: i64, max: i64| {
        let mut t = terms(3_000, 0, 0);
        t.min_duration_secs = min;
        t.max_duration_secs = max;
        t
    };
    assert_err(try_create(&mut env, 1, window(MIN_POSITION_DURATION - 1, 3_600)), E_WINDOW);
    assert_err(try_create(&mut env, 2, window(0, 3_600)), E_WINDOW);
    assert_err(try_create(&mut env, 3, window(-60, 3_600)), E_WINDOW);
    assert_err(try_create(&mut env, 4, window(60, MAX_POSITION_DURATION + 1)), E_WINDOW);
    assert_err(try_create(&mut env, 5, window(3_601, 3_600)), E_WINDOW); // min > max
    assert_err(try_create(&mut env, 6, window(i64::MAX, i64::MIN)), E_WINDOW);
    try_create(&mut env, 7, window(MIN_POSITION_DURATION, MAX_POSITION_DURATION)).unwrap();
    try_create(&mut env, 8, window(3_600, 3_600)).unwrap(); // min == max
    let a = env.agent_state();
    assert_eq!((a.terms.min_duration_secs, a.terms.max_duration_secs), (3_600, 3_600));
}

#[test]
fn identity_rules_and_assets_size_limits() {
    let mut env = Env::new();
    let t = || terms(3_000, 0, 0);

    env.switch_agent(1);
    assert_err(env.create_named("", "d", t()), E_NAME);
    assert_err(env.create_named("   ", "d", t()), E_NAME);
    assert_err(env.create_named(&"n".repeat(MAX_NAME_LEN + 1), "d", t()), E_NAME);
    assert_err(env.create_named("ok", &"d".repeat(MAX_DESCRIPTION_LEN + 1), t()), E_DESCRIPTION);
    env.create_named(&"n".repeat(MAX_NAME_LEN), &"d".repeat(MAX_DESCRIPTION_LEN), t()).unwrap();
    env.switch_agent(2);
    env.create_named("no description", "", t()).unwrap();

    let mut rules = t();
    rules.rules = "r".repeat(MAX_RULES_LEN + 1);
    assert_err(try_create(&mut env, 3, rules), E_RULES);
    let mut rules = t();
    rules.rules = String::new();
    assert_err(try_create(&mut env, 3, rules), E_RULES);
    let mut rules = t();
    rules.rules = "r".repeat(MAX_RULES_LEN);
    try_create(&mut env, 3, rules).unwrap();

    let mut assets = t();
    assets.allowed_assets = (0..MAX_ALLOWED_ASSETS + 1).map(|_| Pubkey::new_unique()).collect();
    assert_err(try_create(&mut env, 4, assets), E_ASSETS);
    let mut assets = t();
    let dup = Pubkey::new_unique();
    assets.allowed_assets = (0..MAX_ALLOWED_ASSETS - 1).map(|_| Pubkey::new_unique()).chain([dup]).collect();
    assets.allowed_assets[0] = dup; // duplicate at the ends
    assert_err(try_create(&mut env, 4, assets), E_ASSETS);
    let mut assets = t();
    assets.allowed_assets = (0..MAX_ALLOWED_ASSETS).map(|_| Pubkey::new_unique()).collect();
    try_create(&mut env, 4, assets).unwrap();
    assert_eq!(env.agent_state().terms.allowed_assets.len(), MAX_ALLOWED_ASSETS);
}

#[test]
fn failed_create_leaves_nothing_behind_and_ids_cannot_be_reused() {
    let mut env = Env::new();
    assert_err(env.create(terms(500, 0, 0)), E_RATIO);
    assert!(env.svm.get_account(&env.agent).map_or(true, |a| a.lamports == 0));
    assert_eq!(env.balance(&env.agent_vault), 0);

    let logs = {
        let ix = env.create_ix("Momentum Bot", "", terms(3_000, 0, 0));
        let op = env.op();
        env.send_logs(ix, &op).unwrap()
    };
    let ev: AgentCreated = one_event(&logs);
    assert_eq!((ev.agent, ev.operator, ev.agent_id), (env.agent, env.operator.pubkey(), env.agent_id));
    // The vault is funded to exactly its rent floor, holding no collateral.
    assert_eq!(env.balance(&env.agent_vault), env.rent_floor());
    env.check_invariants();

    // Same operator + id: the PDA already exists.
    assert!(env.create(terms(3_000, 0, 0)).is_err());
}

#[test]
fn update_validates_everything_create_does_and_keeps_old_terms_on_failure() {
    let mut env = Env::new();
    let original = terms(3_000, 1_000, 2_000);
    env.create(original.clone()).unwrap();
    let op = env.op();

    assert_err(env.update(terms(999, 0, 0), &op), E_RATIO);
    assert_err(env.update(terms(3_000, 1_501, 0), &op), E_FEE);
    assert_err(env.update(terms(3_000, 0, 5_001), &op), E_DRAWDOWN);
    let mut t = terms(3_000, 0, 0);
    t.max_duration_secs = MAX_POSITION_DURATION + 1;
    assert_err(env.update(t, &op), E_WINDOW);
    let mut t = terms(3_000, 0, 0);
    t.allowed_assets.clear();
    assert_err(env.update(t, &op), E_ASSETS);
    let mut t = terms(3_000, 0, 0);
    t.rules = "\n\t ".into();
    assert_err(env.update(t, &op), E_RULES);
    assert_err(env.update_named("", "", terms(3_000, 0, 0), &op), E_NAME);
    assert_err(env.update_named("x", &"d".repeat(MAX_DESCRIPTION_LEN + 1), terms(3_000, 0, 0), &op), E_DESCRIPTION);

    let a = env.agent_state();
    assert_eq!(a.terms, original);
    assert_eq!(a.name, "Momentum Bot");
    assert_eq!(a.description, "SOL/USDC momentum");

    // Updating repeatedly in draft is fine, and paused agents are still locked.
    env.update(terms(5_000, 2_500, 5_000), &op).unwrap();
    env.update(original.clone(), &op).unwrap();
    env.deposit(SOL).unwrap();
    env.publish().unwrap();
    env.set_accepting(false).unwrap();
    assert_err(env.update(original, &op), E_LOCKED);
}

// ---------- operator-only checks ----------

#[test]
fn every_operator_instruction_rejects_other_signers() {
    let mut env = Env::new();
    env.create(terms(3_000, 1_500, 2_000)).unwrap();
    let stranger = env.funded(10 * SOL);
    let trader = env.tr();
    let executor = env.executor.insecure_clone();
    let op = env.op();
    env.bind_executor(executor.pubkey(), &op).unwrap();

    // Draft-time instructions.
    for k in [&stranger, &trader, &executor] {
        assert_err(env.update(terms(5_000, 0, 0), k), E_OPERATOR);
        assert_err(env.deposit_as(SOL, k), E_OPERATOR);
        assert_err(env.publish_as(k), E_OPERATOR);
        assert_err(env.bind_executor(k.pubkey(), k), E_OPERATOR);
    }
    let a = env.agent_state();
    assert_eq!((a.status, a.total_collateral, a.executor), (AgentStatus::Draft, 0, executor.pubkey()));
    assert_eq!(a.terms, terms_without_assets(&a.terms, terms(3_000, 1_500, 2_000)));

    env.deposit(SOL).unwrap();
    for k in [&stranger, &trader, &executor] {
        assert_err(env.withdraw_as(1, k), E_OPERATOR);
    }
    env.publish().unwrap();
    for k in [&stranger, &trader, &executor] {
        assert_err(env.set_accepting_as(false, k), E_OPERATOR);
        assert_err(env.withdraw_as(1, k), E_OPERATOR);
        assert_err(env.bind_executor(k.pubkey(), k), E_OPERATOR);
    }
    let a = env.agent_state();
    assert_eq!(a.status, AgentStatus::Active);
    assert_eq!(a.total_collateral, SOL);
    assert_eq!(a.executor, executor.pubkey());
    assert_eq!(env.balance(&env.agent_vault), SOL + env.rent_floor());
}

/// `terms()` generates fresh random asset keys; compare everything else.
fn terms_without_assets(stored: &AgentTerms, mut expected: AgentTerms) -> AgentTerms {
    expected.allowed_assets = stored.allowed_assets.clone();
    expected
}

#[test]
fn a_stranger_cannot_act_on_an_agent_by_passing_themselves_as_operator_of_another_agent() {
    // The stranger owns their own agent; they still cannot touch ours.
    let mut env = Env::launched();
    let ours = (env.agent, env.agent_vault);
    let stranger = env.funded(10 * SOL);
    let ix = Instruction::new_with_bytes(
        env.program_id,
        &proof_of_agent::instruction::WithdrawCollateral { amount: SOL }.data(),
        proof_of_agent::accounts::WithdrawCollateral {
            operator: stranger.pubkey(),
            agent: ours.0,
            agent_vault: ours.1,
            system_program: system_program::ID,
        }
        .to_account_metas(None),
    );
    assert_err(env.send(ix, &stranger), E_OPERATOR);
    assert_eq!(env.agent_state().total_collateral, SOL);
}

// ---------- draft / unpublished ----------

#[test]
fn draft_agent_rejects_trading_but_allows_collateral_management() {
    let mut env = Env::new();
    env.create(terms(3_000, 1_500, 2_000)).unwrap();
    assert_err(env.open(0, SOL / 10, 3_600), E_NOT_ACCEPTING);
    assert_err(env.set_accepting(false), E_NOT_PUBLISHED);
    assert_err(env.set_accepting(true), E_NOT_PUBLISHED);

    // Collateral may move freely while drafting.
    env.deposit(2 * SOL).unwrap();
    env.withdraw(2 * SOL).unwrap();
    assert_eq!(env.agent_state().total_collateral, 0);
    assert_eq!(env.balance(&env.agent_vault), env.rent_floor());
    assert_err(env.publish(), E_NO_COLLATERAL);

    // Binding a key is allowed in draft too.
    let op = env.op();
    let k = env.executor.pubkey();
    env.bind_executor(k, &op).unwrap();

    env.deposit(1).unwrap();
    env.publish().unwrap();
    let a = env.agent_state();
    assert_eq!((a.status, a.total_collateral), (AgentStatus::Active, 1));
    // Draft terms were not modified by publishing.
    assert_eq!(a.terms.collateral_ratio_bps, 3_000);
}

#[test]
fn publish_revalidates_stored_terms() {
    let mut env = Env::new();
    env.create(terms(3_000, 1_500, 2_000)).unwrap();
    env.deposit(SOL).unwrap();
    // Corrupt the draft's stored terms as a legacy account might hold them.
    let mut a = env.agent_state();
    a.terms.collateral_ratio_bps = 500;
    env.write_agent(&a);
    assert_err(env.publish(), E_RATIO);
    a.terms.collateral_ratio_bps = 6_000;
    a.terms.max_drawdown_bps = 5_000;
    env.write_agent(&a);
    assert_err(env.publish(), E_RATIO_DRAWDOWN);
    assert_eq!(env.agent_state().status, AgentStatus::Draft);
}

#[test]
fn publish_emits_the_final_terms_and_collateral() {
    let mut env = Env::new();
    let t = terms(4_000, 2_000, 3_000);
    env.create(t.clone()).unwrap();
    env.deposit(3 * SOL).unwrap();
    let ix = Instruction::new_with_bytes(
        env.program_id,
        &proof_of_agent::instruction::PublishAgent {}.data(),
        proof_of_agent::accounts::PublishAgent { operator: env.operator.pubkey(), agent: env.agent }
            .to_account_metas(None),
    );
    let op = env.op();
    let logs = env.send_logs(ix, &op).unwrap();
    let ev: AgentPublished = one_event(&logs);
    assert_eq!(ev.agent, env.agent);
    assert_eq!(ev.operator, op.pubkey());
    assert_eq!(ev.terms, t);
    assert_eq!(ev.collateral, 3 * SOL);
    assert_eq!(env.agent_state().published_at, env.now());
}

#[test]
fn withdrawing_everything_after_publish_stops_new_positions_but_keeps_status() {
    let mut env = Env::launched();
    env.withdraw(SOL).unwrap();
    let a = env.agent_state();
    assert_eq!((a.status, a.total_collateral), (AgentStatus::Active, 0));
    assert_err(env.open(0, 1, 3_600), E_INSUFFICIENT);
    assert_err(env.publish(), E_ALREADY);
    env.check_invariants();
}

// ---------- paused ----------

#[test]
fn pausing_blocks_new_positions_but_not_existing_ones() {
    let mut env = Env::launched();
    let op = env.op();
    env.open(0, SOL, 3_600).unwrap();
    env.open(1, SOL, 3_600).unwrap();
    env.open(2, SOL / 2, 3_600).unwrap();
    env.draw(1, &op).unwrap();
    env.draw(2, &op).unwrap();

    env.set_accepting(false).unwrap();
    env.set_accepting(false).unwrap(); // idempotent
    assert_eq!(env.agent_state().status, AgentStatus::Paused);
    assert_err(env.open(3, 1, 3_600), E_NOT_ACCEPTING);
    assert_err(env.open(3, 0, 3_600), E_ZERO); // amount is checked first

    // Existing positions still run their course while paused.
    env.draw(0, &op).unwrap();
    env.settle(0, SOL, &op).unwrap();
    env.settle(1, SOL / 2, &op).unwrap();
    env.advance_time(3_600);
    // Past its deadline, the trader settles the position the agent left behind.
    let tr = env.tr();
    env.settle(2, SOL, &tr).unwrap();
    assert_eq!(env.position_state(2).breach, Breach::MissedDeadline);
    // The operator can still manage collateral while paused.
    env.deposit(SOL).unwrap();
    env.withdraw(SOL / 2).unwrap();
    env.check_invariants();

    let a = env.agent_state();
    assert_counters(&a, 0, 3, 0, 2);
    assert_eq!(a.locked_collateral, 0);
    assert_eq!(a.capital_managed, 0);

    env.set_accepting(true).unwrap();
    env.set_accepting(true).unwrap();
    assert_eq!(env.agent_state().status, AgentStatus::Active);
    env.open(3, SOL, 3_600).unwrap();
}

#[test]
fn a_paused_agent_can_still_be_cancelled_on() {
    let mut env = Env::launched();
    env.open(0, SOL, 3_600).unwrap();
    env.set_accepting(false).unwrap();
    env.cancel(0).unwrap();
    assert_eq!(env.agent_state().open_positions, 0);
}

// ---------- trading key ----------

#[test]
fn rotating_the_trading_key_revokes_the_old_one() {
    let mut env = Env::launched();
    let op = env.op();
    let k1 = env.executor.insecure_clone();
    let k2 = env.funded(10 * SOL);

    let bind = |env: &mut Env, key: Pubkey| {
        let ix = Instruction::new_with_bytes(
            env.program_id,
            &proof_of_agent::instruction::SetExecutor { executor: key }.data(),
            proof_of_agent::accounts::SetExecutor { operator: env.operator.pubkey(), agent: env.agent }
                .to_account_metas(None),
        );
        let op = env.op();
        let logs = env.send_logs(ix, &op).unwrap();
        let ev: ExecutorBound = one_event(&logs);
        assert_eq!((ev.agent, ev.executor), (env.agent, key));
    };

    bind(&mut env, k1.pubkey());
    env.open(0, SOL, 3_600).unwrap();
    env.open(1, SOL, 3_600).unwrap();
    env.draw(0, &k1).unwrap();

    bind(&mut env, k2.pubkey());
    assert_eq!(env.agent_state().executor, k2.pubkey());
    // The old key can neither draw a new position nor settle the one it drew.
    assert_err(env.draw(1, &k1), E_EXECUTOR);
    assert_err(env.settle(0, SOL, &k1), E_EXECUTOR);
    // The new key settles the position the old key drew. The principal is in
    // the vault, so it pays only the fee; the rent the old key put up goes back to it.
    let (before, k1_before) = (env.balance(&k2.pubkey()), env.balance(&k1.pubkey()));
    env.settle(0, SOL, &k2).unwrap();
    assert_eq!(before - env.balance(&k2.pubkey()), TX_FEE);
    assert_eq!(env.balance(&k1.pubkey()) - k1_before, env.custody_rents());
    env.draw(1, &k2).unwrap();

    // Rebinding to the operator's own key leaves the operator as sole executor.
    bind(&mut env, op.pubkey());
    assert_err(env.settle(1, SOL, &k2), E_EXECUTOR);
    env.settle(1, SOL, &op).unwrap();
    assert_counters(&env.agent_state(), 0, 2, 0, 0);
}

#[test]
fn the_trader_is_not_an_executor() {
    let mut env = Env::launched();
    env.open(0, SOL, 3_600).unwrap();
    let trader = env.tr();
    assert_err(env.draw(0, &trader), E_EXECUTOR);
    assert_err(env.settle(0, SOL, &trader), E_EXECUTOR);
}

// ---------- deposit / withdraw ----------

#[test]
fn deposit_and_withdraw_track_the_vault_and_emit_events() {
    let mut env = Env::new();
    env.create(terms(3_000, 1_500, 2_000)).unwrap();
    assert_err(env.deposit(0), E_ZERO);
    assert_err(env.withdraw(0), E_ZERO);
    assert_err(env.withdraw(1), E_INSUFFICIENT);

    let change = |env: &mut Env, amount: u64, deposit: bool| -> CollateralChanged {
        let ix = if deposit {
            Instruction::new_with_bytes(
                env.program_id,
                &proof_of_agent::instruction::DepositCollateral { amount }.data(),
                proof_of_agent::accounts::DepositCollateral {
                    operator: env.operator.pubkey(),
                    agent: env.agent,
                    agent_vault: env.agent_vault,
                    config: env.config,
                    system_program: system_program::ID,
                }
                .to_account_metas(None),
            )
        } else {
            Instruction::new_with_bytes(
                env.program_id,
                &proof_of_agent::instruction::WithdrawCollateral { amount }.data(),
                proof_of_agent::accounts::WithdrawCollateral {
                    operator: env.operator.pubkey(),
                    agent: env.agent,
                    agent_vault: env.agent_vault,
                    system_program: system_program::ID,
                }
                .to_account_metas(None),
            )
        };
        let op = env.op();
        one_event(&env.send_logs(ix, &op).unwrap())
    };

    let op_start = env.balance(&env.operator.pubkey());
    let ev = change(&mut env, SOL, true);
    assert_eq!((ev.agent, ev.delta, ev.total_collateral), (env.agent, SOL as i64, SOL));
    let ev = change(&mut env, 2 * SOL + 7, true);
    assert_eq!((ev.delta, ev.total_collateral), ((2 * SOL + 7) as i64, 3 * SOL + 7));
    let ev = change(&mut env, SOL, false);
    assert_eq!((ev.delta, ev.total_collateral), (-(SOL as i64), 2 * SOL + 7));
    assert_eq!(env.agent_state().total_collateral, 2 * SOL + 7);
    assert_eq!(env.balance(&env.agent_vault), 2 * SOL + 7 + env.rent_floor());
    assert_eq!(op_start - env.balance(&env.operator.pubkey()), 2 * SOL + 7 + 3 * TX_FEE);
    env.check_invariants();

    assert_err(env.withdraw(2 * SOL + 8), E_INSUFFICIENT);
    assert_err(env.withdraw(u64::MAX), E_INSUFFICIENT);
    // Depositing more than the operator owns fails without touching state.
    assert!(env.deposit(1_000 * SOL).is_err());
    assert_eq!(env.agent_state().total_collateral, 2 * SOL + 7);
}
