use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::ErrorCode,
    events::PositionOpened,
    state::{Agent, AgentStatus, AgentTerms, Breach, Config, Position, PositionStatus},
};

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct OpenPosition<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,
    #[account(
        mut,
        seeds = [AGENT_SEED, agent.operator.as_ref(), &agent.agent_id.to_le_bytes()],
        bump = agent.bump,
    )]
    pub agent: Account<'info, Agent>,
    #[account(
        init,
        payer = trader,
        space = 8 + Position::INIT_SPACE,
        seeds = [POSITION_SEED, agent.key().as_ref(), trader.key().as_ref(), &nonce.to_le_bytes()],
        bump
    )]
    pub position: Account<'info, Position>,
    #[account(
        mut,
        seeds = [POSITION_VAULT_SEED, position.key().as_ref()],
        bump
    )]
    pub position_vault: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
    /// Last, so clients built for the previous program version (which had no
    /// config) keep working against it: a trailing extra account is ignored.
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
}

/// Collateral the agent must lock for `principal`, rounded up so that even
/// dust-sized positions are backed.
pub fn required_collateral(principal: u64, ratio_bps: u16) -> Result<u64> {
    (principal as u128)
        .checked_mul(ratio_bps as u128)
        .and_then(|v| v.checked_add(BPS_DENOMINATOR as u128 - 1))
        .and_then(|v| v.checked_div(BPS_DENOMINATOR as u128))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or_else(|| error!(ErrorCode::Overflow))
}

pub fn handle_open_position(
    ctx: Context<OpenPosition>,
    nonce: u64,
    amount: u64,
    duration_secs: i64,
) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroAmount);
    let config = &ctx.accounts.config;
    require!(!config.paused, ErrorCode::ProtocolPaused);
    require!(amount <= config.max_position, ErrorCode::PositionTooLarge);
    let agent = &mut ctx.accounts.agent;
    require!(agent.status == AgentStatus::Active, ErrorCode::AgentNotAccepting);
    let managed_after = agent.capital_managed.checked_add(amount).ok_or(ErrorCode::Overflow)?;
    require!(managed_after <= config.max_agent_capital, ErrorCode::AgentCapReached);
    // The trader picks a deadline inside the window the operator published.
    require!(
        (agent.terms.min_duration_secs..=agent.terms.max_duration_secs).contains(&duration_secs),
        ErrorCode::InvalidDuration
    );

    // Agents published before the ratio + drawdown bound existed may carry
    // terms that no longer validate. Refuse to snapshot them into a position.
    require!(
        AgentTerms::ratio_and_drawdown_fit(
            agent.terms.collateral_ratio_bps,
            agent.terms.max_drawdown_bps
        ),
        ErrorCode::RatioPlusDrawdownTooHigh
    );

    // Lock the agent's guarantee for this position. If the agent cannot
    // back the deposit at its advertised ratio, the position cannot open.
    let locked = required_collateral(amount, agent.terms.collateral_ratio_bps)?;
    require!(
        locked <= agent.free_collateral(),
        ErrorCode::InsufficientFreeCollateral
    );
    agent.locked_collateral = agent
        .locked_collateral
        .checked_add(locked)
        .ok_or(ErrorCode::Overflow)?;
    agent.capital_managed = managed_after;
    agent.open_positions += 1;

    // Move principal (plus the vault rent floor) into the position vault.
    let rent_floor = Rent::get()?.minimum_balance(0);
    super::transfer_from_signer(
        &ctx.accounts.trader.to_account_info(),
        &ctx.accounts.position_vault.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        amount.checked_add(rent_floor).ok_or(ErrorCode::Overflow)?,
    )?;

    let now = Clock::get()?.unix_timestamp;
    let position = &mut ctx.accounts.position;
    position.trader = ctx.accounts.trader.key();
    position.agent = agent.key();
    position.nonce = nonce;
    position.principal = amount;
    position.locked_collateral = locked;
    position.fee_bps = agent.terms.fee_bps;
    position.max_drawdown_bps = agent.terms.max_drawdown_bps;
    position.status = PositionStatus::Open;
    position.breach = Breach::None;
    position.opened_at = now;
    position.deadline = now + duration_secs;
    position.drawn_at = 0;
    position.closed_at = 0;
    position.returned = 0;
    position.slashed = 0;
    position.fee_paid = 0;
    position.bump = ctx.bumps.position;
    position.vault_bump = ctx.bumps.position_vault;

    emit!(PositionOpened {
        position: position.key(),
        agent: agent.key(),
        trader: position.trader,
        principal: amount,
        locked_collateral: locked,
        deadline: position.deadline,
    });
    Ok(())
}
