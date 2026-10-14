# Vigil MVP Brief

## 1. Overview

Vigil is a lending protocol plus a set of Rust microservices that watch it, price risk in real time, and liquidate unhealthy positions. The full project spec is deliberately enormous; this MVP cuts its breadth but keeps its hardest problems intact: reorg-safe event processing, bit-exact math parity between Solidity and Rust, nonce-safe transaction submission, and resistance to oracle manipulation.

Every feature that remains in the MVP must work under adversarial conditions. Teams are graded on a scripted scenario that crashes prices, reorgs the chain, attacks the oracle and kills a service mid-run. A system that only works on the happy path will fail it.

### 1.1 Learning goals

- Design upgrade-safe contract storage and prove it survives an upgrade.
- Write fixed-point financial math with deliberate rounding, and prove it with invariant and differential fuzzing.
- Defend a protocol against hostile tokens, reentrancy and oracle manipulation, and demonstrate the defences with a working attack.
- Build event-driven services that stay correct through chain reorgs, crashes and restarts.
- Manage transaction submission (nonces, fee bumps, dropped transactions) the way production keepers do.

## 2. MVP Scope at a Glance

Anything in the right-hand column is out of scope for the MVP and must not be built until the MVP passes acceptance. It becomes the v2 roadmap (Section 12).

| Area | In the MVP | Deferred to v2 |
|---|---|---|
| Upgradeability | UUPS proxy, ERC-7201 namespaced storage, storage-layout check in CI | EIP-2535 Diamond facets; timelock + multisig governance |
| Positions | Up to 3 collateral assets, 1 debt asset (stablecoin), bitmap config | 8 collateral / 4 debt cross-margin |
| Interest | Scaled balances, per-second compounding (Taylor approximation), kinked rate model | Governance-controlled per-asset parameters |
| Hostile tokens | Fee-on-transfer, missing return values, 6 / 8 / 18 decimals | Rebasing, ERC-777 hooks, blocklist tokens |
| Oracle | Chainlink-style feed + TWAP mock; deviation breaker (Normal / Frozen) | Pyth-style pull oracle, median of 3, Degraded state |
| Liquidation | Linear Dutch auction, 50% close factor, EIP-712 intents, bitmap nonces | Exponential decay, Merkle batches, Permit2, EIP-1271 |
| Safety | Transient-storage lock, read-only reentrancy guard, reserve-backed bad debt | Bad-debt socialization, ERC-3156 flashloans |
| Verification | Invariant suite, TWAP-attack PoC, differential fuzzing, liquidation gas budget | Halmos proofs, mutation testing, full gas table |
| Services | Indexer, Risk Engine, Liquidator (3 services) | Separate State, Strategist, Signer, Relayer; threshold signing |
| Chaos | One service kill, one 2-block reorg | Random kills, bus delays, deep reorgs, multi-chain |

## 3. System Architecture

Three Rust services sit between the chain and the protocol. Events flow one way through NATS JetStream; the Liquidator writes back to the chain.

```
anvil (chain)
   │ new heads + logs
   v
[Indexer] ──events / retracts──> NATS ──> [Risk Engine] <──> Postgres
                                              │
                                liquidatable signals
                                              v
anvil <──── signed intents / txs ──────── [Liquidator]
```

| Component | Responsibility | Talks to |
|---|---|---|
| Contracts | Lending pool, oracle adapter, mock AMM, liquidation settlement | Liquidator (txs), Indexer (logs) |
| Indexer | Ingests blocks and logs, detects reorgs, publishes events and retractions | anvil, NATS, Postgres (cursor) |
| Risk Engine | Maintains positions, mirrors protocol math, emits liquidation signals | NATS, Postgres |
| Liquidator | Chooses when to strike, signs intents, manages nonces and submission | NATS, anvil |

## 4. On-Chain Requirements

Each requirement has an ID. Pull requests must reference the IDs they implement, and each requirement must have the test named in the "Verified by" column.

