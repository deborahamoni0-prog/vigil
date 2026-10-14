//! ABI bindings generated from the Foundry build (SC and RS interfaces).

use alloy::sol;

sol! {
    #[derive(Debug, PartialEq, Eq)]
    struct LiquidationIntent {
        address borrower;
        address collateralAsset;
        uint256 maxRepay;
        uint256 minCollateralOut;
        uint256 nonce;
        uint256 deadline;
    }

    /// Events and views of `LendingPool`. Keep in step with `contracts/src/LendingPool.sol`.
    #[sol(rpc)]
    interface ILendingPool {
        event Supplied(address indexed user, uint256 amount, uint256 scaled);
        event SupplyWithdrawn(address indexed user, uint256 amount, uint256 scaled);
        event CollateralDeposited(address indexed user, address indexed asset, uint256 amount);
        event CollateralWithdrawn(address indexed user, address indexed asset, uint256 amount);
        event CollateralEnabled(address indexed user, address indexed asset);
        event CollateralDisabled(address indexed user, address indexed asset);
        event Borrowed(address indexed user, uint256 amount, uint256 scaled);
        event Repaid(address indexed user, uint256 amount, uint256 scaled);
        event AuctionStarted(address indexed borrower, uint256 startBlock);
        event AuctionReset(address indexed borrower);
        event Liquidated(
            address indexed borrower,
            address indexed liquidator,
            address indexed collateralAsset,
            uint256 repaid,
            uint256 scaledRepaid,
            uint256 seized,
            uint256 bonusBps
        );
        event BadDebtWrittenOff(
            address indexed borrower, uint256 debt, uint256 scaledDebt, uint256 coveredByReserves, uint256 deficit
        );
        event Paused();
        event Unpaused();

        function previewIndexes() external view returns (uint256 supplyIndex, uint256 debtIndex);
        function storedIndexes() external view returns (uint256 supplyIndex, uint256 debtIndex);
        function totals() external view returns (uint256 totalSupplyScaled, uint256 totalDebtScaled, uint256 reserves, uint256 deficit);
        function positionOf(address user) external view returns (uint256 bitmap, uint256 debtScaled);
        function collateralOf(address user, address asset) external view returns (uint256);
        function getHealthFactor(address user) external view returns (uint256);
        function getPrice(address asset) external view returns (uint256);
        function debtOf(address user) external view returns (uint256);
        function supplyOf(address user) external view returns (uint256);
        function auctionStartOf(address borrower) external view returns (uint256);
        function bonusBpsAt(address borrower, address collateralAsset) external view returns (uint256);
        function hashIntent(LiquidationIntent intent) external view returns (bytes32);
        function nonceUsed(address user, uint256 nonce) external view returns (bool);
        function startAuction(address borrower) external;
        function liquidate(LiquidationIntent intent, bytes signature, uint256 repayAmount) external;
        function accrue() external;
        function depositCollateral(address asset, uint256 amount) external;
        function borrow(uint256 amount) external;
        function repay(uint256 amount) external;
        function withdrawCollateral(address asset, uint256 amount) external;
        function supply(uint256 amount) external;
        function withdrawSupply(uint256 amount) external;
        function enableCollateral(address asset) external;
        function disableCollateral(address asset) external;
        function absorbBadDebt(address borrower) external;
        function lastAccrual() external view returns (uint64);
    }

    #[sol(rpc)]
    interface IOracle {
        function getPrice(address asset) external view returns (uint256);
        function isFrozen() external view returns (bool);
    }

    #[sol(rpc)]
    interface IERC20 {
        function balanceOf(address account) external view returns (uint256);
        function mint(address to, uint256 amount) external;
        function approve(address spender, uint256 amount) external returns (bool);
    }
}
