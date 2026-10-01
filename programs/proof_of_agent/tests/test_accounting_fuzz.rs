//! Randomized accounting tests across several agents, traders and seeds.
//!
//! A model predicts the outcome of every instruction (including which error a
//! refused one returns). After every step, successful or not:
//!
//! * each agent vault holds exactly its recorded collateral plus its rent floor;
//! * each open position's vault holds exactly its principal plus the rent floor,
//!   a drawn one only the rent floor, and a closed one nothing;
//! * each agent's locked collateral, managed capital and open count equal the
//!   sums over its open and trading positions;
//! * the SOL held by every wallet and program account together only goes down
//!   by the network fee. No instruction creates or loses a lamport.
//!
//! Settles are also checked against `compute_settlement`: the trader receives
//! the returned SOL minus the fee plus any slash, and the operator the fee.

mod common;

use {
    common::*,
    proof_of_agent::instructions::{compute_settlement, required_collateral},
};

const AGENT_IDS: [u64; 3] = [7, 8, 9];
/// (ratio, fee, drawdown) per agent: a mid, a fully backed and a thin bond.
const TERMS: [(u16, u16, u16); 3] = [(3_000, 1_500, 2_000), (10_000, 5_000, 0), (1_000, 500, 5_000)];
const MIN_DURATION: i64 = 60;
const MAX_DURATION: i64 = 7 * 24 * 3600;

#[derive(Clone, Copy, PartialEq, Debug)]
enum St {
    Open,
    Trading,
    Closed,
}

#[derive(Clone, Debug)]
struct Pos {
    agent: usize,
    trader: usize,
    nonce: u64,
    principal: u64,
    locked: u64,
    deadline: i64,
    st: St,
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct AgentModel {
    total: u64,
    locked: u64,
    capital: u64,
    open: u32,
    active: bool,
}

struct Walk {
    env: Env,
    rng: Rng,
    traders: Vec<Keypair>,
    agents: [AgentModel; 3],
    positions: Vec<Pos>,
    next_nonce: [[u64; 3]; 3],
    paused: bool,
    max_position: u64,
    max_agent: u64,
    /// Accounts whose lamports are summed for the conservation check.
    tracked: Vec<Pubkey>,
    /// Lamports the last step conjured or destroyed in the vault's wSOL account
    /// to stand in for the agent's trading result.
    minted: i128,
    /// How often each kind of step succeeded / was refused as predicted.
    ok: [u32; 10],
    refused: [u32; 10],
}

const OPEN: usize = 0;
const DRAW: usize = 1;
const SETTLE: usize = 2;
const DECLINE: usize = 3;
const CANCEL: usize = 4;
const CLAIM: usize = 5;
const DEPOSIT: usize = 6;
const WITHDRAW: usize = 7;
const ADMIN: usize = 8;
const ACCEPTING: usize = 9;

impl Walk {
    fn new(seed: u64) -> Self {
        let mut env = Env::new();
        let mut rng = Rng::new(seed);
        let op = env.op();
        let executor = env.executor.pubkey();
        let mut agents = [AgentModel::default(); 3];
        for (i, &id) in AGENT_IDS.iter().enumerate() {
            env.switch_agent(id);
            let (r, f, d) = TERMS[i];
            env.create(terms(r, f, d)).unwrap();
            let bond = rng.range(SOL, 5 * SOL);
            env.deposit(bond).unwrap();
            env.publish().unwrap();
            agents[i] = AgentModel { total: bond, active: true, ..Default::default() };
        }
        // Only the first agent has a separate trading key; the others trade with the operator key.
        env.switch_agent(AGENT_IDS[0]);
        env.bind_executor(executor, &op).unwrap();
        let traders = vec![env.tr(), env.funded(1_000 * SOL), env.funded(1_000 * SOL)];

        let mut tracked = vec![env.admin.pubkey(), env.operator.pubkey(), executor, env.config];
        tracked.extend(traders.iter().map(|t| t.pubkey()));
        for &id in &AGENT_IDS {
            env.switch_agent(id);
            tracked.push(env.agent);
            tracked.push(env.agent_vault);
        }
        Walk {
            env,
            rng,
            traders,
            agents,
            positions: Vec::new(),
            next_nonce: [[0; 3]; 3],
            paused: false,
            max_position: u64::MAX,
            max_agent: u64::MAX,
            tracked,
            minted: 0,
            ok: [0; 10],
            refused: [0; 10],
        }
    }

