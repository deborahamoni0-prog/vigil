// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Script} from "forge-std/Script.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

import {LendingPool} from "../src/LendingPool.sol";
import {InterestMath} from "../src/libs/InterestMath.sol";
import {OracleAdapter} from "../src/oracle/OracleAdapter.sol";
import {IAggregatorV3} from "../src/interfaces/IAggregatorV3.sol";
import {ITwapSource} from "../src/interfaces/ITwapSource.sol";
import {MockERC20} from "../src/mocks/MockERC20.sol";
import {MockFeeOnTransferERC20} from "../src/mocks/MockFeeOnTransferERC20.sol";
import {MockAggregator} from "../src/mocks/MockAggregator.sol";
import {MockAMM} from "../src/mocks/MockAMM.sol";

/// @notice Deploys the full Vigil stack, seeds lender liquidity, and writes the addresses to
///         deployments/<chainId>.json for the services and the scenario scripts.
/// @dev Local use: `forge script script/Deploy.s.sol --rpc-url http://127.0.0.1:8545 --broadcast`.
///      DEPLOYER_KEY overrides the default anvil account 0.
contract Deploy is Script {
    uint256 internal constant DEFAULT_KEY = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
    uint256 internal constant SEED_LIQUIDITY = 10_000_000e6;

    function run() external {
        uint256 pk = vm.envOr("DEPLOYER_KEY", DEFAULT_KEY);
        address deployer = vm.addr(pk);

        vm.startBroadcast(pk);

        MockERC20 debt = new MockERC20("Vigil USD", "vUSD", 6);
        MockFeeOnTransferERC20 feeCol = new MockFeeOnTransferERC20("Fee Token", "FEE", 18, 100);
        MockERC20 sixCol = new MockERC20("Six", "SIX", 6);
        MockERC20 eightCol = new MockERC20("Eight", "EIGHT", 8);

        // Starting prices come from config/market.json, in cents. Feeds use 8 decimals, the AMM 18.
        string memory cfg = vm.readFile("config/market.json");
        uint256 debtCents = vm.parseJsonUint(cfg, ".debt.priceCents");
        uint256 feeCents = vm.parseJsonUint(cfg, ".collaterals[0].priceCents");
        uint256 sixCents = vm.parseJsonUint(cfg, ".collaterals[1].priceCents");
        uint256 eightCents = vm.parseJsonUint(cfg, ".collaterals[2].priceCents");

        MockAggregator debtFeed = new MockAggregator(8, int256(debtCents * 1e6));
        MockAggregator feeFeed = new MockAggregator(8, int256(feeCents * 1e6));
        MockAggregator sixFeed = new MockAggregator(8, int256(sixCents * 1e6));
        MockAggregator eightFeed = new MockAggregator(8, int256(eightCents * 1e6));

        MockAMM debtAmm = new MockAMM(debtCents * 1e16);
        MockAMM feeAmm = new MockAMM(feeCents * 1e16);
        MockAMM sixAmm = new MockAMM(sixCents * 1e16);
        MockAMM eightAmm = new MockAMM(eightCents * 1e16);

        OracleAdapter oracle = new OracleAdapter(deployer);
        _feed(oracle, address(debt), debtFeed, debtAmm, 1e18);
        _feed(oracle, address(feeCol), feeFeed, feeAmm, 1e16);
        _feed(oracle, address(sixCol), sixFeed, sixAmm, 1e16);
        _feed(oracle, address(eightCol), eightFeed, eightAmm, 1e16);

        InterestMath.RateParams memory rates = InterestMath.RateParams({
            baseRate: 2e25,
            slope1: 4e25,
            slope2: 75e25,
            optimalUtil: 8e26
        });
        LendingPool impl = new LendingPool();
        bytes memory initData = abi.encodeCall(
            LendingPool.initialize, (deployer, address(debt), address(oracle), rates, 1e26, 1e6)
        );
        LendingPool pool = LendingPool(address(new ERC1967Proxy(address(impl), initData)));

        pool.listCollateral(address(feeCol), 0, 8000, 500);
        pool.listCollateral(address(sixCol), 1, 8500, 500);
        pool.listCollateral(address(eightCol), 2, 7500, 1000);

        // Seed lender liquidity so borrows have cash to draw on.
        debt.mint(deployer, SEED_LIQUIDITY);
        debt.approve(address(pool), type(uint256).max);
        pool.supply(SEED_LIQUIDITY);

        vm.stopBroadcast();

        _write(block.chainid, pool, impl, oracle, debt, feeCol, sixCol, eightCol, debtFeed, feeFeed, sixFeed, eightFeed, feeAmm, sixAmm, eightAmm);
    }

    function _feed(OracleAdapter oracle, address asset, MockAggregator feed, MockAMM amm, uint256 minPrice)
        internal
    {
        oracle.setFeed(
            asset,
            IAggregatorV3(address(feed)),
            ITwapSource(address(amm)),
            7200, // max staleness, seconds
            2, // TWAP window, seconds (about two blocks)
            minPrice,
            1e30,
            1000 // 10% deviation band
        );
    }

    function _write(
        uint256 chainId,
        LendingPool pool,
        LendingPool impl,
        OracleAdapter oracle,
        MockERC20 debt,
        MockFeeOnTransferERC20 feeCol,
        MockERC20 sixCol,
        MockERC20 eightCol,
        MockAggregator debtFeed,
        MockAggregator feeFeed,
        MockAggregator sixFeed,
        MockAggregator eightFeed,
        MockAMM feeAmm,
        MockAMM sixAmm,
        MockAMM eightAmm
    ) internal {
        // Uints are written as 32-byte hex strings. The Rust side reads them as U256.
        InterestMath.RateParams memory r = InterestMath.RateParams({
            baseRate: 2e25, slope1: 4e25, slope2: 75e25, optimalUtil: 8e26
        });
        string memory json = string.concat(
            '{"chainId":', vm.toString(chainId),
            ',"pool":"', vm.toString(address(pool)),
            '","implementation":"', vm.toString(address(impl)),
            '","oracle":"', vm.toString(address(oracle)),
            '","debt":"', vm.toString(address(debt)),
            '","debtDecimals":6,"debtFeed":"', vm.toString(address(debtFeed)),
            '","reserveFactor":"', vm.toString(bytes32(uint256(1e26))),
            '","rates":{"baseRate":"', vm.toString(bytes32(r.baseRate)),
            '","slope1":"', vm.toString(bytes32(r.slope1)),
            '","slope2":"', vm.toString(bytes32(r.slope2)),
            '","optimalUtil":"', vm.toString(bytes32(r.optimalUtil)),
            '"},"collaterals":[',
            _collateralJson("fee", feeCol, 0, 8000, 500, feeFeed, feeAmm), ",",
            _collateralJson("six", sixCol, 1, 8500, 500, sixFeed, sixAmm), ",",
            _collateralJson("eight", eightCol, 2, 7500, 1000, eightFeed, eightAmm),
            "]}"
        );
        vm.writeFile(string.concat("./deployments/", vm.toString(chainId), ".json"), json);
    }

    function _collateralJson(
        string memory name,
        MockERC20 token,
        uint8 bit,
        uint16 threshold,
        uint16 bonus,
        MockAggregator feed,
        MockAMM amm
    ) internal view returns (string memory) {
        return string.concat(
            '{"name":"', name,
            '","address":"', vm.toString(address(token)),
            '","decimals":', vm.toString(uint256(token.decimals())),
            ',"bitIndex":', vm.toString(uint256(bit)),
            ',"liqThresholdBps":', vm.toString(uint256(threshold)),
            ',"liqBonusBps":', vm.toString(uint256(bonus)),
            ',"feed":"', vm.toString(address(feed)),
            '","amm":"', vm.toString(address(amm)), '"}'
        );
    }
}
