// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ITwapSource} from "../interfaces/ITwapSource.sol";

/// @title MockAMM
/// @notice Spot price plus a cumulative-price observation log (SC-15).
/// @dev A new spot price only earns time weight from the moment it is recorded onwards.
///      A price pushed and consumed in the same block therefore has zero TWAP weight,
///      which is the property the TWAP-attack test relies on (SC-17).
contract MockAMM is ITwapSource {
    /// @dev Two storage slots per observation: timestamp and price share one, cumulative has its own.
    struct Observation {
        uint32 timestamp;
        uint224 price; // spot price valid from `timestamp` until the next observation, WAD
        uint256 cumulative; // price * seconds, accumulated up to `timestamp`
    }

    error InsufficientHistory();
    error ZeroWindow();

    Observation[] public observations;

    constructor(uint256 initialPriceWad) {
        observations.push(Observation(uint32(block.timestamp), uint224(initialPriceWad), 0));
    }

    function spot() external view returns (uint256) {
        return observations[observations.length - 1].price;
    }

    function setSpot(uint256 priceWad) external {
        uint256 last = observations.length - 1;
        if (observations[last].timestamp == block.timestamp) {
            // Same block: overwrite the price, accrue no time weight.
            observations[last].price = uint224(priceWad);
            return;
        }
        uint256 cum = _cumulativeAt(block.timestamp);
        observations.push(Observation(uint32(block.timestamp), uint224(priceWad), cum));
    }

    function twap(uint32 window) external view override returns (uint256) {
        if (window == 0) revert ZeroWindow();
        uint256 nowTs = block.timestamp;
        // _cumulativeAt reverts with InsufficientHistory when the window starts before the first observation.
        uint256 cumNow = _cumulativeAt(nowTs);
        uint256 cumPast = _cumulativeAt(nowTs - window);
        return (cumNow - cumPast) / window;
    }

    function _cumulativeAt(uint256 t) internal view returns (uint256) {
        for (uint256 i = observations.length; i > 0; i--) {
            Observation storage o = observations[i - 1];
            if (o.timestamp <= t) {
                return o.cumulative + o.price * (t - o.timestamp);
            }
        }
        revert InsufficientHistory();
    }
}
