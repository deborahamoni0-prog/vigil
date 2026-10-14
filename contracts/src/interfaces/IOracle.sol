// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

interface IOracle {
    function getPrice(address asset) external view returns (uint256 priceWad);
    function isFrozen() external view returns (bool);
}
