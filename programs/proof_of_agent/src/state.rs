use anchor_lang::prelude::*;

use crate::{constants::*, error::ErrorCode};

/// The rules and collateral terms an operator publishes for an agent.
/// Editable while the agent is a draft, permanent once it is published.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug, InitSpace)]
pub struct AgentTerms {
    /// Collateral guaranteed per deposited lamport, in bps (3000 = 30%).
    pub collateral_ratio_bps: u16,
    /// Performance fee on profit, in bps. At most collateral_ratio_bps / FEE_CAP_DIVISOR.
    pub fee_bps: u16,
    /// Loss the agent may incur before its bond pays the trader, in bps of principal.
    pub max_drawdown_bps: u16,
    /// Shortest and longest position a trader may open, in seconds.
    pub min_duration_secs: i64,
    pub max_duration_secs: i64,
    /// Token mints the agent commits to trading.
    #[max_len(MAX_ALLOWED_ASSETS)]
    pub allowed_assets: Vec<Pubkey>,
    /// The agent's full trading rules in plain language.
    #[max_len(MAX_RULES_LEN)]
    pub rules: String,
}

impl AgentTerms {
    pub fn max_fee_bps(collateral_ratio_bps: u16) -> u16 {
        collateral_ratio_bps / FEE_CAP_DIVISOR
    }

    /// Collateral ratio plus tolerated drawdown may not exceed 100% of principal.
    /// Above that, `settle_position(returned = 0)` would slash
    /// `principal - principal * max_drawdown`, which is less than the locked
    /// bond, so walking away would be cheaper than a default.
    pub fn ratio_and_drawdown_fit(collateral_ratio_bps: u16, max_drawdown_bps: u16) -> bool {
        (collateral_ratio_bps as u32) + (max_drawdown_bps as u32) <= BPS_DENOMINATOR as u32
    }

