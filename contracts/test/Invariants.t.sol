// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {StdInvariant} from "forge-std/StdInvariant.sol";
import {BaseTest} from "./Base.t.sol";
import {LendingPool} from "../src/LendingPool.sol";
import {FixedPoint} from "../src/libs/FixedPoint.sol";
import {MockERC20} from "../src/mocks/MockERC20.sol";
import {MockFeeOnTransferERC20} from "../src/mocks/MockFeeOnTransferERC20.sol";
import {MockAggregator} from "../src/mocks/MockAggregator.sol";
import {MockAMM} from "../src/mocks/MockAMM.sol";

/// @notice Random actors act on the pool. Ghost variables record what the invariants need.
/// @dev Every action first books pending interest on its own, so the interest ghost counts only
///      accrual and never mixes in borrows or repays.
contract PoolHandler is Test {
    uint256 internal constant COLLATERAL_PRICE_UNITS = 2000; // fixed reference for I-2 wealth
    uint256 internal constant WINDOW_SKIP = 3;

    LendingPool public immutable p;
    MockERC20 public immutable debt;
    MockFeeOnTransferERC20 public immutable fee;
    MockAggregator public immutable debtFeed;
    MockAggregator public immutable feeFeed;
    MockAMM public immutable feeAmm;
    MockAMM public immutable debtAmm;

    address[] public actors;
    mapping(address => uint256) public keyOf;
    mapping(address => uint256) public endowment; // value put into the system, in debt units

    // ghost state
    uint256 public ghostInterest;
    uint256 public ghostFrozenSnapshot;
    bool public ghostFrozen;
    uint256 public ghostI3Violations;
    uint256 public ghostI4Violations;
    uint256 public ghostLiquidations;

    constructor(
        LendingPool pool_,
        MockERC20 debt_,
        MockFeeOnTransferERC20 fee_,
        MockAggregator debtFeed_,
        MockAggregator feeFeed_,
        MockAMM feeAmm_,
        MockAMM debtAmm_
    ) {
        p = pool_;
        debt = debt_;
        fee = fee_;
        debtFeed = debtFeed_;
        feeFeed = feeFeed_;
        feeAmm = feeAmm_;
        debtAmm = debtAmm_;
    }

    function addActor(uint256 pk) external {
        address a = vm.addr(pk);
        if (keyOf[a] != 0) return; // same key registered twice would double count its debt
        actors.push(a);
        keyOf[a] = pk;
    }

    function actorCount() external view returns (uint256) {
        return actors.length;
    }

    function _actor(uint256 seed) internal view returns (address) {
        return actors[seed % actors.length];
    }

    /// @dev Debt value at the last written index. Pending interest is excluded on purpose,
    ///      so the difference across an accrual is exactly the interest booked.
    function _debtValue() internal view returns (uint256) {
        (, uint256 di) = p.storedIndexes();
        (uint256 tsc,,,) = _totals();
        return FixedPoint.rayMulUp(tsc, di);
    }

    function _totals() internal view returns (uint256, uint256, uint256, uint256) {
        return p.totals();
    }

    /// @dev Books interest that is pending from time passing, with no other state change.
    function _book() internal {
        uint256 before = _debtValue();
        p.accrue();
        uint256 after_ = _debtValue();
        if (after_ > before) ghostInterest += after_ - before;
    }

    /// @dev Re-stamps feeds so they stay fresh. The debt feed stays stale while frozen, which is
    ///      how the freeze is held in place.
    function _refreshFeeds() internal {
        if (!ghostFrozen) debtFeed.setAnswer(debtFeed.answer());
        feeFeed.setAnswer(feeFeed.answer());
    }

    function warp(uint256 seconds_) external {
        seconds_ = bound(seconds_, 1, 3 days);
        skip(seconds_);
        _refreshFeeds();
        _book();
    }

    function supply(uint256 seed, uint256 amount) external {
        address a = _actor(seed);
        amount = bound(amount, 1, 100_000e6);
        _book();
        debt.mint(a, amount);
        endowment[a] += amount;
        vm.startPrank(a);
        debt.approve(address(p), amount);
        try p.supply(amount) {} catch {}
        vm.stopPrank();
    }

    function withdrawSupply(uint256 seed, uint256 amount) external {
        address a = _actor(seed);
        _book();
        uint256 bal = p.supplyOf(a);
        if (bal == 0) return;
        amount = bound(amount, 1, bal);
        vm.prank(a);
        try p.withdrawSupply(amount) {} catch {}
    }

    function depositCollateral(uint256 seed, uint256 amount) external {
        address a = _actor(seed);
        amount = bound(amount, 1e15, 100e18);
        _book();
        fee.mint(a, amount);
        endowment[a] += amount * COLLATERAL_PRICE_UNITS / 1e12;
        vm.startPrank(a);
        fee.approve(address(p), amount);
        try p.depositCollateral(address(fee), amount) {} catch {}
        vm.stopPrank();
    }

    function borrow(uint256 seed, uint256 amount) external {
        address a = _actor(seed);
        amount = bound(amount, 1, 50_000e6);
        _book();
        vm.prank(a);
        try p.borrow(amount) {} catch {}
    }

    function repay(uint256 seed, uint256 amount) external {
        address a = _actor(seed);
        _book();
        uint256 owed = p.debtOf(a);
        if (owed == 0) return;
        amount = bound(amount, 1, owed);
        debt.mint(a, amount);
        endowment[a] += amount;
        vm.startPrank(a);
        debt.approve(address(p), amount);
        try p.repay(amount) {} catch {}
        vm.stopPrank();
    }

    function withdrawCollateral(uint256 seed, uint256 amount) external {
        address a = _actor(seed);
        _book();
        uint256 held = p.collateralOf(a, address(fee));
        if (held == 0) return;
        amount = bound(amount, 1, held);
        vm.prank(a);
        try p.withdrawCollateral(address(fee), amount) {} catch {}
    }

    /// @dev Moves the fee token price. Feed and AMM move together, then the TWAP window fills.
    function movePrice(uint256 usd) external {
        usd = bound(usd, 300, 2500);
        feeFeed.setAnswer(int256(usd) * 1e8);
        feeAmm.setSpot(usd * 1e18);
        skip(WINDOW_SKIP);
        _refreshFeeds();
        _book();
    }

    /// @dev Freezes by letting the debt feed go stale, and snapshots scaled debt at that moment.
    function freeze() external {
        if (ghostFrozen) return;
        _book();
        debtFeed.setUpdatedAt(0);
        ghostFrozen = true;
        (, uint256 tsc,,) = _totals();
        ghostFrozenSnapshot = tsc;
    }

    function thaw() external {
        if (!ghostFrozen) return;
        _refreshFeeds();
        ghostFrozen = false;
    }

    function frozenBorrowAttempt(uint256 seed) external {
        address a = _actor(seed);
        vm.prank(a);
        try p.borrow(1e6) {} catch {}
    }

    /// @dev Starts auctions, rolls blocks and checks I-4: the bonus never falls block over block.
    function auctionStep(uint256 seed) external {
        address a = _actor(seed);
        _book();
        if (p.auctionStartOf(a) == 0) {
            vm.prank(a);
            try p.startAuction(a) {} catch {}
        }
        if (p.auctionStartOf(a) == 0) return;
        uint256 b1 = p.bonusBpsAt(a, address(fee));
        vm.roll(block.number + 1);
        uint256 b2 = p.bonusBpsAt(a, address(fee));
        if (b2 < b1) ghostI4Violations++;
    }

    /// @dev Liquidates a position through a signed intent, then checks I-3.
    function liquidate(uint256 seed, uint256 repayAmount) external {
        address a = _actor(seed);
        _book();
        if (p.auctionStartOf(a) == 0) return;
        uint256 hfBefore = p.getHealthFactor(a);
        if (hfBefore >= 1e18) return;

        repayAmount = bound(repayAmount, 1, 60_000e6);
        debt.mint(address(this), repayAmount);
        debt.approve(address(p), type(uint256).max);

        LendingPool.LiquidationIntent memory intent = LendingPool.LiquidationIntent({
            borrower: a,
            collateralAsset: address(fee),
            maxRepay: repayAmount,
            minCollateralOut: 0,
            nonce: uint256(keccak256(abi.encode(seed, block.number))),
            deadline: block.timestamp + 1 days
        });
        bytes32 digest = p.hashIntent(intent);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(keyOf[a], digest);
        bytes memory sig = abi.encodePacked(r, s, v);

        try p.liquidate(intent, sig, repayAmount) {
            ghostLiquidations++;
            uint256 hfAfter = p.getHealthFactor(a);
            if (hfAfter < hfBefore) ghostI3Violations++;
        } catch {}
    }
}

