//! Decode PositionOpened, PositionClosed and BreachRecorded from the logs and
//! check every field.

mod common;

use {
    common::*,
    proof_of_agent::events::{BreachRecorded, PositionClosed, PositionDeclined, PositionOpened},
};

impl Env {
    fn open_logs(&mut self, nonce: u64, amount: u64, duration: i64) -> Vec<String> {
        let ix = self.open_ix(&self.trader.pubkey(), nonce, amount, duration);
        let t = self.tr();
        self.send_logs(ix, &t).unwrap()
    }

    fn settle_logs(&mut self, nonce: u64, returned: u64) -> Vec<String> {
        let op = self.op();
        let accounts = self.settle_accounts(nonce, &op);
        self.settle_with(accounts, returned, &op).unwrap()
    }

    fn cancel_logs(&mut self, nonce: u64) -> Vec<String> {
        let ix = self.cancel_ix(&self.trader.pubkey(), nonce);
        let t = self.tr();
        self.send_logs(ix, &t).unwrap()
    }

    fn claim_logs(&mut self, nonce: u64) -> Vec<String> {
        let ix = self.claim_ix(&self.trader.pubkey(), nonce);
        let t = self.tr();
        self.send_logs(ix, &t).unwrap()
    }
}

#[test]
fn event_decoder_round_trips() {
    for n in 0..40u8 {
        let bytes: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37).wrapping_add(n)).collect();
        assert_eq!(base64_decode(&base64(&bytes)).unwrap(), bytes);
    }
}

#[test]
fn position_opened_carries_the_lock_and_deadline() {
    let mut env = Env::launched();
    let t0 = env.now();
    let logs = env.open_logs(3, 1_000_000_001, 7_200);
    let ev: PositionOpened = one_event(&logs);
    let (position, _) = env.position_pda(3);
    assert_eq!(ev.position, position);
    assert_eq!(ev.agent, env.agent);
    assert_eq!(ev.trader, env.trader.pubkey());
    assert_eq!(ev.principal, 1_000_000_001);
    assert_eq!(ev.locked_collateral, 300_000_001);
    assert_eq!(ev.deadline, t0 + 7_200);
    // It matches the stored position.
    let p = env.position_state(3);
    assert_eq!((ev.locked_collateral, ev.deadline), (p.locked_collateral, p.deadline));
    assert!(events::<PositionClosed>(&logs).is_empty());
}

#[test]
fn profitable_settle_emits_closed_without_a_breach() {
    let mut env = Env::launched();
    let op = env.op();
    env.open(0, SOL, 3_600).unwrap();
    env.draw(0, &op).unwrap();
    let logs = env.settle_logs(0, 1_200_000_000);
    let ev: PositionClosed = one_event(&logs);
    assert_eq!(ev.position, env.position_pda(0).0);
    assert_eq!(ev.agent, env.agent);
    assert_eq!(ev.trader, env.trader.pubkey());
    assert_eq!(ev.status, PositionStatus::Settled);
    assert_eq!(ev.breach, Breach::None);
    assert_eq!(ev.returned, 1_200_000_000);
    assert_eq!(ev.slashed, 0);
    assert_eq!(ev.fee_paid, 30_000_000);
    assert_eq!(ev.trader_payout, 1_170_000_000);
    assert!(events::<BreachRecorded>(&logs).is_empty());
    assert!(events::<PositionDeclined>(&logs).is_empty());
}

#[test]
fn drawdown_settle_emits_breach_then_closed() {
    let mut env = Env::launched();
    let op = env.op();
    env.open(0, SOL, 3_600).unwrap();
    env.draw(0, &op).unwrap();
    let before = env.balance(&env.trader.pubkey());
    let logs = env.settle_logs(0, 700_000_000); // floor 0.8, slash 0.1
    let (position, _) = env.position_pda(0);

    let breach: BreachRecorded = one_event(&logs);
    assert_eq!(breach.agent, env.agent);
    assert_eq!(breach.position, position);
    assert_eq!(breach.breach, Breach::Drawdown);
    assert_eq!(breach.slashed, 100_000_000);

    let ev: PositionClosed = one_event(&logs);
    assert_eq!((ev.status, ev.breach), (PositionStatus::Settled, Breach::Drawdown));
    assert_eq!((ev.returned, ev.slashed, ev.fee_paid), (700_000_000, 100_000_000, 0));
    assert_eq!(ev.trader_payout, 800_000_000);
    // trader_payout excludes the vault rent floor the trader also receives.
    assert_eq!(env.balance(&env.trader.pubkey()) - before, ev.trader_payout + env.rent_floor());

    // BreachRecorded is logged before PositionClosed.
    let idx = |d: &[u8]| {
        let prefix = format!("Program data: {}", &base64(d)[..8]);
        logs.iter().position(|l| l.starts_with(&prefix)).unwrap()
    };
    assert!(idx(BreachRecorded::DISCRIMINATOR) < idx(PositionClosed::DISCRIMINATOR));
}