    pub fn validate(&self) -> Result<()> {
        require!(
            (MIN_COLLATERAL_RATIO_BPS..=MAX_COLLATERAL_RATIO_BPS).contains(&self.collateral_ratio_bps),
            ErrorCode::InvalidCollateralRatio
        );
        require!(
            self.fee_bps <= Self::max_fee_bps(self.collateral_ratio_bps),
            ErrorCode::FeeTooHigh
        );
        require!(self.max_drawdown_bps <= MAX_DRAWDOWN_BPS, ErrorCode::InvalidDrawdown);
        require!(
            Self::ratio_and_drawdown_fit(self.collateral_ratio_bps, self.max_drawdown_bps),
            ErrorCode::RatioPlusDrawdownTooHigh
        );
        require!(
            self.min_duration_secs >= MIN_POSITION_DURATION
                && self.max_duration_secs <= MAX_POSITION_DURATION
                && self.min_duration_secs <= self.max_duration_secs,
            ErrorCode::InvalidTradingWindow
        );
        require!(
            !self.allowed_assets.is_empty() && self.allowed_assets.len() <= MAX_ALLOWED_ASSETS,
            ErrorCode::InvalidAllowedAssets
        );
        for (i, a) in self.allowed_assets.iter().enumerate() {
            require!(
                !self.allowed_assets[..i].contains(a),
                ErrorCode::InvalidAllowedAssets
            );
        }
        require!(
            !self.rules.trim().is_empty() && self.rules.len() <= MAX_RULES_LEN,
            ErrorCode::InvalidRules
        );
        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum AgentStatus {
    /// Terms can still change; traders cannot allocate.
    Draft,
    /// Published and accepting new positions.
    Active,
    /// Published; the operator has stopped new positions.
    Paused,
}

/// An AI trading agent created and managed by an operator.
#[account(discriminator = AGENT_DISCRIMINATOR)]
#[derive(InitSpace)]
pub struct Agent {
    /// Creator and manager: controls terms, collateral, and the trading key.
    pub operator: Pubkey,
    /// Key allowed to draw and settle positions. Defaults to the operator.
    pub executor: Pubkey,
    /// Per-operator index, part of the PDA seed.
    pub agent_id: u64,
    pub status: AgentStatus,
    #[max_len(MAX_NAME_LEN)]
    pub name: String,
    #[max_len(MAX_DESCRIPTION_LEN)]
    pub description: String,
    pub terms: AgentTerms,
    pub created_at: i64,
    pub published_at: i64,
    /// Lamports the operator has deposited as collateral (excluding the vault rent floor).
    pub total_collateral: u64,
    /// Collateral reserved for open positions.
    pub locked_collateral: u64,
    /// Principal currently under management.
    pub capital_managed: u64,
    pub open_positions: u32,
    /// Positions the agent drew and later settled (on time or late). Positions
    /// declined before a draw are not counted.
    pub settled_positions: u32,
    pub defaulted_positions: u32,
    /// Positions that ended in a breach: a drawdown beyond tolerance, a late
    /// settle, or a claimed default.
    pub breach_count: u32,
    pub slashed_total: u64,
    pub fees_earned: u64,
    pub bump: u8,
    pub vault_bump: u8,
}

impl Agent {
    pub fn free_collateral(&self) -> u64 {
        self.total_collateral.saturating_sub(self.locked_collateral)
    }

    pub fn is_published(&self) -> bool {
        self.status != AgentStatus::Draft
    }

    pub fn can_execute(&self, key: &Pubkey) -> bool {
        *key == self.executor || *key == self.operator
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum PositionStatus {
    /// Trader deposited; the agent has not drawn the funds yet.
    Open,
    /// The agent drew the principal and is trading with it.
    Trading,
    /// The position was settled by the agent's trading key. If `drawn_at` is 0
    /// the agent declined it (never drew the funds) and the trader got the
    /// principal back; otherwise the agent returned funds. `breach` is
    /// `MissedDeadline` when the settle came at or after the deadline.
    Settled,
    /// The agent missed the deadline; collateral was paid to the trader.
    Defaulted,
    /// The trader cancelled before the agent drew funds.
    Cancelled,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum Breach {
    None,
    /// Returned less than the principal minus the tolerated drawdown.
    Drawdown,
    /// Did not settle before the deadline: either the trader claimed the bond
    /// (`Defaulted`) or the agent settled late (`Settled`).
    MissedDeadline,
}

/// A trader's capital allocation to an agent, backed by reserved collateral.
#[account(discriminator = POSITION_DISCRIMINATOR)]
#[derive(InitSpace)]
pub struct Position {
    pub trader: Pubkey,
    pub agent: Pubkey,
    /// Per-trader nonce used in the PDA seed.
    pub nonce: u64,
    pub principal: u64,
    pub locked_collateral: u64,
    /// Snapshot of the agent's terms at open time.
    pub fee_bps: u16,
    pub max_drawdown_bps: u16,
    pub status: PositionStatus,
    pub breach: Breach,
    pub opened_at: i64,
    pub deadline: i64,
    pub drawn_at: i64,
    pub closed_at: i64,
    pub returned: u64,
    pub slashed: u64,
    pub fee_paid: u64,
    pub bump: u8,
    pub vault_bump: u8,
}

/// Protocol-wide limits, set by the program's upgrade authority. They can stop
/// new money from entering and cap how much a position or an agent may hold.
/// They cannot move funds, and nothing here stops a trader from cancelling,
/// an agent from settling, a trader from claiming a default, or an operator
/// from withdrawing free collateral.
#[account(discriminator = CONFIG_DISCRIMINATOR)]
#[derive(InitSpace)]
pub struct Config {
    /// Stops new positions, draws and collateral deposits.
    pub paused: bool,
    /// Largest principal one position may hold, in lamports.
    pub max_position: u64,
    /// Largest principal one agent may manage across its open positions, in lamports.
    pub max_agent_capital: u64,
    pub bump: u8,
    /// Programs `execute_swap` may call. Empty means no position can trade.
    #[max_len(MAX_DEX_PROGRAMS)]
    pub allowed_dex_programs: Vec<Pubkey>,
    /// Owner every price update account must have (the Pyth receiver program).
    pub oracle_program: Pubkey,
    /// Oldest publish time a swap may be priced with, in seconds.
    pub max_price_age_secs: i64,
    /// Largest loss of oracle value one swap may take, in bps.
    pub max_swap_deviation_bps: u16,
    /// Share of the locked bond paid to the trader when a custody position
    /// settles at or after its deadline, in bps.
    pub late_penalty_bps: u16,
    /// Which oracle feed prices which mint. A mint without a feed cannot be traded.
    #[max_len(MAX_FEEDS)]
    pub feeds: Vec<FeedMapping>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub struct FeedMapping {
    pub mint: Pubkey,
    /// Pyth feed id (32 bytes).
    pub feed_id: [u8; 32],
}

impl Config {
    pub fn validate_caps(max_position: u64, max_agent_capital: u64) -> Result<()> {
        require!(max_position > 0 && max_agent_capital > 0, ErrorCode::InvalidCaps);
        Ok(())
    }

    pub fn validate_trading(
        allowed_dex_programs: &[Pubkey],
        max_price_age_secs: i64,
        max_swap_deviation_bps: u16,
        late_penalty_bps: u16,
        feeds: &[FeedMapping],
    ) -> Result<()> {
        require!(allowed_dex_programs.len() <= MAX_DEX_PROGRAMS, ErrorCode::InvalidTradingConfig);
        require!(feeds.len() <= MAX_FEEDS, ErrorCode::InvalidTradingConfig);
        require!(max_price_age_secs > 0, ErrorCode::InvalidTradingConfig);
        require!(max_swap_deviation_bps <= MAX_SWAP_DEVIATION_BPS, ErrorCode::InvalidTradingConfig);
        require!(late_penalty_bps <= MAX_LATE_PENALTY_BPS, ErrorCode::InvalidTradingConfig);
        for (i, f) in feeds.iter().enumerate() {
            require!(
                !feeds[..i].iter().any(|g| g.mint == f.mint),
                ErrorCode::InvalidTradingConfig
            );
        }
        Ok(())
    }

    pub fn feed_for(&self, mint: &Pubkey) -> Result<[u8; 32]> {
        self.feeds
            .iter()
            .find(|f| f.mint == *mint)
            .map(|f| f.feed_id)
            .ok_or_else(|| error!(ErrorCode::NoFeedForMint))
    }
}

/// Created when a position begins trading under vault custody and closed when
/// it settles. Its existence is what tells the program a position's principal
/// is in the vault rather than in the trading wallet.
#[account(discriminator = CUSTODY_DISCRIMINATOR)]
#[derive(InitSpace)]
pub struct Custody {
    pub position: Pubkey,
    /// Paid the rent of this account and of the vault's wrapped SOL account;
    /// gets both back at settlement.
    pub rent_payer: Pubkey,
    /// Bit `i` is set while the vault's token account for
    /// `agent.terms.allowed_assets[i]` holds a non-zero balance. Wrapped SOL
    /// is the base asset and never in the mask.
    pub held_mask: u8,
    pub swaps: u32,
    pub bump: u8,
}

impl Custody {
    /// Position of `mint` in the agent's allowed assets, or None for wrapped
    /// SOL (always allowed) and for any mint the agent did not publish.
    pub fn asset_index(terms: &AgentTerms, mint: &Pubkey) -> Option<usize> {
        terms.allowed_assets.iter().position(|m| m == mint)
    }

    pub fn set_held(&mut self, index: usize, held: bool) {
        let bit = 1u8 << index;
        if held {
            self.held_mask |= bit;
        } else {
            self.held_mask &= !bit;
        }
    }
}
