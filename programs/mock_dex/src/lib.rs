//! A DEX for the test suite. It swaps `amount_in` of one token for `amount_out`
//! of another at whatever rate the caller asks, from reserves it holds, and
//! takes a `mode` switch that makes it misbehave the way a hostile program
//! called with the vault's signature could. Never deployed anywhere.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, spl_token::instruction::AuthorityType, Approve, SetAuthority, Token, TokenAccount, Transfer};

declare_id!("8MqmdJJUUoHnSayNXd4hUmzPEJzvve3k6hZ5ixJacfjm");

pub const DEX_SEED: &[u8] = b"dex";

/// What `swap` does on top of the swap itself.
pub mod mode {
    /// An honest swap.
    pub const HONEST: u8 = 0;
    /// Approves remaining account 0 as a delegate on `vault_in`.
    pub const APPROVE_DELEGATE: u8 = 1;
    /// Also takes `amount_in` from remaining account 0, another token account the authority owns.
    pub const DRAIN_THIRD_ACCOUNT: u8 = 2;
    /// Sets remaining account 0 as the close authority of `vault_out`.
    pub const SET_CLOSE_AUTHORITY: u8 = 3;
    /// Moves one lamport from the authority (a system account) to remaining account 0, via remaining account 1 (the system program).
    pub const TAKE_LAMPORTS: u8 = 4;
    /// Takes the input and gives nothing back.
    pub const KEEP_INPUT: u8 = 5;
}

#[program]
pub mod mock_dex {
    use super::*;

    pub fn swap<'info>(
        ctx: Context<'info, Swap<'info>>,
        amount_in: u64,
        amount_out: u64,
        mode: u8,
    ) -> Result<()> {
        let token_program = ctx.accounts.token_program.key();
        token::transfer(
            CpiContext::new(
                token_program,
                Transfer {
                    from: ctx.accounts.vault_in.to_account_info(),
                    to: ctx.accounts.reserve_in.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                },
            ),
            amount_in,
        )?;
        let bump = ctx.bumps.dex_authority;
        let seeds: &[&[u8]] = &[DEX_SEED, &[bump]];
        if mode != mode::KEEP_INPUT {
            token::transfer(
                CpiContext::new_with_signer(
                    token_program,
                    Transfer {
                        from: ctx.accounts.reserve_out.to_account_info(),
                        to: ctx.accounts.vault_out.to_account_info(),
                        authority: ctx.accounts.dex_authority.to_account_info(),
                    },
                    &[seeds],
                ),
                amount_out,
            )?;
        }
        let extra = ctx.remaining_accounts;
        match mode {
            mode::APPROVE_DELEGATE => token::approve(
                CpiContext::new(
                    token_program,
                    Approve {
                        to: ctx.accounts.vault_in.to_account_info(),
                        delegate: extra[0].clone(),
                        authority: ctx.accounts.authority.to_account_info(),
                    },
                ),
                1,
            )?,
            mode::DRAIN_THIRD_ACCOUNT => token::transfer(
                CpiContext::new(
                    token_program,
                    Transfer {
                        from: extra[0].clone(),
                        to: ctx.accounts.reserve_in.to_account_info(),
                        authority: ctx.accounts.authority.to_account_info(),
                    },
                ),
                amount_in,
            )?,
            mode::SET_CLOSE_AUTHORITY => token::set_authority(
                CpiContext::new(
                    token_program,
                    SetAuthority {
                        current_authority: ctx.accounts.authority.to_account_info(),
                        account_or_mint: ctx.accounts.vault_out.to_account_info(),
                    },
                ),
                AuthorityType::CloseAccount,
                Some(*extra[0].key),
            )?,
            mode::TAKE_LAMPORTS => anchor_lang::system_program::transfer(
                CpiContext::new(
                    *extra[1].key,
                    anchor_lang::system_program::Transfer {
                        from: ctx.accounts.authority.to_account_info(),
                        to: extra[0].clone(),
                    },
                ),
                1,
            )?,
            _ => {}
        }
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Swap<'info> {
    /// The owner of `vault_in` and `vault_out`; the caller signs for it.
    pub authority: Signer<'info>,
    #[account(mut)]
    pub vault_in: Account<'info, TokenAccount>,
    #[account(mut)]
    pub vault_out: Account<'info, TokenAccount>,
    /// The DEX's own token accounts, owned by `dex_authority`.
    #[account(mut)]
    pub reserve_in: Account<'info, TokenAccount>,
    #[account(mut)]
    pub reserve_out: Account<'info, TokenAccount>,
    /// CHECK: PDA that owns the reserves.
    #[account(seeds = [DEX_SEED], bump)]
    pub dex_authority: UncheckedAccount<'info>,
    pub token_program: Program<'info, Token>,
}
