//! Harness additions for vault custody: SPL mints and token accounts, synthetic
//! Pyth price updates, the mock DEX, and the custody instructions.
#![allow(dead_code)]

use {
    super::*,
    anchor_spl::token::spl_token::{self, native_mint, state::Account as TokenAccountState, state::Mint as MintState},
    anchor_lang::solana_program::program_pack::Pack,
    anchor_lang::Space,
    proof_of_agent::state::{Custody, FeedMapping},
    spl_associated_token_account_interface::address::get_associated_token_address,
};

fn account(lamports: u64, data: Vec<u8>, owner: Pubkey) -> solana_account::Account {
    solana_account::Account { lamports, data, owner, executable: false, rent_epoch: 0 }
}

pub const TOKEN_PROGRAM: Pubkey = anchor_spl::token::ID;
pub const ATA_PROGRAM: Pubkey = anchor_spl::associated_token::ID;
pub const TOKEN_ACCOUNT_LEN: usize = TokenAccountState::LEN;
pub const WSOL: Pubkey = native_mint::ID;

/// Pyth-style feed ids for the test mints.
pub const SOL_FEED: [u8; 32] = [1u8; 32];
pub const USDC_FEED: [u8; 32] = [2u8; 32];
pub const OTHER_FEED: [u8; 32] = [3u8; 32];

pub fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    get_associated_token_address(owner, mint)
}

/// A mock-DEX market: the two mints and the DEX's reserves for them.
pub struct Market {
    pub usdc: Pubkey,
    pub mint_authority: Keypair,
    pub dex_authority: Pubkey,
    pub reserve_sol: Pubkey,
    pub reserve_usdc: Pubkey,
    pub oracle_program: Pubkey,
    pub price_sol: Pubkey,
    pub price_usdc: Pubkey,
}

impl Env {
    /// Writes an initialised SPL mint directly into the ledger.
    pub fn create_mint(&mut self, mint: Pubkey, decimals: u8, authority: Option<Pubkey>) {
        let state = MintState {
            mint_authority: authority.into(),
            supply: 0,
            decimals,
            is_initialized: true,
            freeze_authority: None.into(),
        };
        let mut data = vec![0u8; MintState::LEN];
        MintState::pack(state, &mut data).unwrap();
        let rent = self.svm.get_sysvar::<Rent>().minimum_balance(data.len());
        self.svm
            .set_account(mint, account(rent, data, TOKEN_PROGRAM))
            .unwrap();
    }

    /// The wrapped SOL mint, which LiteSVM does not ship.
    pub fn create_native_mint(&mut self) {
        if self.svm.get_account(&WSOL).is_none() {
            self.create_mint(WSOL, 9, None);
        }
    }

    pub fn token_account(&self, key: &Pubkey) -> Option<TokenAccountState> {
        let acc = self.svm.get_account(key)?;
        TokenAccountState::unpack(&acc.data).ok()
    }

    pub fn token_balance(&self, key: &Pubkey) -> u64 {
        self.token_account(key).map(|a| a.amount).unwrap_or(0)
    }

    /// Writes a token account with `amount` of `mint` for `owner` into the ledger.
    /// For wSOL the lamports match the amount plus rent, as the token program keeps them.
    pub fn set_token_account(&mut self, key: Pubkey, mint: Pubkey, owner: Pubkey, amount: u64) {
        let rent = self.svm.get_sysvar::<Rent>().minimum_balance(TOKEN_ACCOUNT_LEN);
        let native = mint == WSOL;
        let state = TokenAccountState {
            mint,
            owner,
            amount,
            delegate: None.into(),
            state: spl_token::state::AccountState::Initialized,
            is_native: if native { Some(rent) } else { None }.into(),
            delegated_amount: 0,
            close_authority: None.into(),
        };
        let mut data = vec![0u8; TOKEN_ACCOUNT_LEN];
        TokenAccountState::pack(state, &mut data).unwrap();
        let lamports = if native { rent + amount } else { rent };
        self.svm
            .set_account(key, account(lamports, data, TOKEN_PROGRAM))
            .unwrap();
    }

