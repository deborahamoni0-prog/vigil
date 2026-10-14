// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {InterestMath} from "../src/libs/InterestMath.sol";
import {FixedPoint} from "../src/libs/FixedPoint.sol";
import {MockERC20} from "../src/mocks/MockERC20.sol";
import {MockNoReturnERC20} from "../src/mocks/MockNoReturnERC20.sol";
import {FixedPointHarness, InterestHarness, SafeTransferHarness} from "./Harness.sol";

/// @notice Unit tests for the fixed-point library and interest model (SC-06 to SC-08, SC-11, SC-13).
contract MathTest is Test {
    FixedPointHarness internal fp;
    InterestHarness internal ih;
    SafeTransferHarness internal st;

    uint256 constant RAY = 1e27;
    uint256 constant WAD = 1e18;

    InterestMath.RateParams internal p =
        InterestMath.RateParams({baseRate: 2e25, slope1: 4e25, slope2: 75e25, optimalUtil: 8e26});

    function setUp() public {
        fp = new FixedPointHarness();
        ih = new InterestHarness();
        st = new SafeTransferHarness();
    }

    // SC-07: rounding direction is explicit and the Up variant never undershoots.
    function test_SC07_mulDivRoundingDirections() public view {
        assertEq(fp.wadMulDown(3, 3e17), 0); // 3 * 0.3 = 0.9 -> 0
        assertEq(fp.wadMulUp(3, 3e17), 1);
        assertEq(fp.wadDivDown(1, 3 * WAD), 0);
        assertEq(fp.wadDivUp(1, 3 * WAD), 1);
        assertEq(fp.rayMulUp(0, RAY), 0);
    }

    function testFuzz_SC07_upNeverBelowDown(uint128 a, uint128 b) public view {
        uint256 d = fp.wadMulDown(a, b);
        uint256 u = fp.wadMulUp(a, b);
        assertGe(u, d);
        assertLe(u - d, 1);
    }

    function testFuzz_SC07_rayDivUpRoundTrip(uint96 amount, uint128 index) public view {
        vm.assume(index >= RAY / 2);
        uint256 scaled = fp.rayDivUp(amount, index);
        // Debt rounds up: the scaled amount, valued back, is never less than what was borrowed.
        assertGe(fp.rayMulUp(scaled, index), amount);
    }

    // SC-13: decimal normalisation for 6, 8 and 18-decimal tokens.
    function test_SC13_decimalNormalisation() public view {
        assertEq(fp.toWadDown(1e6, 6), WAD); // 1.000000 -> 1.0
        assertEq(fp.toWadDown(1e8, 8), WAD);
        assertEq(fp.toWadDown(5, 18), 5);
        assertEq(fp.toWadDown(1234567, 6), 1_234_567e12);
        // Rounding in the protocol's favour: amounts shrinking to WAD round down, debts round up.
        assertEq(fp.fromWadDown(1_500_000_000_000_000_001, 6), 1_500_000);
        assertEq(fp.fromWadUp(1_500_000_000_000_000_001, 6), 1_500_001);
        assertEq(fp.toWadUp(1, 24), 1);
        assertEq(fp.toWadDown(1, 24), 0);
    }

    // SC-06: compounding approximates e^(rt) for small rt; the 3-term series is within 1e-6.
    function test_SC06_compoundOneYearAt10Percent() public view {
        uint256 perSec = ih.ratePerSecond(1e26); // 10% annual
        uint256 factor = ih.compoundFactor(perSec, 365 days);
        // e^0.1 = 1.105170918075648
        assertApproxEqRel(factor, 1_105_170_918_075_648_000_000_000_000, 1e15);
    }

    function test_SC06_compoundZeroTimeIsIdentity() public view {
        assertEq(ih.compoundFactor(123, 0), RAY);
        assertEq(ih.compoundFactor(0, 1000), RAY);
    }

    function test_SC06_compoundIsMonotoneInTime() public view {
        uint256 r = ih.ratePerSecond(5e25);
        uint256 a = ih.compoundFactor(r, 100);
        uint256 b = ih.compoundFactor(r, 200);
        assertGt(b, a);
        assertGt(a, RAY);
    }

    // SC-08: kinked rate model at 0%, optimal and 100% utilisation.
    function test_SC08_kinkedRateAt0Optimal100() public view {
        assertEq(ih.annualBorrowRate(p, 0), 2e25); // base only
        assertEq(ih.annualBorrowRate(p, 8e26), 2e25 + 4e25); // at optimal: base + slope1
        assertEq(ih.annualBorrowRate(p, RAY), 2e25 + 4e25 + 75e25); // 100%: base + slope1 + slope2
    }

    function test_SC08_rateIsMonotoneAcrossKink() public view {
        uint256 below = ih.annualBorrowRate(p, 79e25);
        uint256 atKink = ih.annualBorrowRate(p, 8e26);
        uint256 above = ih.annualBorrowRate(p, 81e25);
        assertLt(below, atKink);
        assertLt(atKink, above);
    }

    function test_SC08_utilizationCapsAtOneHundredPercent() public view {
        assertEq(ih.utilization(200, 100), RAY);
        assertEq(ih.utilization(0, 0), 0);
        assertEq(ih.utilization(50, 100), RAY / 2);
    }

    // SC-11: SafeTransfer handles both value-returning and no-value tokens.
    function test_SC11_safeTransferWithNoReturnToken() public {
        MockNoReturnERC20 nr = new MockNoReturnERC20("NoRet", "NR", 6);
        nr.mint(address(st), 40);
        // The token's transfer returns nothing. SafeTransfer must accept that.
        st.transfer(address(nr), address(this), 40);
        assertEq(nr.balanceOf(address(this)), 40);
        assertEq(nr.balanceOf(address(st)), 0);
    }

    function test_SC11_safeTransferWithStandardToken() public {
        MockERC20 t = new MockERC20("T", "T", 18);
        t.mint(address(st), 10);
        st.transfer(address(t), address(this), 10);
        assertEq(t.balanceOf(address(this)), 10);
    }

    function test_SC11_revertsOnTokenWithoutCode() public {
        vm.expectRevert();
        st.transfer(address(0xdead), address(this), 1);
    }
}
