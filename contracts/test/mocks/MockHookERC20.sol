// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {MockERC20} from "../../src/mocks/MockERC20.sol";

/// @dev Calls an arbitrary hook from inside transferFrom, to test reentrancy (SC-23, SC-24).
contract MockHookERC20 is MockERC20 {
    address public hookTarget;
    bytes public hookData;
    bool public hookOnTransferFrom;

    constructor() MockERC20("Hook", "HOOK", 18) {}

    function setHook(address target, bytes calldata data) external {
        hookTarget = target;
        hookData = data;
        hookOnTransferFrom = true;
    }

    function transferFrom(address from, address to, uint256 amount) external override returns (bool) {
        if (hookOnTransferFrom && hookTarget != address(0)) {
            (bool ok, bytes memory ret) = hookTarget.call(hookData);
            if (!ok) {
                assembly {
                    revert(add(ret, 0x20), mload(ret))
                }
            }
        }
        uint256 allowed = allowance[from][msg.sender];
        if (allowed != type(uint256).max) allowance[from][msg.sender] = allowed - amount;
        _move(from, to, amount);
        return true;
    }
}
