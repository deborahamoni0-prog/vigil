// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title SafeTransfer
/// @notice ERC-20 transfer wrappers written in inline assembly (SC-11).
/// @dev Accepts tokens that return no value (e.g. old USDT), tokens that return `true`,
///      and reverts on `false` or on a failed call. Balance reads are done with staticcall.
library SafeTransfer {
    error TransferFailed(address token);
    error BalanceQueryFailed(address token);

    function safeTransfer(address token, address to, uint256 amount) internal {
        _callOptionalReturn(token, abi.encodeWithSelector(0xa9059cbb, to, amount)); // transfer
    }

    function safeTransferFrom(address token, address from, address to, uint256 amount) internal {
        _callOptionalReturn(token, abi.encodeWithSelector(0x23b872dd, from, to, amount)); // transferFrom
    }

    function balanceOf(address token, address account) internal view returns (uint256 bal) {
        bytes memory data = abi.encodeWithSelector(0x70a08231, account); // balanceOf
        bool ok;
        assembly {
            ok := staticcall(gas(), token, add(data, 0x20), mload(data), 0x00, 0x20)
            bal := mload(0x00)
        }
        if (!ok) revert BalanceQueryFailed(token);
    }

    function _callOptionalReturn(address token, bytes memory data) private {
        // A call to an address with no code succeeds silently, so reject it explicitly.
        if (token.code.length == 0) revert TransferFailed(token);

        (bool ok, bytes memory ret) = token.call(data);
        if (!ok) {
            // Bubble the token's revert reason, so a reentrancy guard inside a hook is visible.
            if (ret.length > 0) {
                assembly {
                    revert(add(ret, 0x20), mload(ret))
                }
            }
            revert TransferFailed(token);
        }
        // Accept no return data, or a return value of exactly true.
        bool success = ret.length == 0 || (ret.length == 32 && abi.decode(ret, (bool)));
        if (!success) revert TransferFailed(token);
    }
}
