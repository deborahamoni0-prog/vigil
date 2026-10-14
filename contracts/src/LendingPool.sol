// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import {UUPSUpgradeable} from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import {OwnableUpgradeable} from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {IERC20Metadata} from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";

import {FixedPoint} from "./libs/FixedPoint.sol";
import {InterestMath} from "./libs/InterestMath.sol";
import {SafeTransfer} from "./libs/SafeTransfer.sol";
import {IOracle} from "./interfaces/IOracle.sol";

/// @title LendingPool
/// @notice Single-debt-asset lending pool with up to three collateral assets (MVP scope).
/// @dev UUPS proxy (SC-01). All state lives in one ERC-7201 namespaced struct (SC-02).
///      Balances are scaled against RAY indexes (SC-05). Liquidations execute from
///      EIP-712 intents signed by the borrower, with Permit2-style bitmap nonces (SC-20).
contract LendingPool is Initializable, OwnableUpgradeable, UUPSUpgradeable {
    using FixedPoint for uint256;
    using SafeTransfer for address;

    uint256 public constant MAX_COLLATERAL = 3;
    uint256 public constant AUCTION_BLOCKS = 10;
    uint256 public constant CLOSE_FACTOR_BPS = 5000;
    uint256 public constant HF_NO_DEBT = type(uint256).max;

    bytes32 public constant LIQUIDATION_INTENT_TYPEHASH = keccak256(
        "LiquidationIntent(address borrower,address collateralAsset,uint256 maxRepay,uint256 minCollateralOut,uint256 nonce,uint256 deadline)"
    );

    /// @dev EIP-712 domain, computed from constants so signing reads no storage (saves cold SLOADs).
    bytes32 private constant EIP712_DOMAIN_TYPEHASH =
        keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    bytes32 private constant NAME_HASH = keccak256("Vigil LendingPool");
    bytes32 private constant VERSION_HASH = keccak256("1");

    /// @dev Transient-storage slot for the reentrancy lock (SC-23).
    bytes32 private constant REENTRANCY_LOCK_SLOT = keccak256("vigil.reentrancy.lock");

    /// @dev keccak256(abi.encode(uint256(keccak256("vigil.storage.LendingPool")) - 1)) & ~bytes32(uint256(0xff))
    bytes32 private constant POOL_STORAGE_LOCATION = 0x9a3c967fed6abe3d5f7f0574872b3b3c77b6edc35b36a42126f37cf5a7159300;

    struct CollateralConfig {
        bool listed;
        uint8 decimals;
        uint8 bitIndex;
        uint16 liqThresholdBps;
        uint16 liqBonusBps; // maximum bonus, reached after AUCTION_BLOCKS
    }

    struct Position {
        uint256 collateralBitmap; // enabled collateral, one bit per asset
        uint256 debtScaled;
    }

    struct LiquidationIntent {
        address borrower;
        address collateralAsset;
        uint256 maxRepay;
        uint256 minCollateralOut;
        uint256 nonce;
        uint256 deadline;
    }

    /// @custom:storage-location erc7201:vigil.storage.LendingPool
    struct PoolStorage {
        // Slot 0: read on nearly every action, packed to save cold SLOADs.
        address debtAsset;
        uint8 debtDecimals;
        bool paused;
        // Slot 1: oracle address and last accrual timestamp.
        address oracle;
        uint64 lastAccrual;
        InterestMath.RateParams rates;
        uint256 reserveFactor; // RAY
        uint256 dustDebt; // debt-asset units
        uint256 supplyIndex; // RAY
        uint256 debtIndex; // RAY
        uint256 totalSupplyScaled;
        uint256 totalDebtScaled;
        uint256 reserves; // debt-asset units
        uint256 deficit; // debt-asset units, bad debt beyond reserves
        mapping(uint256 => address) collateralByMask;
        mapping(address => CollateralConfig) collateral;
        mapping(address => uint256) supplyScaled;
        mapping(address => Position) positions;
        mapping(address => mapping(address => uint256)) collateralAmount;
        mapping(address => uint256) auctionStart; // block number, 0 = no auction
        mapping(address => mapping(uint256 => uint256)) nonceBitmap;
    }

    // ---- Events ----

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
    /// @dev scaledRepaid is emitted so the Risk Engine can replay the exact debt change.
    event Liquidated(
        address indexed borrower,
        address indexed liquidator,
        address indexed collateralAsset,
        uint256 repaid,
        uint256 scaledRepaid,
        uint256 seized,
        uint256 bonusBps
    );
    /// @dev scaledDebt is the debt removed from the position, in scaled units.
    event BadDebtWrittenOff(
        address indexed borrower, uint256 debt, uint256 scaledDebt, uint256 coveredByReserves, uint256 deficit
    );
    event Paused();
    event Unpaused();
    event CollateralListed(address indexed asset, uint8 bitIndex, uint16 liqThresholdBps, uint16 liqBonusBps);

    // ---- Errors ----

    error ZeroAmount();
    error NotListed(address asset);
    error AlreadyListed(address asset);
    error BitIndexTaken(uint8 bitIndex);
    error BitIndexOutOfRange(uint8 bitIndex);
    error InvalidThreshold();
    error NotEnabled(address asset);
    error InsufficientBalance();
    error InsufficientLiquidity();
    error HealthFactorTooLow(uint256 healthFactor);
    error PositionHealthy();
    error NoDebt();
    error NoAuction();
    error AuctionActive();
    error OracleNotNormal();
    error ProtocolPaused();
    error ProtocolNotPaused();
    error IntentExpired();
    error InvalidSignature();
    error NonceUsed();
    error CollateralBelowMin(uint256 seized, uint256 minOut);
    error HealthDecreased(uint256 before, uint256 afterHf);
    error ReentrantCall();
    error NoPauseAuthority();

    // ---- Modifiers ----

    modifier nonReentrant() {
        if (_isLocked()) revert ReentrantCall();
        _setLock(true);
        _;
        _setLock(false);
    }

    modifier whenNotPaused() {
        if (_s().paused) revert ProtocolPaused();
        _;
    }

    /// @dev Frozen oracle: only repay and add-collateral are allowed (SC-16).
    modifier whenOracleNormal() {
        if (IOracle(_s().oracle).isFrozen()) revert OracleNotNormal();
        _;
    }

    /// @dev Reverts while the reentrancy lock is held, so views cannot be read mid-call (SC-24).
    modifier notLocked() {
        if (_isLocked()) revert ReentrantCall();
        _;
    }

    constructor() {
        _disableInitializers();
    }

    // ---- Initialisation and upgrades ----

    function initialize(
        address owner_,
        address debtAsset_,
        address oracle_,
        InterestMath.RateParams memory rates_,
        uint256 reserveFactorRay,
        uint256 dustDebt_
    ) external initializer {
        __Ownable_init(owner_);

        PoolStorage storage s = _s();
        s.debtAsset = debtAsset_;
        s.debtDecimals = IERC20Metadata(debtAsset_).decimals();
        s.oracle = oracle_;
        s.rates = rates_;
        s.reserveFactor = reserveFactorRay;
        s.dustDebt = dustDebt_;
        s.supplyIndex = FixedPoint.RAY;
        s.debtIndex = FixedPoint.RAY;
        // uint64 holds timestamps until the year 584 billion, so the cast is safe.
        // forge-lint: disable-next-line(unsafe-typecast)
        s.lastAccrual = uint64(block.timestamp);
    }

    function _authorizeUpgrade(address) internal override onlyOwner {}

    // ---- Admin ----

    function listCollateral(address asset, uint8 bitIndex, uint16 liqThresholdBps, uint16 liqBonusBps)
        external
        onlyOwner
    {
        if (bitIndex >= MAX_COLLATERAL) revert BitIndexOutOfRange(bitIndex);
        if (liqThresholdBps == 0 || liqThresholdBps > FixedPoint.BPS) revert InvalidThreshold();
        if (liqBonusBps > FixedPoint.BPS) revert InvalidThreshold();

        PoolStorage storage s = _s();
        if (s.collateral[asset].listed) revert AlreadyListed(asset);
        uint256 mask = uint256(1) << bitIndex;
        if (s.collateralByMask[mask] != address(0)) revert BitIndexTaken(bitIndex);

        s.collateralByMask[mask] = asset;
        s.collateral[asset] = CollateralConfig({
            listed: true,
            decimals: IERC20Metadata(asset).decimals(),
            bitIndex: bitIndex,
            liqThresholdBps: liqThresholdBps,
            liqBonusBps: liqBonusBps
        });
        // Owner-only. The event follows the decimals() read, which is a view call on a listed token.
        // forge-lint: disable-next-line(reentrancy-events)
        emit CollateralListed(asset, bitIndex, liqThresholdBps, liqBonusBps);
    }

    function pause() external onlyOwner {
        _s().paused = true;
        // forge-lint: disable-next-line(reentrancy-events)
        emit Paused();
    }

    /// @notice Unpause is only allowed when the oracle is Normal (SC-04).
    function unpause() external onlyOwner {
        PoolStorage storage s = _s();
        if (!s.paused) revert ProtocolNotPaused();
        if (IOracle(s.oracle).isFrozen()) revert OracleNotNormal();
        s.paused = false;
        // Owner-only. The event follows the isFrozen() oracle read, which is a view call.
        // forge-lint: disable-next-line(reentrancy-events)
        emit Unpaused();
    }

    /// @notice Accrues interest now. Keepers call this so reserves and indexes stay current.
    function accrue() external nonReentrant {
        _accrue();
    }

    // ---- Lender side ----

    function supply(uint256 amount) external nonReentrant whenNotPaused {
        if (amount == 0) revert ZeroAmount();
        _accrue();
        PoolStorage storage s = _s();

        uint256 received = _pullMeasured(s.debtAsset, msg.sender, amount);
        uint256 scaled = FixedPoint.rayDivDown(received, s.supplyIndex);
        s.supplyScaled[msg.sender] += scaled;
        s.totalSupplyScaled += scaled;
        // forge-lint: disable-next-line(reentrancy-events)
        emit Supplied(msg.sender, received, scaled);
    }

    function withdrawSupply(uint256 amount) external nonReentrant whenNotPaused {
        if (amount == 0) revert ZeroAmount();
        _accrue();
        PoolStorage storage s = _s();

        uint256 scaled = FixedPoint.rayDivUp(amount, s.supplyIndex);
        if (s.supplyScaled[msg.sender] < scaled) revert InsufficientBalance();
        uint256 cash = SafeTransfer.balanceOf(s.debtAsset, address(this));
        if (cash < s.reserves + amount) revert InsufficientLiquidity();

        s.supplyScaled[msg.sender] -= scaled;
        s.totalSupplyScaled -= scaled;
        // nonReentrant blocks re-entry into the pool; this event follows reads it depends on.
        // forge-lint: disable-next-line(reentrancy-events)
        emit SupplyWithdrawn(msg.sender, amount, scaled);
        s.debtAsset.safeTransfer(msg.sender, amount);
    }

    // ---- Borrower side ----

    function depositCollateral(address asset, uint256 amount) external nonReentrant whenNotPaused {
        if (amount == 0) revert ZeroAmount();
        PoolStorage storage s = _s();
        CollateralConfig storage cfg = s.collateral[asset];
        if (!cfg.listed) revert NotListed(asset);

        uint256 received = _pullMeasured(asset, msg.sender, amount);
        s.collateralAmount[msg.sender][asset] += received;
        s.positions[msg.sender].collateralBitmap |= uint256(1) << cfg.bitIndex;
        // forge-lint: disable-next-line(reentrancy-events)
        emit CollateralDeposited(msg.sender, asset, received);
    }

    function withdrawCollateral(address asset, uint256 amount) external nonReentrant whenNotPaused whenOracleNormal {
        if (amount == 0) revert ZeroAmount();
        _accrue();
        PoolStorage storage s = _s();

        uint256 held = s.collateralAmount[msg.sender][asset];
        if (held < amount) revert InsufficientBalance();
        s.collateralAmount[msg.sender][asset] = held - amount;
        if (held == amount) _clearBit(msg.sender, asset);

        _requireHealthy(msg.sender);
        asset.safeTransfer(msg.sender, amount);
        // nonReentrant blocks re-entry into the pool; this event follows reads it depends on.
        // forge-lint: disable-next-line(reentrancy-events)
        emit CollateralWithdrawn(msg.sender, asset, amount);
    }

    /// @notice Stops counting an asset towards health. The collateral stays deposited.
    function disableCollateral(address asset) external nonReentrant whenNotPaused whenOracleNormal {
        _accrue();
        PoolStorage storage s = _s();
        CollateralConfig storage cfg = s.collateral[asset];
        if (!cfg.listed) revert NotListed(asset);
        uint256 bit = uint256(1) << cfg.bitIndex;
        if (s.positions[msg.sender].collateralBitmap & bit == 0) revert NotEnabled(asset);

        s.positions[msg.sender].collateralBitmap &= ~bit;
        _requireHealthy(msg.sender);
        // nonReentrant blocks re-entry into the pool; this event follows reads it depends on.
        // forge-lint: disable-next-line(reentrancy-events)
        emit CollateralDisabled(msg.sender, asset);
    }

    /// @notice Counts an already-deposited asset towards health again. Adds collateral, so it is allowed while Frozen.
    function enableCollateral(address asset) external nonReentrant whenNotPaused {
        PoolStorage storage s = _s();
        CollateralConfig storage cfg = s.collateral[asset];
        if (!cfg.listed) revert NotListed(asset);
        s.positions[msg.sender].collateralBitmap |= uint256(1) << cfg.bitIndex;
        // nonReentrant blocks re-entry into the pool; this event follows reads it depends on.
        // forge-lint: disable-next-line(reentrancy-events)
        emit CollateralEnabled(msg.sender, asset);
    }

    function borrow(uint256 amount) external nonReentrant whenNotPaused whenOracleNormal {
        if (amount == 0) revert ZeroAmount();
        _accrue();
        PoolStorage storage s = _s();

        uint256 cash = SafeTransfer.balanceOf(s.debtAsset, address(this));
        if (cash < s.reserves + amount) revert InsufficientLiquidity();

        // Debt rounds up so the position never owes less than it received.
        uint256 scaled = FixedPoint.rayDivUp(amount, s.debtIndex);
        s.positions[msg.sender].debtScaled += scaled;
        s.totalDebtScaled += scaled;

        _requireHealthy(msg.sender);
        s.debtAsset.safeTransfer(msg.sender, amount);
        // nonReentrant blocks re-entry into the pool; this event follows reads it depends on.
        // forge-lint: disable-next-line(reentrancy-events)
        emit Borrowed(msg.sender, amount, scaled);
    }

    /// @notice Repay is allowed while Frozen and never requires a price.
    function repay(uint256 amount) external nonReentrant whenNotPaused {
        if (amount == 0) revert ZeroAmount();
        _accrue();
        PoolStorage storage s = _s();
        Position storage p = s.positions[msg.sender];
        if (p.debtScaled == 0) revert NoDebt();

        uint256 debtUnits = FixedPoint.rayMulUp(p.debtScaled, s.debtIndex);
        uint256 pay = amount < debtUnits ? amount : debtUnits;
        uint256 received = _pullMeasured(s.debtAsset, msg.sender, pay);

        // Repaying less reduces the position less, which favours the protocol.
        uint256 scaled = received >= debtUnits ? p.debtScaled : FixedPoint.rayDivDown(received, s.debtIndex);
        if (scaled > p.debtScaled) scaled = p.debtScaled;
        p.debtScaled -= scaled;
        s.totalDebtScaled -= scaled;
        // nonReentrant blocks re-entry into the pool; this event follows reads it depends on.
        // forge-lint: disable-next-line(reentrancy-events)
        emit Repaid(msg.sender, received, scaled);
    }

    // ---- Liquidation ----

    /// @notice Starts the Dutch auction for an unhealthy position. Anyone may call it.
    function startAuction(address borrower) external nonReentrant whenNotPaused whenOracleNormal {
        _accrue();
        PoolStorage storage s = _s();
        if (s.auctionStart[borrower] != 0) revert AuctionActive();

        uint256 hf = _healthFactor(borrower, s.debtIndex);
        if (hf >= FixedPoint.WAD) revert PositionHealthy();

        s.auctionStart[borrower] = block.number;
        // nonReentrant blocks re-entry into the pool; this event follows reads it depends on.
        // forge-lint: disable-next-line(reentrancy-events)
        emit AuctionStarted(borrower, block.number);
    }

    /// @notice Clears an auction once the position is healthy again.
    function resetAuction(address borrower) external whenOracleNormal {
        (, uint256 debtIdx) = _previewIndexes();
        PoolStorage storage s = _s();
        if (s.auctionStart[borrower] == 0) revert NoAuction();
        if (_healthFactor(borrower, debtIdx) < FixedPoint.WAD) revert PositionHealthy();
        s.auctionStart[borrower] = 0;
        // nonReentrant blocks re-entry into the pool; this event follows reads it depends on.
        // forge-lint: disable-next-line(reentrancy-events)
        emit AuctionReset(borrower);
    }

    /// @notice Executes a borrower-signed liquidation intent (SC-18 to SC-22).
    /// @dev If the position has become healthy, the auction resets and the call returns
    ///      without changing debt or collateral. A revert would also undo that reset.
    /// @dev Gas budget note: the global `isFrozen` scan costs about 150k on its own, so this
    ///      path checks only the assets it touches. Each price read reverts when that asset is
    ///      invalid, which blocks liquidation of any position that depends on a broken feed.
    function liquidate(LiquidationIntent calldata intent, bytes calldata signature, uint256 repayAmount)
        external
        nonReentrant
        whenNotPaused
    {
        // Deadline check: validator timestamp drift of a few seconds is acceptable.
        // forge-lint: disable-next-line(block-timestamp)
        if (block.timestamp > intent.deadline) revert IntentExpired();
        _accrue();
        PoolStorage storage s = _s();
        address borrower = intent.borrower;

        if (ECDSA.recover(hashIntent(intent), signature) != borrower) revert InvalidSignature();

        uint256[4] memory px; // prices read once per transaction: [debt, slot1, slot2, slot3]
        uint256 hfBefore = _healthFactorPx(borrower, s.debtIndex, px);
        if (hfBefore >= FixedPoint.WAD) {
            if (s.auctionStart[borrower] != 0) {
                s.auctionStart[borrower] = 0;
                // nonReentrant blocks re-entry into the pool; this event follows reads it depends on.
                // forge-lint: disable-next-line(reentrancy-events)
                emit AuctionReset(borrower);
            }
            return;
        }

        uint256 start = s.auctionStart[borrower];
        if (start == 0) revert NoAuction();
        _useNonce(borrower, intent.nonce);

        CollateralConfig storage cfg = s.collateral[intent.collateralAsset];
        if (!cfg.listed) revert NotListed(intent.collateralAsset);

        Position storage p = s.positions[borrower];
        uint256 debtUnits = FixedPoint.rayMulUp(p.debtScaled, s.debtIndex);
        uint256 maxByClose = (debtUnits * CLOSE_FACTOR_BPS) / FixedPoint.BPS;
        if (debtUnits - maxByClose < s.dustDebt) maxByClose = debtUnits;

        uint256 repayAmt = _min3(repayAmount, intent.maxRepay, maxByClose);
        if (repayAmt == 0) revert ZeroAmount();

        uint256 bonusBps = _bonus(start, cfg.liqBonusBps);
        uint256 seized;
        (repayAmt, seized) = _quote(intent.collateralAsset, cfg, repayAmt, bonusBps, borrower, px);
        if (seized < intent.minCollateralOut) revert CollateralBelowMin(seized, intent.minCollateralOut);

        uint256 received = _pullMeasured(s.debtAsset, msg.sender, repayAmt);
        uint256 scaled = received >= debtUnits ? p.debtScaled : FixedPoint.rayDivDown(received, s.debtIndex);
        if (scaled > p.debtScaled) scaled = p.debtScaled;
        p.debtScaled -= scaled;
        s.totalDebtScaled -= scaled;

        uint256 held = s.collateralAmount[borrower][intent.collateralAsset];
        s.collateralAmount[borrower][intent.collateralAsset] = held - seized;
        if (held == seized) _clearBit(borrower, intent.collateralAsset);
        intent.collateralAsset.safeTransfer(msg.sender, seized);

        if (p.collateralBitmap == 0 && p.debtScaled != 0) {
            _writeOff(borrower);
        }

        uint256 hfAfter = _healthFactorPx(borrower, s.debtIndex, px);
        if (hfAfter < hfBefore) revert HealthDecreased(hfBefore, hfAfter);
        if (hfAfter >= FixedPoint.WAD) {
            s.auctionStart[borrower] = 0;
            // nonReentrant blocks re-entry into the pool; this event follows reads it depends on.
            // forge-lint: disable-next-line(reentrancy-events)
            emit AuctionReset(borrower);
        }

        // nonReentrant blocks re-entry into the pool; this event follows reads it depends on.
        // forge-lint: disable-next-line(reentrancy-events)
        emit Liquidated(borrower, msg.sender, intent.collateralAsset, received, scaled, seized, bonusBps);
    }

    /// @notice Writes off debt of a borrower who has no collateral left (SC-22).
    function absorbBadDebt(address borrower) external nonReentrant {
        _accrue();
        Position storage p = _s().positions[borrower];
        if (p.collateralBitmap != 0 || p.debtScaled == 0) revert PositionHealthy();
        _writeOff(borrower);
    }

    // ---- Views ----

    /// @notice Health factor (WAD) using the indexes as they would be after the next accrual.
    function getHealthFactor(address user) external view notLocked returns (uint256) {
        (, uint256 debtIdx) = _previewIndexes();
        return _healthFactor(user, debtIdx);
    }

    /// @notice Validated oracle price (WAD). Reverts while the oracle is Frozen.
    function getPrice(address asset) external view notLocked returns (uint256) {
        return IOracle(_s().oracle).getPrice(asset);
    }

    function previewIndexes() external view returns (uint256 supplyIndex, uint256 debtIndex) {
        return _previewIndexes();
    }

    /// @notice Timestamp of the last accrual. The Risk Engine needs it to project indexes itself.
    function lastAccrual() external view returns (uint64) {
        return _s().lastAccrual;
    }

    /// @notice Indexes as last written by an accrual, without projecting pending interest.
    function storedIndexes() external view returns (uint256 supplyIndex, uint256 debtIndex) {
        PoolStorage storage s = _s();
        return (s.supplyIndex, s.debtIndex);
    }

    function debtOf(address user) external view returns (uint256) {
        (, uint256 debtIdx) = _previewIndexes();
        return FixedPoint.rayMulUp(_s().positions[user].debtScaled, debtIdx);
    }

    function supplyOf(address user) external view returns (uint256) {
        (uint256 supplyIdx,) = _previewIndexes();
        return FixedPoint.rayMulDown(_s().supplyScaled[user], supplyIdx);
    }

    function collateralOf(address user, address asset) external view returns (uint256) {
        return _s().collateralAmount[user][asset];
    }

    function positionOf(address user) external view returns (uint256 bitmap, uint256 debtScaled) {
        Position storage p = _s().positions[user];
        return (p.collateralBitmap, p.debtScaled);
    }

    function auctionStartOf(address borrower) external view returns (uint256) {
        return _s().auctionStart[borrower];
    }

    function bonusBpsAt(address borrower, address collateralAsset) external view returns (uint256) {
        PoolStorage storage s = _s();
        uint256 start = s.auctionStart[borrower];
        if (start == 0) return 0;
        return _bonus(start, s.collateral[collateralAsset].liqBonusBps);
    }

    function totals()
        external
        view
        returns (uint256 totalSupplyScaled, uint256 totalDebtScaled, uint256 reserves, uint256 deficit)
    {
        PoolStorage storage s = _s();
        return (s.totalSupplyScaled, s.totalDebtScaled, s.reserves, s.deficit);
    }

    function isPaused() external view returns (bool) {
        return _s().paused;
    }

    /// @notice EIP-712 domain separator. Bound to this proxy address and the chain.
    function DOMAIN_SEPARATOR() public view returns (bytes32) {
        return keccak256(
            abi.encode(EIP712_DOMAIN_TYPEHASH, NAME_HASH, VERSION_HASH, block.chainid, address(this))
        );
    }

    function hashIntent(LiquidationIntent calldata intent) public view returns (bytes32) {
        bytes32 structHash = keccak256(
            abi.encode(
                LIQUIDATION_INTENT_TYPEHASH,
                intent.borrower,
                intent.collateralAsset,
                intent.maxRepay,
                intent.minCollateralOut,
                intent.nonce,
                intent.deadline
            )
        );
        return keccak256(abi.encodePacked("\x19\x01", DOMAIN_SEPARATOR(), structHash));
    }

    function nonceUsed(address user, uint256 nonce) external view returns (bool) {
        uint256 word = _s().nonceBitmap[user][nonce >> 8];
        return (word >> (nonce & 0xff)) & 1 == 1;
    }

    // ---- Internal: interest ----

    /// @dev Computes the indexes and the reserve cut for the time since the last accrual.
    function _previewIndexes() internal view returns (uint256 supplyIdx, uint256 debtIdx) {
        PoolStorage storage s = _s();
        supplyIdx = s.supplyIndex;
        debtIdx = s.debtIndex;
        uint256 dt = block.timestamp - s.lastAccrual;
        // Interest accrual tolerates validator timestamp drift.
        // forge-lint: disable-next-line(block-timestamp)
        if (dt == 0) return (supplyIdx, debtIdx);

        uint256 debtValue = FixedPoint.rayMulUp(s.totalDebtScaled, debtIdx);
        uint256 supplyValue = FixedPoint.rayMulDown(s.totalSupplyScaled, supplyIdx);
        uint256 util = InterestMath.utilization(debtValue, supplyValue);
        uint256 rate = InterestMath.ratePerSecond(InterestMath.annualBorrowRate(s.rates, util));

        uint256 newDebtIdx = FixedPoint.rayMulUp(debtIdx, InterestMath.compoundFactor(rate, dt));
        uint256 newDebtValue = FixedPoint.rayMulUp(s.totalDebtScaled, newDebtIdx);
        uint256 interest = newDebtValue - debtValue;
        uint256 reserveCut = FixedPoint.rayMulDown(interest, s.reserveFactor);

        debtIdx = newDebtIdx;
        if (s.totalSupplyScaled != 0) {
            uint256 newSupplyValue = supplyValue + interest - reserveCut;
            supplyIdx = FixedPoint.rayDivDown(newSupplyValue, s.totalSupplyScaled);
        }
    }

    function _accrue() internal {
        PoolStorage storage s = _s();
        // Interest accrual tolerates validator timestamp drift.
        // forge-lint: disable-next-line(block-timestamp)
        if (s.lastAccrual == block.timestamp) return;

        uint256 oldDebtIdx = s.debtIndex;
        uint256 oldDebtValue = FixedPoint.rayMulUp(s.totalDebtScaled, oldDebtIdx);
        (uint256 supplyIdx, uint256 debtIdx) = _previewIndexes();
        uint256 newDebtValue = FixedPoint.rayMulUp(s.totalDebtScaled, debtIdx);

        s.reserves += FixedPoint.rayMulDown(newDebtValue - oldDebtValue, s.reserveFactor);
        s.supplyIndex = supplyIdx;
        s.debtIndex = debtIdx;
        // uint64 holds timestamps until the year 584 billion, so the cast is safe.
        // forge-lint: disable-next-line(unsafe-typecast)
        s.lastAccrual = uint64(block.timestamp);
    }

    // ---- Internal: health ----

    function _healthFactor(address user, uint256 debtIdx) internal view returns (uint256) {
        uint256[4] memory px;
        return _healthFactorPx(user, debtIdx, px);
    }

    /// @dev `px` caches oracle reads for the current transaction. Zero means not yet read,
    ///      which is safe because a valid price is always non-zero.
    function _healthFactorPx(address user, uint256 debtIdx, uint256[4] memory px) internal view returns (uint256) {
        PoolStorage storage s = _s();
        Position storage p = s.positions[user];
        if (p.debtScaled == 0) return HF_NO_DEBT;

        uint256 debtUnits = FixedPoint.rayMulUp(p.debtScaled, debtIdx);
        uint256 debtValue = FixedPoint.wadMulUp(
            FixedPoint.toWadUp(debtUnits, s.debtDecimals), _debtPrice(px)
        );

        uint256 weighted = 0;
        for (uint256 m = p.collateralBitmap; m != 0; m &= m - 1) {
            uint256 bit = m & (~m + 1);
            address asset = s.collateralByMask[bit];
            uint256 amount = s.collateralAmount[user][asset];
            if (amount == 0) continue;
            CollateralConfig storage cfg = s.collateral[asset];
            uint256 price = _collateralPrice(asset, cfg.bitIndex, px);
            uint256 valueWad = FixedPoint.wadMulDown(FixedPoint.toWadDown(amount, cfg.decimals), price);
            weighted += (valueWad * cfg.liqThresholdBps) / FixedPoint.BPS;
        }
        return FixedPoint.wadDivDown(weighted, debtValue);
    }

    function _debtPrice(uint256[4] memory px) internal view returns (uint256 price) {
        price = px[0];
        if (price == 0) {
            price = IOracle(_s().oracle).getPrice(_s().debtAsset);
            px[0] = price;
        }
    }

    function _collateralPrice(address asset, uint8 bitIndex, uint256[4] memory px)
        internal
        view
        returns (uint256 price)
    {
        uint256 slot = uint256(bitIndex) + 1;
        price = px[slot];
        if (price == 0) {
            // Loop is bounded by the number of collateral bits; prices are cached in px.
            // forge-lint: disable-next-line(calls-loop)
            price = IOracle(_s().oracle).getPrice(asset);
            px[slot] = price;
        }
    }

    function _requireHealthy(address user) internal view {
        PoolStorage storage s = _s();
        if (s.positions[user].debtScaled == 0) return;
        uint256 hf = _healthFactor(user, s.debtIndex);
        if (hf < FixedPoint.WAD) revert HealthFactorTooLow(hf);
    }

    // ---- Internal: liquidation ----

    /// @dev Bonus rises linearly from 0 to `maxBonusBps` over AUCTION_BLOCKS, so it never decreases (I-4).
    function _bonus(uint256 startBlock, uint256 maxBonusBps) internal view returns (uint256) {
        uint256 elapsed = block.number - startBlock;
        if (elapsed >= AUCTION_BLOCKS) return maxBonusBps;
        return (maxBonusBps * elapsed) / AUCTION_BLOCKS;
    }

    /// @dev Converts a debt repayment into collateral to seize. If the borrower lacks the
    ///      collateral, seizes all of it and lowers the repayment to match.
    function _quote(
        address collateralAsset,
        CollateralConfig storage cfg,
        uint256 repayIn,
        uint256 bonusBps,
        address borrower,
        uint256[4] memory px
    ) internal view returns (uint256 repayOut, uint256 seized) {
        PoolStorage storage s = _s();
        uint256 debtPrice = _debtPrice(px);
        uint256 colPrice = _collateralPrice(collateralAsset, cfg.bitIndex, px);

        uint256 repayWad = FixedPoint.wadMulDown(FixedPoint.toWadDown(repayIn, s.debtDecimals), debtPrice);
        uint256 seizeWad = (repayWad * (FixedPoint.BPS + bonusBps)) / FixedPoint.BPS;
        seized = FixedPoint.fromWadDown(FixedPoint.wadDivDown(seizeWad, colPrice), cfg.decimals);
        repayOut = repayIn;

        uint256 avail = s.collateralAmount[borrower][collateralAsset];
        if (seized > avail) {
            seized = avail;
            uint256 availWad = FixedPoint.wadMulDown(FixedPoint.toWadDown(avail, cfg.decimals), colPrice);
            uint256 repayCapWad = (availWad * FixedPoint.BPS) / (FixedPoint.BPS + bonusBps);
            uint256 repayCap = FixedPoint.fromWadDown(FixedPoint.wadDivDown(repayCapWad, debtPrice), s.debtDecimals);
            if (repayCap < repayOut) repayOut = repayCap;
        }
        if (repayOut == 0 || seized == 0) revert ZeroAmount();
    }

    function _writeOff(address borrower) internal {
        PoolStorage storage s = _s();
        Position storage p = s.positions[borrower];
        uint256 scaledDebt = p.debtScaled;
        uint256 debtUnits = FixedPoint.rayMulUp(scaledDebt, s.debtIndex);
        uint256 covered = debtUnits < s.reserves ? debtUnits : s.reserves;
        uint256 shortfall = debtUnits - covered;

        s.reserves -= covered;
        s.deficit += shortfall;
        s.totalDebtScaled -= scaledDebt;
        p.debtScaled = 0;
        s.auctionStart[borrower] = 0;
        // nonReentrant blocks re-entry into the pool; this event follows reads it depends on.
        // forge-lint: disable-next-line(reentrancy-events)
        emit BadDebtWrittenOff(borrower, debtUnits, scaledDebt, covered, shortfall);
    }

    function _useNonce(address user, uint256 nonce) internal {
        PoolStorage storage s = _s();
        uint256 wordPos = nonce >> 8;
        uint256 bit = uint256(1) << (nonce & 0xff);
        uint256 word = s.nonceBitmap[user][wordPos];
        if (word & bit != 0) revert NonceUsed();
        s.nonceBitmap[user][wordPos] = word | bit;
    }

    // ---- Internal: helpers ----

    function _clearBit(address user, address asset) internal {
        PoolStorage storage s = _s();
        s.positions[user].collateralBitmap &= ~(uint256(1) << s.collateral[asset].bitIndex);
    }

    /// @dev Pulls `amount` and returns what the pool actually received (SC-12).
    function _pullMeasured(address token, address from, uint256 amount) internal returns (uint256 received) {
        uint256 before = SafeTransfer.balanceOf(token, address(this));
        token.safeTransferFrom(from, address(this), amount);
        received = SafeTransfer.balanceOf(token, address(this)) - before;
    }

    function _isLocked() internal view returns (bool locked) {
        bytes32 slot = REENTRANCY_LOCK_SLOT;
        assembly {
            locked := tload(slot)
        }
    }

    function _setLock(bool value) internal {
        bytes32 slot = REENTRANCY_LOCK_SLOT;
        assembly {
            tstore(slot, value)
        }
    }

    function _s() internal pure returns (PoolStorage storage s) {
        bytes32 loc = POOL_STORAGE_LOCATION;
        assembly {
            s.slot := loc
        }
    }

    function _min3(uint256 a, uint256 b, uint256 c) internal pure returns (uint256) {
        uint256 m = a < b ? a : b;
        return m < c ? m : c;
    }
}
