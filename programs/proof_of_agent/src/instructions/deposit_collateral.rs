use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::ErrorCode,
    events::CollateralChanged,
    state::{Agent, Config},
};

#[derive(Accounts)]
pub struct DepositCollateral<'info> {
    #[account(mut)]
    pub operator: Signer<'info>,
    #[account(
        mut,
        seeds = [AGENT_SEED, agent.operator.as_ref(), &agent.agent_id.to_le_bytes()],
        bump = agent.bump,
        has_one = operator @ ErrorCode::UnauthorizedOperator,
    )]
    pub agent: Account<'info, Agent>,
    #[account(
        mut,
        seeds = [AGENT_VAULT_SEED, agent.key().as_ref()],
        bump = agent.vault_bump
    )]
    pub agent_vault: SystemAccount<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    pub system_program: Program<'info, System>,
}

pub fn handle_deposit_collateral(ctx: Context<DepositCollateral>, amount: u64) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroAmount);
    require!(!ctx.accounts.config.paused, ErrorCode::ProtocolPaused);
    super::transfer_from_signer(
        &ctx.accounts.operator.to_account_info(),
        &ctx.accounts.agent_vault.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        amount,
    )?;
    let agent = &mut ctx.accounts.agent;
    agent.total_collateral = agent
        .total_collateral
        .checked_add(amount)
        .ok_or(ErrorCode::Overflow)?;
    emit!(CollateralChanged {
        agent: agent.key(),
        delta: amount as i64,
        total_collateral: agent.total_collateral,
    });
    Ok(())
}
