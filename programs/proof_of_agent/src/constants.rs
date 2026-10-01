use anchor_lang::prelude::*;

#[constant]
pub const AGENT_SEED: &[u8] = b"agent";
#[constant]
pub const AGENT_VAULT_SEED: &[u8] = b"agent_vault";
#[constant]
pub const POSITION_SEED: &[u8] = b"position";
#[constant]
pub const POSITION_VAULT_SEED: &[u8] = b"position_vault";
#[constant]
pub const CONFIG_SEED: &[u8] = b"config";
#[constant]
pub const CUSTODY_SEED: &[u8] = b"custody";

pub const BPS_DENOMINATOR: u64 = 10_000;

/// Minimum collateral an agent must guarantee per unit of capital (10%).
#[constant]
pub const MIN_COLLATERAL_RATIO_BPS: u16 = 1_000;
/// Maximum collateral ratio (100%: every deposited lamport is fully backed).
#[constant]
pub const MAX_COLLATERAL_RATIO_BPS: u16 = 10_000;
/// Maximum drawdown an operator may declare as tolerated trading loss (50%).
#[constant]
pub const MAX_DRAWDOWN_BPS: u16 = 5_000;

/// The fee an operator may charge is capped by its collateral:
/// fee_bps <= collateral_ratio_bps / FEE_CAP_DIVISOR.
/// 30% collateral -> at most 15% of profit, 100% collateral -> at most 50%.
#[constant]
pub const FEE_CAP_DIVISOR: u16 = 2;

/// Bounds for the trading window an operator may offer.
#[constant]
pub const MIN_POSITION_DURATION: i64 = 60; // 1 minute
#[constant]
pub const MAX_POSITION_DURATION: i64 = 90 * 24 * 60 * 60; // 90 days

pub const MAX_NAME_LEN: usize = 32;
pub const MAX_DESCRIPTION_LEN: usize = 128;
pub const MAX_RULES_LEN: usize = 512;
pub const MAX_ALLOWED_ASSETS: usize = 8;

/// DEX programs `execute_swap` may call, and mint -> oracle feed mappings,
/// both set in the protocol config.
pub const MAX_DEX_PROGRAMS: usize = 4;
pub const MAX_FEEDS: usize = 16;
/// Largest share of the locked bond a late settlement may cost (50%).
pub const MAX_LATE_PENALTY_BPS: u16 = 5_000;
/// Largest loss of oracle value one swap may be allowed to take (50%).
pub const MAX_SWAP_DEVIATION_BPS: u16 = 5_000;

/// Pyth receiver `PriceUpdateV2` account: Anchor discriminator and layout.
pub const PRICE_UPDATE_V2_DISCRIMINATOR: [u8; 8] = [34, 241, 35, 99, 157, 126, 244, 205];
/// Borsh tag of `VerificationLevel::Full`.
pub const PRICE_VERIFICATION_FULL: u8 = 1;

/// Account discriminators. Custom values keep v2 accounts distinct from the
/// v1 accounts that already exist on devnet under the same program id.
pub const AGENT_DISCRIMINATOR: &[u8] = b"CAagent2";
pub const POSITION_DISCRIMINATOR: &[u8] = b"CApos_v2";
pub const CONFIG_DISCRIMINATOR: &[u8] = b"CAconfig";
pub const CUSTODY_DISCRIMINATOR: &[u8] = b"CAcustdy";
