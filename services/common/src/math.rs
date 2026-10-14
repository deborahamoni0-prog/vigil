//! Fixed-point math that mirrors `contracts/src/libs/FixedPoint.sol` and
//! `contracts/src/libs/InterestMath.sol` line for line (RS-07).
//!
//! Every function names its rounding direction. `*_down` floors and `*_up` ceils. Keep the
//! operation order identical to the Solidity, because rounding at each step is what makes the
//! two sides agree bit for bit.

use alloy::primitives::U256;

pub const WAD: U256 = U256::from_limbs([1_000_000_000_000_000_000, 0, 0, 0]);
pub const RAY: U256 = U256::from_limbs([0x9fd0803ce8000000, 0x33b2e3c, 0, 0]);
pub const BPS: U256 = U256::from_limbs([10_000, 0, 0, 0]);
pub const SECONDS_PER_YEAR: U256 = U256::from_limbs([31_536_000, 0, 0, 0]);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MathError {
    #[error("division by zero")]
    DivisionByZero,
    #[error("arithmetic overflow")]
    Overflow,
}

/// floor(a * b / d)
pub fn mul_div_down(a: U256, b: U256, d: U256) -> Result<U256, MathError> {
    if d.is_zero() {
        return Err(MathError::DivisionByZero);
    }
    let p = a.checked_mul(b).ok_or(MathError::Overflow)?;
    Ok(p / d)
}

/// ceil(a * b / d)
pub fn mul_div_up(a: U256, b: U256, d: U256) -> Result<U256, MathError> {
    if d.is_zero() {
        return Err(MathError::DivisionByZero);
    }
    let p = a.checked_mul(b).ok_or(MathError::Overflow)?;
    if p.is_zero() {
        return Ok(U256::ZERO);
    }
    Ok((p - U256::from(1)) / d + U256::from(1))
}

pub fn wad_mul_down(a: U256, b: U256) -> Result<U256, MathError> {
    mul_div_down(a, b, WAD)
}
pub fn wad_mul_up(a: U256, b: U256) -> Result<U256, MathError> {
    mul_div_up(a, b, WAD)
}
pub fn wad_div_down(a: U256, b: U256) -> Result<U256, MathError> {
    mul_div_down(a, WAD, b)
}
pub fn wad_div_up(a: U256, b: U256) -> Result<U256, MathError> {
    mul_div_up(a, WAD, b)
}
pub fn ray_mul_down(a: U256, b: U256) -> Result<U256, MathError> {
    mul_div_down(a, b, RAY)
}
pub fn ray_mul_up(a: U256, b: U256) -> Result<U256, MathError> {
    mul_div_up(a, b, RAY)
}
pub fn ray_div_down(a: U256, b: U256) -> Result<U256, MathError> {
    mul_div_down(a, RAY, b)
}
pub fn ray_div_up(a: U256, b: U256) -> Result<U256, MathError> {
    mul_div_up(a, RAY, b)
}

fn pow10(n: u32) -> U256 {
    U256::from(10u64).pow(U256::from(n))
}

/// Token amount with `decimals` into WAD, rounding down.
pub fn to_wad_down(amount: U256, decimals: u8) -> U256 {
    match decimals.cmp(&18) {
        std::cmp::Ordering::Equal => amount,
        std::cmp::Ordering::Less => amount * pow10(18 - decimals as u32),
        std::cmp::Ordering::Greater => amount / pow10(decimals as u32 - 18),
    }
}

/// Token amount with `decimals` into WAD, rounding up.
pub fn to_wad_up(amount: U256, decimals: u8) -> U256 {
    if decimals >= 18 {
        let f = pow10(decimals as u32 - 18);
        (amount + f - U256::from(1)) / f
    } else {
        amount * pow10(18 - decimals as u32)
    }
}

/// WAD into token units with `decimals`, rounding down.
pub fn from_wad_down(amount: U256, decimals: u8) -> U256 {
    match decimals.cmp(&18) {
        std::cmp::Ordering::Equal => amount,
        std::cmp::Ordering::Less => amount / pow10(18 - decimals as u32),
        std::cmp::Ordering::Greater => amount * pow10(decimals as u32 - 18),
    }
}

// ---- InterestMath ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateParams {
    pub base_rate: U256,
    pub slope1: U256,
    pub slope2: U256,
    pub optimal_util: U256,
}

/// Utilisation = debt / supply, capped at 100%. Zero supply gives zero.
pub fn utilization(total_debt: U256, total_supply: U256) -> Result<U256, MathError> {
    if total_supply.is_zero() {
        return Ok(U256::ZERO);
    }
    let u = ray_div_down(total_debt, total_supply)?;
    Ok(if u > RAY { RAY } else { u })
}

