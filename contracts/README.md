# Vigil Contracts: a step-by-step guide

This guide explains the smart contracts in `contracts/` in plain language. It is written for someone who has never written Solidity. It goes file by file and walks through each section in the order it appears in the code.

If you only read one thing: **Vigil is a lending pool.** People deposit money to earn interest. Other people deposit collateral (other tokens) and borrow the money. If a borrower's collateral becomes worth too little, anyone can buy that collateral at a discount to repay the loan. The contracts enforce all of these rules on the blockchain, so nobody has to be trusted.

---

## Part 0: Vocabulary

| Word | Meaning |
|---|---|
| **Contract** | A program stored on the blockchain. Once deployed, anyone can call its functions. |
| **Token** | A digital currency defined by a contract (for example, a "stablecoin" worth one dollar). |
| **Collateral** | Tokens a borrower locks up as a guarantee for a loan. |
| **Debt** | The money a borrower owes. |
| **Health factor (HF)** | A number showing how safe a loan is. Above 1 is safe. Below 1 means the position can be liquidated. |
| **Liquidation** | Someone else repays part of a bad loan and receives the borrower's collateral at a discount. |
| **Oracle** | A source of prices. The contracts need prices to know what collateral is worth. |
| **Proxy** | A small contract that users talk to. It forwards calls to the real logic contract, so the logic can be upgraded later without moving anyone's funds. |
| **Wei / WAD / RAY** | Tiny units. Solidity has no decimals, so amounts are stored as whole numbers scaled by 10^18 (WAD) or 10^27 (RAY). `1e18` means "one". |
| **Basis points (BPS)** | One hundredth of a percent. `10000` BPS is 100%, `8000` is 80%. |
| **Event** | A message the contract writes to the blockchain log. Off-chain services listen for these. |
| **Nonce** | A number used only once. It stops the same signed permission from being used twice. |

---

## Part 1: The folder layout

```
contracts/
├── src/                 The contracts themselves
│   ├── LendingPool.sol          The main lending logic (825 lines)
│   ├── libs/                    Reusable math and safe-transfer helpers
│   │   ├── FixedPoint.sol       WAD/RAY decimal arithmetic
│   │   ├── InterestMath.sol     Interest rate model
│   │   └── SafeTransfer.sol     Safe token transfers
│   ├── oracle/
│   │   └── OracleAdapter.sol    Turns price feeds into one trusted price
│   ├── interfaces/              "Shapes" other contracts must have
│   └── mocks/                   Fake tokens, feeds and an AMM for testing
├── script/
│   └── Deploy.s.sol             Deploys everything to a chain
├── test/                        Automated tests
├── config/market.json           Starting prices for the local test market
├── deployments/                 Output addresses written after deploying
├── foundry.toml                 Build settings
└── storage-layout.baseline.json A saved snapshot used to check upgrades are safe
```

`lib/` holds third-party code (OpenZeppelin, a standard library). It is not written by this project.

---

## Part 2: `src/libs/FixedPoint.sol`: decimal math

**Why this exists.** Solidity only does whole-number math. `0.5 * 3` is not expressible, so we store numbers scaled up (`0.5` becomes `5e17` in WAD). This file provides the multiply and divide helpers for that.

**Step by step:**

1. **Constants (lines 9–11).** `WAD = 1e18`, `RAY = 1e27`, `BPS = 1e4`, and one year in seconds.
2. **`mulDivDown(a, b, d)`.** Computes `a × b ÷ d`, rounding *down*. It first checks that `d` isn't zero, because dividing by zero would crash.
3. **`mulDivUp(a, b, d)`.** Same, but rounds *up*. It does `(p - 1) / d + 1`, a trick that rounds up with integer math. Rounding direction matters: the contract always rounds in the protocol's favour, so tiny rounding errors can't be exploited to steal value.
4. **`wadMul…`, `wadDiv…`, `rayMul…`, `rayDiv…`.** Shortcuts that plug in `WAD` or `RAY` for `d`.
5. **`toWadDown`, `toWadUp`, `fromWadDown`, `fromWadUp`.** Convert between token decimals and WAD. A USDC-like token has 6 decimals, so `1 USDC` is `1e6` raw units and becomes `1e18` WAD.

---

## Part 3: `src/libs/InterestMath.sol`: how interest grows

**What it does.** It decides how fast debt grows. The rate depends on **utilization**: how much of the deposited money is currently borrowed.

**Step by step:**

