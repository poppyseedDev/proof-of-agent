//! Reads Pyth pull-oracle `PriceUpdateV2` accounts without depending on the
//! receiver SDK, and compares the oracle value of a swap's two legs.
//!
//! Layout (borsh, after the 8-byte Anchor discriminator):
//!   write_authority: Pubkey
//!   verification_level: enum { Partial { num_signatures: u8 } = [0, n], Full = [1] }
//!   price_message: feed_id [u8; 32], price i64, conf u64, exponent i32,
//!                  publish_time i64, prev_publish_time i64, ema_price i64, ema_conf u64
//!   posted_slot: u64

use anchor_lang::prelude::*;

use crate::{constants::*, error::ErrorCode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OraclePrice {
    pub price: i64,
    pub exponent: i32,
    pub publish_time: i64,
}

fn read_i64(data: &[u8], at: usize) -> Result<i64> {
    let bytes: [u8; 8] = data
        .get(at..at + 8)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| error!(ErrorCode::OracleAccountInvalid))?;
    Ok(i64::from_le_bytes(bytes))
}

fn read_i32(data: &[u8], at: usize) -> Result<i32> {
    let bytes: [u8; 4] = data
        .get(at..at + 4)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| error!(ErrorCode::OracleAccountInvalid))?;
    Ok(i32::from_le_bytes(bytes))
}

/// Parses a price update for `feed_id`, requiring full verification and a
/// publish time within `max_age_secs` of `now`.
pub fn read_price_update(
    account: &AccountInfo,
    oracle_program: &Pubkey,
    feed_id: &[u8; 32],
    now: i64,
    max_age_secs: i64,
) -> Result<OraclePrice> {
    require_keys_eq!(*account.owner, *oracle_program, ErrorCode::OracleAccountInvalid);
    let data = account.try_borrow_data()?;
    require!(data.len() >= 8 + 32 + 1, ErrorCode::OracleAccountInvalid);
    require!(data[..8] == PRICE_UPDATE_V2_DISCRIMINATOR, ErrorCode::OracleAccountInvalid);
    let mut at = 8 + 32;
    let level_tag = data[at];
    at += 1;
    if level_tag == PRICE_VERIFICATION_FULL {
        // no payload
    } else if level_tag == 0 {
        // Partial { num_signatures }: not good enough to price a swap.
        return Err(error!(ErrorCode::OracleNotVerified));
    } else {
        return Err(error!(ErrorCode::OracleAccountInvalid));
    }
    let message_feed = data
        .get(at..at + 32)
        .ok_or_else(|| error!(ErrorCode::OracleAccountInvalid))?;
    require!(message_feed == feed_id, ErrorCode::OracleFeedMismatch);
    at += 32;
    let price = read_i64(&data, at)?;
    at += 8;
    let _conf = read_i64(&data, at)?;
    at += 8;
    let exponent = read_i32(&data, at)?;
    at += 4;
    let publish_time = read_i64(&data, at)?;
    require!(price > 0, ErrorCode::OraclePriceInvalid);
    require!(
        publish_time.saturating_add(max_age_secs) >= now,
        ErrorCode::OraclePriceStale
    );
    Ok(OraclePrice { price, exponent, publish_time })
}

fn pow10(n: u32) -> Result<u128> {
    10u128.checked_pow(n).ok_or_else(|| error!(ErrorCode::Overflow))
}

/// `amount × price / 10^decimals`: the oracle value of a token amount in the
/// price's own exponent. Rounded down for the input leg and up for the output
/// leg by the caller's choice of `round_up`.
fn value(amount: u64, price: i64, decimals: u8, round_up: bool) -> Result<u128> {
    let scaled = (amount as u128)
        .checked_mul(price as u128)
        .ok_or_else(|| error!(ErrorCode::Overflow))?;
    let div = pow10(decimals as u32)?;
    let mut v = scaled / div;
    if round_up && scaled % div != 0 {
        v += 1;
    }
    Ok(v)
}

