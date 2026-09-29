//! Shared LiteSVM harness for the proof_of_agent integration tests.
//!
//! Each file under `tests/` is its own crate and uses a different subset of
//! these helpers, hence the `dead_code` allowance.
#![allow(dead_code, unused_imports)]

pub use {
    anchor_lang::{
        prelude::{Clock, Pubkey, Rent},
        solana_program::{instruction::Instruction, system_program},
        AccountDeserialize, AccountSerialize, Discriminator, Event, InstructionData, ToAccountMetas,
    },
    litesvm::LiteSVM,
    proof_of_agent::{
        constants::*,
        state::{Agent, AgentStatus, AgentTerms, Breach, Config, Position, PositionStatus},
    },
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
};

pub const SOL: u64 = 1_000_000_000;
pub const TX_FEE: u64 = 5_000;

pub fn terms(ratio: u16, fee: u16, drawdown: u16) -> AgentTerms {
    AgentTerms {
        collateral_ratio_bps: ratio,
        fee_bps: fee,
        max_drawdown_bps: drawdown,
        min_duration_secs: 60,
        max_duration_secs: 7 * 24 * 3600,
        allowed_assets: vec![Pubkey::new_unique(), Pubkey::new_unique()],
        rules: "Trade SOL/USDC only. Max 50% of a position in USDC.".into(),
    }
}

pub struct Env {
    pub svm: LiteSVM,
    pub program_id: Pubkey,
    /// The program's upgrade authority, and so the protocol admin.
    pub admin: Keypair,
    pub config: Pubkey,
    pub program_data: Pubkey,
    pub operator: Keypair,
    pub executor: Keypair,
    pub trader: Keypair,
    pub agent: Pubkey,
    pub agent_vault: Pubkey,
    pub agent_id: u64,
}

impl Env {
    /// A fresh program with its config initialised, no caps in effect.
    pub fn new() -> Self {
        let mut env = Self::without_config();
        let admin = env.admin.insecure_clone();
        env.init_config_as(u64::MAX, u64::MAX, &admin).unwrap();
        env
    }

    /// A fresh program whose admin has not created the config yet.
    pub fn without_config() -> Self {
        let program_id = proof_of_agent::id();
        let mut svm = LiteSVM::new();
        let bytes = include_bytes!(concat!(env!("CARGO_TARGET_TMPDIR"), "/../deploy/proof_of_agent.so"));
        svm.add_program(program_id, bytes).unwrap();
        // The simulated clock starts at 0; use a realistic date.
        let mut clock: Clock = svm.get_sysvar();
        clock.unix_timestamp = 1_789_000_000;
        svm.set_sysvar(&clock);
        // LiteSVM loads the program with no upgrade authority; make `admin` the authority.
        let admin = Keypair::new();
        let upgradeable = anchor_lang::solana_program::bpf_loader_upgradeable::ID;
        let program_data = Pubkey::find_program_address(&[program_id.as_ref()], &upgradeable).0;
        let mut pd = svm.get_account(&program_data).unwrap();
        // ProgramData metadata: u32 variant, u64 slot, Option<Pubkey> authority.
        pd.data[12] = 1;
        pd.data[13..45].copy_from_slice(admin.pubkey().as_ref());
        svm.set_account(program_data, pd).unwrap();
        let config = Pubkey::find_program_address(&[CONFIG_SEED], &program_id).0;
        let operator = Keypair::new();
        let executor = Keypair::new();
        let trader = Keypair::new();
        for k in [&admin, &operator, &executor, &trader] {
            svm.airdrop(&k.pubkey(), 100 * SOL).unwrap();
        }
        let agent_id = 7u64;
        let agent = Pubkey::find_program_address(
            &[AGENT_SEED, operator.pubkey().as_ref(), &agent_id.to_le_bytes()],
            &program_id,
        )
        .0;
        let agent_vault = Pubkey::find_program_address(&[AGENT_VAULT_SEED, agent.as_ref()], &program_id).0;
        Self { svm, program_id, admin, config, program_data, operator, executor, trader, agent, agent_vault, agent_id }
    }

    // ---- protocol admin ----