    fn total_lamports(&self) -> u64 {
        self.tracked.iter().map(|k| self.env.balance(k)).sum()
    }

    fn signer_for(&self, agent: usize) -> Keypair {
        if agent == 0 { self.env.executor.insecure_clone() } else { self.env.op() }
    }

    fn pick(&mut self, want: &[St]) -> Option<usize> {
        let idx: Vec<usize> = (0..self.positions.len()).filter(|&i| want.contains(&self.positions[i].st)).collect();
        if idx.is_empty() {
            return None;
        }
        Some(idx[self.rng.range(0, idx.len() as u64 - 1) as usize])
    }

    /// Runs one random step; returns whether a transaction was sent and whether it succeeded.
    fn step(&mut self) -> Option<bool> {
        match self.rng.range(0, 99) {
            0..=29 => Some(self.open()),
            30..=44 => self.draw(),
            45..=56 => self.settle(),
            57..=63 => self.cancel(),
            64..=69 => self.claim(),
            70..=75 => self.late_settle(),
            76..=81 => {
                self.env.advance_time(self.rng.range(1, 3_600) as i64);
                None
            }
            82..=89 => Some(self.collateral()),
            90..=95 => Some(self.admin()),
            _ => self.accepting().then_some(true),
        }
    }

    fn open(&mut self) -> bool {
        let a = self.rng.range(0, 2) as usize;
        let t = self.rng.range(0, 2) as usize;
        let (ratio, _, _) = TERMS[a];
        let m = self.agents[a];
        let free = m.total - m.locked;
        let backed = free * BPS_DENOMINATOR / ratio as u64;
        // Mostly ordinary sizes; sometimes exactly at, or one lamport past, a limit.
        let amount = match self.rng.range(0, 9) {
            0 => backed,
            1 => backed + 1,
            2 if self.max_position != u64::MAX => self.max_position + self.rng.range(0, 1),
            3 if self.max_agent != u64::MAX => self.max_agent.saturating_sub(m.capital) + self.rng.range(0, 1),
            4 => 0,
            _ => self.rng.range(1, 2 * SOL),
        };
        let duration = if self.rng.range(0, 19) == 0 { MIN_DURATION - 1 } else { self.rng.range(60, 3_600) as i64 };
        let locked = required_collateral(amount, ratio).unwrap();

        // The program checks in this order.
        let expected = if amount == 0 {
            Some(E_ZERO)
        } else if self.paused {
            Some(E_PAUSED)
        } else if amount > self.max_position {
            Some(E_TOO_LARGE)
        } else if !m.active {
            Some(E_NOT_ACCEPTING)
        } else if m.capital + amount > self.max_agent {
            Some(E_AGENT_CAP)
        } else if !(MIN_DURATION..=MAX_DURATION).contains(&duration) {
            Some(E_DURATION)
        } else if locked > free {
            Some(E_INSUFFICIENT)
        } else if amount.saturating_add(self.env.rent_floor()).saturating_add(TX_FEE) > self.env.balance(&self.traders[t].pubkey()) {
            // Every program check passed; the system transfer of the principal fails.
            Some(E_SYSTEM_INSUFFICIENT)
        } else {
            None
        };

        self.env.switch_agent(AGENT_IDS[a]);
        let nonce = self.next_nonce[a][t];
        let trader = self.traders[t].insecure_clone();
        let (position, vault) = self.env.position_pda_for(&trader.pubkey(), nonce);
        let res = self.env.open_by(&trader, nonce, amount, duration);
        match expected {
            Some(code) => {
                assert_err(res, code);
                self.refused[OPEN] += 1;
                false
            }
            None => {
                res.unwrap();
                self.next_nonce[a][t] += 1;
                self.tracked.push(position);
                self.tracked.push(vault);
                // Created at begin_trading, closed at settle; both hold lamports in between.
                self.tracked.push(self.env.custody_pda(&position));
                self.tracked.push(ata(&vault, &WSOL));
                let m = &mut self.agents[a];
                m.locked += locked;
                m.capital += amount;
                m.open += 1;
                self.positions.push(Pos {
                    agent: a,
                    trader: t,
                    nonce,
                    principal: amount,
                    locked,
                    deadline: self.env.now() + duration,
                    st: St::Open,
                });
                self.ok[OPEN] += 1;
                true
            }
        }
    }

