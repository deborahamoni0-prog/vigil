// Human-readable ABIs for the contracts the UI calls. Kept in step with contracts/src.
// Custom errors, so reverts can be shown by name. Matches contracts/src/LendingPool.sol and OracleAdapter.sol.
export const POOL_ERRORS = [
  "error ZeroAmount()",
  "error NotListed(address asset)",
  "error InsufficientBalance()",
  "error InsufficientLiquidity()",
  "error HealthFactorTooLow(uint256 healthFactor)",
  "error PositionHealthy()",
  "error NoDebt()",
  "error NoAuction()",
  "error AuctionActive()",
  "error OracleNotNormal()",
  "error ProtocolPaused()",
  "error IntentExpired()",
  "error InvalidSignature()",
  "error NonceUsed()",
  "error CollateralBelowMin(uint256 seized, uint256 minOut)",
  "error HealthDecreased(uint256 before, uint256 afterHf)",
  "error ReentrantCall()",
  "error NotEnabled(address asset)",
  "error OracleFrozen(address asset)",
];

export const POOL_ABI = [
  "function previewIndexes() view returns (uint256 supplyIndex, uint256 debtIndex)",
  "function totals() view returns (uint256 totalSupplyScaled, uint256 totalDebtScaled, uint256 reserves, uint256 deficit)",
  "function positionOf(address user) view returns (uint256 bitmap, uint256 debtScaled)",
  "function collateralOf(address user, address asset) view returns (uint256)",
  "function debtOf(address user) view returns (uint256)",
  "function supplyOf(address user) view returns (uint256)",
  "function getHealthFactor(address user) view returns (uint256)",
  "function getPrice(address asset) view returns (uint256)",
  "function auctionStartOf(address borrower) view returns (uint256)",
  "function bonusBpsAt(address borrower, address collateralAsset) view returns (uint256)",
  "function isPaused() view returns (bool)",
  "function hashIntent((address borrower,address collateralAsset,uint256 maxRepay,uint256 minCollateralOut,uint256 nonce,uint256 deadline) intent) view returns (bytes32)",
  "function nonceUsed(address user, uint256 nonce) view returns (bool)",
  "function supply(uint256 amount)",
  "function withdrawSupply(uint256 amount)",
  "function depositCollateral(address asset, uint256 amount)",
  "function withdrawCollateral(address asset, uint256 amount)",
  "function enableCollateral(address asset)",
  "function disableCollateral(address asset)",
  "function borrow(uint256 amount)",
  "function repay(uint256 amount)",
  "function startAuction(address borrower)",
  "function resetAuction(address borrower)",
  "function liquidate((address borrower,address collateralAsset,uint256 maxRepay,uint256 minCollateralOut,uint256 nonce,uint256 deadline) intent, bytes signature, uint256 repayAmount)",
  "function absorbBadDebt(address borrower)",
  "event Supplied(address indexed user, uint256 amount, uint256 scaled)",
  "event CollateralDeposited(address indexed user, address indexed asset, uint256 amount)",
  "event CollateralWithdrawn(address indexed user, address indexed asset, uint256 amount)",
  "event Borrowed(address indexed user, uint256 amount, uint256 scaled)",
  "event Repaid(address indexed user, uint256 amount, uint256 scaled)",
  "event AuctionStarted(address indexed borrower, uint256 startBlock)",
  "event Liquidated(address indexed borrower, address indexed liquidator, address indexed collateralAsset, uint256 repaid, uint256 scaledRepaid, uint256 seized, uint256 bonusBps)",
  "event BadDebtWrittenOff(address indexed borrower, uint256 debt, uint256 scaledDebt, uint256 coveredByReserves, uint256 deficit)",
];

export const TOKEN_ABI = [
  "function balanceOf(address) view returns (uint256)",
  "function decimals() view returns (uint8)",
  "function symbol() view returns (string)",
  "function allowance(address owner, address spender) view returns (uint256)",
  "function approve(address spender, uint256 amount) returns (bool)",
  "function mint(address to, uint256 amount)",
];

export const ORACLE_ABI = ["function isFrozen() view returns (bool)"];

// Dev-only price levers. Mocks accept calls from anyone.
export const FEED_ABI = ["function setAnswer(int256 answer)", "function answer() view returns (int256)"];
export const AMM_ABI = ["function setSpot(uint256 priceWad)", "function spot() view returns (uint256)"];