contract InvariantTest is StdInvariant, BaseTest {
    PoolHandler internal handler;

    function setUp() public override {
        super.setUp();
        supplyLiquidity(10_000_000e6);

        handler = new PoolHandler(pool, debt, feeCol, debtFeed, feeFeed, feeAmm, debtAmm);
        for (uint256 i; i < 8; i++) {
            handler.addActor(uint256(keccak256(abi.encode("actor", i))) | 1);
        }

        targetContract(address(handler));
    }

    function _n() internal view returns (uint256) {
        return handler.actorCount();
    }

    /// @dev I-1: scaled debt across accounts equals the recorded total, and the value sum
    ///      agrees with the total within 1 wei per account.
    function invariant_I1_scaledSumsMatchTotals() public view {
        uint256 sumScaled;
        (, uint256 di) = pool.previewIndexes();
        uint256 sumValue;
        for (uint256 i; i < _n(); i++) {
            address a = handler.actors(i);
            (, uint256 ds) = pool.positionOf(a);
            sumScaled += ds;
            sumValue += FixedPoint.rayMulUp(ds, di);
        }
        (, uint256 totDebtScaled,,) = pool.totals();
        assertEq(sumScaled, totDebtScaled, "scaled debt sum mismatch");

        uint256 totalValue = FixedPoint.rayMulUp(totDebtScaled, di);
        uint256 diff = totalValue > sumValue ? totalValue - sumValue : sumValue - totalValue;
        assertLe(diff, _n(), "value sum differs by more than 1 wei per account");
    }

    /// @dev I-2: no account's net position exceeds its endowment plus the interest pool-wide.
    ///      Wealth = wallet debt + lending balance + collateral (at the fixed reference price) - debt.
    function invariant_I2_noExtraction() public view {
        uint256 interestBudget = handler.ghostInterest();
        for (uint256 i; i < _n(); i++) {
            address a = handler.actors(i);
            uint256 colTokens = pool.collateralOf(a, address(feeCol)) + feeCol.balanceOf(a);
            uint256 colValue = colTokens * 2000 / 1e12;
            uint256 assets = debt.balanceOf(a) + pool.supplyOf(a) + colValue;
            uint256 owed = pool.debtOf(a);
            uint256 net = assets > owed ? assets - owed : 0;
            // Slack of 0.01 debt units covers rounding in the 18-to-6 decimal collateral conversion.
            assertLe(net, handler.endowment(a) + interestBudget + 1e4, "account extracted more than it put in");
        }
    }

    /// @dev I-3: every liquidation leaves the health factor at or above its starting value.
    function invariant_I3_liquidationNeverLowersHealthFactor() public view {
        assertEq(handler.ghostI3Violations(), 0, "a liquidation lowered the health factor");
    }

    /// @dev I-4: during an auction the bonus is non-decreasing block over block.
    function invariant_I4_bonusNonDecreasing() public view {
        assertEq(handler.ghostI4Violations(), 0, "bonus decreased between blocks");
    }

    /// @dev I-5: while Frozen, total scaled debt never rises. Only interest accrual adds value.
    function invariant_I5_frozenDebtOnlyAccrues() public view {
        if (!handler.ghostFrozen()) return;
        (, uint256 tsc,,) = pool.totals();
        assertLe(tsc, handler.ghostFrozenSnapshot(), "debt grew while Frozen");
    }
}