    fn draw(&mut self) -> Option<bool> {
        let i = self.pick(&[St::Open])?;
        let p = self.positions[i].clone();
        self.env.switch_agent(AGENT_IDS[p.agent]);
        let signer = self.signer_for(p.agent);
        let tk = self.traders[p.trader].pubkey();
        let before = self.env.balance(&signer.pubkey());
        let res = self.env.draw_for(&tk, p.nonce, &signer);
        if self.paused {
            assert_err(res, E_PAUSED);
        } else if self.env.now() >= p.deadline {
            assert_err(res, E_DEADLINE_PASSED);
        } else {
            res.unwrap();
            // The principal is wrapped in the vault; the signer only put up the rent.
            assert_eq!(before - self.env.balance(&signer.pubkey()), TX_FEE + self.env.custody_rents());
            self.positions[i].st = St::Trading;
            self.ok[DRAW] += 1;
            return Some(true);
        }
        self.refused[DRAW] += 1;
        Some(false)
    }

    fn settle(&mut self) -> Option<bool> {
        let i = self.pick(&[St::Open, St::Trading])?;
        let p = self.positions[i].clone();
        let (_, fee_bps, dd_bps) = TERMS[p.agent];
        self.env.switch_agent(AGENT_IDS[p.agent]);
        let signer = self.signer_for(p.agent);
        let trader = self.traders[p.trader].pubkey();
        let operator = self.env.operator.pubkey();
        let declined = p.st == St::Open;
        let returned = self.rng.range(0, 2 * p.principal);
        let effective = if declined { p.principal } else { returned };
        let s = compute_settlement(p.principal, effective, fee_bps, dd_bps, p.locked).unwrap();

        let (trader_before, operator_before) = (self.env.balance(&trader), self.env.balance(&operator));
        // The harness sets the vault's wSOL from the principal to `returned`.
        if !declined {
            self.minted = returned as i128 - p.principal as i128;
        }
        self.env.settle_for(&trader, p.nonce, returned, &signer).unwrap();

        // Every wallet is well funded, so the fee is always paid out.
        let payout = effective - s.fee + s.slash + self.env.rent_floor();
        assert_eq!(self.env.balance(&trader), trader_before + payout, "trader payout");
        let operator_fee_paid = if signer.pubkey() == operator { TX_FEE } else { 0 };
        // The signer that drew the position paid its custody rent and gets it back now.
        let rent_refund = if declined || signer.pubkey() != operator { 0 } else { self.env.custody_rents() };
        let operator_after_expected = operator_before + s.fee - operator_fee_paid + rent_refund;
        assert_eq!(self.env.balance(&operator), operator_after_expected, "operator balance");

        let m = &mut self.agents[p.agent];
        m.total -= s.slash;
        m.locked -= p.locked;
        m.capital -= p.principal;
        m.open -= 1;
        self.positions[i].st = St::Closed;
        self.ok[if declined { DECLINE } else { SETTLE }] += 1;
        Some(true)
    }

    fn cancel(&mut self) -> Option<bool> {
        // Mostly open positions; sometimes one that can no longer be cancelled.
        let i = if self.rng.range(0, 2) == 0 {
            self.pick(&[St::Trading, St::Closed])?
        } else {
            self.pick(&[St::Open])?
        };
        let p = self.positions[i].clone();
        self.env.switch_agent(AGENT_IDS[p.agent]);
        let trader = self.traders[p.trader].insecure_clone();
        let res = self.env.cancel_by(&trader, p.nonce);
        if p.st != St::Open {
            assert_err(res, E_STATUS);
            self.refused[CANCEL] += 1;
            return Some(false);
        }
        res.unwrap();
        let m = &mut self.agents[p.agent];
        m.locked -= p.locked;
        m.capital -= p.principal;
        m.open -= 1;
        self.positions[i].st = St::Closed;
        self.ok[CANCEL] += 1;
        Some(true)
    }

    fn claim(&mut self) -> Option<bool> {
        // Mostly drawn positions; sometimes an undrawn one, which cannot default.
        let i = if self.rng.range(0, 4) == 0 { self.pick(&[St::Open])? } else { self.pick(&[St::Trading])? };
        let p = self.positions[i].clone();
        self.env.switch_agent(AGENT_IDS[p.agent]);
        let trader = self.traders[p.trader].insecure_clone();
        let before = self.env.balance(&trader.pubkey());
        let res = self.env.claim_default_by(&trader, p.nonce);
        if p.st == St::Open {
            assert_err(res, E_STATUS);
        } else {
            // Custody positions never default: the principal is in the vault.
            assert_err(res, E_USE_SETTLE);
            assert_eq!(self.env.balance(&trader.pubkey()) + TX_FEE, before);
        }
        self.refused[CLAIM] += 1;
        Some(false)
    }