### 4.1 Proxy and storage

| ID | Requirement | Verified by |
|---|---|---|
| SC-01 | Protocol is deployed behind a UUPS proxy. The implementation disables initializers in its constructor. | Test: calling `initialize` on the implementation reverts. |
| SC-02 | All protocol state lives in one ERC-7201 namespaced struct. No state variables are declared in the implementation. | CI script diffs `forge inspect` storageLayout against a committed baseline. |
| SC-03 | Upgrading v1 → v2, where v2 appends a field to the Position struct, preserves every balance. | Upgrade test with 50 open positions; state compared before and after. |
| SC-04 | Owner can pause. Unpause is only allowed when the oracle is in the Normal state. | Unit tests. |

### 4.2 Accounting and interest

| ID | Requirement | Verified by |
|---|---|---|
| SC-05 | Deposits and debt are stored as scaled balances against cumulative indexes (RAY precision). | Unit tests on index updates. |
| SC-06 | Indexes compound per second using a 3-term Taylor approximation of (1 + r)^t, in a fixed-point library the team writes (no imports). | Differential fuzz against the Rust implementation. |
| SC-07 | Every mul/div states its rounding direction. Debt rounds up, deposits round down. | Invariant I-2. |
| SC-08 | Kinked utilization rate model: base rate, slope 1, slope 2, optimal utilization. | Unit tests at 0%, optimal and 100% utilization. |
| SC-09 | Enabled collateral is a uint256 bitmap; the health-factor loop walks only set bits. | Gas test: cost scales with enabled collateral. |
| SC-10 | Borrow, withdraw and disable-collateral revert if the resulting health factor is below 1. | Unit tests. |

### 4.3 Hostile tokens

| ID | Requirement | Verified by |
|---|---|---|
| SC-11 | SafeTransfer library written in inline assembly that handles tokens returning no value. | Unit tests with no-return-value mock. |
| SC-12 | Fee-on-transfer tokens: credit the measured balance delta, never the amount argument. | Unit tests with fee-on-transfer mock. |
| SC-13 | Decimal normalization for 6, 8 and 18-decimal tokens, rounding in the protocol's favour. | Unit tests per decimal width. |

### 4.4 Oracle

| ID | Requirement | Verified by |
|---|---|---|
| SC-14 | Chainlink-style feed with staleness, zero/negative-answer and min/max bound checks. | Unit tests per check. |
| SC-15 | TWAP read from a mock AMM the team writes, storing cumulative price observations. | Unit tests on observations. |
| SC-16 | If either source is invalid, or they deviate beyond a threshold, the oracle is Frozen: only repay and add-collateral are allowed. No borrows, no liquidations. | Unit tests per state. |
| SC-17 | Attacker contract flash-borrows, pushes the AMM spot price, and tries to trigger an unfair liquidation. The attack must fail. | TWAP-attack PoC test. |

### 4.5 Liquidation

| ID | Requirement | Verified by |
|---|---|---|
| SC-18 | Once health factor < 1, anyone may call `startAuction(borrower)`. The liquidation bonus rises linearly from 0 to the max over N blocks. The auction resets if the position becomes healthy. | Unit tests. |
| SC-19 | Close factor: at most 50% of debt per liquidation, unless remaining debt would fall below a dust threshold. | Unit tests. |
| SC-20 | Liquidations execute from EIP-712 signed intents (borrower, collateral asset, max repay, min collateral out, nonce, deadline). Unordered nonces stored in a bitmap, Permit2-style. | Unit tests on signature and nonce handling. |
| SC-21 | A liquidation never lowers the position's health factor. | Invariant I-3. |
| SC-22 | If collateral runs out, remaining debt is written off against the reserve. A shortfall is recorded as protocol deficit — no underflow, no revert. | Unit tests. |

### 4.6 Reentrancy and gas

