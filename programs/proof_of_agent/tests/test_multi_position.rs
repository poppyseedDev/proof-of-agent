//! Several positions from two traders on one agent, interleaving open, draw,
//! settle, decline, default and cancel. After every step the agent's counters
//! are compared to a model and the accounting invariants are checked.

mod common;

use {common::*, proof_of_agent::instructions::required_collateral};

/// What the agent account should say, maintained by the test.
#[derive(Default, Debug, Clone, PartialEq)]
struct Model {
    total: u64,
    locked: u64,
    capital: u64,
    open: u32,
    settled: u32,
    defaulted: u32,
    breaches: u32,
    slashed: u64,
    fees: u64,
}

impl Model {
    fn opened(&mut self, principal: u64, ratio: u16) {
        self.locked += required_collateral(principal, ratio).unwrap();
        self.capital += principal;
        self.open += 1;
    }

    /// Close a position that was not drawn (cancel or decline).
    fn released(&mut self, p: &Position) {
        self.locked -= p.locked_collateral;
        self.capital -= p.principal;
        self.open -= 1;
    }
}

fn check(env: &Env, m: &Model) {
    env.check_invariants();
    let a = env.agent_state();
    let got = Model {
        total: a.total_collateral,
        locked: a.locked_collateral,
        capital: a.capital_managed,
        open: a.open_positions,
        settled: a.settled_positions,
        defaulted: a.defaulted_positions,
        breaches: a.breach_count,
        slashed: a.slashed_total,
        fees: a.fees_earned,
    };
    assert_eq!(&got, m);
    // Nothing else moves collateral in or out, so the vault holds exactly
    // the recorded collateral plus its rent floor.
    assert_eq!(env.balance(&env.agent_vault), a.total_collateral + env.rent_floor());
}

#[test]
fn interleaved_positions_from_two_traders_keep_the_counters_consistent() {
    let mut env = Env::launched(); // 30% ratio, 15% fee, 20% tolerance, 1 SOL
    let ratio = 3_000;
    let op = env.op();
    let key = env.executor.insecure_clone();
    env.bind_executor(key.pubkey(), &op).unwrap();
    env.deposit(2 * SOL).unwrap();
    let a = env.tr();
    let b = env.funded(100 * SOL);
    let (ak, bk) = (a.pubkey(), b.pubkey());

    let mut m = Model { total: 3 * SOL, ..Model::default() };
    check(&env, &m);

    // 1. Both traders open; nonces overlap across traders without clashing.
    env.open_by(&a, 0, SOL, 3_600).unwrap();
    m.opened(SOL, ratio);
    check(&env, &m);
    env.open_by(&b, 0, 2 * SOL, 3_600).unwrap();
    m.opened(2 * SOL, ratio);
    check(&env, &m);
    env.open_by(&a, 1, SOL / 2, 3_600).unwrap();
    m.opened(SOL / 2, ratio);
    check(&env, &m);
    env.open_by(&b, 1, SOL, 60).unwrap();
    m.opened(SOL, ratio);
    check(&env, &m);
    assert_ne!(env.position_pda_for(&ak, 0), env.position_pda_for(&bk, 0));
    assert_eq!(m.locked, 1_350_000_000);

    // 2. Draw A0 and B0; drawing changes no agent counter.
    env.draw_for(&ak, 0, &key).unwrap();
    check(&env, &m);
    env.draw_for(&bk, 0, &key).unwrap();
    check(&env, &m);

    // 3. A cancels A1 (still open).
    let p = env.position_state_for(&ak, 1);
    env.cancel_by(&a, 1).unwrap();
    m.released(&p);
    check(&env, &m);

    // 4. A0 settles with a 10% profit: fee 15% of 0.1 SOL.
    env.settle_for(&ak, 0, 1_100_000_000, &key).unwrap();
    m.locked -= 300_000_000;
    m.capital -= SOL;
    m.open -= 1;
    m.settled += 1;
    m.fees += 15_000_000;
    check(&env, &m);

    // 5. B opens a large position with the freed collateral.
    env.open_by(&b, 2, 3 * SOL, 3_600).unwrap();
    m.opened(3 * SOL, ratio);
    check(&env, &m);
    // Free collateral is now 3 - (0.6 + 0.3 + 0.9) = 1.2 SOL.
    assert_eq!(env.agent_state().free_collateral(), 1_200_000_000);
    assert_err(env.open_by(&a, 2, 4_000_000_001, 3_600), E_INSUFFICIENT);
    check(&env, &m);

    // 6. B1 was drawn the legacy way and misses its 60 s deadline; B claims the default.
    env.legacy_draw_for(&bk, 1, &key.pubkey());
    env.advance_time(60);
    let b_before = env.balance(&bk);
    env.claim_default_by(&b, 1).unwrap();
    assert_eq!(env.balance(&bk) + TX_FEE - b_before, 300_000_000 + env.rent_floor());
    m.total -= 300_000_000;
    m.locked -= 300_000_000;
    m.capital -= SOL;
    m.open -= 1;
    m.defaulted += 1;
    m.breaches += 1;
    m.slashed += 300_000_000;
    check(&env, &m);

    // 7. B0 settles at half its principal: loss 1, tolerance 0.4, slash 0.6 (full lock).
    env.settle_for(&bk, 0, SOL, &key).unwrap();
    m.total -= 600_000_000;
    m.locked -= 600_000_000;
    m.capital -= 2 * SOL;
    m.open -= 1;
    m.settled += 1;
    m.breaches += 1;
    m.slashed += 600_000_000;
    check(&env, &m);

    // 8. The agent declines B2 (never drawn): no reputation change.
    let p = env.position_state_for(&bk, 2);
    env.settle_for(&bk, 2, 0, &key).unwrap();
    m.released(&p);
    check(&env, &m);

    // 9. A opens again, it is drawn, and settles inside tolerance late.
    env.open_by(&a, 2, SOL, 120).unwrap();
    m.opened(SOL, ratio);
    check(&env, &m);
    env.draw_for(&ak, 2, &key).unwrap();
    env.advance_time(120);
    env.settle_for(&ak, 2, 900_000_000, &key).unwrap();
    m.locked -= 300_000_000;
    m.capital -= SOL;
    m.open -= 1;
    m.settled += 1;
    m.breaches += 1; // late
    check(&env, &m);
    assert_eq!(env.position_state_for(&ak, 2).breach, Breach::MissedDeadline);

    // 10. Everything is closed: all remaining collateral is free and withdrawable.
    assert_eq!((m.open, m.locked, m.capital), (0, 0, 0));
    assert_eq!(m.total, 2_100_000_000);
    env.withdraw(m.total).unwrap();
    m.total = 0;
    check(&env, &m);
    assert_eq!(env.balance(&env.agent_vault), env.rent_floor());

    // Every position ended in the status the scenario drove it to.
    let status = |env: &Env, t: &Pubkey, n| env.position_state_for(t, n).status;
    assert_eq!(status(&env, &ak, 0), PositionStatus::Settled);
    assert_eq!(status(&env, &ak, 1), PositionStatus::Cancelled);
    assert_eq!(status(&env, &ak, 2), PositionStatus::Settled);
    assert_eq!(status(&env, &bk, 0), PositionStatus::Settled);
    assert_eq!(status(&env, &bk, 1), PositionStatus::Defaulted);
    assert_eq!(status(&env, &bk, 2), PositionStatus::Settled);
}

