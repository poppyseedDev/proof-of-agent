use anchor_lang::{prelude::*, solana_program::bpf_loader_upgradeable};

use crate::{
    constants::*,
    error::ErrorCode,
    events::{ConfigChanged, TradingConfigChanged},
    state::{Config, FeedMapping},
};

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
    // No DEX, no oracle, no feeds: nothing can trade until set_trading_config.
    config.allowed_dex_programs = Vec::new();
    config.oracle_program = Pubkey::default();
    config.max_price_age_secs = 60;
    config.max_swap_deviation_bps = 0;
    config.late_penalty_bps = 0;
    config.feeds = Vec::new();
    emit_changed(config);
    Ok(())
}

/// What custody positions may trade through, and how a swap is priced.
pub fn handle_set_trading_config(
    ctx: Context<SetConfig>,
    allowed_dex_programs: Vec<Pubkey>,
    oracle_program: Pubkey,
    max_price_age_secs: i64,
    max_swap_deviation_bps: u16,
    late_penalty_bps: u16,
    feeds: Vec<FeedMapping>,
) -> Result<()> {
    Config::validate_trading(
        &allowed_dex_programs,
        max_price_age_secs,
        max_swap_deviation_bps,
        late_penalty_bps,
        &feeds,
    )?;
    let config = &mut ctx.accounts.config;
    config.allowed_dex_programs = allowed_dex_programs;
    config.oracle_program = oracle_program;
    config.max_price_age_secs = max_price_age_secs;
    config.max_swap_deviation_bps = max_swap_deviation_bps;
    config.late_penalty_bps = late_penalty_bps;
    config.feeds = feeds;
    emit!(TradingConfigChanged {
        allowed_dex_programs: config.allowed_dex_programs.clone(),
        oracle_program,
        max_price_age_secs,
        max_swap_deviation_bps,
        late_penalty_bps,
        feed_count: config.feeds.len() as u8,
    });
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