| ID | Requirement | Verified by |
|---|---|---|
| SC-23 | Reentrancy lock implemented with EIP-1153 transient storage (tstore/tload in assembly). | Unit tests. |
| SC-24 | View functions (`getHealthFactor`, `getPrice`) revert while the lock is held, preventing read-only reentrancy. | Unit tests. |
| SC-25 | Single-asset liquidation costs at most 220k gas. | Gas test. |

### 4.7 Required invariants

Use handler contracts and ghost variables. Each invariant runs at least 256 runs × depth 100 in CI.

| ID | Invariant |
|---|---|
| I-1 | Sum over accounts of scaled balance × index equals recorded totals, within 1 wei per account. |
| I-2 | No sequence of deposit, borrow, repay and withdraw lets an account extract more than it deposited plus accrued interest. |
| I-3 | Every liquidation leaves the liquidated position's health factor greater than or equal to before. |
| I-4 | During an auction, the liquidation bonus is non-decreasing block over block. |
| I-5 | While the oracle is Frozen, total debt increases only through interest accrual. |

## 5. Off-Chain Requirements

### 5.1 Indexer

| ID | Requirement |
|---|---|
| RS-01 | Subscribes to new heads, fetches logs, decodes them with alloy `sol!` bindings and publishes to NATS JetStream. |
| RS-02 | Keeps the last 10 block hashes. On a parent-hash mismatch, walks back to the common ancestor and publishes Retract events for orphaned logs before publishing new ones. |
| RS-03 | Persists its cursor (block number + hash) in Postgres. A restart resumes with no gaps and no duplicates. |
| RS-04 | Every event carries an idempotency key (block_hash, log_index) and a trace ID. |

### 5.2 Risk Engine

| ID | Requirement |
|---|---|
| RS-05 | Maintains positions in Postgres. Processing is exactly-once: the idempotency key is recorded in the same database transaction as the state change. |
| RS-06 | Retract events restore state to its pre-orphan values (via an event log or stored reverse deltas). |
| RS-07 | Fixed-point math (WAD/RAY, rounding directions) matches Solidity bit-for-bit. |
| RS-08 | Mirrors the oracle breaker and emits no liquidation signals while Frozen. |
| RS-09 | Signals carry the block number they were computed at. Signals older than 2 blocks are stale. |
| RS-10 | Exposes a position-snapshot endpoint (gRPC or HTTP). |

### 5.3 Liquidator

| ID | Requirement |
|---|---|
| RS-11 | Defines a Strategy trait with at least two implementations, e.g. strike immediately vs. wait until bonus ≥ gas cost × margin. |
| RS-12 | Signing sits behind a Signer trait. The key is loaded from an encrypted keystore, never logged, and touched by no other module. |
| RS-13 | Nonce manager: one wallet, concurrent submissions, no gaps or collisions. Reconciles with the chain on startup. |
| RS-14 | A transaction unconfirmed after N blocks is replaced with at least a 12.5% fee bump. |
| RS-15 | Detects its own transaction being reorged out and resubmits, or drops it if the position is no longer liquidatable. |
| RS-16 | Re-checks signal freshness and health factor immediately before signing. Never acts on a stale signal. |

## 6. Rust Microservice Design Guide

This section explains how the three services should be designed, what makes each one hard, and the decisions teams must defend in their design write-up. The contracts fail loudly in Foundry; the services fail quietly, in timing-dependent ways, and usually only under the conditions the acceptance scenario creates.

### 6.1 Overall shape

Three services live in one Cargo workspace with a shared `common` crate. They never share memory or call each other's internals. They communicate only through NATS JetStream messages and, for snapshots, a small HTTP or gRPC API. Every message can arrive twice, late, out of order, or describe a block that no longer exists, and each service has to stay correct anyway.

The `common` crate must be built first. It holds the fixed-point math (Wad and Ray newtypes over U256 with explicit `mul_down` / `mul_up` methods), the message schemas, and the tracing setup. If each service grows its own copy of the math, parity with Solidity is lost before week 4 starts.