1. **`RateParams` struct.** Four numbers: a base rate, `slope1`, `slope2`, and the "optimal" utilization point.
2. **`utilization(totalDebt, totalSupply)`.** Returns debt ÷ supply, capped at 100%. If nobody has deposited anything, it returns 0 so there's no divide-by-zero.
3. **`annualBorrowRate(p, util)`.** The rate is a "kinked" line:
   - Below the optimal point: the rate climbs gently (`slope1`).
   - Above it: the rate climbs steeply (`slope2`). This pushes borrowers to repay when cash runs low.
4. **`ratePerSecond`.** Converts the yearly rate into a per-second rate, rounded up.
5. **`compoundFactor(rate, seconds)`.** Interest compounds, meaning interest earns interest. Computing `(1 + r)^t` exactly is expensive, so the code uses the first three terms of a Taylor series (a standard approximation). Each term is rounded up so debt is never under-counted.

---

## Part 4: `src/libs/SafeTransfer.sol`: moving tokens safely

**Why this exists.** Some tokens are badly written. Some don't return `true` after a transfer, some return nothing at all (old USDT is one). The standard Solidity call would fail on those.

**Step by step:**

1. **`safeTransfer` / `safeTransferFrom`.** Call the token's `transfer` or `transferFrom` function, using the raw function ID (`0xa9059cbb` is "transfer").
2. **`balanceOf`.** Reads a token balance with a `staticcall`, which is a read-only call that can't change anything.
3. **`_callOptionalReturn`.**
   - It first checks the token actually has code. An address with no code would appear to succeed silently.
   - It makes the call. If the call fails, it passes the token's revert reason back so the cause is visible.
   - It accepts "no return value" or "returned true", and rejects anything else.

---

## Part 5: `src/interfaces/`: the shapes other contracts must have

These files contain no logic. They just list the functions another contract must offer.

- **`IOracle.sol`**: `getPrice(asset)` and `isFrozen()`. The lending pool talks to any oracle through this shape.
- **`IAggregatorV3.sol`**: a subset of the Chainlink price-feed interface (`decimals`, `latestRoundData`).
- **`ITwapSource.sol`**: `twap(window)`, a time-averaged price from an exchange.

---

## Part 6: `src/oracle/OracleAdapter.sol`: deciding what a price is worth

**Why this exists.** A single price feed can be wrong or manipulated. This contract combines two checks, and if either fails, the whole system freezes.

**Step by step:**

1. **Feed config.** For each asset: the spot-price feed, the TWAP source, how old a price may be (`maxStaleness`), the price bounds (`minPrice`, `maxPrice`), and the allowed deviation (`maxDeviationBps`). Fields are ordered to pack into fewer storage slots, which saves gas.
2. **`setFeed`** (owner only). Stores the config. It rejects impossible bounds, and it tracks each asset once in the `assets` list.
3. **`getPrice(asset)`.** Returns a validated price in WAD, or reverts with `OracleFrozen` if the price isn't trustworthy.
4. **`_read(asset)`.** The actual check, returning `(price, valid)`. It never reverts:
   - The feed's answer must be positive.
   - The update time must not be in the future, and not older than `maxStaleness`.
   - The price must be inside `[minPrice, maxPrice]`.
   - The TWAP must exist, and it must agree with the spot price within the deviation limit.
5. **`isFrozen()`.** Loops over all assets. If any one fails `_read`, the oracle is frozen.
6. **`_withinDeviation(a, b, maxBps)`.** Checks `|a − b| ≤ maxBps × a`, using integer math to avoid division.

**Plain-language summary:** prices are trusted only when the feed is fresh, in range, and matches the time-averaged price from the exchange. Otherwise the pool pauses risky actions.

---

## Part 7: `src/LendingPool.sol`: the main contract (825 lines)

This is the heart of the system. It is walked through in the order it appears.

### 7.1 Header and imports (lines 1–12)

- `Initializable`, `UUPSUpgradeable`, `OwnableUpgradeable`: standard building blocks. They let the contract be upgraded by its owner, and they make sure the setup function runs only once.
- `ECDSA`: checks signatures.
- `IERC20Metadata`: lets the contract ask a token for its name and decimals.

### 7.2 Constants (lines 22–34)