#[test]
fn late_settle_emits_a_missed_deadline_breach_even_without_a_slash() {
    let mut env = Env::launched();
    let op = env.op();
    env.open(0, SOL, 3_600).unwrap();
    env.draw(0, &op).unwrap();
    env.advance_time(3_600);
    let logs = env.settle_logs(0, SOL);
    let breach: BreachRecorded = one_event(&logs);
    assert_eq!((breach.breach, breach.slashed), (Breach::MissedDeadline, 0));
    let ev: PositionClosed = one_event(&logs);
    assert_eq!((ev.status, ev.breach), (PositionStatus::Settled, Breach::MissedDeadline));
    assert_eq!((ev.returned, ev.trader_payout), (SOL, SOL));
}

#[test]
fn claim_default_emits_breach_and_closed_with_the_bond_as_payout() {
    // A position drawn by the previous program's draw_funds: no custody.
    let mut env = Env::launched();
    let op = env.op();
    env.open(0, SOL, 60).unwrap();
    env.legacy_draw(0, &op.pubkey());
    env.advance_time(60);
    let logs = env.claim_logs(0);
    let (position, _) = env.position_pda(0);

    let breach: BreachRecorded = one_event(&logs);
    assert_eq!(
        (breach.agent, breach.position, breach.breach, breach.slashed),
        (env.agent, position, Breach::MissedDeadline, 300_000_000)
    );
    let ev: PositionClosed = one_event(&logs);
    assert_eq!(ev.position, position);
    assert_eq!(ev.trader, env.trader.pubkey());
    assert_eq!((ev.status, ev.breach), (PositionStatus::Defaulted, Breach::MissedDeadline));
    assert_eq!((ev.returned, ev.slashed, ev.fee_paid, ev.trader_payout), (0, 300_000_000, 0, 300_000_000));
}

#[test]
fn cancel_and_decline_emit_closed_with_the_principal_and_no_breach() {
    let mut env = Env::launched();
    env.open(0, SOL / 2, 3_600).unwrap();
    let logs = env.cancel_logs(0);
    let ev: PositionClosed = one_event(&logs);
    assert_eq!(ev.position, env.position_pda(0).0);
    assert_eq!((ev.status, ev.breach), (PositionStatus::Cancelled, Breach::None));
    assert_eq!((ev.returned, ev.slashed, ev.fee_paid, ev.trader_payout), (SOL / 2, 0, 0, SOL / 2));
    assert!(events::<BreachRecorded>(&logs).is_empty());

    env.open(1, SOL / 2, 3_600).unwrap();
    let logs = env.settle_logs(1, 123);
    let ev: PositionClosed = one_event(&logs);
    assert_eq!((ev.status, ev.breach), (PositionStatus::Settled, Breach::None));
    assert_eq!((ev.returned, ev.slashed, ev.fee_paid, ev.trader_payout), (SOL / 2, 0, 0, SOL / 2));
    let declined: PositionDeclined = one_event(&logs);
    assert_eq!(declined.principal, SOL / 2);
    assert!(events::<BreachRecorded>(&logs).is_empty());
}

#[test]
fn full_bond_slash_event_matches_state_with_odd_principal() {
    let mut env = Env::new();
    env.create(terms(5_000, 2_500, 5_000)).unwrap();
    env.deposit(2 * SOL).unwrap();
    env.publish().unwrap();
    let op = env.op();
    let logs = env.open_logs(0, 1_000_000_001, 3_600);
    assert_eq!(one_event::<PositionOpened>(&logs).locked_collateral, 500_000_001);
    env.draw(0, &op).unwrap();
    let logs = env.settle_logs(0, 0);
    let p = env.position_state(0);
    let breach: BreachRecorded = one_event(&logs);
    let ev: PositionClosed = one_event(&logs);
    assert_eq!(breach.slashed, p.locked_collateral);
    assert_eq!(ev.slashed, p.slashed);
    assert_eq!(ev.trader_payout, 500_000_001);
    assert_eq!((ev.returned, ev.fee_paid), (p.returned, p.fee_paid));
}