    /// Creates `owner`'s ATA for `mint` holding `amount`.
    pub fn fund_ata(&mut self, owner: &Pubkey, mint: &Pubkey, amount: u64) -> Pubkey {
        let key = ata(owner, mint);
        self.set_token_account(key, *mint, *owner, amount);
        key
    }

    /// Writes a fully verified `PriceUpdateV2` for `feed` at `price × 10^exponent`,
    /// published at `publish_time`, owned by `oracle_program`.
    pub fn set_price(&mut self, key: Pubkey, oracle_program: Pubkey, feed: [u8; 32], price: i64, exponent: i32, publish_time: i64) {
        let mut data = Vec::with_capacity(134);
        data.extend_from_slice(&PRICE_UPDATE_V2_DISCRIMINATOR);
        data.extend_from_slice(Pubkey::new_unique().as_ref()); // write_authority
        data.push(PRICE_VERIFICATION_FULL);
        data.extend_from_slice(&feed);
        data.extend_from_slice(&price.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes()); // conf
        data.extend_from_slice(&exponent.to_le_bytes());
        data.extend_from_slice(&publish_time.to_le_bytes());
        data.extend_from_slice(&publish_time.to_le_bytes()); // prev_publish_time
        data.extend_from_slice(&price.to_le_bytes()); // ema_price
        data.extend_from_slice(&0u64.to_le_bytes()); // ema_conf
        data.extend_from_slice(&0u64.to_le_bytes()); // posted_slot
        let rent = self.svm.get_sysvar::<Rent>().minimum_balance(data.len());
        self.svm
            .set_account(key, account(rent, data, oracle_program))
            .unwrap();
    }

    /// Loads the mock DEX and sets up a SOL/USDC market with deep reserves and
    /// fresh oracle prices: SOL at `sol_price_usd`, USDC at $1, both at expo -8.
    pub fn market(&mut self, sol_price_usd: i64) -> Market {
        self.create_native_mint();
        let bytes = std::fs::read(concat!(env!("CARGO_TARGET_TMPDIR"), "/../deploy/mock_dex.so"))
            .expect("run `anchor build` first: target/deploy/mock_dex.so is missing");
        self.svm.add_program(mock_dex::ID, &bytes).unwrap();
        let mint_authority = Keypair::new();
        let usdc = Pubkey::new_unique();
        self.create_mint(usdc, 6, Some(mint_authority.pubkey()));
        let dex_authority = Pubkey::find_program_address(&[mock_dex::DEX_SEED], &mock_dex::ID).0;
        let reserve_sol = self.fund_ata(&dex_authority, &WSOL, 1_000_000 * SOL);
        let reserve_usdc = self.fund_ata(&dex_authority, &usdc, 1_000_000_000 * 1_000_000);
        let oracle_program = Pubkey::new_unique();
        let price_sol = Pubkey::new_unique();
        let price_usdc = Pubkey::new_unique();
        let now = self.now();
        self.set_price(price_sol, oracle_program, SOL_FEED, sol_price_usd * 100_000_000, -8, now);
        self.set_price(price_usdc, oracle_program, USDC_FEED, 100_000_000, -8, now);
        Market { usdc, mint_authority, dex_authority, reserve_sol, reserve_usdc, oracle_program, price_sol, price_usdc }
    }

    /// Refreshes both oracle prices to the current clock, keeping their values.
    pub fn refresh_prices(&mut self, m: &Market, sol_price_usd: i64) {
        let now = self.now();
        self.set_price(m.price_sol, m.oracle_program, SOL_FEED, sol_price_usd * 100_000_000, -8, now);
        self.set_price(m.price_usdc, m.oracle_program, USDC_FEED, 100_000_000, -8, now);
    }

    // ---- admin ----