/// Annual borrow rate (RAY) at a utilisation level (RAY).
pub fn annual_borrow_rate(p: &RateParams, util: U256) -> Result<U256, MathError> {
    if util <= p.optimal_util {
        Ok(p.base_rate + mul_div_up(p.slope1, util, p.optimal_util)?)
    } else {
        let excess = util - p.optimal_util;
        Ok(p.base_rate + p.slope1 + mul_div_up(p.slope2, excess, RAY - p.optimal_util)?)
    }
}

/// Per-second rate (RAY), rounded up.
pub fn rate_per_second(annual: U256) -> Result<U256, MathError> {
    mul_div_up(annual, U256::from(1), SECONDS_PER_YEAR)
}

/// 3-term binomial Taylor approximation of (1 + r)^t, RAY-scaled, rounded up.
pub fn compound_factor(r_per_sec: U256, t: U256) -> Result<U256, MathError> {
    if t.is_zero() || r_per_sec.is_zero() {
        return Ok(RAY);
    }
    let term1 = r_per_sec.checked_mul(t).ok_or(MathError::Overflow)?;
    let r2 = ray_mul_up(r_per_sec, r_per_sec)?;
    let r3 = ray_mul_up(r2, r_per_sec)?;
    let one = U256::from(1);
    let two = U256::from(2);
    let three = U256::from(3);
    let k2 = if t >= two {
        t * (t - one) / two
    } else {
        U256::ZERO
    };
    let k3 = if t >= three {
        t * (t - one) * (t - two) / U256::from(6)
    } else {
        U256::ZERO
    };
    let sum = RAY
        .checked_add(term1)
        .and_then(|v| v.checked_add(r2.checked_mul(k2)?))
        .and_then(|v| v.checked_add(r3.checked_mul(k3)?))
        .ok_or(MathError::Overflow)?;
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(x: u128) -> U256 {
        U256::from(x)
    }

    #[test]
    fn constants_match_solidity() {
        assert_eq!(WAD, U256::from(10).pow(U256::from(18)));
        assert_eq!(RAY, U256::from(10).pow(U256::from(27)));
    }

    #[test]
    fn rounding_directions() {
        assert_eq!(wad_mul_down(u(3), u(3e17 as u128)).unwrap(), U256::ZERO);
        assert_eq!(wad_mul_up(u(3), u(3e17 as u128)).unwrap(), U256::from(1));
        assert_eq!(wad_div_down(u(1), u(3) * WAD).unwrap(), U256::ZERO);
        assert_eq!(wad_div_up(u(1), u(3) * WAD).unwrap(), U256::from(1));
        assert_eq!(ray_mul_up(U256::ZERO, RAY).unwrap(), U256::ZERO);
    }

    #[test]
    fn division_by_zero_is_an_error() {
        assert_eq!(
            wad_div_down(WAD, U256::ZERO),
            Err(MathError::DivisionByZero)
        );
    }

    #[test]
    fn decimal_normalisation() {
        assert_eq!(to_wad_down(u(1_000_000), 6), WAD);
        assert_eq!(to_wad_down(u(1_234_567), 6), u(1_234_567_000_000_000_000));
        assert_eq!(to_wad_down(u(5), 18), u(5));
        assert_eq!(from_wad_down(u(1_500_000_000_000_000_001), 6), u(1_500_000));
        assert_eq!(to_wad_up(u(1), 24), u(1));
        assert_eq!(to_wad_down(u(1), 24), U256::ZERO);
        assert_eq!(to_wad_up(u(1), 8), u(10_000_000_000));
    }

    #[test]
    fn kinked_rate_matches_contract_vectors() {
        let p = RateParams {
            base_rate: u(2e25 as u128),
            slope1: u(4e25 as u128),
            slope2: u(75e25 as u128),
            optimal_util: u(8e26 as u128),
        };
        assert_eq!(annual_borrow_rate(&p, U256::ZERO).unwrap(), p.base_rate);
        assert_eq!(
            annual_borrow_rate(&p, p.optimal_util).unwrap(),
            p.base_rate + p.slope1
        );
        assert_eq!(
            annual_borrow_rate(&p, RAY).unwrap(),
            p.base_rate + p.slope1 + p.slope2
        );
    }

    #[test]
    fn compound_one_year_at_ten_percent_is_close_to_e_pow_0_1() {
        let r = rate_per_second(u(1e26 as u128)).unwrap();
        let f = compound_factor(r, u(31_536_000)).unwrap();
        // e^0.1 = 1.105170918075648 (RAY). A 3-term series at 10% a year is off by about 4e-6
        // relative, since the next term is (rt)^4 / 24. Bound the error at 1e-5, which is the
        // approximation's true limit. Parity with the contract is tested separately.
        let expected = u(1_105_170_918_075_648_000_000_000_000);
        let diff = if f > expected {
            f - expected
        } else {
            expected - f
        };
        assert!(
            diff * U256::from(100_000) < expected,
            "factor {f} too far from e^0.1"
        );
    }
}