| Concern | Recommended crates |
|---|---|
| Async runtime | tokio |
| RPC and contract bindings | alloy (`sol!` bindings generated from the Foundry ABI) |
| Message bus | async-nats (JetStream) |
| Database | sqlx with Postgres |
| Observability | tracing, tracing-opentelemetry, Prometheus exporter |
| Snapshot API | axum (HTTP) or tonic (gRPC) |
| Testing | proptest, testcontainers |

### 6.2 Indexer

On paper the Indexer only watches blocks, decodes logs and publishes them. The hard part is that the chain can change its mind.

Its core data structure is a ring of the last ~10 (number, hash) pairs. For every new head it checks that `parent_hash` matches the stored hash at `number − 1`. If not, the chain has reorged: the Indexer walks back, fetching blocks by number, until it finds a block whose hash matches what it stored — the common ancestor. Every log published from orphaned blocks gets a Retract message, in reverse order, before any logs from the new canonical blocks.

**Common mistakes**

- **Trusting the subscription.** A WebSocket `newHeads` stream can skip blocks, especially across a reconnect. Treat each head as a hint that something new exists up to N, and fill any gap by fetching blocks by number.
- **Ordering cursor and publish wrongly.** Publish then crash before saving the cursor, and events are republished on restart — acceptable, because consumers deduplicate on (block_hash, log_index). Save the cursor then crash before publishing, and events are lost forever. Teams must explain why they chose at-least-once delivery with idempotent consumers.
- **Querying logs by block number.** During a reorg this can return logs from a different block than the one just validated. Fetch logs by block hash (`eth_getLogs` with `blockHash`).

### 6.3 Risk Engine

This service carries the most state and the hardest correctness requirements.

**Exactly-once processing.** Record the idempotency key in the same Postgres transaction as the state change: insert into `processed_events` (with a unique constraint), apply the position update, commit. If the insert conflicts, the event was already applied, so skip it. In-memory deduplication breaks on the first restart.

**Retractions.** Teams pick one approach and justify it in the write-up:

- **Event log:** store every applied event and rebuild affected positions without the orphaned events. Simple and correct, but slow if rebuilding from far back.
- **Reverse deltas:** store each event's before-state and undo in reverse order. Fast, but every event type needs a correct inverse, and that is where bugs hide.

Only the last ~10 blocks can ever be retracted, so a hybrid — snapshots at finalized depth plus a short replayable tail — is what production indexers actually do.

**Math parity.** Health factor in Rust must match Solidity bit for bit: same operation order, same rounding direction on every step, same Taylor-approximation terms for interest. The proptest harness generates random positions and prices, sets that state on anvil, calls `getHealthFactor`, and asserts equality with the Rust result. On failure, proptest shrinks the input to a minimal counterexample. Expect the first run to diverge on interest accrual: computing rate × time before or after scaling changes the last digits.

**Freshness and the breaker.** Each signal carries the block number it was computed at. The engine mirrors the oracle breaker and stays silent while the protocol is Frozen, instead of sending the Liquidator into transactions that will revert.

### 6.4 Liquidator

The Liquidator combines a strategy problem with a classic distributed-systems problem: getting a transaction mined exactly once.

**Strategy trait.** Keeping strategies behind a trait makes them pluggable and testable without a chain:

```rust
#[async_trait]
pub trait Strategy: Send + Sync {
    async fn decide(&self, signal: &LiquidationSignal, market: &MarketView)
        -> anyhow::Result<Decision>; // Strike(params) | Wait(until_block) | Skip(reason)
}
```

"Strike immediately" is trivial. "Wait until the auction bonus beats gas cost by a margin" forces teams to model the auction curve in Rust, which must also match the contract.

**Signer isolation.** The key lives in one module behind a Signer trait, loaded from an encrypted keystore and never formatted into logs. A CI grep that fails on `Debug` derives or `{:?}` near key types is crude but effective. In v2 the Signer becomes a separate service with threshold signing, and only the trait implementation changes.

