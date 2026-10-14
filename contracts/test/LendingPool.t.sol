// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {BaseTest} from "./Base.t.sol";
import {Vm} from "forge-std/Vm.sol";
import {LendingPool} from "../src/LendingPool.sol";
import {OracleAdapter} from "../src/oracle/OracleAdapter.sol";
import {IAggregatorV3} from "../src/interfaces/IAggregatorV3.sol";
import {ITwapSource} from "../src/interfaces/ITwapSource.sol";
import {InterestMath} from "../src/libs/InterestMath.sol";
import {MockAggregator} from "../src/mocks/MockAggregator.sol";
import {MockAMM} from "../src/mocks/MockAMM.sol";
import {MockHookERC20} from "./mocks/MockHookERC20.sol";
import {LendingPoolV2} from "./mocks/LendingPoolV2.sol";

contract LendingPoolTest is BaseTest {
    // ---------- helpers ----------

    function signIntent(uint256 pk, LendingPool.LiquidationIntent memory intent) internal view returns (bytes memory) {
        bytes32 digest = pool.hashIntent(intent);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, digest);
        return abi.encodePacked(r, s, v);
    }

    function intentFor(address borrower, uint256 maxRepay, uint256 minOut, uint256 nonce)
        internal
        view
        returns (LendingPool.LiquidationIntent memory)
    {
        return LendingPool.LiquidationIntent({
            borrower: borrower,
            collateralAsset: address(feeCol),
            maxRepay: maxRepay,
            minCollateralOut: minOut,
            nonce: nonce,
            deadline: block.timestamp + 1 days
        });
    }

    /// @dev Borrower with a known key, holding 10 fee tokens (9.9 after the 1% fee) borrowing 10,000 vUSD.
    function _unhealthyBorrower() internal returns (address who, uint256 pk) {
        (who, pk) = makeAddrAndKey("borrowerKey");
        supplyLiquidity(1_000_000e6);
        openFeePosition(who, 10e18, 10_000e6);
        // Fee token falls to $1200: weighted collateral 9.9 * 1200 * 80% = 9504 < 10000 debt.
        setPrices(1200e8, 2e8, 60_000e8);
    }

    // ============================================================
    // SC-01: UUPS proxy, initializers disabled on the implementation
    // ============================================================

    function test_SC01_implementationCannotBeInitialized() public {
        vm.expectRevert(Initializable.InvalidInitialization.selector);
        impl.initialize(owner, address(debt), address(oracle), rates, RESERVE_FACTOR, DUST);
    }

    function test_SC01_proxyCannotBeReinitialized() public {
        vm.expectRevert(Initializable.InvalidInitialization.selector);
        pool.initialize(owner, address(debt), address(oracle), rates, RESERVE_FACTOR, DUST);
    }

    function test_SC01_onlyOwnerCanUpgrade() public {
        LendingPool next = new LendingPool();
        vm.prank(alice);
        vm.expectRevert();
        pool.upgradeToAndCall(address(next), "");
    }

    // ============================================================
    // SC-03: upgrade v1 -> v2 preserves every balance
    // ============================================================

    function test_SC03_upgradePreservesFiftyPositions() public {
        supplyLiquidity(10_000_000e6);
        address[] memory users = new address[](50);
        for (uint256 i; i < 50; i++) {
            users[i] = makeAddr(string.concat("user", vm.toString(i)));
            openFeePosition(users[i], (10 + i) * 1e18, (1000 + i) * 1e6);
        }

        uint256[] memory debts = new uint256[](50);
        uint256[] memory colls = new uint256[](50);
        for (uint256 i; i < 50; i++) {
            debts[i] = pool.debtOf(users[i]);
            colls[i] = pool.collateralOf(users[i], address(feeCol));
        }
        uint256 supplyBefore = pool.supplyOf(lender);

        LendingPoolV2 v2 = new LendingPoolV2();
        vm.prank(owner);
        pool.upgradeToAndCall(address(v2), "");

        assertEq(LendingPoolV2(address(pool)).version(), 2);
        assertEq(pool.supplyOf(lender), supplyBefore);
        for (uint256 i; i < 50; i++) {
            assertEq(pool.debtOf(users[i]), debts[i], "debt changed");
            assertEq(pool.collateralOf(users[i], address(feeCol)), colls[i], "collateral changed");
        }
    }

    // ============================================================
    // SC-04: pause, and unpause only when the oracle is Normal
    // ============================================================

    function test_SC04_pauseBlocksUserActions() public {
        vm.prank(owner);
        pool.pause();
        fund(debt, lender, 1000e6);
        vm.prank(lender);
        vm.expectRevert(LendingPool.ProtocolPaused.selector);
        pool.supply(1000e6);
    }

    function test_SC04_unpauseRequiresNormalOracle() public {
        vm.prank(owner);
        pool.pause();

        feeFeed.setUpdatedAt(0); // stale -> Frozen
        vm.prank(owner);
        vm.expectRevert(LendingPool.OracleNotNormal.selector);
        pool.unpause();

        _refreshAll();
        vm.prank(owner);
        pool.unpause();
        assertFalse(pool.isPaused());
    }

    // ============================================================
    // SC-05: scaled balances and index growth
    // ============================================================

    function test_SC05_debtGrowsWithIndexNotStoredBalance() public {
        supplyLiquidity(1_000_000e6);
        openFeePosition(alice, 10e18, 10_000e6);
        (, uint256 scaledBefore) = pool.positionOf(alice);

        skip(365 days);
        _refreshAll();
        (, uint256 scaledAfter) = pool.positionOf(alice);

        assertEq(scaledAfter, scaledBefore, "scaled balance must not change with time");
        assertGt(pool.debtOf(alice), 10_000e6, "debt must accrue interest");
    }

    function test_SC05_lenderEarnsInterest() public {
        supplyLiquidity(1_000_000e6);
        openFeePosition(alice, 1000e18, 500_000e6); // ~50% utilisation, so the lender earns
        skip(365 days);
        _refreshAll();
        assertGt(pool.supplyOf(lender), 1_000_000e6);
    }

    // ============================================================
    // SC-09: enabled collateral is a bitmap; the health loop walks set bits only
    // ============================================================

    function test_SC09_gasScalesWithEnabledBits() public {
        supplyLiquidity(1_000_000e6);
        address one = makeAddr("one");
        address three = makeAddr("three");

        openFeePosition(one, 10e18, 1000e6);

        fund(sixCol, three, 1000e6);
        fund(eightCol, three, 1e8);
        vm.startPrank(three);
        pool.depositCollateral(address(sixCol), 1000e6);
        pool.depositCollateral(address(eightCol), 1e8);
        vm.stopPrank();
        fundFee(three, 10e18);
        vm.startPrank(three);
        pool.depositCollateral(address(feeCol), 10e18);
        pool.borrow(1000e6);
        vm.stopPrank();

        (uint256 bitmapOne,) = pool.positionOf(one);
        (uint256 bitmapThree,) = pool.positionOf(three);
        assertEq(bitmapOne, 1);
        assertEq(bitmapThree, 7);

        uint256 g = gasleft();
        pool.getHealthFactor(one);
        uint256 gasOne = g - gasleft();

        g = gasleft();
        pool.getHealthFactor(three);
        uint256 gasThree = g - gasleft();

        assertGt(gasThree, gasOne);
    }

    // ============================================================
    // SC-10: borrow, withdraw and disable-collateral revert below HF 1
    // ============================================================

    function test_SC10_borrowRevertsBelowHealthOne() public {
        supplyLiquidity(1_000_000e6);
        fundFee(alice, 10e18);
        vm.startPrank(alice);
        pool.depositCollateral(address(feeCol), 10e18);
        // 9.9 * 2000 * 80% = 15840 weighted; borrowing 16000 is over the line.
        vm.expectRevert();
        pool.borrow(16_000e6);
        vm.stopPrank();
    }

    function test_SC10_withdrawRevertsBelowHealthOne() public {
        supplyLiquidity(1_000_000e6);
        openFeePosition(alice, 10e18, 10_000e6);
        vm.prank(alice);
        vm.expectRevert();
        pool.withdrawCollateral(address(feeCol), 5e18);
    }

    function test_SC10_disableCollateralRevertsBelowHealthOne() public {
        supplyLiquidity(1_000_000e6);
        openFeePosition(alice, 10e18, 10_000e6);
        vm.prank(alice);
        vm.expectRevert();
        pool.disableCollateral(address(feeCol));
    }

    // ============================================================
    // SC-12: fee-on-transfer credits the measured delta
    // ============================================================

    function test_SC12_creditsMeasuredBalanceDelta() public {
        fundFee(alice, 10e18);
        vm.prank(alice);
        pool.depositCollateral(address(feeCol), 10e18);
        // 1% fee on transfer: the pool receives 9.9, not the 10 that was passed in.
        assertEq(pool.collateralOf(alice, address(feeCol)), 9.9e18);
    }

    // ============================================================
    // SC-13: health factor is exact across 6, 8 and 18-decimal collateral
    // ============================================================

    function test_SC13_healthFactorAcrossDecimals() public {
        supplyLiquidity(1_000_000e6);
        fund(sixCol, alice, 1000e6);
        fund(eightCol, alice, 1e8);
        vm.startPrank(alice);
        pool.depositCollateral(address(sixCol), 1000e6); // $2000, 85%  -> 1700
        pool.depositCollateral(address(eightCol), 1e8); // $60000, 75% -> 45000
        pool.borrow(10_000e6);
        vm.stopPrank();

        // Weighted 46700 / debt 10000 = 4.67. The debt rounds up by a unit of the 6-decimal token,
        // so the value sits a hair above 4.67 (debt rounds up, the protocol's favour).
        assertApproxEqRel(pool.getHealthFactor(alice), 4.67e18, 1e10);
    }

    // ============================================================
    // SC-14: staleness, zero/negative answers and bounds freeze the oracle
    // ============================================================

    function test_SC14_staleFeedFreezesOracle() public {
        feeFeed.setUpdatedAt(block.timestamp - 7201);
        assertTrue(oracle.isFrozen());
        vm.expectRevert(abi.encodeWithSelector(OracleAdapter.OracleFrozen.selector, address(feeCol)));
        pool.getPrice(address(feeCol));
    }

    function test_SC14_nonPositiveAnswerFreezesOracle() public {
        feeFeed.setAnswer(0);
        assertTrue(oracle.isFrozen());
        feeFeed.setAnswer(-5);
        assertTrue(oracle.isFrozen());
    }

    function test_SC14_outOfBoundsFreezesOracle() public {
        // maxPrice is 1e30 WAD = $1e22 in 8-decimal terms. Answer past the cap.
        feeFeed.setAnswer(int256(1e30) / 1e10 + 1);
        feeAmm.setSpot(uint256(int256(1e30) / 1e10 + 1) * 1e10);
        assertTrue(oracle.isFrozen());
    }

    function test_SC14_healthyOracleIsNormal() public view {
        assertFalse(oracle.isFrozen());
        assertEq(pool.getPrice(address(feeCol)), 2000e18);
    }

    // ============================================================
    // SC-15: TWAP is the time-weighted average of spot prices
    // ============================================================

    function test_SC15_twapAveragesSpotOverWindow() public {
        // Feed and AMM both start at $2000. Move the AMM to $3000 for the last 300s of a 600s window.
        // Spot is $2000 before t1 and $3000 after. A 2s window straddling t1 averages to 2500.
        feeAmm.setSpot(3000e18);
        skip(1);
        assertEq(feeAmm.twap(2), 2500e18);
    }

    function test_SC15_twapRevertsWithoutHistory() public {
        MockAMM fresh = new MockAMM(1e18);
        vm.expectRevert(MockAMM.InsufficientHistory.selector);
        fresh.twap(2);
    }

    // ============================================================
    // SC-16: Frozen permits only repay and add-collateral
    // ============================================================

    function test_SC16_frozenOracleBlocksBorrowAndLiquidationsButNotRepay() public {
        supplyLiquidity(1_000_000e6);
        openFeePosition(alice, 10e18, 1000e6);

        // Push the AMM 30% away from the feed for the whole window, so TWAP and feed disagree.
        feeAmm.setSpot(2600e18);
        skip(TWAP_WINDOW + 1);
        _refreshAll();
        assertTrue(oracle.isFrozen());

        vm.prank(alice);
        vm.expectRevert(LendingPool.OracleNotNormal.selector);
        pool.borrow(1e6);

        fund(debt, alice, 100e6);
        vm.prank(alice);
        pool.repay(100e6);

        fundFee(alice, 1e18);
        vm.prank(alice);
        pool.depositCollateral(address(feeCol), 1e18);

        vm.prank(alice);
        vm.expectRevert(LendingPool.OracleNotNormal.selector);
        pool.withdrawCollateral(address(feeCol), 1e18);
    }

    // ============================================================
    // SC-17: TWAP-manipulation attacker fails
    // ============================================================

    function test_SC17_sameBlockSpotPushDoesNotMoveTwapOrLiquidate() public {
        supplyLiquidity(1_000_000e6);
        openFeePosition(alice, 10e18, 10_000e6); // healthy: 15840 / 10000

        // Attacker crashes the AMM spot to $100 in the same block and tries to force a liquidation.
        address attacker = makeAddr("attacker");
        feeAmm.setSpot(100e18);

        assertFalse(oracle.isFrozen(), "one-block spot move must not trip the breaker");
        assertEq(pool.getPrice(address(feeCol)), 2000e18, "price must still come from the feed");
        assertGt(pool.getHealthFactor(alice), 1e18);

        vm.prank(attacker);
        vm.expectRevert(LendingPool.PositionHealthy.selector);
        pool.startAuction(alice);
    }

    function test_SC17_sustainedManipulationTripsBreaker() public {
        // The defence is a breaker, not invisibility. A manipulated price sustained across the window
        // makes TWAP and feed disagree, so the oracle freezes.
        feeAmm.setSpot(1000e18);
        skip(TWAP_WINDOW + 1);
        _refreshAll();
        assertTrue(oracle.isFrozen());
    }

    // ============================================================
    // SC-18: linear Dutch auction, resets when the position becomes healthy
    // ============================================================

    function test_SC18_bonusRisesLinearlyAndCaps() public {
        (address who,) = _unhealthyBorrower();
        vm.prank(liquidator);
        pool.startAuction(who);
        uint256 start = pool.auctionStartOf(who);
        assertEq(start, block.number);

        // Use absolute block targets: block.number can be cached across vm.roll within one call.
        assertEq(pool.bonusBpsAt(who, address(feeCol)), 0);
        vm.roll(start + 5);
        assertEq(pool.bonusBpsAt(who, address(feeCol)), 250); // half of 500
        vm.roll(start + 10);
        assertEq(pool.bonusBpsAt(who, address(feeCol)), 500);
        vm.roll(start + 60);
        assertEq(pool.bonusBpsAt(who, address(feeCol)), 500, "bonus must cap at the maximum");
    }

    function test_SC18_auctionResetsWhenHealthy() public {
        (address who,) = _unhealthyBorrower();
        vm.prank(liquidator);
        pool.startAuction(who);

        // Collateral recovers, so the position is healthy again.
        setPrices(2000e8, 2e8, 60_000e8);
        pool.resetAuction(who);
        assertEq(pool.auctionStartOf(who), 0);
    }

    function test_SC18_cannotStartAuctionOnHealthyPosition() public {
        supplyLiquidity(1_000_000e6);
        openFeePosition(alice, 10e18, 10_000e6);
        vm.prank(liquidator);
        vm.expectRevert(LendingPool.PositionHealthy.selector);
        pool.startAuction(alice);
    }

    // ============================================================
    // SC-19: close factor at most 50%, unless the remainder would be dust
    // ============================================================

    function test_SC19_closeFactorCapsRepayAtHalfTheDebt() public {
        (address who, uint256 pk) = _unhealthyBorrower();
        vm.prank(liquidator);
        pool.startAuction(who);
        vm.roll(block.number + 10);

        fund(debt, liquidator, 1_000_000e6);
        vm.prank(liquidator);
        debt.approve(address(pool), type(uint256).max);

        LendingPool.LiquidationIntent memory intent = intentFor(who, type(uint256).max, 0, 1);
        bytes memory sig = signIntent(pk, intent);
        vm.prank(liquidator);
        pool.liquidate(intent, sig, type(uint256).max);

        // Tolerance covers the few seconds of interest that accrue during the setup.
        assertApproxEqAbs(pool.debtOf(who), 5000e6, 100, "half the debt should remain");
    }

    function test_SC19_dustRemainderLiftsCloseFactor() public {
        (address who, uint256 pk) = makeAddrAndKey("dustBorrower");
        supplyLiquidity(1_000_000e6);
        // 1.8 tokens are worth $3600 at first, so borrowing 1.5 vUSD is easy.
        openFeePosition(who, 18e17, 1_500_000);
        // At $1 the credited 1.782 tokens weigh 1.4256 < 1.5 debt, so the position is unhealthy.
        // They still cover 1.5 * 1.05 = 1.575, so the whole debt can be repaid.
        setPrices(1e8, 2e8, 60_000e8);

        vm.prank(liquidator);
        pool.startAuction(who);
        vm.roll(block.number + 10);

        fund(debt, liquidator, 10e6);
        LendingPool.LiquidationIntent memory intent = intentFor(who, type(uint256).max, 0, 1);
        bytes memory sig = signIntent(pk, intent);
        vm.prank(liquidator);
        pool.liquidate(intent, sig, type(uint256).max);

        // 50% of 1.5 is 0.75, and the 0.75 left is under the 1 vUSD dust threshold, so the full debt goes.
        assertEq(pool.debtOf(who), 0);
    }

    // ============================================================
    // SC-20: EIP-712 intents, Permit2-style bitmap nonces, deadlines
    // ============================================================

    function test_SC20_forgedSignatureRejected() public {
        (address who,) = _unhealthyBorrower();
        (, uint256 wrongPk) = makeAddrAndKey("impostor");
        vm.prank(liquidator);
        pool.startAuction(who);
        vm.roll(block.number + 10);

        LendingPool.LiquidationIntent memory intent = intentFor(who, 1000e6, 0, 1);
        bytes memory sig = signIntent(wrongPk, intent);
        vm.prank(liquidator);
        vm.expectRevert(LendingPool.InvalidSignature.selector);
        pool.liquidate(intent, sig, 1000e6);
    }

    function test_SC20_expiredIntentRejected() public {
        (address who, uint256 pk) = _unhealthyBorrower();
        vm.prank(liquidator);
        pool.startAuction(who);
        vm.roll(block.number + 10);

        LendingPool.LiquidationIntent memory intent = intentFor(who, 1000e6, 0, 1);
        intent.deadline = block.timestamp - 1;
        bytes memory sig = signIntent(pk, intent);
        vm.prank(liquidator);
        vm.expectRevert(LendingPool.IntentExpired.selector);
        pool.liquidate(intent, sig, 1000e6);
    }

    function test_SC20_nonceCannotBeReplayed() public {
        (address who, uint256 pk) = _unhealthyBorrower();
        vm.prank(liquidator);
        pool.startAuction(who);
        vm.roll(block.number + 10);

        fund(debt, liquidator, 1_000_000e6);
        LendingPool.LiquidationIntent memory intent = intentFor(who, 1000e6, 0, 7);
        bytes memory sig = signIntent(pk, intent);

        vm.prank(liquidator);
        pool.liquidate(intent, sig, 1000e6);
        assertTrue(pool.nonceUsed(who, 7));

        // Drop the price again so the position is liquidatable, then replay the same nonce.
        setPrices(300e8, 2e8, 60_000e8);
        vm.prank(liquidator);
        vm.expectRevert(LendingPool.NonceUsed.selector);
        pool.liquidate(intent, sig, 1000e6);
    }

    function test_SC20_nonceBitmapIsUnordered() public {
        (address who, uint256 pk) = _unhealthyBorrower();
        vm.prank(liquidator);
        pool.startAuction(who);
        vm.roll(block.number + 10);
        fund(debt, liquidator, 1_000_000e6);

        // A high nonce is accepted before a lower one. There is no ordering requirement.
        LendingPool.LiquidationIntent memory hi = intentFor(who, 100e6, 0, 1000);
        bytes memory sig = signIntent(pk, hi);
        vm.prank(liquidator);
        pool.liquidate(hi, sig, 100e6);
        assertTrue(pool.nonceUsed(who, 1000));
        assertFalse(pool.nonceUsed(who, 999));
    }

    // ============================================================
    // SC-21: a liquidation never lowers the health factor
    // ============================================================

    function test_SC21_liquidationNeverLowersHealthFactor() public {
        (address who, uint256 pk) = _unhealthyBorrower();
        uint256 hfBefore = pool.getHealthFactor(who);
        vm.prank(liquidator);
        pool.startAuction(who);
        vm.roll(block.number + 10);

        fund(debt, liquidator, 1_000_000e6);
        LendingPool.LiquidationIntent memory intent = intentFor(who, type(uint256).max, 0, 1);
        bytes memory sig = signIntent(pk, intent);
        vm.prank(liquidator);
        pool.liquidate(intent, sig, type(uint256).max);

        assertGe(pool.getHealthFactor(who), hfBefore);
    }

    // ============================================================
    // SC-22: bad debt is written off against reserves, never reverts
    // ============================================================

    function test_SC22_collateralRunsOutAndDebtIsWrittenOff() public {
        (address who, uint256 pk) = makeAddrAndKey("badDebtBorrower");
        supplyLiquidity(1_000_000e6);
        openFeePosition(who, 10e18, 10_000e6);
        // Crash to $100: the 9.9 tokens are now worth $990, far less than the $10,000 debt.
        setPrices(100e8, 2e8, 60_000e8);

        vm.prank(liquidator);
        pool.startAuction(who);
        vm.roll(block.number + 10);

        fund(debt, liquidator, 1_000_000e6);
        LendingPool.LiquidationIntent memory intent = intentFor(who, type(uint256).max, 0, 1);
        bytes memory sig = signIntent(pk, intent);
        vm.prank(liquidator);
        pool.liquidate(intent, sig, type(uint256).max);

        (uint256 bitmap, uint256 debtScaled) = pool.positionOf(who);
        assertEq(bitmap, 0);
        assertEq(debtScaled, 0, "remaining debt must be written off");
        (,,, uint256 deficit) = pool.totals();
        assertGt(deficit, 0, "shortfall beyond reserves must be recorded");
    }

    function test_SC22_reservesCoverShortfallFirst() public {
        (address who, uint256 pk) = makeAddrAndKey("reserveBorrower");
        supplyLiquidity(1_000_000e6);
        openFeePosition(who, 10e18, 10_000e6);
        skip(365 days); // accrue interest, so reserves build up
        _refreshAll();
        pool.accrue();
        setPrices(100e8, 2e8, 60_000e8);

        (,,uint256 reservesBefore,) = pool.totals();
        assertGt(reservesBefore, 0, "reserves should exist before the write-off");

        vm.prank(liquidator);
        pool.startAuction(who);
        vm.roll(block.number + 10);
        fund(debt, liquidator, 1_000_000e6);
        LendingPool.LiquidationIntent memory intent = intentFor(who, type(uint256).max, 0, 1);
        bytes memory sig = signIntent(pk, intent);
        vm.prank(liquidator);
        pool.liquidate(intent, sig, type(uint256).max);

        (,,uint256 reservesAfter, uint256 deficit) = pool.totals();
        assertEq(reservesAfter, 0, "reserves are spent first");
        assertGt(deficit, 0, "only the uncovered part becomes deficit");
    }

    // ============================================================
    // SC-23: transient-storage reentrancy lock
    // ============================================================

    function test_SC23_reentrantRepayFromTokenHookReverts() public {
        (LendingPool hookPool, MockHookERC20 hookToken) = _deployHookPool();
        hookToken.mint(alice, 10e18);
        vm.prank(alice);
        hookToken.approve(address(hookPool), type(uint256).max);

        hookToken.setHook(address(hookPool), abi.encodeCall(LendingPool.repay, (1)));
        vm.prank(alice);
        vm.expectRevert(LendingPool.ReentrantCall.selector);
        hookPool.depositCollateral(address(hookToken), 1e18);
    }

    // ============================================================
    // SC-24: views revert while the reentrancy lock is held
    // ============================================================

    function test_SC24_viewsRevertDuringReentrancy() public {
        (LendingPool hookPool, MockHookERC20 hookToken) = _deployHookPool();
        hookToken.mint(alice, 10e18);
        vm.prank(alice);
        hookToken.approve(address(hookPool), type(uint256).max);

        hookToken.setHook(address(hookPool), abi.encodeCall(LendingPool.getHealthFactor, (alice)));
        vm.prank(alice);
        vm.expectRevert(LendingPool.ReentrantCall.selector);
        hookPool.depositCollateral(address(hookToken), 1e18);
    }

    function test_SC24_viewsWorkOutsideReentrancy() public view {
        pool.getHealthFactor(alice);
        pool.getPrice(address(feeCol));
    }

    // ============================================================
    // SC-25: single-asset liquidation costs at most 220k gas
    // ============================================================

    function test_SC25_liquidationGasBudget() public {
        (address who, uint256 pk) = _unhealthyBorrower();
        vm.prank(liquidator);
        pool.startAuction(who);
        vm.roll(block.number + 10);

        fund(debt, liquidator, 1_000_000e6);
        LendingPool.LiquidationIntent memory intent = intentFor(who, 1000e6, 0, 1);
        bytes memory sig = signIntent(pk, intent);

        vm.prank(liquidator);
        uint256 g = gasleft();
        pool.liquidate(intent, sig, 1000e6);
        uint256 used = g - gasleft();
        // lastFrameGas is the cost of the call alone, without the test's own argument handling.
        Vm.Gas memory callGas = vm.lastFrameGas();
        emit log_named_uint("gasleft delta", used);
        emit log_named_uint("lastFrameGas.gasTotalUsed", callGas.gasTotalUsed);
        assertLe(callGas.gasTotalUsed, 220_000);
    }

    // ============================================================
    // Hook deployment for reentrancy tests
    // ============================================================

    function _deployHookPool() internal returns (LendingPool hookPool, MockHookERC20 hookToken) {
        hookToken = new MockHookERC20();
        MockAggregator hf = new MockAggregator(8, 1e8);
        MockAMM ha = new MockAMM(1e18);
        OracleAdapter o2 = new OracleAdapter(owner);
        vm.startPrank(owner);
        o2.setFeed(
            address(hookToken),
            IAggregatorV3(address(hf)),
            ITwapSource(address(ha)),
            7200,
            TWAP_WINDOW,
            1e18,
            1e30,
            MAX_DEVIATION_BPS
        );
        o2.setFeed(
            address(debt),
            IAggregatorV3(address(debtFeed)),
            ITwapSource(address(debtAmm)),
            7200,
            TWAP_WINDOW,
            1e18,
            1e30,
            MAX_DEVIATION_BPS
        );
        vm.stopPrank();
        skip(1 hours);
        hf.setAnswer(1e8);
        ha.setSpot(1e18);

        LendingPool impl2 = new LendingPool();
        bytes memory initData = abi.encodeCall(
            LendingPool.initialize, (owner, address(debt), address(o2), rates, RESERVE_FACTOR, DUST)
        );
        hookPool = LendingPool(address(new ERC1967Proxy(address(impl2), initData)));
        vm.prank(owner);
        hookPool.listCollateral(address(hookToken), 0, 8000, 500);
    }
}
