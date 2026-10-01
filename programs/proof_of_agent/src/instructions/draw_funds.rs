use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::ErrorCode,
    state::{Agent, Config, Position},
};

/// Disabled. Drawing moved the principal to the trading wallet; positions now
/// trade under vault custody through `begin_trading`. The instruction and its
/// accounts stay so a client built for the previous program gets
/// `DrawDisabled` instead of a wiring error.
#[derive(Accounts)]
pub struct DrawFunds<'info> {
    #[account(mut, constraint = agent.can_execute(&executor.key()) @ ErrorCode::UnauthorizedExecutor)]
    pub executor: Signer<'info>,
    #[account(
        mut,
        seeds = [AGENT_SEED, agent.operator.as_ref(), &agent.agent_id.to_le_bytes()],
        bump = agent.bump,
    )]
    pub agent: Account<'info, Agent>,
    #[account(
        mut,
        seeds = [POSITION_SEED, agent.key().as_ref(), position.trader.as_ref(), &position.nonce.to_le_bytes()],
        bump = position.bump,
        constraint = position.agent == agent.key() @ ErrorCode::InvalidStatus,
    )]
    pub position: Account<'info, Position>,
    #[account(
        mut,
        seeds = [POSITION_VAULT_SEED, position.key().as_ref()],
        bump = position.vault_bump
    )]
    pub position_vault: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
}

pub fn handle_draw_funds(_ctx: Context<DrawFunds>) -> Result<()> {
    Err(error!(ErrorCode::DrawDisabled))
}