**Nonce manager.** This is the component most likely to fail the acceptance scenario. Submissions are concurrent, nonces must have no gaps, and any transaction may be dropped, replaced or reorged out. A workable design:

- One task owns nonce state; other tasks request nonces over a channel. A shared `AtomicU64` does not work: a nonce handed out for a transaction that then fails to sign or send leaves a gap.
- Track every in-flight transaction: nonce, hash, fee, block sent, and the intent it carries.
- After N blocks unconfirmed, resend with the same nonce and at least a 12.5% fee bump, the minimum most nodes accept for replacement.
- On startup and after any reorg, reconcile with the chain by comparing `eth_getTransactionCount` (latest and pending) against the in-flight table.
- If a transaction is reorged out and the position is now healthy, the nonce must still be filled (typically with a zero-value self-transfer), or every later transaction stalls behind the gap. Almost every team misses this until the scenario kills the Liquidator mid-crash.

**Last-moment check.** Immediately before signing, re-read health factor and signal age. A signal from block N acted on at block N+3 during a 25% crash can easily target a position someone else has already liquidated.

### 6.5 Cross-service concerns

- **Tracing:** a trace ID created at the Indexer travels in NATS headers, so one liquidation appears as a single span chain: block seen, event applied, signal emitted, decision made, intent signed, transaction confirmed. When the scenario fails, this is how teams find out why.
- **Fail closed:** if the Risk Engine is down or NATS is lagging, the Liquidator does nothing. Missing a liquidation is better than firing one from stale state.

**Testing layers**

| Layer | What it covers | Tools |
|---|---|---|
| 1. Unit | Fixed-point math, strategies, message schemas | cargo test, proptest |
| 2. Integration | Services against real anvil and Postgres | testcontainers or the compose stack |
| 3. Chaos | Reorgs, crashes, stranded transactions | evm_snapshot / evm_revert, docker kill, paused mining |

Most real bugs only appear at layer 3, which is why chaos testing is mandatory in week 6 rather than optional.

### 6.6 Instructor checkpoints

The most valuable checkpoint is the end of week 4. By then the math-parity proptest must run 10,000 cases with zero divergence and the reorg test must pass. Teams that get there usually finish; teams that don't are typically skipping the common crate or deduplicating in memory, and catching that early saves them.

In code review, ask every team these three questions. Their answers must also appear in the design write-up:

1. What happens if the service crashes between any two lines of your event handler?
2. Which of your messages can arrive twice, and what stops a double-apply?
3. If your transaction at nonce 17 is reorged out and the position is now healthy, what happens to nonces 18 and 19?

## 7. Cross-Cutting Requirements

- **One command:** `docker compose up` starts anvil, deploys and seeds the contracts, and starts all services.
- **Tracing:** a trace ID propagates through NATS headers, so a single liquidation is traceable from block to confirmation.
- **Metrics:** Prometheus counters for events processed, reorgs handled, signals emitted, and transactions submitted, confirmed and replaced.
- **Fail closed:** each service has a health endpoint. If an upstream dependency is unhealthy, the Liquidator stops acting rather than guessing.
- **CI:** `forge test`, `forge snapshot --check`, the storage-layout check, `cargo test` and `cargo clippy -- -D warnings` all pass on every PR.

## 8. Acceptance Scenario

The instructor runs this script against each team's system. Teams may dry-run it in week 6 but do not see the exact seed values in advance.

1. Deploy and list three collateral assets — one fee-on-transfer, one 6-decimal, one 8-decimal — plus the stablecoin debt asset.
2. Open 100 borrower positions with health factors randomly between 1.05 and 1.6.
3. Drop collateral prices 25% over 15 blocks.
4. At crash block 8, inject a 2-block reorg.
5. At crash block 10, run the TWAP-manipulation attacker.
6. At crash block 12, kill the Liquidator container and restart it 3 blocks later.
7. Continue for 30 blocks after the crash ends, then collect metrics and chain state.

