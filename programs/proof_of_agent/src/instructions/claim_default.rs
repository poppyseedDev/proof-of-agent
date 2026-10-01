use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::ErrorCode,
    events::{BreachRecorded, PositionClosed},
    state::{Agent, Breach, Position, PositionStatus},
};

/// The agent drew the funds into its wallet (a position from before vault
/// custody) and did not settle before the deadline. The trader claims the
/// full locked guarantee from the agent's collateral vault.
#[derive(Accounts)]
pub struct ClaimDefault<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,
    #[account(
        mut,
        seeds = [AGENT_SEED, agent.operator.as_ref(), &agent.agent_id.to_le_bytes()],
        bump = agent.bump,
    )]
    pub agent: Account<'info, Agent>,
    #[account(
        mut,
        seeds = [AGENT_VAULT_SEED, agent.key().as_ref()],
        bump = agent.vault_bump
    )]
    pub agent_vault: SystemAccount<'info>,
    #[account(
        mut,
        seeds = [POSITION_SEED, agent.key().as_ref(), trader.key().as_ref(), &position.nonce.to_le_bytes()],
        bump = position.bump,
        has_one = trader @ ErrorCode::UnauthorizedTrader,
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
    /// CHECK: the position's custody PDA. It exists only for positions that
    /// trade under vault custody, which cannot default: the principal is in
    /// the vault, so the trader unwinds and settles instead.
    #[account(seeds = [CUSTODY_SEED, position.key().as_ref()], bump)]
    pub custody: UncheckedAccount<'info>,
}

pub fn handle_claim_default(ctx: Context<ClaimDefault>) -> Result<()> {
    let position = &mut ctx.accounts.position;
    let agent = &mut ctx.accounts.agent;
    require!(position.status == PositionStatus::Trading, ErrorCode::InvalidStatus);
    require!(ctx.accounts.custody.data_is_empty(), ErrorCode::UseSettle);
    let now = Clock::get()?.unix_timestamp;
    require!(now >= position.deadline, ErrorCode::DeadlineNotReached);

    let sys = ctx.accounts.system_program.to_account_info();
    let slash = position.locked_collateral;

    let agent_key = agent.key();
    let agent_seeds: &[&[u8]] = &[AGENT_VAULT_SEED, agent_key.as_ref(), &[agent.vault_bump]];
    super::transfer_from_vault(
        &ctx.accounts.agent_vault.to_account_info(),
        &ctx.accounts.trader.to_account_info(),
        &sys,
        agent_seeds,
        slash,
    )?;

    // Return whatever is left in the position vault (the rent floor).
    let position_key = position.key();
    let position_seeds: &[&[u8]] =
        &[POSITION_VAULT_SEED, position_key.as_ref(), &[position.vault_bump]];
    let remaining = ctx.accounts.position_vault.lamports();
    super::transfer_from_vault(
        &ctx.accounts.position_vault.to_account_info(),
        &ctx.accounts.trader.to_account_info(),
        &sys,
        position_seeds,
        remaining,
    )?;

    agent.total_collateral -= slash;
    agent.locked_collateral -= slash;
    agent.capital_managed -= position.principal;
    agent.open_positions -= 1;
    agent.defaulted_positions += 1;
    agent.breach_count += 1;
    agent.slashed_total = agent
        .slashed_total
        .checked_add(slash)
        .ok_or(ErrorCode::Overflow)?;

    position.status = PositionStatus::Defaulted;
    position.breach = Breach::MissedDeadline;
    position.slashed = slash;
    position.closed_at = now;

    emit!(BreachRecorded {
        agent: agent_key,
        position: position_key,
        breach: Breach::MissedDeadline,
        slashed: slash,
    });
    emit!(PositionClosed {
        position: position_key,
        agent: agent_key,
        trader: position.trader,
        status: PositionStatus::Defaulted,
        breach: Breach::MissedDeadline,
        returned: 0,
        slashed: slash,
        fee_paid: 0,
        trader_payout: slash,
    });
    Ok(())
}
