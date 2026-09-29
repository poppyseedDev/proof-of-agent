use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::ErrorCode,
    events::FundsDrawn,
    state::{Agent, Config, Position, PositionStatus},
};

/// The agent's trading key pulls the trader's principal out of the position
/// vault to trade with it. From this moment the reserved collateral is at
/// risk: the agent must settle before the deadline or the trader claims it.
///
/// While the position is Open, this races the trader's `cancel_position`:
/// whichever transaction lands first wins. There is no grace period.
///
/// Stopped while the protocol is paused. An undrawn position cannot default,
/// so the trader can still cancel it and the agent can still decline it.
#[derive(Accounts)]
pub struct DrawFunds<'info> {
    /// The bound trading key or the operator. Receives the principal.
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
    /// Last, so clients built for the previous program version (which had no
    /// config) keep working against it: a trailing extra account is ignored.
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
}

pub fn handle_draw_funds(ctx: Context<DrawFunds>) -> Result<()> {
    require!(!ctx.accounts.config.paused, ErrorCode::ProtocolPaused);
    let position = &mut ctx.accounts.position;
    require!(position.status == PositionStatus::Open, ErrorCode::InvalidStatus);
    let now = Clock::get()?.unix_timestamp;
    require!(now < position.deadline, ErrorCode::DeadlinePassed);

    position.status = PositionStatus::Trading;
    position.drawn_at = now;

    let position_key = position.key();
    let seeds: &[&[u8]] = &[POSITION_VAULT_SEED, position_key.as_ref(), &[position.vault_bump]];
    super::transfer_from_vault(
        &ctx.accounts.position_vault.to_account_info(),
        &ctx.accounts.executor.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        seeds,
        position.principal,
    )?;

    emit!(FundsDrawn {
        position: position_key,
        agent: ctx.accounts.agent.key(),
        trader: position.trader,
        executor: ctx.accounts.executor.key(),
        principal: position.principal,
        deadline: position.deadline,
    });
    Ok(())
}