    pub fn init_config_as(&mut self, max_position: u64, max_agent_capital: u64, signer: &Keypair) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::InitConfig { max_position, max_agent_capital }.data(),
            proof_of_agent::accounts::InitConfig {
                admin: signer.pubkey(),
                config: self.config,
                program_data: self.program_data,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        );
        self.send(ix, signer)
    }

    fn set_config_accounts(&self, signer: &Keypair) -> Vec<anchor_lang::prelude::AccountMeta> {
        proof_of_agent::accounts::SetConfig {
            admin: signer.pubkey(),
            config: self.config,
            program_data: self.program_data,
        }
        .to_account_metas(None)
    }

    pub fn set_paused_as(&mut self, paused: bool, signer: &Keypair) -> Result<Vec<String>, String> {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::SetPaused { paused }.data(),
            self.set_config_accounts(signer),
        );
        self.send_logs(ix, signer)
    }

    pub fn set_paused(&mut self, paused: bool) -> Result<(), String> {
        let admin = self.admin.insecure_clone();
        self.set_paused_as(paused, &admin).map(|_| ())
    }

    pub fn set_caps_as(&mut self, max_position: u64, max_agent_capital: u64, signer: &Keypair) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::SetCaps { max_position, max_agent_capital }.data(),
            self.set_config_accounts(signer),
        );
        self.send(ix, signer)
    }

    pub fn set_caps(&mut self, max_position: u64, max_agent_capital: u64) -> Result<(), String> {
        let admin = self.admin.insecure_clone();
        self.set_caps_as(max_position, max_agent_capital, &admin)
    }

    pub fn config_state(&self) -> Config {
        let acc = self.svm.get_account(&self.config).unwrap();
        Config::try_deserialize(&mut &acc.data[..]).unwrap()
    }

    /// A funded keypair that is neither operator, executor nor the default trader.
    pub fn funded(&mut self, lamports: u64) -> Keypair {
        let k = Keypair::new();
        self.svm.airdrop(&k.pubkey(), lamports).unwrap();
        k
    }

    pub fn send(&mut self, ix: Instruction, signer: &Keypair) -> Result<(), String> {
        self.send_logs(ix, signer).map(|_| ())
    }

    /// Like `send`, but returns the program logs on success.
    pub fn send_logs(&mut self, ix: Instruction, signer: &Keypair) -> Result<Vec<String>, String> {
        self.svm.expire_blockhash();
        let blockhash = self.svm.latest_blockhash();
        let msg = Message::new_with_blockhash(&[ix], Some(&signer.pubkey()), &blockhash);
        let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[signer]).unwrap();
        self.svm.send_transaction(tx).map(|m| m.logs).map_err(|e| format!("{:?}", e.err))
    }

    /// Point the helpers at another agent owned by the same operator.
    pub fn switch_agent(&mut self, agent_id: u64) {
        self.agent_id = agent_id;
        self.agent = Pubkey::find_program_address(
            &[AGENT_SEED, self.operator.pubkey().as_ref(), &agent_id.to_le_bytes()],
            &self.program_id,
        )
        .0;
        self.agent_vault =
            Pubkey::find_program_address(&[AGENT_VAULT_SEED, self.agent.as_ref()], &self.program_id).0;
    }

    pub fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    pub fn set_time(&mut self, t: i64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp = t;
        self.svm.set_sysvar(&clock);
    }

    pub fn advance_time(&mut self, secs: i64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp += secs;
        self.svm.set_sysvar(&clock);
    }

    pub fn balance(&self, key: &Pubkey) -> u64 {
        self.svm.get_balance(key).unwrap_or(0)
    }

    pub fn rent_floor(&self) -> u64 {
        self.svm.get_sysvar::<Rent>().minimum_balance(0)
    }

    pub fn agent_state(&self) -> Agent {
        let acc = self.svm.get_account(&self.agent).unwrap();
        Agent::try_deserialize(&mut &acc.data[..]).unwrap()
    }

    /// Overwrite the stored agent account (same size, same field order).
    pub fn write_agent(&mut self, a: &Agent) {
        let mut acc = self.svm.get_account(&self.agent).unwrap();
        let mut data = Vec::new();
        a.try_serialize(&mut data).unwrap();
        acc.data[..data.len()].copy_from_slice(&data);
        self.svm.set_account(self.agent, acc).unwrap();
    }

    pub fn position_pda_for(&self, trader: &Pubkey, nonce: u64) -> (Pubkey, Pubkey) {
        let position = Pubkey::find_program_address(
            &[POSITION_SEED, self.agent.as_ref(), trader.as_ref(), &nonce.to_le_bytes()],
            &self.program_id,
        )
        .0;
        let vault = Pubkey::find_program_address(&[POSITION_VAULT_SEED, position.as_ref()], &self.program_id).0;
        (position, vault)
    }

    pub fn position_pda(&self, nonce: u64) -> (Pubkey, Pubkey) {
        self.position_pda_for(&self.trader.pubkey(), nonce)
    }

    pub fn position_state_for(&self, trader: &Pubkey, nonce: u64) -> Position {
        let acc = self.svm.get_account(&self.position_pda_for(trader, nonce).0).unwrap();
        Position::try_deserialize(&mut &acc.data[..]).unwrap()
    }

    pub fn position_state(&self, nonce: u64) -> Position {
        self.position_state_for(&self.trader.pubkey(), nonce)
    }

    pub fn op(&self) -> Keypair {
        self.operator.insecure_clone()
    }

    pub fn tr(&self) -> Keypair {
        self.trader.insecure_clone()
    }

    // ---- operator ----

    pub fn create_ix(&self, name: &str, description: &str, t: AgentTerms) -> Instruction {
        Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::CreateAgent {
                agent_id: self.agent_id,
                name: name.into(),
                description: description.into(),
                terms: t,
            }
            .data(),
            proof_of_agent::accounts::CreateAgent {
                operator: self.operator.pubkey(),
                agent: self.agent,
                agent_vault: self.agent_vault,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        )
    }

    pub fn create_named(&mut self, name: &str, description: &str, t: AgentTerms) -> Result<(), String> {
        let ix = self.create_ix(name, description, t);
        let s = self.op();
        self.send(ix, &s)
    }

    pub fn create(&mut self, t: AgentTerms) -> Result<(), String> {
        self.create_named("Momentum Bot", "SOL/USDC momentum", t)
    }

    pub fn update_named(
        &mut self,
        name: &str,
        description: &str,
        t: AgentTerms,
        signer: &Keypair,
    ) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::UpdateAgent {
                name: name.into(),
                description: description.into(),
                terms: t,
            }
            .data(),
            proof_of_agent::accounts::UpdateAgent { operator: signer.pubkey(), agent: self.agent }
                .to_account_metas(None),
        );
        self.send(ix, signer)
    }

    pub fn update(&mut self, t: AgentTerms, signer: &Keypair) -> Result<(), String> {
        self.update_named("Momentum Bot v2", "edited", t, signer)
    }

    pub fn publish_as(&mut self, signer: &Keypair) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::PublishAgent {}.data(),
            proof_of_agent::accounts::PublishAgent { operator: signer.pubkey(), agent: self.agent }
                .to_account_metas(None),
        );
        self.send(ix, signer)
    }

    pub fn publish(&mut self) -> Result<(), String> {
        let s = self.op();
        self.publish_as(&s)
    }

    pub fn bind_executor(&mut self, executor: Pubkey, signer: &Keypair) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::SetExecutor { executor }.data(),
            proof_of_agent::accounts::SetExecutor { operator: signer.pubkey(), agent: self.agent }
                .to_account_metas(None),
        );
        self.send(ix, signer)
    }

    pub fn set_accepting_as(&mut self, accepting: bool, signer: &Keypair) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::SetAccepting { accepting }.data(),
            proof_of_agent::accounts::SetAccepting { operator: signer.pubkey(), agent: self.agent }
                .to_account_metas(None),
        );
        self.send(ix, signer)
    }

    pub fn set_accepting(&mut self, accepting: bool) -> Result<(), String> {
        let s = self.op();
        self.set_accepting_as(accepting, &s)
    }

    pub fn deposit_as(&mut self, amount: u64, signer: &Keypair) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::DepositCollateral { amount }.data(),
            proof_of_agent::accounts::DepositCollateral {
                operator: signer.pubkey(),
                agent: self.agent,
                agent_vault: self.agent_vault,
                config: self.config,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        );
        self.send(ix, signer)
    }

    pub fn deposit(&mut self, amount: u64) -> Result<(), String> {
        let s = self.op();
        self.deposit_as(amount, &s)
    }

    pub fn withdraw_as(&mut self, amount: u64, signer: &Keypair) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::WithdrawCollateral { amount }.data(),
            proof_of_agent::accounts::WithdrawCollateral {
                operator: signer.pubkey(),
                agent: self.agent,
                agent_vault: self.agent_vault,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        );
        self.send(ix, signer)
    }

    pub fn withdraw(&mut self, amount: u64) -> Result<(), String> {
        let s = self.op();
        self.withdraw_as(amount, &s)
    }

    /// Standard setup: 30% ratio, 15% fee, 20% tolerance, 1 SOL bond, published.
    pub fn launched() -> Self {
        let mut env = Env::new();
        env.create(terms(3_000, 1_500, 2_000)).unwrap();
        env.deposit(SOL).unwrap();
        env.publish().unwrap();
        env
    }

    // ---- trading key ----

    pub fn draw_ix(&self, trader: &Pubkey, nonce: u64, signer: &Pubkey) -> Instruction {
        let (position, position_vault) = self.position_pda_for(trader, nonce);
        Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::DrawFunds {}.data(),
            proof_of_agent::accounts::DrawFunds {
                executor: *signer,
                agent: self.agent,
                position,
                position_vault,
                config: self.config,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        )
    }

    pub fn draw_for(&mut self, trader: &Pubkey, nonce: u64, signer: &Keypair) -> Result<(), String> {
        let ix = self.draw_ix(trader, nonce, &signer.pubkey());
        self.send(ix, signer)
    }

    pub fn draw(&mut self, nonce: u64, signer: &Keypair) -> Result<(), String> {
        let trader = self.trader.pubkey();
        self.draw_for(&trader, nonce, signer)
    }

    pub fn settle_accounts_for(
        &self,
        trader: &Pubkey,
        nonce: u64,
        signer: &Keypair,
    ) -> proof_of_agent::accounts::SettlePosition {
        let (position, position_vault) = self.position_pda_for(trader, nonce);
        proof_of_agent::accounts::SettlePosition {
            executor: signer.pubkey(),
            operator: self.operator.pubkey(),
            agent: self.agent,
            agent_vault: self.agent_vault,
            position,
            position_vault,
            trader: *trader,
            system_program: system_program::ID,
        }
    }

    pub fn settle_accounts(&self, nonce: u64, signer: &Keypair) -> proof_of_agent::accounts::SettlePosition {
        self.settle_accounts_for(&self.trader.pubkey(), nonce, signer)
    }

    pub fn settle_with(
        &mut self,
        accounts: proof_of_agent::accounts::SettlePosition,
        returned: u64,
        signer: &Keypair,
    ) -> Result<Vec<String>, String> {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::SettlePosition { returned }.data(),
            accounts.to_account_metas(None),
        );
        self.send_logs(ix, signer)
    }

    pub fn settle_for(
        &mut self,
        trader: &Pubkey,
        nonce: u64,
        returned: u64,
        signer: &Keypair,
    ) -> Result<(), String> {
        let accounts = self.settle_accounts_for(trader, nonce, signer);
        self.settle_with(accounts, returned, signer).map(|_| ())
    }

    pub fn settle(&mut self, nonce: u64, returned: u64, signer: &Keypair) -> Result<(), String> {
        let trader = self.trader.pubkey();
        self.settle_for(&trader, nonce, returned, signer)
    }

    // ---- trader ----

    pub fn open_ix(&self, trader: &Pubkey, nonce: u64, amount: u64, duration: i64) -> Instruction {
        let (position, position_vault) = self.position_pda_for(trader, nonce);
        Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::OpenPosition { nonce, amount, duration_secs: duration }.data(),
            proof_of_agent::accounts::OpenPosition {
                trader: *trader,
                agent: self.agent,
                position,
                position_vault,
                config: self.config,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        )
    }

    pub fn open_by(&mut self, trader: &Keypair, nonce: u64, amount: u64, duration: i64) -> Result<(), String> {
        let ix = self.open_ix(&trader.pubkey(), nonce, amount, duration);
        self.send(ix, trader)
    }

    pub fn open(&mut self, nonce: u64, amount: u64, duration: i64) -> Result<(), String> {
        let s = self.tr();
        self.open_by(&s, nonce, amount, duration)
    }

    pub fn claim_ix(&self, trader: &Pubkey, nonce: u64) -> Instruction {
        let (position, position_vault) = self.position_pda_for(trader, nonce);
        Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::ClaimDefault {}.data(),
            proof_of_agent::accounts::ClaimDefault {
                trader: *trader,
                agent: self.agent,
                agent_vault: self.agent_vault,
                position,
                position_vault,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        )
    }

    pub fn claim_default_by(&mut self, trader: &Keypair, nonce: u64) -> Result<(), String> {
        let ix = self.claim_ix(&trader.pubkey(), nonce);
        self.send(ix, trader)
    }

    pub fn claim_default(&mut self, nonce: u64) -> Result<(), String> {
        let s = self.tr();
        self.claim_default_by(&s, nonce)
    }

    pub fn cancel_ix(&self, trader: &Pubkey, nonce: u64) -> Instruction {
        let (position, position_vault) = self.position_pda_for(trader, nonce);
        Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::CancelPosition {}.data(),
            proof_of_agent::accounts::CancelPosition {
                trader: *trader,
                agent: self.agent,
                position,
                position_vault,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        )
    }

    pub fn cancel_by(&mut self, trader: &Keypair, nonce: u64) -> Result<(), String> {
        let ix = self.cancel_ix(&trader.pubkey(), nonce);
        self.send(ix, trader)
    }

    pub fn cancel(&mut self, nonce: u64) -> Result<(), String> {
        let s = self.tr();
        self.cancel_by(&s, nonce)
    }

    /// Accounting invariants that must hold after every instruction:
    /// the lock never exceeds the collateral, and the agent vault always holds
    /// at least the recorded collateral plus its rent floor.
    pub fn check_invariants(&self) {
        let a = self.agent_state();
        assert!(
            a.locked_collateral <= a.total_collateral,
            "locked {} > total {}",
            a.locked_collateral,
            a.total_collateral
        );
        let vault = self.balance(&self.agent_vault);
        let floor = a.total_collateral + self.rent_floor();
        assert!(vault >= floor, "vault {vault} < total_collateral + rent {floor}");
    }
}