- `MAX_COLLATERAL = 3`: at most three collateral types.
- `AUCTION_BLOCKS = 10`: a liquidation auction's bonus reaches its full size after 10 blocks.
- `CLOSE_FACTOR_BPS = 5000`: a single liquidation can repay at most half the debt.
- `HF_NO_DEBT`: a sentinel meaning "no debt", set to the largest possible number.
- `LIQUIDATION_INTENT_TYPEHASH` and the domain constants: fingerprints used for signing (see 7.13).
- `REENTRANCY_LOCK_SLOT`: a storage location used as a "door lock" (see 7.4).
- `POOL_STORAGE_LOCATION`: where all the pool's state is kept (see 7.3).

### 7.3 Data structures (lines 36–84)

- **`CollateralConfig`**: for each collateral: whether it is listed, its decimals, which bit it uses in a position's bitmap, its liquidation threshold (for example `8000` = 80%), and its maximum liquidation bonus.
- **`Position`**: a borrower's collateral bitmap (which collateral types are switched on) and their scaled debt.
- **`LiquidationIntent`**: a signed permission to liquidate, with the borrower, collateral asset, maximum repay, minimum collateral to receive, nonce, and deadline.
- **`PoolStorage`**: every piece of state in one struct. It uses the ERC-7201 standard, which stores it at a fixed hashed location. That keeps the layout safe when the contract is upgraded. Key fields include the debt asset, paused flag, oracle address, interest indexes, totals, reserves, and mappings from addresses to collateral, positions, and nonces.

### 7.4 Events (lines 86–109)

Events are log entries that services listen to. Examples: `Borrowed`, `Repaid`, `Liquidated`, `BadDebtWrittenOff`. The Rust risk engine replays these to keep its own copy of every position.

### 7.5 Errors (lines 111–141)

Named failure reasons. For example, `HealthFactorTooLow` means a withdrawal or borrow would leave the position unsafe. Named errors are cheaper than text messages.

### 7.6 Modifiers: reusable checks (lines 143–170)

A modifier is a check that runs before a function. The functions below use these:

- **`nonReentrant`**: sets a "locked" flag in transient storage before the function runs, and clears it afterwards. If a function is entered again while locked (a reentrancy attack), it reverts. Think of it as a door that only one person can go through at a time.
- **`whenNotPaused`**: reverts if the owner has paused the pool.
- **`whenOracleNormal`**: reverts if the oracle is frozen.
- **`notLocked`**: reverts if a call is in progress. Used on read-only views, so nobody reads half-updated state.

### 7.7 Constructor and initialize (lines 172–212)

- **`constructor`** calls `_disableInitializers()`. This stops anyone from calling `initialize` directly on the logic contract. Only the proxy can be initialized.
- **`initialize`** runs once, through the proxy. It sets the owner, debt token, oracle, interest rate settings, reserve factor, and dust threshold. It also starts both interest indexes at 1 (`RAY`) and records the time.

### 7.8 Upgrades and admin (lines 214–260)

- **`_authorizeUpgrade`**: only the owner can upgrade the logic.
- **`listCollateral`** (owner only): adds a collateral type. It checks the bit index is free, the threshold is valid, and the token isn't already listed. Each collateral gets its own bit in the position bitmap.
- **`pause` / `unpause`** (owner only): pausing stops most actions. Unpausing is refused while the oracle is frozen, so the pool can't resume on bad prices.
- **`accrue`**: anyone can call it to update interest now.

### 7.9 Lender actions (lines 262–290)

- **`supply(amount)`**: a lender deposits debt tokens.
  1. Update interest first (`_accrue`), so the deposit is priced correctly.
  2. Pull the tokens and measure how many actually arrived (`_pullMeasured`). A token that takes a fee on transfer would otherwise over-credit the depositor.
  3. Convert the amount into "scaled" units, which grow with interest, and add it to the lender's balance.
- **`withdrawSupply(amount)`**: a lender takes money out.
  1. Accrue interest.
  2. Convert the amount to scaled units, rounding up so the lender can't take out slightly more than they own.
  3. Check there is enough cash left after reserving the reserves.
  4. Update the balances, emit the event, and then send the tokens. The balance update comes before the transfer, so re-entering the function would see the new state.

### 7.10 Borrower actions (lines 292–390)

- **`depositCollateral(asset, amount)`**: check the asset is listed, pull the tokens (measuring them), add to the borrower's balance, and switch on that collateral's bit.
- **`withdrawCollateral(asset, amount)`**: needs the oracle to be normal. It subtracts the amount, switches off the bit if the balance hits zero, and then checks the position is still healthy (`_requireHealthy`). If not, the whole transaction reverts, so the tokens are never sent.
- **`disableCollateral(asset)`**: stops counting an asset towards health, but leaves the tokens in place. It also checks the position stays healthy.
- **`enableCollateral(asset)`**: the reverse. Allowed while the oracle is frozen, because it only adds to safety.
- **`borrow(amount)`**:
  1. Check there is enough cash.
  2. Convert the amount to scaled debt, rounding up (the borrower never owes less than they got).
  3. Add the debt, then check health. If the position is unhealthy, the whole transaction reverts.
  4. Send the tokens and emit an event.
