// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {LendingPool} from "../src/LendingPool.sol";

/// @dev Not deployed. Declares the ERC-7201 struct as a state variable so `forge inspect` can
///      report its field order, offsets and types. The real slot is the namespaced one.
contract StorageProbe {
    LendingPool.PoolStorage internal probe;
}