    /// What replaces a default under custody: past the deadline, the trader
    /// settles the position from the vault. Modelled like a settle, with the
    /// trader as signer and the rent going back to whoever began trading.
    fn late_settle(&mut self) -> Option<bool> {
        let i = self.pick(&[St::Trading])?;
        let p = self.positions[i].clone();
        if self.env.now() < p.deadline {
            return None;
        }
        let (_, fee_bps, dd_bps) = TERMS[p.agent];
        self.env.switch_agent(AGENT_IDS[p.agent]);
        let trader = self.traders[p.trader].insecure_clone();
        let payer = self.signer_for(p.agent).pubkey();
        let operator = self.env.operator.pubkey();
        let returned = self.rng.range(0, 2 * p.principal);
        let s = compute_settlement(p.principal, returned, fee_bps, dd_bps, p.locked).unwrap();
        let (trader_before, payer_before, operator_before) =
            (self.env.balance(&trader.pubkey()), self.env.balance(&payer), self.env.balance(&operator));
        self.minted = returned as i128 - p.principal as i128;
        self.env.settle_for(&trader.pubkey(), p.nonce, returned, &trader).unwrap();

        let payout = returned - s.fee + s.slash + self.env.rent_floor();
        assert_eq!(self.env.balance(&trader.pubkey()) + TX_FEE, trader_before + payout, "trader payout");
        let rents = self.env.custody_rents();
        if payer == operator {
            assert_eq!(self.env.balance(&operator), operator_before + s.fee + rents, "operator fee and rent");
        } else {
            assert_eq!(self.env.balance(&payer), payer_before + rents, "rent refund");
            assert_eq!(self.env.balance(&operator), operator_before + s.fee, "operator fee");
        }
        let st = self.env.position_state_for(&trader.pubkey(), p.nonce);
        assert_eq!((st.status, st.breach), (PositionStatus::Settled, Breach::MissedDeadline));

        let m = &mut self.agents[p.agent];
        m.total -= s.slash;
        m.locked -= p.locked;
        m.capital -= p.principal;
        m.open -= 1;
        self.positions[i].st = St::Closed;
        self.ok[CLAIM] += 1;
        Some(true)
    }

    fn collateral(&mut self) -> bool {
        let a = self.rng.range(0, 2) as usize;
        self.env.switch_agent(AGENT_IDS[a]);
        let free = self.agents[a].total - self.agents[a].locked;
        if self.rng.range(0, 1) == 0 {
            let amount = self.rng.range(1, 2 * SOL);
            let res = self.env.deposit(amount);
            if self.paused {
                assert_err(res, E_PAUSED);
                self.refused[DEPOSIT] += 1;
                return false;
            }
            res.unwrap();
            self.agents[a].total += amount;
            self.ok[DEPOSIT] += 1;
            true
        } else {
            // Withdrawing works while paused; one lamport more than is free never does.
            let amount = if free == 0 || self.rng.range(0, 3) == 0 { free + 1 } else { self.rng.range(1, free) };
            let res = self.env.withdraw(amount);
            if amount > free {
                assert_err(res, E_INSUFFICIENT);
                self.refused[WITHDRAW] += 1;
                return false;
            }
            res.unwrap();
            self.agents[a].total -= amount;
            self.ok[WITHDRAW] += 1;
            true
        }
    }

    fn admin(&mut self) -> bool {
        // Pauses are short: a paused protocol always resumes on the next admin step.
        if self.paused || self.rng.range(0, 2) == 0 {
            self.paused = !self.paused;
            self.env.set_paused(self.paused).unwrap();
        } else {
            let cap = |rng: &mut Rng, lo: u64, hi: u64| if rng.range(0, 2) == 0 { u64::MAX } else { rng.range(lo, hi) };
            self.max_position = cap(&mut self.rng, SOL / 2, 3 * SOL);
            self.max_agent = cap(&mut self.rng, SOL, 8 * SOL);
            self.env.set_caps(self.max_position, self.max_agent).unwrap();
        }
        self.ok[ADMIN] += 1;
        true
    }

    fn accepting(&mut self) -> bool {
        // A stopped agent always resumes; a live one stops now and then.
        let a = self.rng.range(0, 2) as usize;
        if self.agents[a].active && self.rng.range(0, 1) == 0 {
            return false;
        }
        self.env.switch_agent(AGENT_IDS[a]);
        let accepting = !self.agents[a].active;
        self.env.set_accepting(accepting).unwrap();
        self.agents[a].active = accepting;
        self.ok[ACCEPTING] += 1;
        true
    }

