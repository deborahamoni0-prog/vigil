// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {FixedPoint} from "../src/libs/FixedPoint.sol";
import {InterestMath} from "../src/libs/InterestMath.sol";
import {SafeTransfer} from "../src/libs/SafeTransfer.sol";

/// @dev Exposes internal library functions so tests can call them directly.
contract FixedPointHarness {
    function wadMulDown(uint256 a, uint256 b) external pure returns (uint256) { return FixedPoint.wadMulDown(a, b); }
    function wadMulUp(uint256 a, uint256 b) external pure returns (uint256) { return FixedPoint.wadMulUp(a, b); }
    function wadDivDown(uint256 a, uint256 b) external pure returns (uint256) { return FixedPoint.wadDivDown(a, b); }
    function wadDivUp(uint256 a, uint256 b) external pure returns (uint256) { return FixedPoint.wadDivUp(a, b); }
    function rayMulDown(uint256 a, uint256 b) external pure returns (uint256) { return FixedPoint.rayMulDown(a, b); }
    function rayMulUp(uint256 a, uint256 b) external pure returns (uint256) { return FixedPoint.rayMulUp(a, b); }
    function rayDivDown(uint256 a, uint256 b) external pure returns (uint256) { return FixedPoint.rayDivDown(a, b); }
    function rayDivUp(uint256 a, uint256 b) external pure returns (uint256) { return FixedPoint.rayDivUp(a, b); }
    function toWadDown(uint256 a, uint8 d) external pure returns (uint256) { return FixedPoint.toWadDown(a, d); }
    function toWadUp(uint256 a, uint8 d) external pure returns (uint256) { return FixedPoint.toWadUp(a, d); }
    function fromWadDown(uint256 a, uint8 d) external pure returns (uint256) { return FixedPoint.fromWadDown(a, d); }
    function fromWadUp(uint256 a, uint8 d) external pure returns (uint256) { return FixedPoint.fromWadUp(a, d); }
}

contract InterestHarness {
    function annualBorrowRate(InterestMath.RateParams memory p, uint256 util) external pure returns (uint256) {
        return InterestMath.annualBorrowRate(p, util);
    }
    function utilization(uint256 debt, uint256 supply) external pure returns (uint256) {
        return InterestMath.utilization(debt, supply);
    }
    function compoundFactor(uint256 r, uint256 t) external pure returns (uint256) {
        return InterestMath.compoundFactor(r, t);
    }
    function ratePerSecond(uint256 annual) external pure returns (uint256) {
        return InterestMath.ratePerSecond(annual);
    }
}

contract SafeTransferHarness {
    function transfer(address token, address to, uint256 amount) external {
        SafeTransfer.safeTransfer(token, to, amount);
    }
    function balanceOf(address token, address who) external view returns (uint256) {
        return SafeTransfer.balanceOf(token, who);
    }
}
