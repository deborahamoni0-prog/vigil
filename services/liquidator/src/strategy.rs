//! Strategies decide when to strike (RS-11). Behind a trait, so they are testable without a chain.

use alloy::primitives::U256;
use vigil_common::messages::LiquidationSignal;

/// Market facts a strategy may use. All amounts are in debt-asset units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketView {
    pub current_block: u64,
    /// Current liquidation bonus for this borrower, in bps (0 when no auction is running).
    pub bonus_bps: u64,
    /// Repayment the liquidator would make now (after the close factor).
    pub repay: U256,
    /// Estimated gas cost of the liquidation, in debt units.
    pub gas_cost_debt: U256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Strike { repay: U256 },
    Wait { until_block: u64 },
    Skip { reason: &'static str },
}

pub trait Strategy: Send + Sync {
    fn decide(&self, signal: &LiquidationSignal, market: &MarketView) -> Decision;
}

/// Strikes as soon as an auction exists and the bonus is positive.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImmediateStrike;

impl Strategy for ImmediateStrike {
    fn decide(&self, _signal: &LiquidationSignal, market: &MarketView) -> Decision {
        if market.bonus_bps == 0 {
            Decision::Wait {
                until_block: market.current_block + 1,
            }
        } else {
            Decision::Strike {
                repay: market.repay,
            }
        }
    }
}

/// Waits until the bonus, in debt units, covers the gas cost with a safety margin.
#[derive(Debug, Clone, Copy)]
pub struct WaitForMargin {
    /// Required coverage, in bps. 12_000 means the bonus must be 1.2x the gas cost.
    pub margin_bps: u64,
}

impl WaitForMargin {
    /// Bonus value (repay * bonus) in debt units. Returns None when there is no bonus yet.
    pub fn bonus_value(market: &MarketView) -> U256 {
        market.repay * U256::from(market.bonus_bps) / U256::from(10_000u64)
    }
}

impl Strategy for WaitForMargin {
    fn decide(&self, _signal: &LiquidationSignal, market: &MarketView) -> Decision {
        if market.repay.is_zero() {
            return Decision::Skip {
                reason: "nothing to repay",
            };
        }
        let bonus = Self::bonus_value(market);
        let needed = market.gas_cost_debt * U256::from(self.margin_bps) / U256::from(10_000u64);
        if bonus >= needed && !bonus.is_zero() {
            Decision::Strike {
                repay: market.repay,
            }
        } else {
            Decision::Wait {
                until_block: market.current_block + 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use alloy::primitives::Address;

    use super::*;

    fn signal() -> LiquidationSignal {
        LiquidationSignal {
            borrower: Address::ZERO,
            block_number: 10,
            health_factor: U256::from(9u64),
            trace_id: String::new(),
        }
    }

    fn market(bonus_bps: u64, repay: u64, gas: u64) -> MarketView {
        MarketView {
            current_block: 10,
            bonus_bps,
            repay: U256::from(repay),
            gas_cost_debt: U256::from(gas),
        }
    }

    #[test]
    fn immediate_waits_for_a_bonus_then_strikes() {
        let s = ImmediateStrike;
        assert_eq!(
            s.decide(&signal(), &market(0, 100, 1)),
            Decision::Wait { until_block: 11 }
        );
        assert_eq!(
            s.decide(&signal(), &market(50, 100, 1)),
            Decision::Strike {
                repay: U256::from(100)
            }
        );
    }

    #[test]
    fn margin_strategy_waits_until_bonus_beats_gas_with_margin() {
        let s = WaitForMargin { margin_bps: 12_000 };
        // repay 10_000 at 100 bps gives a bonus of 100. Gas 83 * 1.2 = 99.6, which is below 100.
        assert!(matches!(
            s.decide(&signal(), &market(100, 10_000, 83)),
            Decision::Strike { .. }
        ));
        // Gas 85 * 1.2 = 102, which is above 100, so wait. Values avoid the integer-floor boundary.
        assert!(matches!(
            s.decide(&signal(), &market(100, 10_000, 85)),
            Decision::Wait { .. }
        ));
    }

    #[test]
    fn margin_strategy_skips_when_nothing_to_repay() {
        let s = WaitForMargin { margin_bps: 12_000 };
        assert_eq!(
            s.decide(&signal(), &market(100, 0, 1)),
            Decision::Skip {
                reason: "nothing to repay"
            }
        );
    }
}