#[test]
fn a_trader_cannot_touch_another_traders_position() {
    let mut env = Env::launched();
    let op = env.op();
    let a = env.tr();
    let b = env.funded(10 * SOL);
    env.open_by(&a, 0, SOL, 60).unwrap();
    env.legacy_draw_for(&a.pubkey(), 0, &op.pubkey());
    env.advance_time(60);

    // B passes A's position with B as signer: the PDA seeds use B's key.
    let mut ix = env.claim_ix(&a.pubkey(), 0);
    ix.accounts[0].pubkey = b.pubkey();
    assert_err(env.send(ix, &b), E_SEEDS);
    let mut ix = env.cancel_ix(&a.pubkey(), 0);
    ix.accounts[0].pubkey = b.pubkey();
    assert_err(env.send(ix, &b), E_SEEDS);
    env.check_invariants();
    env.claim_default_by(&a, 0).unwrap();
}

/// A seeded random walk over many positions and actions. Every successful or
/// failed instruction must leave the invariants and the model intact.
#[test]
fn random_walk_over_positions_keeps_the_invariants() {
    let mut env = Env::new();
    let ratio = 2_500;
    env.create(terms(ratio, 1_000, 3_000)).unwrap();
    env.deposit(5 * SOL).unwrap();
    env.publish().unwrap();
    let op = env.op();
    let traders = [env.tr(), env.funded(100 * SOL), env.funded(100 * SOL)];
    let mut m = Model { total: 5 * SOL, ..Model::default() };
    // (trader index, nonce) of every position ever opened.
    let mut positions: Vec<(usize, u64)> = Vec::new();
    let mut next_nonce = [0u64; 3];
    let mut rng = Rng::new(0x5EED_1234);
    // How often each path ran: [draw, cancel, decline, settle, default, closed].
    let mut hits = [0u32; 6];

    for _ in 0..300 {
        match rng.range(0, 5) {
            0 | 1 => {
                let t = rng.range(0, 2) as usize;
                let amount = rng.range(1, 3 * SOL);
                let dur = rng.range(60, 3 * 3_600) as i64;
                let nonce = next_nonce[t];
                let need = required_collateral(amount, ratio).unwrap();
                let res = env.open_by(&traders[t], nonce, amount, dur);
                if need <= m.total - m.locked {
                    res.unwrap();
                    m.opened(amount, ratio);
                    positions.push((t, nonce));
                    next_nonce[t] += 1;
                } else {
                    assert_err(res, E_INSUFFICIENT);
                }
            }
            2..=4 if !positions.is_empty() => {
                // Mostly act on a live position; sometimes poke a closed one.
                let live: Vec<(usize, u64)> = positions
                    .iter()
                    .copied()
                    .filter(|&(t, n)| {
                        matches!(
                            env.position_state_for(&traders[t].pubkey(), n).status,
                            PositionStatus::Open | PositionStatus::Trading
                        )
                    })
                    .collect();
                let pool = if live.is_empty() || rng.range(0, 5) == 0 { &positions } else { &live };
                let (t, n) = pool[rng.range(0, pool.len() as u64 - 1) as usize];
                let tk = traders[t].pubkey();
                let p = env.position_state_for(&tk, n);
                let late = env.now() >= p.deadline;
                match (rng.range(0, 3), p.status) {
                    (0 | 1, PositionStatus::Open) if !late => {
                        env.draw_for(&tk, n, &op).unwrap();
                        hits[0] += 1;
                    }
                    (0 | 1, PositionStatus::Open) => {
                        assert_err(env.draw_for(&tk, n, &op), E_DEADLINE_PASSED)
                    }
                    (2, PositionStatus::Open) => {
                        env.cancel_by(&traders[t], n).unwrap();
                        m.released(&p);
                        hits[1] += 1;
                    }
                    (_, PositionStatus::Open) => {
                        // Decline: returned is ignored and no reputation changes.
                        env.settle_for(&tk, n, rng.next_u64(), &op).unwrap();
                        m.released(&p);
                        hits[2] += 1;
                    }
                    (0, PositionStatus::Trading) => {
                        let returned = rng.range(0, 2 * p.principal);
                        env.settle_for(&tk, n, returned, &op).unwrap();
                        let s = env.position_state_for(&tk, n);
                        assert_eq!(s.status, PositionStatus::Settled);
                        assert!(s.slashed <= p.locked_collateral);
                        m.total -= s.slashed;
                        m.locked -= p.locked_collateral;
                        m.capital -= p.principal;
                        m.open -= 1;
                        m.settled += 1;
                        m.slashed += s.slashed;
                        m.fees += s.fee_paid;
                        if s.breach != Breach::None {
                            m.breaches += 1;
                        }
                        assert_eq!(s.breach == Breach::MissedDeadline, late);
                        hits[3] += 1;
                    }
                    (1, PositionStatus::Trading) if late => {
                        // A custody position cannot default; the trader settles it late instead.
                        assert_err(env.claim_default_by(&traders[t], n), E_USE_SETTLE);
                        let returned = rng.range(0, 2 * p.principal);
                        env.settle_for(&tk, n, returned, &traders[t]).unwrap();
                        let s = env.position_state_for(&tk, n);
                        assert_eq!((s.status, s.breach), (PositionStatus::Settled, Breach::MissedDeadline));
                        assert!(s.slashed <= p.locked_collateral);
                        m.total -= s.slashed;
                        m.locked -= p.locked_collateral;
                        m.capital -= p.principal;
                        m.open -= 1;
                        m.settled += 1;
                        m.breaches += 1;
                        m.slashed += s.slashed;
                        m.fees += s.fee_paid;
                        hits[4] += 1;
                    }
                    (1, PositionStatus::Trading) => {
                        assert_err(env.claim_default_by(&traders[t], n), E_USE_SETTLE);
                        assert_err(env.cancel_by(&traders[t], n), E_STATUS);
                    }
                    (_, PositionStatus::Trading) => env.advance_time(rng.range(1, 3_600) as i64),
                    _ => {
                        // Closed positions reject every state change.
                        assert_err(env.cancel_by(&traders[t], n), E_STATUS);
                        assert_err(env.draw_for(&tk, n, &op), E_STATUS);
                        assert_err(env.settle_for(&tk, n, 0, &op), E_STATUS);
                        assert_err(env.claim_default_by(&traders[t], n), E_STATUS);
                        hits[5] += 1;
                    }
                }
            }
            _ => {
                // Try to withdraw one lamport more than is free, then some of it.
                let free = m.total - m.locked;
                assert_err(env.withdraw(free + 1), E_INSUFFICIENT);
                if free > 0 && rng.range(0, 3) == 0 {
                    let amt = rng.range(1, free.min(SOL / 2));
                    env.withdraw(amt).unwrap();
                    m.total -= amt;
                } else if rng.range(0, 1) == 0 {
                    env.deposit(SOL).unwrap();
                    m.total += SOL;
                }
            }
        }
        check(&env, &m);
    }
    assert!(positions.len() > 10, "walk opened too few positions: {}", positions.len());
    assert!(hits.iter().all(|&h| h >= 3), "some path barely ran: {hits:?}");
}
