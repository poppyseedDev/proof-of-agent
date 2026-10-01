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
    assert_eq!(env.balance(&vault), env.rent_floor());
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
