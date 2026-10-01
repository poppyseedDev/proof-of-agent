use anchor_lang::{
    prelude::*,
    solana_program::{
        instruction::{AccountMeta, Instruction},
        program::invoke_signed,
    },
};
use anchor_spl::{
    token::spl_token::native_mint,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    constants::*,
    error::ErrorCode,
    events::SwapExecuted,
    oracle,
    state::{Agent, Config, Custody, Position, PositionStatus},
};

/// Swaps between two of the vault's token accounts through an allowlisted
/// DEX. The DEX instruction is forwarded as given (its accounts as the
/// remaining accounts, its data as `data`), signed by the vault PDA. The
/// program checks what the swap did, not how: input taken, output received,
/// the output's oracle value against the input's, and that the vault's
/// accounts came back untouched apart from their balances.
#[derive(Accounts)]
pub struct ExecuteSwap<'info> {
    /// The bound trading key or the operator. At or after the deadline, also
    /// the trader, who may then only swap back into wSOL.
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
        mut,
        seeds = [CUSTODY_SEED, position.key().as_ref()],
        bump = custody.bump,
        constraint = custody.position == position.key() @ ErrorCode::InvalidStatus,
    )]
    pub custody: Account<'info, Custody>,
    /// The token authority of every vault token account. Signs the DEX call.
    #[account(
        seeds = [POSITION_VAULT_SEED, position.key().as_ref()],
        bump = position.vault_bump
    )]
    pub position_vault: SystemAccount<'info>,
    pub mint_in: InterfaceAccount<'info, Mint>,
    pub mint_out: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        associated_token::mint = mint_in,
        associated_token::authority = position_vault,
        associated_token::token_program = token_program_in,
    )]
    pub vault_in: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        associated_token::mint = mint_out,
        associated_token::authority = position_vault,
        associated_token::token_program = token_program_out,
    )]
    pub vault_out: InterfaceAccount<'info, TokenAccount>,
    /// CHECK: a Pyth price update for `mint_in`, parsed and checked in `oracle`.
    pub price_in: UncheckedAccount<'info>,
    /// CHECK: a Pyth price update for `mint_out`, parsed and checked in `oracle`.
    pub price_out: UncheckedAccount<'info>,
    /// CHECK: must be one of the config's allowed DEX programs.
    pub dex_program: UncheckedAccount<'info>,
    pub token_program_in: Interface<'info, TokenInterface>,
    pub token_program_out: Interface<'info, TokenInterface>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
}

/// A token account (either token program; 165 bytes before any extension) whose
/// owner field is `owner`.
fn is_token_account_owned_by(a: &AccountInfo, owner: &Pubkey) -> bool {
    if *a.owner != anchor_spl::token::ID && *a.owner != anchor_spl::token_2022::ID {
        return false;
    }
    match a.try_borrow_data() {
        Ok(d) => d.len() >= 165 && d[32..64] == owner.to_bytes(),
        Err(_) => false,
    }
}

