// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title FixedPoint
/// @notice WAD (1e18) and RAY (1e27) fixed-point math written for Vigil (SC-06).
/// @dev Every mul/div names its rounding direction (SC-07): the `Down` variants round
///      toward zero, the `Up` variants round away from zero. Callers pick the direction
///      that favours the protocol. No imports, by requirement.
library FixedPoint {
    uint256 internal constant WAD = 1e18;
    uint256 internal constant RAY = 1e27;
    uint256 internal constant BPS = 1e4;
    uint256 internal constant SECONDS_PER_YEAR = 365 days;

    error DivisionByZero();

    /// @dev floor(a * b / d)
    function mulDivDown(uint256 a, uint256 b, uint256 d) internal pure returns (uint256) {
        if (d == 0) revert DivisionByZero();
        return (a * b) / d;
    }

    /// @dev ceil(a * b / d)
    function mulDivUp(uint256 a, uint256 b, uint256 d) internal pure returns (uint256) {
        if (d == 0) revert DivisionByZero();
        uint256 p = a * b;
        return p == 0 ? 0 : (p - 1) / d + 1;
    }

    // ---- WAD ----

    function wadMulDown(uint256 a, uint256 b) internal pure returns (uint256) {
        return mulDivDown(a, b, WAD);
    }

    function wadMulUp(uint256 a, uint256 b) internal pure returns (uint256) {
        return mulDivUp(a, b, WAD);
    }

    function wadDivDown(uint256 a, uint256 b) internal pure returns (uint256) {
        return mulDivDown(a, WAD, b);
    }

    function wadDivUp(uint256 a, uint256 b) internal pure returns (uint256) {
        return mulDivUp(a, WAD, b);
    }

    // ---- RAY ----

    function rayMulDown(uint256 a, uint256 b) internal pure returns (uint256) {
        return mulDivDown(a, b, RAY);
    }

    function rayMulUp(uint256 a, uint256 b) internal pure returns (uint256) {
        return mulDivUp(a, b, RAY);
    }

    function rayDivDown(uint256 a, uint256 b) internal pure returns (uint256) {
        return mulDivDown(a, RAY, b);
    }

    function rayDivUp(uint256 a, uint256 b) internal pure returns (uint256) {
        return mulDivUp(a, RAY, b);
    }

    // ---- Decimal normalisation (SC-13) ----

    /// @notice Converts a token amount with `decimals` into WAD, rounding down.
    function toWadDown(uint256 amount, uint8 decimals) internal pure returns (uint256) {
        if (decimals == 18) return amount;
        if (decimals < 18) return amount * (10 ** (18 - decimals));
        return amount / (10 ** (decimals - 18));
    }

    /// @notice Converts a token amount with `decimals` into WAD, rounding up.
    function toWadUp(uint256 amount, uint8 decimals) internal pure returns (uint256) {
        if (decimals >= 18) {
            uint256 f = 10 ** (decimals - 18);
            return (amount + f - 1) / f;
        }
        return amount * (10 ** (18 - decimals));
    }

    /// @notice Converts a WAD amount into token units with `decimals`, rounding up.
    function fromWadUp(uint256 amount, uint8 decimals) internal pure returns (uint256) {
        if (decimals == 18) return amount;
        if (decimals < 18) {
            uint256 f = 10 ** (18 - decimals);
            return (amount + f - 1) / f;
        }
        return amount * (10 ** (decimals - 18));
    }

    /// @notice Converts a WAD amount into token units with `decimals`, rounding down.
    function fromWadDown(uint256 amount, uint8 decimals) internal pure returns (uint256) {
        if (decimals == 18) return amount;
        if (decimals < 18) return amount / (10 ** (18 - decimals));
        return amount * (10 ** (decimals - 18));
    }
}
