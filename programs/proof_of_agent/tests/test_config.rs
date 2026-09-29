mod common;

use {common::*, proof_of_agent::events::ConfigChanged};

/// Rewrite the ProgramData authority, as `solana program set-upgrade-authority` would.
fn set_upgrade_authority(env: &mut Env, authority: Option<Pubkey>) {
    let mut pd = env.svm.get_account(&env.program_data).unwrap();
    match authority {
        Some(k) => {
            pd.data[12] = 1;
            pd.data[13..45].copy_from_slice(k.as_ref());
        }
        None => {
            pd.data[12] = 0;
            pd.data[13..45].fill(0);
        }
    }
    env.svm.set_account(env.program_data, pd).unwrap();
}

// ---------- who is admin ----------

#[test]
fn only_the_upgrade_authority_can_create_the_config_once() {
    let mut env = Env::without_config();
    let stranger = env.funded(SOL);
    assert_err(env.init_config_as(SOL, 10 * SOL, &stranger), E_ADMIN);
    let admin = env.admin.insecure_clone();
    assert_err(env.init_config_as(0, 10 * SOL, &admin), E_CAPS);
    env.init_config_as(SOL, 10 * SOL, &admin).unwrap();
    let c = env.config_state();
    assert!(!c.paused);
    assert_eq!((c.max_position, c.max_agent_capital), (SOL, 10 * SOL));
    // The config PDA exists now; a second init cannot overwrite it.
    assert!(env.init_config_as(u64::MAX, u64::MAX, &admin).is_err());
    assert_eq!(env.config_state().max_position, SOL);
}

#[test]
fn money_cannot_enter_before_the_config_exists() {
    let mut env = Env::without_config();
    env.create(terms(3_000, 1_500, 2_000)).unwrap();
    assert_err(env.deposit(SOL), E_NOT_INITIALIZED);
}

#[test]
fn only_the_upgrade_authority_can_pause_or_set_caps() {
    let mut env = Env::new();
    let stranger = env.funded(SOL);
    assert_err(env.set_paused_as(true, &stranger).map(|_| ()), E_ADMIN);
    assert_err(env.set_caps_as(1, 1, &stranger), E_ADMIN);
    assert_err(env.set_caps(0, SOL), E_CAPS);
    assert_err(env.set_caps(SOL, 0), E_CAPS);
    assert!(!env.config_state().paused);
}

#[test]
fn moving_the_upgrade_authority_moves_the_admin() {
    let mut env = Env::new();
    let multisig = env.funded(SOL);
    set_upgrade_authority(&mut env, Some(multisig.pubkey()));
    let old = env.admin.insecure_clone();
    assert_err(env.set_paused_as(true, &old).map(|_| ()), E_ADMIN);
    env.set_paused_as(true, &multisig).unwrap();
    assert!(env.config_state().paused);
}

#[test]
fn an_immutable_program_has_no_admin() {
    let mut env = Env::new();
    set_upgrade_authority(&mut env, None);
    let admin = env.admin.insecure_clone();
    assert_err(env.set_paused_as(true, &admin).map(|_| ()), E_ADMIN);
    assert_err(env.set_caps_as(SOL, SOL, &admin), E_ADMIN);
}

#[test]
fn config_changes_are_announced() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let logs = env.set_paused_as(true, &admin).unwrap();
    assert!(has_event(
        &logs,
        &ConfigChanged { paused: true, max_position: u64::MAX, max_agent_capital: u64::MAX }
    ));
}

// ---------- pause ----------

#[test]
fn pause_stops_new_positions_draws_and_deposits() {
    let mut env = Env::launched();
    let op = env.op();
    env.open(0, SOL, 3_600).unwrap();
    env.set_paused(true).unwrap();

    assert_err(env.open(1, SOL / 10, 3_600), E_PAUSED);
    assert_err(env.draw(0, &op), E_PAUSED);
    assert_err(env.deposit(SOL), E_PAUSED);

    env.set_paused(false).unwrap();
    env.draw(0, &op).unwrap();
    env.open(1, SOL / 10, 3_600).unwrap();
    env.deposit(SOL).unwrap();
    env.check_invariants();
}

#[test]
fn pause_never_stops_money_going_back() {
    let mut env = Env::launched();
    let op = env.op();
    for nonce in 0..4 {
        env.open(nonce, SOL / 2, 3_600).unwrap();
    }
    env.draw(0, &op).unwrap();
    env.draw(1, &op).unwrap();
    env.set_paused(true).unwrap();

    // The trader cancels an undrawn position; the agent declines another.
    env.cancel(2).unwrap();
    env.settle(3, 0, &op).unwrap();
    assert_eq!(env.position_state(3).status, PositionStatus::Settled);
    // The agent settles a drawn position.
    env.settle(0, SOL / 2, &op).unwrap();
    // A drawn position left past its deadline can still be claimed.
    env.advance_time(3_601);
    env.claim_default(1).unwrap();
    assert_eq!(env.position_state(1).status, PositionStatus::Defaulted);
    // The operator withdraws what is left.
    let free = env.agent_state().free_collateral();
    env.withdraw(free).unwrap();
    assert_eq!(env.agent_state().total_collateral, 0);
    env.check_invariants();
}

// ---------- caps ----------

#[test]
fn a_position_cannot_exceed_the_position_cap() {
    let mut env = Env::launched();
    env.set_caps(SOL / 2, u64::MAX).unwrap();
    assert_err(env.open(0, SOL / 2 + 1, 3_600), E_TOO_LARGE);
    env.open(0, SOL / 2, 3_600).unwrap();
}

#[test]
fn an_agent_cannot_manage_more_than_the_agent_cap() {
    let mut env = Env::launched();
    let op = env.op();
    env.deposit(9 * SOL).unwrap(); // 10 SOL bond at 30% backs 33 SOL, so the cap binds first
    env.set_caps(u64::MAX, 2 * SOL).unwrap();

    env.open(0, SOL, 3_600).unwrap();
    env.open(1, SOL, 3_600).unwrap();
    assert_err(env.open(2, 1, 3_600), E_AGENT_CAP);

    // Closing a position frees room under the cap, whichever way it ends.
    env.cancel(0).unwrap();
    env.open(2, SOL, 3_600).unwrap();
    env.draw(1, &op).unwrap();
    env.settle(1, SOL, &op).unwrap();
    env.open(3, SOL, 3_600).unwrap();
    assert_eq!(env.agent_state().capital_managed, 2 * SOL);
    env.check_invariants();
}

#[test]
fn the_agent_cap_applies_to_each_agent_separately() {
    let mut env = Env::launched();
    env.set_caps(u64::MAX, SOL).unwrap();
    env.open(0, SOL, 3_600).unwrap();
    assert_err(env.open(1, 1, 3_600), E_AGENT_CAP);

    env.switch_agent(8);
    env.create(terms(3_000, 1_500, 2_000)).unwrap();
    env.deposit(SOL).unwrap();
    env.publish().unwrap();
    env.open(0, SOL, 3_600).unwrap();
}

#[test]
fn lowering_caps_leaves_open_positions_alone() {
    let mut env = Env::launched();
    let op = env.op();
    env.open(0, 2 * SOL, 3_600).unwrap();
    env.set_caps(SOL / 10, SOL / 10).unwrap();
    assert_err(env.open(1, SOL / 10, 3_600), E_AGENT_CAP);
    env.draw(0, &op).unwrap();
    env.settle(0, 2 * SOL, &op).unwrap();
    assert_eq!(env.position_state(0).status, PositionStatus::Settled);
    env.open(1, SOL / 10, 3_600).unwrap();
}