### 8.1 Pass criteria

| Metric | Required |
|---|---|
| Duplicate liquidations | 0 |
| Nonce gaps after the run | 0 |
| Successful manipulation attacks | 0 |
| Rust / Solidity health-factor divergences | 0 |
| Indexer gaps or duplicates after the reorg | 0 |
| Positions still liquidatable at the end (outside Frozen periods) | 0 |
| Bad debt left in the protocol | Reported; scored relative to the cohort |

Failing any hard-zero criterion caps the scenario score at 50%.

## 9. Grading Rubric

| Component | Weight | What earns full marks |
|---|---|---|
| Smart contracts | 30% | All SC requirements and invariants pass; attack PoC fails; gas budget met |
| Off-chain services | 25% | All RS requirements pass, including reorg and restart tests |
| Acceptance scenario | 25% | All hard-zero criteria met; low bad debt relative to cohort |
| Engineering quality | 10% | Green CI, clean clippy, traceable liquidations, useful metrics |
| Design write-up | 10% | Clear reasoning on reorg strategy, rounding, nonce design and known limits |

### 9.1 Design write-up

A 3–5 page document submitted with the final code. It must explain the reorg-handling strategy, every rounding decision in the fixed-point library, how the nonce manager avoids gaps, how the oracle breaker was tuned, and what the team knows is still weak. It must also answer the three review questions in Section 6.6.

## 10. Milestones

| Week | Focus | Exit criteria |
|---|---|---|
| 1 | Proxy, storage, fixed-point library, accounting, interest | SC-01 – SC-10 passing; differential-fuzz harness in place |
| 2 | Hostile tokens, oracle, liquidation, reentrancy, invariants | SC-11 – SC-25 and I-1 – I-5 passing; attack PoC committed |
| 3 | Indexer with reorg handling; Docker Compose setup | RS-01 – RS-04 passing; compose brings up chain + Indexer |
| 4 | Risk Engine and math parity | RS-05 – RS-10 passing; 10k-case parity run with zero divergence |
| 5 | Liquidator, nonce manager, end-to-end flow | RS-11 – RS-16 passing; one liquidation traced end to end |
| 6 | Chaos testing, scenario dry run, write-up | Dry run passes; write-up submitted; final graded run |

## 11. Suggested Repository Layout

```
vigil/
├── contracts/            # Foundry project
│   ├── src/              # pool, oracle, AMM mock, settlement, libs
│   ├── test/             # unit, invariant handlers, attack PoC
│   └── script/           # deploy + seed scripts
├── services/             # Cargo workspace
│   ├── common/           # fixed-point math, message types, tracing
│   ├── indexer/
│   ├── risk-engine/
│   └── liquidator/
├── scenario/             # acceptance + chaos scripts
├── migrations/           # Postgres schema
├── docker-compose.yml
└── docs/design.md
```

## 12. Post-MVP Roadmap (v2)

Teams that pass acceptance early can pick up v2 items. Suggested order, from most to least instructive:

| Track | v2 additions |
|---|---|
| Contracts | Diamond facets with namespaced storage; timelock + multisig governance; exponential auction decay; Merkle batch liquidations; Permit2 and EIP-1271 signatures; ERC-3156 flash loans; bad-debt socialization; full hostile-token matrix |
| Oracle | Pyth-style pull oracle; median of three sources; Normal / Degraded / Frozen states |
| Verification | Halmos symbolic proofs for the math library; mutation testing at ≥90% kill rate; CI gas budgets for every operation |
| Services | Split into State, Strategist, Signer and Relayer; 2-of-3 threshold signing; private bundle submission; multi-wallet relayer |
| Chaos | Random service kills, bus delays, deeper reorgs, multi-chain deployment with per-chain nonce management |
| Red team | Teams swap codebases for a week, submit findings with PoCs, and fix findings against their own code |
