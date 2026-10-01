use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token::{self, Mint, SyncNative, Token, TokenAccount},
};

use crate::{
    constants::*,
    error::ErrorCode,
    events::FundsDrawn,
    state::{Agent, Config, Custody, Position, PositionStatus},
};

/// Starts trading a position under vault custody. The principal stays in the
/// position vault: it is wrapped into the vault's wSOL token account, which
/// the vault PDA owns, and from then on it can only move through
/// `execute_swap`. Replaces `draw_funds`.
#[derive(Accounts)]
pub struct BeginTrading<'info> {
    /// The bound trading key or the operator. Pays the rent of the custody
    /// account and of the vault's wSOL account, and gets it back at settlement.
    #[account(mut, constraint = agent.can_execute(&executor.key()) @ ErrorCode::UnauthorizedExecutor)]
    pub executor: Signer<'info>,
    #[account(
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
    #[account(
        init,
        payer = executor,
        space = 8 + Custody::INIT_SPACE,
        seeds = [CUSTODY_SEED, position.key().as_ref()],
        bump
    )]
    pub custody: Account<'info, Custody>,
    #[account(address = token::spl_token::native_mint::ID @ ErrorCode::MintNotAllowed)]
    pub wsol_mint: Account<'info, Mint>,
    #[account(
        init_if_needed,
        payer = executor,
        associated_token::mint = wsol_mint,
        associated_token::authority = position_vault,
    )]
    pub vault_wsol: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
}

pub fn handle_begin_trading(ctx: Context<BeginTrading>) -> Result<()> {
    require!(!ctx.accounts.config.paused, ErrorCode::ProtocolPaused);
    let position = &mut ctx.accounts.position;
    require!(position.status == PositionStatus::Open, ErrorCode::InvalidStatus);
    let now = Clock::get()?.unix_timestamp;
    require!(now < position.deadline, ErrorCode::DeadlinePassed);

    position.status = PositionStatus::Trading;
    position.drawn_at = now;

    let custody = &mut ctx.accounts.custody;
    custody.position = position.key();
    custody.rent_payer = ctx.accounts.executor.key();
    custody.held_mask = 0;
    custody.swaps = 0;
    custody.bump = ctx.bumps.custody;

    // Wrap the principal: lamports move from the vault into its wSOL account,
    // then the token program is told to count them.
    let position_key = position.key();
    let seeds: &[&[u8]] = &[POSITION_VAULT_SEED, position_key.as_ref(), &[position.vault_bump]];
    super::transfer_from_vault(
        &ctx.accounts.position_vault.to_account_info(),
        &ctx.accounts.vault_wsol.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        seeds,
        position.principal,
    )?;
    token::sync_native(CpiContext::new(
        ctx.accounts.token_program.key(),
        SyncNative { account: ctx.accounts.vault_wsol.to_account_info() },
    ))?;

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
