//! Deployment file produced by `contracts/script/Deploy.s.sol`. Services read it at startup, so
//! addresses and parameters come from one place and cannot drift from the contracts.

use std::path::Path;

use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};

use crate::math::RateParams;
use crate::model::{CollateralParams, PoolParams};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployedCollateral {
    pub name: String,
    pub address: Address,
    pub decimals: u8,
    pub bit_index: u8,
    pub liq_threshold_bps: u16,
    pub liq_bonus_bps: u16,
    pub feed: Address,
    /// Mock AMM the TWAP reads from. Used by the scenario to move spot prices.
    pub amm: Address,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    pub chain_id: u64,
    pub pool: Address,
    pub implementation: Address,
    pub oracle: Address,
    pub debt: Address,
    pub debt_decimals: u8,
    pub debt_feed: Address,
    pub reserve_factor: U256,
    pub rates: RateParams,
    pub collaterals: Vec<DeployedCollateral>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading deployment file: {0}")]
    Io(#[from] std::io::Error),
    #[error("parsing deployment file: {0}")]
    Json(#[from] serde_json::Error),
}

impl Deployment {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    /// Pool parameters the math needs. Collateral order follows the deployment file.
    pub fn pool_params(&self) -> PoolParams {
        PoolParams {
            debt_asset: self.debt,
            debt_decimals: self.debt_decimals,
            reserve_factor: self.reserve_factor,
            rates: self.rates,
            collaterals: self
                .collaterals
                .iter()
                .map(|c| CollateralParams {
                    asset: c.address,
                    bit_index: c.bit_index,
                    decimals: c.decimals,
                    liq_threshold_bps: c.liq_threshold_bps,
                    liq_bonus_bps: c.liq_bonus_bps,
                })
                .collect(),
        }
    }
}
