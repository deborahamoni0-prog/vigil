// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {FixedPoint} from "../libs/FixedPoint.sol";
import {IAggregatorV3} from "../interfaces/IAggregatorV3.sol";
import {ITwapSource} from "../interfaces/ITwapSource.sol";
import {IOracle} from "../interfaces/IOracle.sol";

/// @title OracleAdapter
/// @notice Chainlink-style spot feed cross-checked against a TWAP (SC-14, SC-15, SC-16).
/// @dev An asset is valid only when its feed answer is positive, fresh and within bounds,
///      and its TWAP exists and agrees with the feed within `maxDeviationBps`. If any asset
///      is invalid the whole oracle is Frozen. Frozen means the pool allows only repay and
///      add-collateral.
contract OracleAdapter is Ownable, IOracle {
    using FixedPoint for uint256;

    /// @dev Field order is chosen for storage packing: feed takes one slot; the TWAP source
    ///      and the small fields fill a second slot exactly; the bounds share a third.
    struct FeedConfig {
        IAggregatorV3 feed; // slot 0
        ITwapSource twapSource; // slot 1, with the fields below
        uint32 maxStaleness; // seconds
        uint32 twapWindow; // seconds
        uint16 maxDeviationBps;
        uint8 feedDecimals; // cached so reads avoid an extra external call
        bool configured;
        uint128 minPrice; // slot 2, WAD. Both bounds fit in 128 bits.
        uint128 maxPrice; // WAD
    }

    error OracleFrozen(address asset);
    error AssetNotConfigured(address asset);
    error InvalidBounds();

    mapping(address => FeedConfig) public configs;
    address[] public assets;

    constructor(address owner_) Ownable(owner_) {}

    function setFeed(
        address asset,
        IAggregatorV3 feed,
        ITwapSource twapSource,
        uint32 maxStaleness,
        uint32 twapWindow,
        uint256 minPrice,
        uint256 maxPrice,
        uint16 maxDeviationBps
    ) external onlyOwner {
        if (minPrice == 0 || maxPrice < minPrice) revert InvalidBounds();
        if (!configs[asset].configured) assets.push(asset);
        if (maxPrice > type(uint128).max) revert InvalidBounds();
        configs[asset] = FeedConfig({
            feed: feed,
            twapSource: twapSource,
            maxStaleness: maxStaleness,
            twapWindow: twapWindow,
            minPrice: uint128(minPrice),
            maxPrice: uint128(maxPrice),
            maxDeviationBps: maxDeviationBps,
            feedDecimals: feed.decimals(),
            configured: true
        });
    }

    /// @notice Returns the validated WAD price, or reverts with OracleFrozen.
    function getPrice(address asset) external view override returns (uint256) {
        if (!configs[asset].configured) revert AssetNotConfigured(asset);
        (uint256 price, bool ok) = _read(asset);
        if (!ok) revert OracleFrozen(asset);
        return price;
    }

    /// @notice True when any configured asset fails validation.
    function isFrozen() public view override returns (bool) {
        for (uint256 i; i < assets.length; i++) {
            (, bool ok) = _read(assets[i]);
            if (!ok) return true;
        }
        return false;
    }

    function assetCount() external view returns (uint256) {
        return assets.length;
    }

    /// @dev Returns (price, valid). Never reverts, so `isFrozen` can use it safely.
    function _read(address asset) internal view returns (uint256, bool) {
        FeedConfig storage cfg = configs[asset];

        (, int256 answer,, uint256 updatedAt,) = cfg.feed.latestRoundData();
        if (answer <= 0) return (0, false);
        if (updatedAt > block.timestamp || block.timestamp - updatedAt > cfg.maxStaleness) {
            return (0, false);
        }

        uint256 price = FixedPoint.toWadDown(uint256(answer), cfg.feedDecimals);
        if (price < cfg.minPrice || price > cfg.maxPrice) return (0, false);

        try cfg.twapSource.twap(cfg.twapWindow) returns (uint256 twap) {
            if (!_withinDeviation(price, twap, cfg.maxDeviationBps)) return (0, false);
        } catch {
            return (0, false);
        }

        return (price, true);
    }

    /// @dev |a - b| * BPS <= maxBps * a, using `a` (the feed price) as the reference.
    function _withinDeviation(uint256 a, uint256 b, uint256 maxBps) internal pure returns (bool) {
        uint256 diff = a > b ? a - b : b - a;
        return diff * FixedPoint.BPS <= maxBps * a;
    }
}
