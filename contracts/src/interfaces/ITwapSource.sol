// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @notice Time-weighted price source (SC-15). Price is WAD-denominated.
interface ITwapSource {
    function twap(uint32 window) external view returns (uint256);
}