- **`repay(amount)`**: anyone with debt can repay, even while the oracle is frozen. The repayment is capped at the amount owed. The code subtracts scaled debt and rounds in the protocol's favour.

### 7.11 Liquidation: starting the auction (lines 392–420)

- **`startAuction(borrower)`**: anyone can call this on an unhealthy position. It records the current block number, which starts a clock. It refuses if the position is healthy or an auction is already running.
- **`resetAuction(borrower)`**: clears the auction once the position is healthy again.

### 7.12 Liquidation: executing it (lines 422–500)

This is the most complicated function. Read it in this order:

1. **Deadline check.** The signed permission has an expiry time. Past it, reject.
2. **Accrue interest.**
3. **Verify the signature.** `ECDSA.recover` works out who signed the intent. It must be the borrower. Otherwise, reject.
4. **Check the health factor before.** If the borrower is already healthy, clear the auction and return *without reverting*. Reverting would also undo the auction reset, which would be pointless.
5. **Check the auction is running.** Without one, reject.
6. **Use the nonce.** The intent's nonce is marked as spent, so it can't be used twice (`_useNonce`).
7. **Check the collateral is listed.**
8. **Work out the maximum repayment.** Up to 50% of debt (`CLOSE_FACTOR_BPS`), and not more than the intent allows.
9. **Work out the bonus.** `_bonus` grows linearly from 0 to the maximum over 10 blocks. The liquidator gets a better deal the longer they wait, but the bonus only goes up, never down.
10. **Quote the trade (`_quote`).** Convert the repayment into collateral, including the bonus. If the borrower doesn't have enough collateral, take all of it and lower the repayment to match.
11. **Check the minimum payout.** The liquidator's `minCollateralOut` protects them from getting less than they agreed to.
12. **Take the repayment** (`_pullMeasured`) and reduce the debt.
13. **Take the collateral** from the borrower and send it to the liquidator.
14. **Write off bad debt** if the borrower has no collateral left but still owes debt.
15. **Check the health factor after.** If it went *down*, revert with `HealthDecreased`. This is invariant **I3**: a liquidation must never make a position less healthy. This is the rule behind the `HealthDecreased` reverts in the logs.
16. **If the position is healthy again**, end the auction.
17. **Emit `Liquidated`.** The Rust services use this event to update their copy of the position.

### 7.13 Bad debt (lines 502–508)

**`absorbBadDebt(borrower)`**: when a borrower has no collateral left, the debt is written off. The pool covers it with its reserves first. Anything left over becomes a `deficit`.

### 7.14 Views: read-only functions (lines 510–575)

These cost nothing to call and don't change state:

- **`getHealthFactor(user)`**: the current health factor, projected to the next interest update.
- **`getPrice(asset)`**: the oracle's validated price.
- **`debtOf`, `supplyOf`, `collateralOf`, `positionOf`**: quick ways to read a user's balances.
- **`bonusBpsAt`**: the liquidation bonus right now for a borrower.
- **`totals`**: total supply, total debt, reserves and deficit.
- **`nonceUsed`**: whether a signed nonce has already been spent.

### 7.15 Signing: `DOMAIN_SEPARATOR` and `hashIntent` (lines 577–600)

These produce the exact digest the borrower signs (EIP-712). The domain is tied to the contract's address and the chain ID, so a signature for one deployment can't be replayed on another. `hashIntent` builds a hash of the intent's six fields and combines it with the domain.

**Why the frontend and Rust services use this function:** they ask the contract for its own digest instead of recomputing it, so the signature always matches.

### 7.16 Internal: interest (lines 602–640)

- **`_previewIndexes()`**: works out what the indexes would be right now, without writing anything. It uses the utilization and the rate model, compounds for the time passed, and splits the interest between lenders and reserves.
- **`_accrue()`**: does the same and saves the result. The reserves grow by the reserve share of new interest.

### 7.17 Internal: health (lines 642–700)

- **`_healthFactorPx`**: adds up each collateral's value, multiplies by its liquidation threshold, and divides by debt value. The `px` array caches oracle prices during one transaction, so each price is read at most once.
- **`_requireHealthy`**: reverts if the health factor is below 1.