    fn check(&mut self) {
        let floor = self.env.rent_floor();
        for (a, &id) in AGENT_IDS.iter().enumerate() {
            self.env.switch_agent(id);
            let s = self.env.agent_state();
            let m = self.agents[a];
            assert_eq!(
                (s.total_collateral, s.locked_collateral, s.capital_managed, s.open_positions),
                (m.total, m.locked, m.capital, m.open),
                "agent {id} counters"
            );
            assert_eq!(s.status == AgentStatus::Active, m.active);
            assert_eq!(self.env.balance(&self.env.agent_vault), s.total_collateral + floor, "agent {id} vault");
            let live = self.positions.iter().filter(|p| p.agent == a && p.st != St::Closed);
            let (locked, capital, open) = live.fold((0, 0, 0), |(l, c, n), p| (l + p.locked, c + p.principal, n + 1));
            assert_eq!((locked, capital, open), (s.locked_collateral, s.capital_managed, s.open_positions), "agent {id} sums");
        }
        for p in &self.positions {
            self.env.switch_agent(AGENT_IDS[p.agent]);
            let (_, vault) = self.env.position_pda_for(&self.traders[p.trader].pubkey(), p.nonce);
            // While trading, the principal is wrapped in the vault's wSOL account and
            // the rent floor is parked in the custody account; the vault itself is empty.
            let want = match p.st {
                St::Open => p.principal + floor,
                St::Trading | St::Closed => 0,
            };
            assert_eq!(self.env.balance(&vault), want, "position vault {p:?}");
            if p.st == St::Trading {
                let (position, _) = self.env.position_pda_for(&self.traders[p.trader].pubkey(), p.nonce);
                assert_eq!(self.env.vault_wsol_for(&self.traders[p.trader].pubkey(), p.nonce), p.principal, "vault wSOL {p:?}");
                assert_eq!(self.env.balance(&self.env.custody_pda(&position)), self.env.custody_rent() + floor, "custody {p:?}");
            }
            let s = self.env.position_state_for(&self.traders[p.trader].pubkey(), p.nonce);
            let st = match s.status {
                PositionStatus::Open => St::Open,
                PositionStatus::Trading => St::Trading,
                _ => St::Closed,
            };
            assert_eq!(st, p.st, "position status {p:?}");
        }
        let c = self.env.config_state();
        assert_eq!((c.paused, c.max_position, c.max_agent_capital), (self.paused, self.max_position, self.max_agent));
    }
}

fn walk(seed: u64, steps: usize) -> Walk {
    let mut w = Walk::new(seed);
    w.check();
    for _ in 0..steps {
        w.minted = 0;
        let before = w.total_lamports();
        let sent = w.step();
        let after = (w.total_lamports() as i128 - w.minted) as u64;
        match sent {
            // One signer per transaction: the network fee is the only SOL that leaves.
            Some(true) => assert_eq!(before - after, TX_FEE, "seed {seed}: SOL created or lost"),
            // A refused transaction moves nothing except, at most, its fee.
            Some(false) => assert!(before - after == 0 || before - after == TX_FEE, "seed {seed}: refused tx moved SOL"),
            None => assert_eq!(before, after),
        }
        w.check();
    }
    w
}

#[test]
fn random_walks_keep_every_lamport_accounted_for() {
    let mut ok = [0u32; 10];
    let mut refused = [0u32; 10];
    for seed in [1, 2, 3, 0xBEEF, 0x5EED, 0xC0FFEE, 42, 7_777] {
        let w = walk(seed, 400);
        for k in 0..10 {
            ok[k] += w.ok[k];
            refused[k] += w.refused[k];
        }
    }
    println!("succeeded {ok:?}\nrefused   {refused:?}");
    // Every path ran often enough to mean something.
    for (k, name) in ["open", "draw", "settle", "decline", "cancel", "claim", "deposit", "withdraw", "admin", "accepting"]
        .iter()
        .enumerate()
    {
        assert!(ok[k] >= 10, "{name} succeeded only {} times", ok[k]);
    }
    for (k, name) in [(OPEN, "open"), (DRAW, "draw"), (CANCEL, "cancel"), (CLAIM, "claim"), (DEPOSIT, "deposit"), (WITHDRAW, "withdraw")] {
        assert!(refused[k] >= 5, "{name} was refused only {} times", refused[k]);
    }
}
