// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {LendingPool} from "../../src/LendingPool.sol";

/// @dev Upgrade target for SC-03. It keeps the v1 layout and adds a version view. Its own new
///      state lives in a separate ERC-7201 namespace, so no existing slot moves.
contract LendingPoolV2 is LendingPool {
    /// @custom:storage-location erc7201:vigil.storage.LendingPoolV2
    struct V2Storage {
        uint256 upgradeMarker;
    }

    bytes32 private constant V2_STORAGE_LOCATION = keccak256("vigil.storage.LendingPoolV2");

    function version() external pure returns (uint256) {
        return 2;
    }

    function setUpgradeMarker(uint256 v) external onlyOwner {
        V2Storage storage s;
        bytes32 loc = V2_STORAGE_LOCATION;
        assembly {
            s.slot := loc
        }
        s.upgradeMarker = v;
    }
}