/// Oracle values of both legs, brought to the same exponent so they compare
/// directly. The input leg is rounded down and the output leg up, so rounding
/// can never fail an honest swap.
pub fn leg_values(
    amount_in: u64,
    price_in: &OraclePrice,
    decimals_in: u8,
    amount_out: u64,
    price_out: &OraclePrice,
    decimals_out: u8,
) -> Result<(u128, u128)> {
    let mut v_in = value(amount_in, price_in.price, decimals_in, false)?;
    let mut v_out = value(amount_out, price_out.price, decimals_out, true)?;
    // A larger exponent means a coarser unit; scale that side up to the finer one.
    let diff = price_in.exponent as i64 - price_out.exponent as i64;
    if diff > 0 {
        v_in = v_in
            .checked_mul(pow10(diff as u32)?)
            .ok_or_else(|| error!(ErrorCode::Overflow))?;
    } else if diff < 0 {
        v_out = v_out
            .checked_mul(pow10((-diff) as u32)?)
            .ok_or_else(|| error!(ErrorCode::Overflow))?;
    }
    Ok((v_in, v_out))
}

/// `v_out ≥ v_in × (1 − max_deviation)`.
pub fn within_deviation(v_in: u128, v_out: u128, max_deviation_bps: u16) -> Result<bool> {
    let lhs = v_out
        .checked_mul(BPS_DENOMINATOR as u128)
        .ok_or_else(|| error!(ErrorCode::Overflow))?;
    let rhs = v_in
        .checked_mul((BPS_DENOMINATOR - max_deviation_bps as u64) as u128)
        .ok_or_else(|| error!(ErrorCode::Overflow))?;
    Ok(lhs >= rhs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(price: i64, exponent: i32) -> OraclePrice {
        OraclePrice { price, exponent, publish_time: 0 }
    }

    #[test]
    fn sol_to_usdc_at_oracle_price_is_within_zero_deviation() {
        // 1 SOL at $150 -> 150 USDC. SOL feed expo -8, USDC feed expo -8.
        let sol = p(150_0000_0000, -8);
        let usdc = p(1_0000_0000, -8);
        let (v_in, v_out) = leg_values(1_000_000_000, &sol, 9, 150_000_000, &usdc, 6).unwrap();
        assert_eq!(v_in, v_out);
        assert!(within_deviation(v_in, v_out, 0).unwrap());
        // One micro-USDC short fails at zero deviation but passes at 1 bp.
        let (v_in, v_out) = leg_values(1_000_000_000, &sol, 9, 149_999_999, &usdc, 6).unwrap();
        assert!(!within_deviation(v_in, v_out, 0).unwrap());
        assert!(within_deviation(v_in, v_out, 1).unwrap());
    }

    #[test]
    fn different_exponents_are_aligned() {
        let sol = p(150_000, -3); // $150.000
        let usdc = p(1_000_000_00, -8); // $1.00000000
        let (v_in, v_out) = leg_values(2_000_000_000, &sol, 9, 300_000_000, &usdc, 6).unwrap();
        assert_eq!(v_in, v_out);
        let (v_in, v_out) = leg_values(300_000_000, &usdc, 6, 2_000_000_000, &sol, 9).unwrap();
        assert_eq!(v_in, v_out);
    }

    #[test]
    fn deviation_bounds_the_loss() {
        let sol = p(100_0000_0000, -8);
        let usdc = p(1_0000_0000, -8);
        // 1 SOL -> 98 USDC is a 2% loss.
        let (v_in, v_out) = leg_values(1_000_000_000, &sol, 9, 98_000_000, &usdc, 6).unwrap();
        assert!(within_deviation(v_in, v_out, 200).unwrap());
        assert!(!within_deviation(v_in, v_out, 199).unwrap());
        // A gain always passes.
        let (v_in, v_out) = leg_values(1_000_000_000, &sol, 9, 101_000_000, &usdc, 6).unwrap();
        assert!(within_deviation(v_in, v_out, 0).unwrap());
    }
}