pub fn handle_execute_swap<'info>(
    ctx: Context<'info, ExecuteSwap<'info>>,
    amount_in_max: u64,
    min_out: u64,
    data: Vec<u8>,
) -> Result<()> {
    let config = &ctx.accounts.config;
    let agent = &ctx.accounts.agent;
    let position = &ctx.accounts.position;
    let executor = ctx.accounts.executor.key();
    require!(position.status == PositionStatus::Trading, ErrorCode::InvalidStatus);

    let mint_in = ctx.accounts.mint_in.key();
    let mint_out = ctx.accounts.mint_out.key();
    require!(mint_in != mint_out, ErrorCode::SameMint);
    let unwinding = mint_out == native_mint::ID;

    // Who may swap, and into what.
    let now = Clock::get()?.unix_timestamp;
    let late = now >= position.deadline;
    let can_execute = agent.can_execute(&executor);
    if late {
        require!(can_execute || executor == position.trader, ErrorCode::UnauthorizedExecutor);
        require!(unwinding, ErrorCode::AfterDeadlineOnlyUnwind);
    } else {
        require!(can_execute, ErrorCode::UnauthorizedExecutor);
    }
    // A pause stops trading but never stops money going back into SOL.
    require!(!config.paused || unwinding, ErrorCode::ProtocolPaused);

    // Both legs must be assets the agent published (wSOL is always allowed).
    let index_in = Custody::asset_index(&agent.terms, &mint_in);
    let index_out = Custody::asset_index(&agent.terms, &mint_out);
    require!(mint_in == native_mint::ID || index_in.is_some(), ErrorCode::MintNotAllowed);
    require!(unwinding || index_out.is_some(), ErrorCode::MintNotAllowed);

    let dex = ctx.accounts.dex_program.key();
    require!(config.allowed_dex_programs.contains(&dex), ErrorCode::DexNotAllowed);

    // The vault signs the DEX call, so any other vault token account the
    // instruction could reach would be drainable. Only the two legs may appear.
    let vault_key = ctx.accounts.position_vault.key();
    let vault_in_key = ctx.accounts.vault_in.key();
    let vault_out_key = ctx.accounts.vault_out.key();
    for a in ctx.remaining_accounts {
        if *a.key == vault_in_key || *a.key == vault_out_key {
            continue;
        }
        require!(!is_token_account_owned_by(a, &vault_key), ErrorCode::ExtraVaultAccount);
    }

    // Oracle prices for both legs, from the feeds the config maps the mints to.
    let price_in = oracle::read_price_update(
        &ctx.accounts.price_in,
        &config.oracle_program,
        &config.feed_for(&mint_in)?,
        now,
        config.max_price_age_secs,
    )?;
    let price_out = oracle::read_price_update(
        &ctx.accounts.price_out,
        &config.oracle_program,
        &config.feed_for(&mint_out)?,
        now,
        config.max_price_age_secs,
    )?;

    let pre_in = ctx.accounts.vault_in.amount;
    let pre_out = ctx.accounts.vault_out.amount;
    let vault_lamports = ctx.accounts.position_vault.lamports();

    // Forward the DEX instruction, with the vault marked as signer wherever it appears.
    let metas: Vec<AccountMeta> = ctx
        .remaining_accounts
        .iter()
        .map(|a| AccountMeta {
            pubkey: *a.key,
            is_signer: a.is_signer || *a.key == vault_key,
            is_writable: a.is_writable,
        })
        .collect();
    let ix = Instruction { program_id: dex, accounts: metas, data };
    let mut infos: Vec<AccountInfo> = ctx.remaining_accounts.to_vec();
    infos.push(ctx.accounts.dex_program.to_account_info());
    let position_key = position.key();
    let seeds: &[&[u8]] = &[POSITION_VAULT_SEED, position_key.as_ref(), &[position.vault_bump]];
    invoke_signed(&ix, &infos, &[seeds])?;

    // What the swap did.
    ctx.accounts.vault_in.reload()?;
    ctx.accounts.vault_out.reload()?;
    let vault_in = &ctx.accounts.vault_in;
    let vault_out = &ctx.accounts.vault_out;
    for acc in [vault_in, vault_out] {
        require!(
            acc.owner == vault_key && acc.delegate.is_none() && acc.close_authority.is_none(),
            ErrorCode::VaultAccountTampered
        );
    }
    require!(
        ctx.accounts.position_vault.lamports() == vault_lamports,
        ErrorCode::VaultAccountTampered
    );
    let amount_in = pre_in.checked_sub(vault_in.amount).ok_or(ErrorCode::VaultAccountTampered)?;
    let amount_out = vault_out.amount.checked_sub(pre_out).ok_or(ErrorCode::VaultAccountTampered)?;
    require!(amount_in <= amount_in_max, ErrorCode::SwapTooMuchIn);
    require!(amount_out >= min_out, ErrorCode::SwapTooLittleOut);

    let (value_in, value_out) = oracle::leg_values(
        amount_in,
        &price_in,
        ctx.accounts.mint_in.decimals,
        amount_out,
        &price_out,
        ctx.accounts.mint_out.decimals,
    )?;
    require!(
        oracle::within_deviation(value_in, value_out, config.max_swap_deviation_bps)?,
        ErrorCode::SwapBelowOracle
    );

    let custody = &mut ctx.accounts.custody;
    if let Some(i) = index_in.filter(|_| mint_in != native_mint::ID) {
        custody.set_held(i, vault_in.amount > 0);
    }
    if let Some(i) = index_out.filter(|_| !unwinding) {
        custody.set_held(i, vault_out.amount > 0);
    }
    custody.swaps = custody.swaps.checked_add(1).ok_or(ErrorCode::Overflow)?;

    emit!(SwapExecuted {
        position: position_key,
        agent: agent.key(),
        executor,
        dex_program: dex,
        mint_in,
        mint_out,
        amount_in,
        amount_out,
        value_in,
        value_out,
        held_mask: custody.held_mask,
    });
    Ok(())
}
