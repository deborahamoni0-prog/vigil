// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {FixedPoint} from "./FixedPoint.sol";

/// @title InterestMath
/// @notice Kinked utilisation rate model (SC-08) and per-second compounding (SC-06).
/// @dev All rates are RAY-scaled. Rates are annual in the parameter struct and converted
///      to per-second rates before compounding. Rounding always favours the protocol:
///      debt-side factors round up.
library InterestMath {
    using FixedPoint for uint256;

    struct RateParams {
        uint256 baseRate; // annual, RAY
        uint256 slope1; // annual, RAY, below optimal utilisation
        uint256 slope2; // annual, RAY, above optimal utilisation
        uint256 optimalUtil; // RAY, strictly between 0 and RAY
    }

    /// @notice Utilisation = debt / supply, capped at 100%. Zero supply gives zero utilisation.
    function utilization(uint256 totalDebt, uint256 totalSupply) internal pure returns (uint256) {
        if (totalSupply == 0) return 0;
        uint256 u = FixedPoint.rayDivDown(totalDebt, totalSupply);
        return u > FixedPoint.RAY ? FixedPoint.RAY : u;
    }

    /// @notice Annual borrow rate (RAY) for a utilisation level (RAY).
    function annualBorrowRate(RateParams memory p, uint256 util) internal pure returns (uint256) {
        if (util <= p.optimalUtil) {
            // base + slope1 * util / optimal
            return p.baseRate + FixedPoint.mulDivUp(p.slope1, util, p.optimalUtil);
        }
        // base + slope1 + slope2 * (util - optimal) / (1 - optimal)
        uint256 excess = util - p.optimalUtil;
        return p.baseRate + p.slope1
            + FixedPoint.mulDivUp(p.slope2, excess, FixedPoint.RAY - p.optimalUtil);
    }

    /// @notice Per-second rate (RAY), rounded up.
    function ratePerSecond(uint256 annualRate) internal pure returns (uint256) {
        return FixedPoint.mulDivUp(annualRate, 1, FixedPoint.SECONDS_PER_YEAR);
    }

    /// @notice Approximates (1 + r)^t with a 3-term binomial Taylor series, in RAY.
    /// @param rPerSec per-second rate, RAY
    /// @param t elapsed seconds
    /// @return factor RAY-scaled growth factor, rounded up
    /// @dev (1 + r)^t ~= 1 + t*r + t(t-1)/2 * r^2 + t(t-1)(t-2)/6 * r^3
    ///      Each term is rounded up, so the approximation never under-accrues debt.
    function compoundFactor(uint256 rPerSec, uint256 t) internal pure returns (uint256) {
        if (t == 0 || rPerSec == 0) return FixedPoint.RAY;

        uint256 term1 = rPerSec * t; // exact in RAY
        uint256 r2 = FixedPoint.rayMulUp(rPerSec, rPerSec);
        uint256 r3 = FixedPoint.rayMulUp(r2, rPerSec);

        uint256 k2 = t >= 2 ? (t * (t - 1)) / 2 : 0;
        uint256 k3 = t >= 3 ? (t * (t - 1) * (t - 2)) / 6 : 0;

        return FixedPoint.RAY + term1 + r2 * k2 + r3 * k3;
    }
}
