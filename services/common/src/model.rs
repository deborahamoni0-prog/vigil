//! Position and pool model. `health_factor` and `preview_indexes` follow
//! `LendingPool._healthFactorPx` and `LendingPool._previewIndexes` step for step (RS-07).

use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};

use crate::math::{self, MathError, RateParams, BPS, WAD};

/// Maximum collateral assets, matching `LendingPool.MAX_COLLATERAL`.
pub const MAX_COLLATERAL: usize = 3;

/// Health factor when there is no debt, matching `LendingPool.HF_NO_DEBT`.
pub const HF_NO_DEBT: U256 = U256::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollateralParams {
    pub asset: Address,
    pub bit_index: u8,
    pub decimals: u8,
    pub liq_threshold_bps: u16,
    pub liq_bonus_bps: u16,
}

/// Static pool configuration, read from the deployment file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoolParams {
    pub debt_asset: Address,
    pub debt_decimals: u8,
    pub reserve_factor: U256,
    pub rates: RateParams,
    pub collaterals: Vec<CollateralParams>,
}

impl PoolParams {
    pub fn collateral_by_bit(&self, bit: u8) -> Option<&CollateralParams> {
        self.collaterals.iter().find(|c| c.bit_index == bit)
    }

    pub fn collateral_by_asset(&self, asset: Address) -> Option<&CollateralParams> {
        self.collaterals.iter().find(|c| c.asset == asset)
    }
}

/// Global accounting, as read from `LendingPool.totals()` and the stored indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolState {
    pub total_supply_scaled: U256,
    pub total_debt_scaled: U256,
    pub supply_index: U256,
    pub debt_index: U256,
    pub last_accrual: u64,
}

/// One borrower's position. Amounts are indexed by collateral bit index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub bitmap: u8,
    pub debt_scaled: U256,
    pub amounts: [U256; MAX_COLLATERAL],
}

/// Prices and indexes used to evaluate one block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    pub debt_index: U256,
    pub debt_price: U256,
    /// WAD price per collateral bit index.
    pub collateral_prices: [U256; MAX_COLLATERAL],
}

/// Mirrors `LendingPool._previewIndexes`. Returns (supply index, debt index).
pub fn preview_indexes(
    state: &PoolState,
    params: &PoolParams,
    now: u64,
) -> Result<(U256, U256), MathError> {
    let dt = now.saturating_sub(state.last_accrual);
    if dt == 0 {
        return Ok((state.supply_index, state.debt_index));
    }
    let debt_value = math::ray_mul_up(state.total_debt_scaled, state.debt_index)?;
    let supply_value = math::ray_mul_down(state.total_supply_scaled, state.supply_index)?;
    let util = math::utilization(debt_value, supply_value)?;
    let rate = math::rate_per_second(math::annual_borrow_rate(&params.rates, util)?)?;

    let new_debt_index = math::ray_mul_up(
        state.debt_index,
        math::compound_factor(rate, U256::from(dt))?,
    )?;
    let new_debt_value = math::ray_mul_up(state.total_debt_scaled, new_debt_index)?;
    let interest = new_debt_value - debt_value;
    let reserve_cut = math::ray_mul_down(interest, params.reserve_factor)?;

    let mut supply_index = state.supply_index;
    if !state.total_supply_scaled.is_zero() {
        let new_supply_value = supply_value + interest - reserve_cut;
        supply_index = math::ray_div_down(new_supply_value, state.total_supply_scaled)?;
    }
    Ok((supply_index, new_debt_index))
}

/// Mirrors `LendingPool._healthFactorPx`. Returns WAD, or `HF_NO_DEBT` when there is no debt.
pub fn health_factor(
    position: &Position,
    params: &PoolParams,
    quote: &Quote,
) -> Result<U256, MathError> {
    if position.debt_scaled.is_zero() {
        return Ok(HF_NO_DEBT);
    }
    let debt_units = math::ray_mul_up(position.debt_scaled, quote.debt_index)?;
    let debt_value = math::wad_mul_up(
        math::to_wad_up(debt_units, params.debt_decimals),
        quote.debt_price,
    )?;

    let mut weighted = U256::ZERO;
    for bit in 0..MAX_COLLATERAL {
        if position.bitmap & (1 << bit) == 0 {
            continue;
        }
        let amount = position.amounts[bit];
        if amount.is_zero() {
            continue;
        }
        let Some(c) = params.collateral_by_bit(bit as u8) else {
            continue;
        };
        let value_wad = math::wad_mul_down(
            math::to_wad_down(amount, c.decimals),
            quote.collateral_prices[bit],
        )?;
        weighted += (value_wad * U256::from(c.liq_threshold_bps)) / BPS;
    }
    math::wad_div_down(weighted, debt_value)
}

/// Debt in debt-asset units at the given index, rounded up (mirrors `debtOf`).
pub fn debt_units(position: &Position, debt_index: U256) -> Result<U256, MathError> {
    math::ray_mul_up(position.debt_scaled, debt_index)
}

pub fn is_liquidatable(hf: U256) -> bool {
    hf < WAD
}
