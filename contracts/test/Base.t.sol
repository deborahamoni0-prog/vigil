// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

import {LendingPool} from "../src/LendingPool.sol";
import {InterestMath} from "../src/libs/InterestMath.sol";
import {FixedPoint} from "../src/libs/FixedPoint.sol";
import {OracleAdapter} from "../src/oracle/OracleAdapter.sol";
import {MockERC20} from "../src/mocks/MockERC20.sol";
import {MockFeeOnTransferERC20} from "../src/mocks/MockFeeOnTransferERC20.sol";
import {MockNoReturnERC20} from "../src/mocks/MockNoReturnERC20.sol";
import {MockAggregator} from "../src/mocks/MockAggregator.sol";
import {MockAMM} from "../src/mocks/MockAMM.sol";
import {IAggregatorV3} from "../src/interfaces/IAggregatorV3.sol";
import {ITwapSource} from "../src/interfaces/ITwapSource.sol";

/// @notice Shared deployment for the protocol tests.
/// @dev Debt asset is a 6-decimal stablecoin. Collateral: a 18-decimal fee-on-transfer token
///      (1% fee), a 6-decimal token and an 8-decimal token (acceptance-scenario mix).
abstract contract BaseTest is Test {
    using FixedPoint for uint256;

    uint256 internal constant WAD = 1e18;
    uint256 internal constant RAY = 1e27;
    uint256 internal constant BPS = 1e4;

    uint256 internal constant RESERVE_FACTOR = 1e26; // 10%
    uint256 internal constant DUST = 1e6; // 1 stablecoin

    /// @dev TWAP window tuned to about two blocks. A 25% crash over 15 blocks moves roughly
    ///      1.7% per block, so a short window keeps honest moves inside the breaker band.
    uint32 internal constant TWAP_WINDOW = 2;
    uint16 internal constant MAX_DEVIATION_BPS = 1000; // 10%

    address internal owner = makeAddr("owner");
    address internal lender = makeAddr("lender");
    address internal alice = makeAddr("alice");
    address internal bob = makeAddr("bob");
    address internal liquidator = makeAddr("liquidator");

    MockERC20 internal debt;
    MockFeeOnTransferERC20 internal feeCol; // 18 decimals, 1% fee
    MockERC20 internal sixCol; // 6 decimals
    MockERC20 internal eightCol; // 8 decimals

    MockAggregator internal debtFeed;
    MockAggregator internal feeFeed;
    MockAggregator internal sixFeed;
    MockAggregator internal eightFeed;

    MockAMM internal debtAmm;
    MockAMM internal feeAmm;
    MockAMM internal sixAmm;
    MockAMM internal eightAmm;

    OracleAdapter internal oracle;
    LendingPool internal impl;
    LendingPool internal pool;

    InterestMath.RateParams internal rates = InterestMath.RateParams({
        baseRate: 2e25, // 2%
        slope1: 4e25, // 4%
        slope2: 75e25, // 75%
        optimalUtil: 8e26 // 80%
    });

    function setUp() public virtual {
        // Start well past zero so TWAP windows and staleness checks behave like production.
        vm.warp(1_700_000_000);

        debt = new MockERC20("Vigil USD", "vUSD", 6);
        feeCol = new MockFeeOnTransferERC20("Fee Token", "FEE", 18, 100);
        sixCol = new MockERC20("Six", "SIX", 6);
        eightCol = new MockERC20("Eight", "EIGHT", 8);

        // Chainlink-style feeds use 8 decimals. AMMs are WAD-denominated.
        debtFeed = new MockAggregator(8, 1e8);
        feeFeed = new MockAggregator(8, 2000e8);
        sixFeed = new MockAggregator(8, 2e8);
        eightFeed = new MockAggregator(8, 60_000e8);

        debtAmm = new MockAMM(1e18);
        feeAmm = new MockAMM(2000e18);
        sixAmm = new MockAMM(2e18);
        eightAmm = new MockAMM(60_000e18);

        oracle = new OracleAdapter(owner);
        vm.startPrank(owner);
        _configureFeed(address(debt), debtFeed, debtAmm, 1e18);
        _configureFeed(address(feeCol), feeFeed, feeAmm, 1e16);
        _configureFeed(address(sixCol), sixFeed, sixAmm, 1e16);
        _configureFeed(address(eightCol), eightFeed, eightAmm, 1e16);
        vm.stopPrank();

        impl = new LendingPool();
        bytes memory initData = abi.encodeCall(
            LendingPool.initialize, (owner, address(debt), address(oracle), rates, RESERVE_FACTOR, DUST)
        );
        pool = LendingPool(address(new ERC1967Proxy(address(impl), initData)));

        vm.startPrank(owner);
        pool.listCollateral(address(feeCol), 0, 8000, 500);
        pool.listCollateral(address(sixCol), 1, 8500, 500);
        pool.listCollateral(address(eightCol), 2, 7500, 1000);
        vm.stopPrank();

        // Let the TWAP window fill with history before any test acts.
        skip(1 hours);
        _refreshAll();
    }

    function _configureFeed(address asset, MockAggregator feed, MockAMM amm, uint256 minPrice) internal {
        oracle.setFeed(
            asset,
            IAggregatorV3(address(feed)),
            ITwapSource(address(amm)),
            7200, // max staleness
            TWAP_WINDOW,
            minPrice,
            1e30, // max price
            MAX_DEVIATION_BPS
        );
    }

    /// @dev Sets one asset's USD price (8-decimal feed answer) and the matching AMM spot.
    function _setUsd(MockAggregator feed, MockAMM amm, int256 usd8) internal {
        feed.setAnswer(usd8);
        amm.setSpot(uint256(usd8) * 1e10);
    }

    /// @dev Moves all collateral prices, then lets the TWAP window fill with the new level.
    ///      Without the skip, the TWAP still shows the old price and the breaker trips.
    function setPrices(int256 feeUsd8, int256 sixUsd8, int256 eightUsd8) internal {
        _setUsd(feeFeed, feeAmm, feeUsd8);
        _setUsd(sixFeed, sixAmm, sixUsd8);
        _setUsd(eightFeed, eightAmm, eightUsd8);
        skip(TWAP_WINDOW + 1);
        _refreshAll();
    }

    /// @dev Re-stamps every feed so staleness checks pass after a time skip.
    function _refreshAll() internal {
        debtFeed.setAnswer(debtFeed.answer());
        feeFeed.setAnswer(feeFeed.answer());
        sixFeed.setAnswer(sixFeed.answer());
        eightFeed.setAnswer(eightFeed.answer());
    }

    /// @dev Mints `amount` of `token` to `user` and approves the pool for it.
    function fund(MockERC20 token, address user, uint256 amount) internal {
        token.mint(user, amount);
        vm.prank(user);
        token.approve(address(pool), type(uint256).max);
    }

    function fundFee(address user, uint256 amount) internal {
        feeCol.mint(user, amount);
        vm.prank(user);
        feeCol.approve(address(pool), type(uint256).max);
    }

    function supplyLiquidity(uint256 amount) internal {
        fund(debt, lender, amount);
        vm.prank(lender);
        pool.supply(amount);
    }

    /// @dev Opens a position: deposits `collateral` of the 18-decimal fee token and borrows `borrowAmt`.
    function openFeePosition(address user, uint256 collateral, uint256 borrowAmt) internal {
        fundFee(user, collateral);
        vm.startPrank(user);
        pool.depositCollateral(address(feeCol), collateral);
        pool.borrow(borrowAmt);
        vm.stopPrank();
    }

    function hf(address user) internal view returns (uint256) {
        return pool.getHealthFactor(user);
    }
}