pub fn assert_err(res: Result<(), String>, code: u32) {
    let err = res.expect_err("expected failure");
    let needle = format!("Custom({code})");
    assert!(err.contains(&needle), "expected {needle}, got {err}");
}

// Error codes, in declaration order.
pub const E_RATIO: u32 = 6000;
pub const E_DRAWDOWN: u32 = 6001;
pub const E_NAME: u32 = 6002;
pub const E_DESCRIPTION: u32 = 6003;
pub const E_ZERO: u32 = 6004;
pub const E_INSUFFICIENT: u32 = 6005;
pub const E_OPERATOR: u32 = 6006;
pub const E_TRADER: u32 = 6007;
pub const E_STATUS: u32 = 6008;
pub const E_DURATION: u32 = 6009;
pub const E_NOT_REACHED: u32 = 6010;
pub const E_DEADLINE_PASSED: u32 = 6011;
pub const E_NOT_ACCEPTING: u32 = 6012;
pub const E_FEE: u32 = 6014;
pub const E_WINDOW: u32 = 6015;
pub const E_ASSETS: u32 = 6016;
pub const E_RULES: u32 = 6017;
pub const E_LOCKED: u32 = 6018;
pub const E_NOT_PUBLISHED: u32 = 6019;
pub const E_ALREADY: u32 = 6020;
pub const E_NO_COLLATERAL: u32 = 6021;
pub const E_EXECUTOR: u32 = 6022;
pub const E_RATIO_DRAWDOWN: u32 = 6023;
pub const E_PAUSED: u32 = 6024;
pub const E_TOO_LARGE: u32 = 6025;
pub const E_AGENT_CAP: u32 = 6026;
pub const E_ADMIN: u32 = 6027;
pub const E_CAPS: u32 = 6028;
/// Anchor's AccountNotInitialized.
pub const E_NOT_INITIALIZED: u32 = 3012;
/// Anchor's ConstraintSeeds.
pub const E_SEEDS: u32 = 2006;

pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn has_event<E: Event>(logs: &[String], event: &E) -> bool {
    let needle = format!("Program data: {}", base64(&event.data()));
    logs.iter().any(|l| *l == needle)
}

pub fn has_event_prefix(logs: &[String], discriminator: &[u8]) -> bool {
    // The discriminator is 8 bytes; its first 6 map to exactly 8 base64 chars.
    let prefix = format!("Program data: {}", &base64(discriminator)[..8]);
    logs.iter().any(|l| l.starts_with(&prefix))
}

pub fn assert_counters(a: &Agent, open: u32, settled: u32, defaulted: u32, breaches: u32) {
    assert_eq!(
        (a.open_positions, a.settled_positions, a.defaulted_positions, a.breach_count),
        (open, settled, defaulted, breaches),
        "(open, settled, defaulted, breach_count)"
    );
}

/// Deterministic xorshift64* PRNG for property-style loops (no proptest offline).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform-ish value in `lo..=hi`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        let span = hi - lo;
        if span == u64::MAX {
            return self.next_u64();
        }
        lo + self.next_u64() % (span + 1)
    }
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let s = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..chunk.len().saturating_sub(1)]);
    }
    Some(out)
}

/// Decode every event of type `E` emitted in `logs`, in order.
pub fn events<E: Event + anchor_lang::AnchorDeserialize>(logs: &[String]) -> Vec<E> {
    logs.iter()
        .filter_map(|l| l.strip_prefix("Program data: "))
        .filter_map(base64_decode)
        .filter(|d| d.starts_with(E::DISCRIMINATOR))
        .map(|d| E::deserialize(&mut &d[E::DISCRIMINATOR.len()..]).expect("event decodes"))
        .collect()
}

/// Exactly one event of type `E` in `logs`.
pub fn one_event<E: Event + anchor_lang::AnchorDeserialize>(logs: &[String]) -> E {
    let mut v = events::<E>(logs);
    assert_eq!(v.len(), 1, "expected exactly one event");
    v.pop().unwrap()
}
