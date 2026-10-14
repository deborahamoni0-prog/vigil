// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {MockERC20} from "./MockERC20.sol";

/// @notice Burns a fee on every transfer, so the recipient receives less than `amount`.
contract MockFeeOnTransferERC20 is MockERC20 {
    uint256 public immutable feeBps;

    constructor(string memory name_, string memory symbol_, uint8 decimals_, uint256 feeBps_)
        MockERC20(name_, symbol_, decimals_)
    {
        feeBps = feeBps_;
    }

    function _move(address from, address to, uint256 amount) internal override {
        uint256 fee = (amount * feeBps) / 10_000;
        balanceOf[from] -= amount;
        balanceOf[to] += amount - fee;
        totalSupply -= fee;
        emit Transfer(from, to, amount - fee);
    }
}
