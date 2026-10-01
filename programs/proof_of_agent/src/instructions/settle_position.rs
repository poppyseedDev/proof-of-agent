use anchor_lang::{prelude::*, system_program};
use anchor_spl::{
    associated_token::get_associated_token_address,
    token::{self, spl_token::native_mint, CloseAccount, Token, TokenAccount},
};

use crate::{
    constants::*,
    error::ErrorCode,
    events::{BreachRecorded, PositionClosed, PositionDeclined},
    state::{Agent, Breach, Config, Custody, Position, PositionStatus},
};

/// Closes a position and pays everyone out. Outcomes:
///   * profit  -> the operator earns `fee_bps` of the profit, trader gets the rest.
///     If the fee would leave the operator wallet below the rent-exempt
///     minimum (an empty wallet and a fee under ~0.00089 SOL), the trader
///     keeps it instead and `fee_paid` is 0, so settlement cannot be blocked.
///   * loss within max_drawdown -> tolerated trading loss, no fee, no slash
///   * loss beyond max_drawdown -> breach: the shortfall is paid to the trader
///     from the agent's reserved collateral (up to the guarantee)
///   * position still Open (never drawn) -> decline: the principal goes back
///     to the trader and no reputation counter changes.
///
/// Where the returned amount comes from depends on how the position traded:
///
///   * **Vault custody** (a `Custody` account exists): the vault's wSOL account
///     is closed into the vault and `returned` is what the vault holds above
///     its rent floor. The `returned` argument is ignored. Every other vault
///     token account must be empty (`held_mask == 0`). The trading key or the
///     operator may settle; at or after the deadline the trader may too, and
///     the slash then includes `late_penalty_bps` of the locked bond as a
///     `MissedDeadline` breach.
///   * **Legacy** (positions drawn with `draw_funds` before custody): the
///     signer sends `returned` lamports. Settling at or after the deadline is
///     a `MissedDeadline` breach with the same payout rule; because terms
///     enforce ratio + max_drawdown <= 100%, returning nothing costs the whole
///     locked bond, the same as `claim_default`.
#[derive(Accounts)]
pub struct SettlePosition<'info> {
    /// The bound trading key or the operator (or, for a custody position at
    /// or after the deadline, the trader). On the legacy path, sends the
    /// returned SOL.
    #[account(mut)]
    pub executor: Signer<'info>,
    /// CHECK: must be the agent's operator; receives the performance fee.
    #[account(mut, address = agent.operator @ ErrorCode::UnauthorizedOperator)]
    pub operator: UncheckedAccount<'info>,
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
        seeds = [POSITION_SEED, agent.key().as_ref(), position.trader.as_ref(), &position.nonce.to_le_bytes()],
        bump = position.bump,
        constraint = position.agent == agent.key() @ ErrorCode::InvalidStatus,
        has_one = trader @ ErrorCode::UnauthorizedTrader,
    )]
    pub position: Account<'info, Position>,
    #[account(
        mut,
        seeds = [POSITION_VAULT_SEED, position.key().as_ref()],
        bump = position.vault_bump
    )]
    pub position_vault: SystemAccount<'info>,
    /// CHECK: validated against position.trader via has_one; receives the payout.
    #[account(mut)]
    pub trader: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    // ---- vault custody, appended so clients built before it keep working ----
    /// CHECK: the position's custody PDA. Holds a `Custody` account for a
    /// position that trades under vault custody and is empty otherwise.
    #[account(mut, seeds = [CUSTODY_SEED, position.key().as_ref()], bump)]
    pub custody: UncheckedAccount<'info>,
    /// The vault's wSOL account. Required for a custody position.
    #[account(mut)]
    pub vault_wsol: Option<Account<'info, TokenAccount>>,
    /// CHECK: `Custody.rent_payer`; gets the custody and wSOL account rent back.
    #[account(mut)]
    pub rent_payer: Option<UncheckedAccount<'info>>,
    pub token_program: Option<Program<'info, Token>>,
    /// The protocol config, for the late penalty. Required for a custody position.
    pub config: Option<Account<'info, Config>>,
}

pub struct Settlement {
    pub fee: u64,
    pub slash: u64,
}

pub fn compute_settlement(
    principal: u64,
    returned: u64,
    fee_bps: u16,
    max_drawdown_bps: u16,
    locked_collateral: u64,
) -> Result<Settlement> {
    let bps = |value: u64, rate: u16| -> Result<u64> {
        (value as u128)
            .checked_mul(rate as u128)
            .and_then(|v| v.checked_div(BPS_DENOMINATOR as u128))
            .and_then(|v| u64::try_from(v).ok())
            .ok_or_else(|| error!(ErrorCode::Overflow))
    };
    if returned >= principal {
        let profit = returned - principal;
        Ok(Settlement {
            fee: bps(profit, fee_bps)?,
            slash: 0,
        })
    } else {
        let loss = principal - returned;
        let allowed_loss = bps(principal, max_drawdown_bps)?;
        let shortfall = loss.saturating_sub(allowed_loss);
        Ok(Settlement {
            fee: 0,
            slash: shortfall.min(locked_collateral),
        })
    }
}