    pub fn set_trading_config_as(
        &mut self,
        dex_programs: Vec<Pubkey>,
        oracle_program: Pubkey,
        max_price_age_secs: i64,
        max_swap_deviation_bps: u16,
        late_penalty_bps: u16,
        feeds: Vec<FeedMapping>,
        signer: &Keypair,
    ) -> Result<Vec<String>, String> {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::SetTradingConfig {
                allowed_dex_programs: dex_programs,
                oracle_program,
                max_price_age_secs,
                max_swap_deviation_bps,
                late_penalty_bps,
                feeds,
            }
            .data(),
            proof_of_agent::accounts::SetConfig {
                admin: signer.pubkey(),
                config: self.config,
                program_data: self.program_data,
            }
            .to_account_metas(None),
        );
        self.send_logs(ix, signer)
    }

    /// Allows the mock DEX, maps wSOL and the market's USDC to their feeds.
    pub fn allow_market(&mut self, m: &Market, max_swap_deviation_bps: u16, late_penalty_bps: u16) {
        let admin = self.admin.insecure_clone();
        self.set_trading_config_as(
            vec![mock_dex::ID],
            m.oracle_program,
            60,
            max_swap_deviation_bps,
            late_penalty_bps,
            vec![
                FeedMapping { mint: WSOL, feed_id: SOL_FEED },
                FeedMapping { mint: m.usdc, feed_id: USDC_FEED },
            ],
            &admin,
        )
        .unwrap();
    }

    // ---- trading key ----

    pub fn begin_trading_ix(&self, trader: &Pubkey, nonce: u64, signer: &Pubkey) -> Instruction {
        let (position, position_vault) = self.position_pda_for(trader, nonce);
        Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::BeginTrading {}.data(),
            proof_of_agent::accounts::BeginTrading {
                executor: *signer,
                agent: self.agent,
                position,
                position_vault,
                custody: self.custody_pda(&position),
                wsol_mint: WSOL,
                vault_wsol: ata(&position_vault, &WSOL),
                token_program: TOKEN_PROGRAM,
                associated_token_program: ATA_PROGRAM,
                system_program: system_program::ID,
                config: self.config,
            }
            .to_account_metas(None),
        )
    }

    pub fn begin_trading_for(&mut self, trader: &Pubkey, nonce: u64, signer: &Keypair) -> Result<(), String> {
        self.create_native_mint();
        let ix = self.begin_trading_ix(trader, nonce, &signer.pubkey());
        self.send(ix, signer)
    }

    pub fn begin_trading(&mut self, nonce: u64, signer: &Keypair) -> Result<(), String> {
        let trader = self.trader.pubkey();
        self.begin_trading_for(&trader, nonce, signer)
    }

    pub fn custody_state(&self, position: &Pubkey) -> Option<Custody> {
        let acc = self.svm.get_account(&self.custody_pda(position))?;
        if acc.data.is_empty() {
            return None;
        }
        Custody::try_deserialize(&mut &acc.data[..]).ok()
    }

    /// The vault's wSOL balance of a position.
    pub fn vault_wsol(&self, nonce: u64) -> u64 {
        self.vault_wsol_for(&self.trader.pubkey(), nonce)
    }

    pub fn vault_wsol_for(&self, trader: &Pubkey, nonce: u64) -> u64 {
        let (_, vault) = self.position_pda_for(trader, nonce);
        self.token_balance(&ata(&vault, &WSOL))
    }

    /// Pretends the agent traded: sets the vault's wSOL account to `amount`.
    pub fn set_vault_wsol(&mut self, trader: &Pubkey, nonce: u64, amount: u64) {
        let (_, vault) = self.position_pda_for(trader, nonce);
        self.set_token_account(ata(&vault, &WSOL), WSOL, vault, amount);
    }

    pub fn custody_settle_accounts_for(
        &self,
        trader: &Pubkey,
        nonce: u64,
        signer: &Pubkey,
        rent_payer: &Pubkey,
    ) -> proof_of_agent::accounts::SettlePosition {
        let (position, position_vault) = self.position_pda_for(trader, nonce);
        proof_of_agent::accounts::SettlePosition {
            executor: *signer,
            operator: self.operator.pubkey(),
            agent: self.agent,
            agent_vault: self.agent_vault,
            position,
            position_vault,
            trader: *trader,
            system_program: system_program::ID,
            custody: self.custody_pda(&position),
            vault_wsol: Some(ata(&position_vault, &WSOL)),
            rent_payer: Some(*rent_payer),
            token_program: Some(TOKEN_PROGRAM),
            config: Some(self.config),
        }
    }

    /// Settles a custody position. `rent_payer` defaults to the signer.
    pub fn custody_settle_for(
        &mut self,
        trader: &Pubkey,
        nonce: u64,
        signer: &Keypair,
        rent_payer: Option<Pubkey>,
    ) -> Result<Vec<String>, String> {
        let payer = rent_payer.unwrap_or(signer.pubkey());
        let accounts = self.custody_settle_accounts_for(trader, nonce, &signer.pubkey(), &payer);
        self.settle_send(accounts, 0, signer)
    }

    pub fn custody_settle(&mut self, nonce: u64, signer: &Keypair) -> Result<Vec<String>, String> {
        let trader = self.trader.pubkey();
        self.custody_settle_for(&trader, nonce, signer, None)
    }

    pub fn open_vault_ata(&mut self, nonce: u64, mint: &Pubkey, signer: &Keypair) -> Result<(), String> {
        let (position, position_vault) = self.position_pda(nonce);
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::OpenVaultTokenAccount {}.data(),
            proof_of_agent::accounts::OpenVaultTokenAccount {
                executor: signer.pubkey(),
                agent: self.agent,
                position,
                position_vault,
                mint: *mint,
                vault_ata: ata(&position_vault, mint),
                token_program: TOKEN_PROGRAM,
                associated_token_program: ATA_PROGRAM,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        );
        self.send(ix, signer)
    }

    pub fn close_vault_ata(&mut self, nonce: u64, mint: &Pubkey, signer: &Keypair) -> Result<(), String> {
        let (position, position_vault) = self.position_pda(nonce);
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::CloseVaultTokenAccount {}.data(),
            proof_of_agent::accounts::CloseVaultTokenAccount {
                executor: signer.pubkey(),
                agent: self.agent,
                position,
                position_vault,
                mint: *mint,
                vault_ata: ata(&position_vault, mint),
                token_program: TOKEN_PROGRAM,
            }
            .to_account_metas(None),
        );
        self.send(ix, signer)
    }

    /// Builds an `execute_swap` through the mock DEX. `mint_in`/`mint_out` are
    /// wSOL or the market's USDC. `extra` are appended to the DEX call for the
    /// misbehaving modes.
    #[allow(clippy::too_many_arguments)]
    pub fn swap_ix(
        &self,
        m: &Market,
        trader: &Pubkey,
        nonce: u64,
        signer: &Pubkey,
        mint_in: Pubkey,
        mint_out: Pubkey,
        amount_in: u64,
        amount_out: u64,
        amount_in_max: u64,
        min_out: u64,
        mode: u8,
        extra: &[Pubkey],
    ) -> Instruction {
        let (position, position_vault) = self.position_pda_for(trader, nonce);
        let vault_in = ata(&position_vault, &mint_in);
        let vault_out = ata(&position_vault, &mint_out);
        let (reserve_in, reserve_out) = if mint_in == WSOL { (m.reserve_sol, m.reserve_usdc) } else { (m.reserve_usdc, m.reserve_sol) };
        let (price_in, price_out) = if mint_in == WSOL { (m.price_sol, m.price_usdc) } else { (m.price_usdc, m.price_sol) };
        let data = mock_dex::instruction::Swap { amount_in, amount_out, mode }.data();
        let mut metas = proof_of_agent::accounts::ExecuteSwap {
            executor: *signer,
            agent: self.agent,
            position,
            custody: self.custody_pda(&position),
            position_vault,
            mint_in,
            mint_out,
            vault_in,
            vault_out,
            price_in,
            price_out,
            dex_program: mock_dex::ID,
            token_program_in: TOKEN_PROGRAM,
            token_program_out: TOKEN_PROGRAM,
            config: self.config,
        }
        .to_account_metas(None);
        // The DEX instruction's accounts, as the client built them: the vault
        // is not a transaction signer; the program marks it in the CPI.
        let mut dex_metas = mock_dex::accounts::Swap {
            authority: position_vault,
            vault_in,
            vault_out,
            reserve_in,
            reserve_out,
            dex_authority: m.dex_authority,
            token_program: TOKEN_PROGRAM,
        }
        .to_account_metas(None);
        for meta in dex_metas.iter_mut() {
            meta.is_signer = false;
        }
        for k in extra {
            dex_metas.push(anchor_lang::prelude::AccountMeta::new(*k, false));
        }
        metas.extend(dex_metas);
        Instruction::new_with_bytes(
            self.program_id,
            &proof_of_agent::instruction::ExecuteSwap { amount_in_max, min_out, data }.data(),
            metas,
        )
    }

    /// An honest swap by `signer` with `amount_in_max = amount_in` and `min_out = amount_out`.
    pub fn swap(
        &mut self,
        m: &Market,
        nonce: u64,
        signer: &Keypair,
        mint_in: Pubkey,
        mint_out: Pubkey,
        amount_in: u64,
        amount_out: u64,
    ) -> Result<Vec<String>, String> {
        let trader = self.trader.pubkey();
        let ix = self.swap_ix(m, &trader, nonce, &signer.pubkey(), mint_in, mint_out, amount_in, amount_out, amount_in, amount_out, mock_dex::mode::HONEST, &[]);
        self.send_logs(ix, signer)
    }

    // ---- legacy positions (drawn by draw_funds under the previous program) ----

    /// Overwrites the stored position account.
    pub fn write_position(&mut self, trader: &Pubkey, nonce: u64, p: &Position) {
        let (key, _) = self.position_pda_for(trader, nonce);
        let mut acc = self.svm.get_account(&key).unwrap();
        let mut data = Vec::new();
        p.try_serialize(&mut data).unwrap();
        acc.data[..data.len()].copy_from_slice(&data);
        self.svm.set_account(key, acc).unwrap();
    }

    /// Rent the executor pays at `begin_trading`: the custody account and the
    /// vault's wSOL account. Both come back at settlement.
    pub fn custody_rents(&self) -> u64 {
        self.custody_rent() + self.svm.get_sysvar::<Rent>().minimum_balance(TOKEN_ACCOUNT_LEN)
    }

    pub fn custody_rent(&self) -> u64 {
        self.svm.get_sysvar::<Rent>().minimum_balance(8 + Custody::INIT_SPACE)
    }

    /// Reproduces what the previous program's `draw_funds` did: the principal
    /// moves to `executor` and the position becomes Trading, with no custody.
    pub fn legacy_draw(&mut self, nonce: u64, executor: &Pubkey) {
        let trader = self.trader.pubkey();
        self.legacy_draw_for(&trader, nonce, executor);
    }

    pub fn legacy_draw_for(&mut self, trader: &Pubkey, nonce: u64, executor: &Pubkey) {
        let trader = *trader;
        let (_, vault) = self.position_pda_for(&trader, nonce);
        let mut p = self.position_state_for(&trader, nonce);
        assert_eq!(p.status, PositionStatus::Open);
        let mut vault_acc = self.svm.get_account(&vault).unwrap();
        vault_acc.lamports -= p.principal;
        self.svm.set_account(vault, vault_acc).unwrap();
        let mut exec_acc = self.svm.get_account(executor).unwrap();
        exec_acc.lamports += p.principal;
        self.svm.set_account(*executor, exec_acc).unwrap();
        p.status = PositionStatus::Trading;
        p.drawn_at = self.now();
        self.write_position(&trader, nonce, &p);
    }
}
