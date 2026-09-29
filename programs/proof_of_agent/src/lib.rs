//! Proof of Agent — an over-collateralised marketplace for AI trading agents on Solana.
//!
//! * Operators create agents as drafts, publish their rules and collateral terms
//!   (ratio, fee, trading window, max drawdown, allowed assets), and deposit SOL
//!   collateral. Published terms are permanent.
//! * The higher the ratio an agent guarantees, the higher the fee it may charge.
//! * When a trader allocates capital, `principal * ratio` of the agent's collateral is
//!   locked as a guarantee. An agent cannot accept capital it cannot back.
//! * If the agent misbehaves (loses beyond its declared drawdown, or never returns the
//!   funds by the deadline) the locked collateral is paid out to the trader.
//! * The program's upgrade authority can pause new positions, draws and deposits,
//!   and cap the size of positions and agents. It cannot move funds or stop
//!   anyone from getting money back.

pub mod constants;
pub mod error;
pub mod events;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("49aHwbzdT1iN8WYWdUZxrGoZpjSryyugMm4q9VTjXgSr");

#[program]
pub mod proof_of_agent {
    use super::*;

    // ---- operator: create, configure, publish ----

    pub fn create_agent(
        ctx: Context<CreateAgent>,
        agent_id: u64,
        name: String,
        description: String,
        terms: AgentTerms,
    ) -> Result<()> {
        instructions::create_agent::handle_create_agent(ctx, agent_id, name, description, terms)
    }

    pub fn update_agent(
        ctx: Context<UpdateAgent>,
        name: String,
        description: String,
        terms: AgentTerms,
    ) -> Result<()> {
        instructions::update_agent::handle_update_agent(ctx, name, description, terms)
    }

    pub fn publish_agent(ctx: Context<PublishAgent>) -> Result<()> {
        instructions::publish_agent::handle_publish_agent(ctx)
    }

    pub fn set_executor(ctx: Context<SetExecutor>, executor: Pubkey) -> Result<()> {
        instructions::set_executor::handle_set_executor(ctx, executor)
    }

    pub fn set_accepting(ctx: Context<SetAccepting>, accepting: bool) -> Result<()> {
        instructions::set_accepting::handle_set_accepting(ctx, accepting)
    }

    pub fn deposit_collateral(ctx: Context<DepositCollateral>, amount: u64) -> Result<()> {
        instructions::deposit_collateral::handle_deposit_collateral(ctx, amount)
    }

    pub fn withdraw_collateral(ctx: Context<WithdrawCollateral>, amount: u64) -> Result<()> {
        instructions::withdraw_collateral::handle_withdraw_collateral(ctx, amount)
    }

    // ---- agent trading key ----

    pub fn draw_funds(ctx: Context<DrawFunds>) -> Result<()> {
        instructions::draw_funds::handle_draw_funds(ctx)
    }

    pub fn settle_position(ctx: Context<SettlePosition>, returned: u64) -> Result<()> {
        instructions::settle_position::handle_settle_position(ctx, returned)
    }

    // ---- trader ----

    pub fn open_position(
        ctx: Context<OpenPosition>,
        nonce: u64,
        amount: u64,
        duration_secs: i64,
    ) -> Result<()> {
        instructions::open_position::handle_open_position(ctx, nonce, amount, duration_secs)
    }

    pub fn cancel_position(ctx: Context<CancelPosition>) -> Result<()> {
        instructions::cancel_position::handle_cancel_position(ctx)
    }

    pub fn claim_default(ctx: Context<ClaimDefault>) -> Result<()> {
        instructions::claim_default::handle_claim_default(ctx)
    }

    // ---- protocol admin: the program's upgrade authority ----

    pub fn init_config(ctx: Context<InitConfig>, max_position: u64, max_agent_capital: u64) -> Result<()> {
        instructions::config::handle_init_config(ctx, max_position, max_agent_capital)
    }

    pub fn set_paused(ctx: Context<SetConfig>, paused: bool) -> Result<()> {
        instructions::config::handle_set_paused(ctx, paused)
    }

    pub fn set_caps(ctx: Context<SetConfig>, max_position: u64, max_agent_capital: u64) -> Result<()> {
        instructions::config::handle_set_caps(ctx, max_position, max_agent_capital)
    }
}