/// The late penalty on a custody position: `late_penalty_bps` of the locked
/// bond, but never more than what the drawdown slash left of it.
pub fn late_penalty(locked_collateral: u64, drawdown_slash: u64, late_penalty_bps: u16) -> Result<u64> {
    let penalty = (locked_collateral as u128)
        .checked_mul(late_penalty_bps as u128)
        .and_then(|v| v.checked_div(BPS_DENOMINATOR as u128))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or_else(|| error!(ErrorCode::Overflow))?;
    Ok(penalty.min(locked_collateral.saturating_sub(drawdown_slash)))
}

/// Moves everything out of a program-owned account and hands it back to the
/// system program, the way Anchor's `close` does.
fn close_account_to<'info>(account: &AccountInfo<'info>, destination: &AccountInfo<'info>) -> Result<()> {
    let lamports = account.lamports();
    **destination.try_borrow_mut_lamports()? += lamports;
    **account.try_borrow_mut_lamports()? = 0;
    account.assign(&system_program::ID);
    account.resize(0)?;
    Ok(())
}

pub fn handle_settle_position(ctx: Context<SettlePosition>, returned: u64) -> Result<()> {
    let position = &mut ctx.accounts.position;
    let agent = &mut ctx.accounts.agent;
    require!(
        matches!(position.status, PositionStatus::Open | PositionStatus::Trading),
        ErrorCode::InvalidStatus
    );

    let now = Clock::get()?.unix_timestamp;
    let declined = position.status == PositionStatus::Open;
    let late = !declined && now >= position.deadline;
    let executor = ctx.accounts.executor.key();
    let can_execute = agent.can_execute(&executor);

    let sys = ctx.accounts.system_program.to_account_info();
    let position_key = position.key();
    let position_seeds: &[&[u8]] =
        &[POSITION_VAULT_SEED, position_key.as_ref(), &[position.vault_bump]];
    let rent_floor = Rent::get()?.minimum_balance(0);

    let in_custody = !ctx.accounts.custody.data_is_empty();
    let late_penalty_bps;
    // Closed at the very end: moving lamports by hand before a CPI that
    // touches the same wallet makes the runtime see an unbalanced instruction.
    // (custody account, rent payer, lamports parked for the trader)
    let mut close_custody: Option<(AccountInfo, AccountInfo, u64)> = None;
    let effective_returned = if declined {
        // The agent never drew the funds: the principal is still in the vault
        // and counts as "returned in full" without the agent sending anything.
        require!(can_execute, ErrorCode::UnauthorizedExecutor);
        late_penalty_bps = 0;
        position.principal
    } else if in_custody {
        require!(
            can_execute || (late && executor == position.trader),
            ErrorCode::UnauthorizedExecutor
        );
        let custody_info = ctx.accounts.custody.to_account_info();
        let custody = Custody::try_deserialize(&mut &custody_info.try_borrow_data()?[..])?;
        require!(custody.position == position_key, ErrorCode::InvalidStatus);
        require!(custody.held_mask == 0, ErrorCode::VaultNotUnwound);
        let (Some(vault_wsol), Some(rent_payer), Some(token_program)) = (
            ctx.accounts.vault_wsol.as_ref(),
            ctx.accounts.rent_payer.as_ref(),
            ctx.accounts.token_program.as_ref(),
        ) else {
            return Err(error!(ErrorCode::CustodyAccountsMissing));
        };
        let vault_key = ctx.accounts.position_vault.key();
        require_keys_eq!(
            vault_wsol.key(),
            get_associated_token_address(&vault_key, &native_mint::ID),
            ErrorCode::VaultAccountTampered
        );
        require_keys_eq!(rent_payer.key(), custody.rent_payer, ErrorCode::UnauthorizedOperator);

        // Unwrap: closing the wSOL account sends its balance and its rent to
        // the vault. The rent goes back to whoever paid it; what is left above
        // the vault's own rent floor is what the agent returned.
        let wsol_rent = vault_wsol.to_account_info().lamports().saturating_sub(vault_wsol.amount);
        token::close_account(CpiContext::new_with_signer(
            token_program.key(),
            CloseAccount {
                account: vault_wsol.to_account_info(),
                destination: ctx.accounts.position_vault.to_account_info(),
                authority: ctx.accounts.position_vault.to_account_info(),
            },
            &[position_seeds],
        ))?;
        super::transfer_from_vault(
            &ctx.accounts.position_vault.to_account_info(),
            &rent_payer.to_account_info(),
            &sys,
            position_seeds,
            wsol_rent,
        )?;
        // The vault's rent floor, parked in the custody account, goes to the
        // trader with the payout; the custody rent goes to whoever paid it.
        // Both are moved by hand after every CPI (see `close_custody`).
        let custody_rent = Rent::get()?.minimum_balance(custody_info.data_len());
        let parked = custody_info.lamports().saturating_sub(custody_rent);
        close_custody = Some((custody_info, rent_payer.to_account_info(), parked));

        let config = ctx
            .accounts
            .config
            .as_ref()
            .ok_or_else(|| error!(ErrorCode::CustodyAccountsMissing))?;
        let (config_key, _) = Pubkey::find_program_address(&[CONFIG_SEED], ctx.program_id);
        require_keys_eq!(config.key(), config_key, ErrorCode::CustodyAccountsMissing);
        late_penalty_bps = config.late_penalty_bps;
        // The vault held nothing while trading, so everything it holds now
        // came out of the wSOL account.
        ctx.accounts.position_vault.lamports()
    } else {
        require!(can_execute, ErrorCode::UnauthorizedExecutor);
        // A custody position parks the vault's rent floor in its custody
        // account, so a client that does not know about custody cannot settle
        // one through this path.
        require!(
            ctx.accounts.position_vault.lamports() >= rent_floor,
            ErrorCode::CustodyAccountsMissing
        );
        late_penalty_bps = 0;
        super::transfer_from_signer(
            &ctx.accounts.executor.to_account_info(),
            &ctx.accounts.position_vault.to_account_info(),
            &sys,
            returned,
        )?;
        returned
    };

    let s = compute_settlement(
        position.principal,
        effective_returned,
        position.fee_bps,
        position.max_drawdown_bps,
        position.locked_collateral,
    )?;
    let penalty = if late && in_custody {
        late_penalty(position.locked_collateral, s.slash, late_penalty_bps)?
    } else {
        0
    };
    let slash = s.slash + penalty;

    // Pay the performance fee to the operator from the position vault. The
    // runtime rejects a transaction that leaves an account holding lamports
    // below its rent-exempt minimum, so a fee too small to lift an empty
    // operator wallet over it would block settlement on every retry. Such a
    // fee stays in the vault and goes to the trader with the rest.
    let operator = ctx.accounts.operator.to_account_info();
    let fee = if operator.key() == ctx.accounts.trader.key()
        || operator.lamports().saturating_add(s.fee)
            >= Rent::get()?.minimum_balance(operator.data_len())
    {
        s.fee
    } else {
        0
    };
    super::transfer_from_vault(
        &ctx.accounts.position_vault.to_account_info(),
        &operator,
        &sys,
        position_seeds,
        fee,
    )?;

    // Slash the agent's collateral to cover the shortfall and any late penalty.
    if slash > 0 {
        let agent_key = agent.key();
        let agent_seeds: &[&[u8]] = &[AGENT_VAULT_SEED, agent_key.as_ref(), &[agent.vault_bump]];
        super::transfer_from_vault(
            &ctx.accounts.agent_vault.to_account_info(),
            &ctx.accounts.trader.to_account_info(),
            &sys,
            agent_seeds,
            slash,
        )?;
        agent.total_collateral -= slash;
        agent.slashed_total = agent
            .slashed_total
            .checked_add(slash)
            .ok_or(ErrorCode::Overflow)?;
    }

    // Drain the rest of the position vault (returned - fee + rent floor) to the trader.
    let remaining = ctx.accounts.position_vault.lamports();
    super::transfer_from_vault(
        &ctx.accounts.position_vault.to_account_info(),
        &ctx.accounts.trader.to_account_info(),
        &sys,
        position_seeds,
        remaining,
    )?;

    // Release the guarantee.
    agent.locked_collateral -= position.locked_collateral;
    agent.capital_managed -= position.principal;
    agent.open_positions -= 1;
    // A decline is not a track record: only drawn positions count as settled.
    if !declined {
        agent.settled_positions += 1;
    }
    agent.fees_earned = agent
        .fees_earned
        .checked_add(fee)
        .ok_or(ErrorCode::Overflow)?;

    let breach = if late {
        Breach::MissedDeadline
    } else if slash > 0 {
        Breach::Drawdown
    } else {
        Breach::None
    };
    if breach != Breach::None {
        agent.breach_count += 1;
        emit!(BreachRecorded {
            agent: agent.key(),
            position: position_key,
            breach,
            slashed: slash,
        });
    }

    position.status = PositionStatus::Settled;
    position.breach = breach;
    position.returned = effective_returned;
    position.slashed = slash;
    position.fee_paid = fee;
    position.closed_at = now;

    if declined {
        emit!(PositionDeclined {
            position: position_key,
            agent: agent.key(),
            trader: position.trader,
            principal: position.principal,
        });
    }

    emit!(PositionClosed {
        position: position_key,
        agent: agent.key(),
        trader: position.trader,
        status: PositionStatus::Settled,
        breach,
        returned: effective_returned,
        slashed: slash,
        fee_paid: fee,
        trader_payout: effective_returned - fee + slash,
    });

    if let Some((custody, rent_payer, parked)) = close_custody {
        **ctx.accounts.trader.to_account_info().try_borrow_mut_lamports()? += parked;
        **custody.try_borrow_mut_lamports()? -= parked;
        close_account_to(&custody, &rent_payer)?;
    }
    Ok(())
}