### 7.18 Internal: liquidation helpers (lines 702–760)

- **`_bonus`**: the linear bonus described above.
- **`_quote`**: the price maths for a liquidation.
- **`_writeOff`**: the bad-debt write-off.
- **`_useNonce`**: marks a nonce as spent. Nonces are stored as bits in a bitmap, so one storage slot can track 256 nonces.

### 7.19 Internal: helpers (lines 762–825)

- **`_clearBit`**: switches off a collateral bit.
- **`_pullMeasured`**: pulls tokens and returns the amount that actually arrived.
- **`_isLocked` / `_setLock`**: read and write the reentrancy flag in transient storage, using assembly (`tload` / `tstore`).
- **`_s()`**: returns a pointer to the storage struct at its fixed location.

---

## Part 8: `src/mocks/`: test doubles

These are fake versions of real things, used for testing and the local demo:

- **`MockERC20`**: a basic token with a `mint` function anyone can call.
- **`MockFeeOnTransferERC20`**: takes a 1% fee on each transfer, to test that the pool credits only what arrives.
- **`MockNoReturnERC20`**: a token whose transfer returns nothing, like old USDT.
- **`MockAggregator`**: a price feed whose value the test sets directly.
- **`MockAMM`**: a fake exchange with a spot price and a history of prices, so a TWAP can be calculated. It records prices so a price pushed in the same block carries no time weight. That is what the TWAP attack test checks.

---

## Part 9: `script/Deploy.s.sol`: setting everything up

This script deploys the whole system in one go. It is run with `forge script`.

**Step by step:**

1. **Choose the deployer key.** Uses `DEPLOYER_KEY` if set, otherwise anvil's first test account.
2. **Deploy the tokens.** A debt token (vUSD, 6 decimals), a fee token, a 6-decimal token, and an 8-decimal token.
3. **Read starting prices** from `config/market.json`.
4. **Deploy price feeds and AMMs** for each token, using those prices.
5. **Deploy and configure the oracle** (`OracleAdapter`). Each token gets its feed, AMM, staleness limit, price bounds, and a 10% deviation band.
6. **Set the interest rates** (base 2%, slope1 4%, slope2 75%, optimal utilization 80%).
7. **Deploy the lending pool.** It deploys the logic contract, then a proxy pointing to it, and calls `initialize` through the proxy.
8. **List the collateral** with their thresholds and bonuses:
   - fee token: 80% threshold, 5% bonus
   - six-decimal token: 85% threshold, 5% bonus
   - eight-decimal token: 75% threshold, 10% bonus
9. **Seed liquidity.** Mints 10 million vUSD and supplies it, so borrowers have cash to draw on.
10. **Write `deployments/<chainId>.json`.** The Rust services and the frontend read addresses from this file.

---

## Part 10: `test/`: the automated checks

- **`Base.t.sol`**: shared setup used by the other tests.
- **`LendingPool.t.sol`**: unit tests for each action (deposit, borrow, liquidate and so on).
- **`Math.t.sol`**: tests the FixedPoint and InterestMath rounding.
- **`Invariants.t.sol`**: runs random sequences of actions and checks rules that must always hold. For example, `I3`: a liquidation never lowers the health factor.
- **`Harness.sol`** and **`StorageProbe.sol`**: helpers for the invariant tests and the storage-layout check.
- **`mocks/`**: test-only tokens and attackers.

Run them with:

```bash
cd contracts
forge test
```

---

## Part 11: Running the contracts

```bash
# Build
forge build

# Test (all)
forge test

# Start a local chain in another terminal
anvil --port 8545

# Deploy to it (writes deployments/31337.json)
forge script script/Deploy.s.sol --broadcast --rpc-url http://127.0.0.1:8545
```

---

## Glossary of the main ideas

- **Scaled balances.** Balances are stored divided by an index that grows with interest. Multiplying by the current index gives the real balance, so interest is applied to everyone at once without updating each account.
- **Invariant I3.** A liquidation must never reduce the health factor. If it would, the transaction reverts. This is strict. It means a position that is below 1 but where the bonus would push the health factor down cannot be liquidated, which is a known open question for the project.
- **Frozen oracle.** When prices are untrustworthy, only actions that reduce risk (repay, add collateral) are allowed.
- **Reserves and bad debt.** Part of the interest goes to reserves. If a borrower can't cover their debt, reserves absorb it first. Any shortfall is recorded as a deficit.
