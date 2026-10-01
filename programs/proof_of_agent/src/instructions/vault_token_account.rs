use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token::spl_token::native_mint,
    token_interface::{self, CloseAccount, Mint, TokenAccount, TokenInterface},
};

use crate::{
    constants::*,
    error::ErrorCode,
    state::{Agent, Custody, Position, PositionStatus},
};

/// Creates the vault's associated token account for one of the agent's
/// allowed assets, so `execute_swap` can receive it. The signer pays the
/// rent and gets it back from `close_vault_token_account`.
#[derive(Accounts)]
pub struct OpenVaultTokenAccount<'info> {
    #[account(mut, constraint = agent.can_execute(&executor.key()) @ ErrorCode::UnauthorizedExecutor)]
    pub executor: Signer<'info>,
    #[account(
        seeds = [AGENT_SEED, agent.operator.as_ref(), &agent.agent_id.to_le_bytes()],
        bump = agent.bump,
    )]
    pub agent: Account<'info, Agent>,
    #[account(
        seeds = [POSITION_SEED, agent.key().as_ref(), position.trader.as_ref(), &position.nonce.to_le_bytes()],
        bump = position.bump,
        constraint = position.agent == agent.key() @ ErrorCode::InvalidStatus,
    )]
    pub position: Account<'info, Position>,
    #[account(
        seeds = [POSITION_VAULT_SEED, position.key().as_ref()],
        bump = position.vault_bump
    )]
    pub position_vault: SystemAccount<'info>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(
        init,
        payer = executor,
        associated_token::mint = mint,
        associated_token::authority = position_vault,
        associated_token::token_program = token_program,
    )]
    pub vault_ata: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn handle_open_vault_token_account(ctx: Context<OpenVaultTokenAccount>) -> Result<()> {
    let mint = ctx.accounts.mint.key();
    require!(
        mint == native_mint::ID || Custody::asset_index(&ctx.accounts.agent.terms, &mint).is_some(),
        ErrorCode::MintNotAllowed
    );
    require!(
        matches!(ctx.accounts.position.status, PositionStatus::Open | PositionStatus::Trading),
        ErrorCode::InvalidStatus
    );
    Ok(())
}

/// Closes an empty vault token account and returns its rent to the signer.
/// Allowed in any position status, so dust accounts can be cleaned up after
/// settlement; the wSOL account is the exception while the position trades,
/// because settlement reads it.
#[derive(Accounts)]
pub struct CloseVaultTokenAccount<'info> {
    #[account(mut, constraint = agent.can_execute(&executor.key()) @ ErrorCode::UnauthorizedExecutor)]
    pub executor: Signer<'info>,
    #[account(
        seeds = [AGENT_SEED, agent.operator.as_ref(), &agent.agent_id.to_le_bytes()],
        bump = agent.bump,
    )]
    pub agent: Account<'info, Agent>,
    #[account(
        seeds = [POSITION_SEED, agent.key().as_ref(), position.trader.as_ref(), &position.nonce.to_le_bytes()],
        bump = position.bump,
        constraint = position.agent == agent.key() @ ErrorCode::InvalidStatus,
    )]
    pub position: Account<'info, Position>,
    #[account(
        seeds = [POSITION_VAULT_SEED, position.key().as_ref()],
        bump = position.vault_bump
    )]
    pub position_vault: SystemAccount<'info>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = position_vault,
        associated_token::token_program = token_program,
    )]
    pub vault_ata: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
}

pub fn handle_close_vault_token_account(ctx: Context<CloseVaultTokenAccount>) -> Result<()> {
    require!(ctx.accounts.vault_ata.amount == 0, ErrorCode::TokenAccountNotEmpty);
    require!(
        ctx.accounts.mint.key() != native_mint::ID
            || ctx.accounts.position.status != PositionStatus::Trading,
        ErrorCode::WsolAccountInUse
    );
    let position_key = ctx.accounts.position.key();
    let seeds: &[&[u8]] =
        &[POSITION_VAULT_SEED, position_key.as_ref(), &[ctx.accounts.position.vault_bump]];
    token_interface::close_account(CpiContext::new_with_signer(
        ctx.accounts.token_program.key(),
        CloseAccount {
            account: ctx.accounts.vault_ata.to_account_info(),
            destination: ctx.accounts.executor.to_account_info(),
            authority: ctx.accounts.position_vault.to_account_info(),
        },
        &[seeds],
    ))
}
