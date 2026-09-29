use anchor_lang::{prelude::*, solana_program::bpf_loader_upgradeable};

use crate::{constants::*, error::ErrorCode, events::ConfigChanged, state::Config};

// The admin is whoever holds the program's upgrade authority, read from its
// ProgramData account on every call. Moving the upgrade authority (to a
// multisig, say) moves these powers with it. A program made immutable has no
// admin, and its config stays as it was.

#[derive(Accounts)]
pub struct InitConfig<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(
        init,
        payer = admin,
        space = 8 + Config::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump
    )]
    pub config: Account<'info, Config>,
    #[account(
        seeds = [crate::ID.as_ref()],
        bump,
        seeds::program = bpf_loader_upgradeable::ID,
        constraint = program_data.upgrade_authority_address == Some(admin.key()) @ ErrorCode::UnauthorizedAdmin,
    )]
    pub program_data: Account<'info, ProgramData>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetConfig<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        seeds = [crate::ID.as_ref()],
        bump,
        seeds::program = bpf_loader_upgradeable::ID,
        constraint = program_data.upgrade_authority_address == Some(admin.key()) @ ErrorCode::UnauthorizedAdmin,
    )]
    pub program_data: Account<'info, ProgramData>,
}

fn emit_changed(config: &Config) {
    emit!(ConfigChanged {
        paused: config.paused,
        max_position: config.max_position,
        max_agent_capital: config.max_agent_capital,
    });
}

pub fn handle_init_config(ctx: Context<InitConfig>, max_position: u64, max_agent_capital: u64) -> Result<()> {
    Config::validate_caps(max_position, max_agent_capital)?;
    let config = &mut ctx.accounts.config;
    config.paused = false;
    config.max_position = max_position;
    config.max_agent_capital = max_agent_capital;
    config.bump = ctx.bumps.config;
    emit_changed(config);
    Ok(())
}

pub fn handle_set_paused(ctx: Context<SetConfig>, paused: bool) -> Result<()> {
    let config = &mut ctx.accounts.config;
    config.paused = paused;
    emit_changed(config);
    Ok(())
}

/// New caps apply to new positions only. Positions already open keep their
/// principal even if it is above the new caps.
pub fn handle_set_caps(ctx: Context<SetConfig>, max_position: u64, max_agent_capital: u64) -> Result<()> {
    Config::validate_caps(max_position, max_agent_capital)?;
    let config = &mut ctx.accounts.config;
    config.max_position = max_position;
    config.max_agent_capital = max_agent_capital;
    emit_changed(config);
    Ok(())
}
