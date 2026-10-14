# Vigil Services: a step-by-step guide to the Rust code

This guide explains the Rust services in `services/` in plain language. It assumes you know what a program and a database are, but not blockchains or Rust. It walks through each crate in the order that data flows through the system, and explains what each part does and why.

If you only read one thing: **the services watch the lending pool on the blockchain, decide which loans are unsafe, and send the transactions that fix them.** Each service does one job and passes messages to the next through a message queue (NATS).

---

## Part 0: Vocabulary

| Word | Meaning |
|---|---|
| **Crate** | One Rust program or library. Each folder under `services/` is a crate. |
| **Workspace** | A set of crates built together. `services/Cargo.toml` lists them. |
| **Async / tokio** | Lets a program wait on the network without freezing. `async fn` and `.await` mean "this may take a while". |
| **Block** | A batch of transactions added to the chain. Blocks come every few seconds. |
| **Log / event** | A record the contract writes when something happens (a deposit, a borrow). |
| **Reorg** | The chain replaces its most recent blocks with a different version. Events from the replaced blocks must be undone. |
| **NATS JetStream** | A message queue. Services publish messages to named "subjects" and other services subscribe. JetStream remembers messages, so a restarted service can catch up. |
| **Postgres** | A database. The services store their state here. |
| **Health factor (HF)** | Shows how safe a loan is. Below 1 means it can be liquidated. |
| **Nonce** | A number that goes up by one for every transaction a wallet sends. The chain refuses a transaction whose nonce is out of order. |
| **Keystore** | An encrypted file holding a private key. It needs a password to open. |
| **Idempotent** | Doing the same thing twice has the same result as doing it once. Important when messages can be delivered twice. |

---

## Part 1: The folder layout

```
services/
├── Cargo.toml          Lists all crates and their shared versions
├── Cargo.lock          Exact versions of every dependency
├── Dockerfile          Container build (not tested locally, no Docker installed)
├── common/             Shared code used by several services
├── indexer/            Step 1: reads the chain and publishes events
├── risk-engine/        Step 2: keeps position records and finds unsafe loans
├── liquidator/         Step 3: sends liquidation transactions
├── gateway/            The web-facing API and live stream for the frontend
├── scenario/           The acceptance test: runs everything under stress
├── keystore/           The liquidator's encrypted key (must never be shared)
├── logs/               Log files written by the scenario run
├── scenario-report.json  The latest scenario result
└── target/             Compiled output (generated, safe to delete)
```

The data flows in one direction:

```
blockchain ──> Indexer ──> NATS ──> Risk Engine ──> NATS ──> Liquidator ──> blockchain
                  │                      │                        │
                  └──────── Postgres ◄───┴────────────────────────┘
                                 ▲
                             Gateway ──> frontend (web app)
```

The **Indexer** is the eyes (it watches the chain). The **Risk Engine** is the brain (it decides what is unsafe). The **Liquidator** is the hands (it acts). The **Gateway** is the window for people. The **scenario** tests all of them together.

---

## Part 2: `Cargo.toml`: the workspace

**What it does.** It names the crates that belong together and pins shared library versions in one place, so every crate uses the same ones.

**Steps:**

1. `[workspace] members = [...]` lists the crates.
2. `[workspace.dependencies]` lists the libraries they all use: `alloy` (Ethereum), `tokio` (async), `sqlx` (Postgres), `async-nats` (NATS), `axum` (web server), `serde` (JSON), and others.
3. Each crate says `alloy.workspace = true`, meaning "use the version declared above".

---

## Part 3: `common/`: code shared by everyone

This crate has no program of its own. Other crates import from it. It is the single source of truth for the data formats and the maths, so every service agrees.

### 3.1 `lib.rs`

Lists the modules: `config`, `contracts`, `math`, `messages`, `model`, `telemetry`. Each becomes available as `vigil_common::<module>`.

### 3.2 `telemetry.rs`: logging

1. `init(service)` sets up logging in JSON format, so each line is machine-readable.
2. It reads `RUST_LOG` to choose how much to print (default `info`).
3. It logs a first line naming the service, so you can tell which program wrote what.

### 3.3 `config.rs`: reading the deployment file

**Why.** The contracts write their addresses to `contracts/deployments/31337.json` when deployed. The services read that file, so they always use the right addresses.

1. **`DeployedCollateral`**: one collateral asset, with its address, decimals, bit index, liquidation threshold and bonus, price feed, and AMM.
2. **`Deployment`**: the whole file: chain ID, pool address, oracle address, debt token, interest rates, and the list of collaterals.
3. **`Deployment::load(path)`**: reads the file and parses the JSON.
4. **`pool_params()`**: converts the file into the `PoolParams` the maths needs.

### 3.4 `contracts.rs`: the contract bindings

**What it does.** Uses the `sol!` macro to generate Rust types and functions that match the Solidity contracts exactly. For example, `ILendingPool::liquidateCall` is the Rust version of calling `liquidate` on the pool. This is how Rust builds correctly encoded transactions.

### 3.5 `math.rs`: the same maths as the contracts

**Why.** The Risk Engine must compute health factors *exactly* as the chain does. If the numbers differ by even one unit of rounding, the engine might think a loan is safe when the contract thinks it isn't.

**Steps:**

1. **Constants.** `WAD`, `RAY`, `BPS` and seconds per year, copied from `FixedPoint.sol`.
2. **`mul_div_down` / `mul_div_up`.** The same rounding helpers as the Solidity, with an error for division by zero.
3. **`wad_*` and `ray_*` helpers.** Shortcuts, as in the contract.
4. **`to_wad_*` / `from_wad_down`.** Decimal conversions for tokens.
5. **`RateParams`, `utilization`, `annual_borrow_rate`, `rate_per_second`, `compound_factor`.** Interest maths, copied from `InterestMath.sol`, in the same order of operations.

**Rule noted in the file:** keep the operation order the same as Solidity, because rounding at each step is what makes the two sides agree.

### 3.6 `model.rs`: positions and health

1. **Limits.** `MAX_COLLATERAL = 3`, and `HF_NO_DEBT` for a position with no debt.
2. **`CollateralParams` and `PoolParams`.** Settings for each collateral and for the pool.
3. **`PoolState` and `Quote`.** The pool's indexes and totals, and the oracle prices, at one block.
4. **`Position`.** A borrower's collateral amounts, bitmap and scaled debt.
5. **`preview_indexes`.** Works out the interest indexes at a given time. It follows `_previewIndexes` in the contract step by step.
6. **`health_factor`.** Adds up collateral value times threshold, divides by debt value. It follows `_healthFactorPx`.
7. **`debt_units`** and **`is_liquidatable(hf)`.** Turns scaled debt into real units, and checks whether HF is below 1.

### 3.7 `messages.rs`: the messages on the queue

This file defines what travels between services.

1. **Subjects.** The named channels: `vigil.events`, `vigil.blocks`, `vigil.signals`.
2. **`MessageKind`.** `Log` means "this happened". `Retract` means "undo what you did for this, it was on a block that was replaced".
3. **`PoolEvent`.** The events the Risk Engine needs: deposits, withdrawals, borrows, repays, liquidations, bad debt. Any other event becomes `Other`, so the stream stays complete.
4. **`EventMessage`.** One event, with its block number, block hash, log index, trace ID, and the event itself.
5. **`idempotency_key()`.** The pair (block hash, log index). Each event has a unique key, so receiving it twice does nothing the second time.
6. **`BlockMessage`.** The state of the oracle and pool at one block. The Risk Engine uses it to check health.
7. **`LiquidationSignal`.** "This borrower is unsafe as of this block." It is stale after 2 blocks.
8. **`trace_id_for`.** Builds a trace ID from the block hash and log index, so one liquidation can be followed through all the logs.

---

## Part 4: `indexer/`: watching the chain

**Job:** read every new block and its events, publish them to NATS, and handle reorgs.

### 4.1 `main.rs`: starting the indexer

1. **Read settings** from the command line or environment: RPC address, deployment file, database URL, NATS URL, start block, poll interval (default 200 ms).
2. **Load the deployment file.**
3. **Connect to Postgres** and run the database migrations (creating tables if they don't exist).
4. **Connect to NATS** and create the message sink.
5. **Connect to the chain** over HTTP.
6. **Build the collateral map.** A list from bit index to collateral address, so events can be decoded in a known order.
7. **Resume** from the saved cursor (the last block it processed).
8. **Loop forever:**
   - `catch_up()` processes every new block.
   - If a reorg is deeper than the saved history, **stop** (fail closed). A human should check that.
   - Any other error is logged, and the loop retries.
   - Sleep for the poll interval.

### 4.2 `engine.rs`: the main logic

The engine is written against *traits*, meaning interfaces. The real chain, sink and database are plugged in at runtime, and tests use fake ones. This lets the reorg logic be tested without a blockchain.

1. **`resume`.** Loads the cursor and rebuilds the list of recent blocks from the chain.
2. **`catch_up`.** While the next block number is at or below the head:
   - Fetch the block by number. If the node doesn't have it yet, stop and try later.
   - Check continuity: does this block's parent match the last block we saw?
     - **Extends or Fresh**: apply the block.
     - **Reorged**: handle the reorg, then continue from the common ancestor.
3. **`apply_block`.** Processes one block:
   1. Fetch the logs for that block, by its **hash** (so a reorg can't mix up blocks).
   2. Turn each log into an `EventMessage` with its trace ID.
   3. **Publish** every event to NATS.
   4. Read the oracle and pool state at that block and publish a `BlockMessage`.
   5. **Save** the logs to Postgres, then save the cursor.
   6. Remember the block in the recent-blocks list, and move on.
   
   The order matters. If the program crashes after publishing but before saving, the block is processed again on restart. Consumers ignore duplicates, so nothing is counted twice.
4. **`handle_reorg`.**
   1. Walk back through recent blocks until finding one whose hash still matches the chain. That's the **ancestor**.
   2. If none is found within the history (the ring of about 10 blocks), stop with an error.
   3. Load the logs from blocks above the ancestor, newest first, and publish a **Retract** for each. This tells the Risk Engine to undo them.
   4. Delete those saved logs, trim the recent list, save the cursor to the ancestor, and set the next block to process.
   
   Retracts are published before any event from the new chain, so the Risk Engine always undoes before it applies.

### 4.3 `reorg.rs`: reorg bookkeeping

Pure logic, no network. It holds a small ring of recent blocks, decides whether a new block continues the chain (`continuity`), and finds the common ancestor (`find_ancestor`). Keeping it separate makes it easy to test.

### 4.4 `chain.rs`: reading from the blockchain

1. **`RpcChain::new`** stores the provider (the connection to the node) and the contract addresses.
2. **`head_number`** and **`block_by_number`** return chain height and block information.
3. **`logs_by_hash`** fetches the logs for one block, filtered to the pool and the oracle.
4. **`decode`** turns raw log bytes into a `PoolEvent`. Each event has a known signature, so the code checks which one it is and extracts the fields. Unknown events become `Other`.
5. **`block_message`** reads the oracle prices and the pool's totals at that block. It also projects the debt index, so the Risk Engine can compute health without calling the chain again.

### 4.5 `sink.rs`: publishing to NATS

1. **`connect`** creates the JetStream stream named `VIGIL` if it doesn't exist.
2. **`publish_event` and `publish_block`** send messages. Each message has a `Nats-Msg-Id` header. JetStream uses it to drop duplicates sent within its window, so a replay after a crash doesn't double-count.

### 4.6 `store.rs`: saving state to Postgres

Implements the storage trait:

- **`load_cursor` / `save_cursor`**: the last processed block (table `indexer_cursor`, one row).
- **`save_block_logs`**: the messages published for a block (table `indexer_block_logs`), replacing any earlier copy for that number.
- **`logs_above` / `forget_above`**: find and delete logs above a block number, used during a reorg.

---

## Part 5: `risk-engine/`: keeping positions and finding danger

**Job:** rebuild every borrower's position from the events, check every position's health after each block, and send a signal for each unsafe one.

### 5.1 `main.rs`: starting the risk engine

1. **Read settings**: deployment file, database URL, NATS URL, HTTP address (default `127.0.0.1:8080`).
2. **Connect to Postgres** and run migrations.
3. **Create the engine** and wrap it in a mutex, so the HTTP server and the message loop can share it safely.
4. **Start the HTTP server** in the background (for inspecting positions).
5. **Connect to NATS**, and create a *durable* consumer named `risk-engine`. A durable consumer remembers where it stopped, so a restart picks up exactly where it left off.
6. **Loop over messages:**
   - An **event** message goes to `handle_event`.
   - A **block** message goes to `handle_block`, and any signals it returns are published.
   - The message is **acknowledged only after** the change is saved. If the program crashes first, the message is delivered again.
   - A failed message is left unacknowledged, and NATS retries it. Duplicate processing is prevented by the idempotency keys.

`publish_signal` sends each signal with a `Nats-Msg-Id` of "signal, borrower, block", so a republished signal is deduplicated too.

### 5.2 `engine.rs`: the logic

1. **`apply_to_position`.** Applies one event to one position, step by step:
   - **CollateralDeposited**: add to the amount, switch on the collateral's bit.
   - **CollateralWithdrawn**: subtract; if zero, switch the bit off.
   - **CollateralEnabled / Disabled**: flip the bit.
   - **Borrowed**: add scaled debt.
   - **Repaid**: subtract scaled debt (errors if it would go negative).
   - **Liquidated**: subtract debt and collateral by the logged amounts.
   - **BadDebt**: subtract the written-off debt.
   - **Other**: nothing.
   
   Every subtraction is checked. A bad event causes an error, not a silent wrong number.
2. **`handle_event`.**
   - A **Retract** calls `undo`, which restores the position from the saved "before" state.
   - A **Log** loads the position, applies the event to a copy, and saves both in one database transaction. If the event was already applied, nothing changes.
3. **`handle_block`.**
   - If the oracle is **frozen**, return no signals. Don't liquidate on untrusted prices.
   - Otherwise, loop over every position with debt, compute its health factor, and emit a signal for each unsafe one.
   - Store the block as the latest state.
4. **`health_factor_of`.** Gives the current health of one borrower, for the HTTP endpoint.

### 5.3 `store.rs`: the Postgres store

- **`load_position`** and **`all_with_debt`** read positions.
- **`commit_applied`** writes the new position, and records the event as processed and the "before" state as applied. It runs in one transaction, so either all of it happens or none of it does.
- **`undo`** restores the "before" state for a retracted event.

### 5.4 `api.rs`: HTTP endpoints

- **`/healthz`** returns `ok`, for checking the service is alive.
- **`/positions/{borrower}`** returns one borrower's snapshot: their collateral, debt, and health factor at the last block.

---

## Part 6: `liquidator/`: acting on signals

**Job:** receive a signal, check it is still valid, sign and send a liquidation transaction, and keep track of every transaction until it is confirmed.

### 6.1 `main.rs`: startup and the main loop

1. **Read settings**, including the keystore path and password (from `KEYSTORE_PASSWORD`, never a command-line argument), the RPC address, the strategy and its settings, and the replacement and poll timings.
2. **Load the wallet** from the encrypted keystore. The password is read from the environment.
3. **Connect to Postgres** and the chain.
4. **Choose a strategy:** `immediate` strikes as soon as there's a bonus, or `margin` waits until the bonus covers the gas cost with a safety margin.
5. **Build the executor**, which is the component that makes decisions and sends transactions.
6. **Reconcile the nonce** with the chain, so the next transaction uses the right number.
7. **Subscribe** to the `vigil.signals` subject with a durable consumer named `liquidator`.
8. **Loop:**
   - **A new signal arrives:** handle it. If handling succeeds, acknowledge. If it fails, leave it unacknowledged so it's retried.
   - **A new block appears:** run `tick()`, which checks and maintains the transactions in flight.

### 6.2 `executor.rs`: the decision-maker (768 lines)

This is the most important file in the liquidator.

**`handle_signal(signal)`** runs these checks in order. The first one that fails stops the handling:

1. **Stale?** If the chain is more than 2 blocks past the signal, skip it. The information is too old.
2. **Already in flight?** If a transaction for this borrower is already being sent, skip it. This prevents double liquidations.
3. **Still unhealthy?** Read the health factor from the chain now, not from the signal. If it's 1 or more, skip.
4. **Recently reverted?** If a liquidation for this borrower reverted in the last 10 blocks, skip it (the cooldown).
5. **Intents?** Load the borrower's signed intents from the database. If there are none, skip.
6. **Pick a usable intent.** Go through them oldest first. Skip any that have expired, or whose nonce is already spent on-chain. Use the first one left.
7. **Auction started?** If no auction is running, send a `startAuction` transaction and stop there. The liquidation happens in a later signal, once the auction has run for a while.
8. **Decide to strike.** Ask the strategy whether now is the time. It may say strike, wait, or skip.
9. **Send the liquidation.** If the send is rejected (for example, "replacement transaction underpriced"), the nonce book is re-synced from the chain and the signal is skipped. Otherwise, it's logged as struck.

**`tick()`** runs once per new block:

1. **Check every transaction in flight.** Ask the chain for its receipt:
   - **Confirmed**: mark it done in the database and start watching it for reorgs.
   - **Reverted**: mark it failed, and record a cooldown for that borrower.
   - **Pending**: leave it alone.
   - **Unknown**: the node doesn't know it. Treat it as dropped.
2. **Watch confirmed transactions for reorgs** for 10 blocks. If one disappears, send it again.
3. **Plan actions** for the rest:
   - **Replace** a transaction that has waited 3 blocks, with a higher fee.
   - **Gap-fill** a dropped transaction. Resend it if it's still needed, or send a zero-value self-transfer if not, so no nonce is left empty.

**`send_start_auction` and `send_liquidate`** build the transaction, sign and send it, and record it in the nonce book and the database.

### 6.3 `nonce.rs`: the nonce book

Tracks which nonces are in flight, which are confirmed, and what the next nonce is.

- **`reserve()`** hands out the next nonce in order.
- **`record_sent`** adds a transaction to the in-flight set.
- **`confirmed`** removes one that was mined.
- **`plan`** decides which in-flight transactions to replace or gap-fill.
- **`reconcile(latest, pending)`** resets the book from the chain's view. It never moves the next nonce backwards.
- **Leftover rows.** At startup, the executor also checks every database row still marked `pending` against the chain. A mined transaction becomes `confirmed` or `failed`, and one the node no longer knows about becomes `reorged`.
- **`bump_fee`** raises a fee by at least 12.5%, which is the minimum most nodes require for a replacement.

### 6.4 `strategy.rs`: when to strike

- **`ImmediateStrike`**: strike as soon as there is any bonus.
- **`WaitForMargin`**: strike only when the bonus value is at least the gas cost times a margin (for example, 1.2×).
- Both are behind a `Strategy` trait, so they can be swapped and tested.

### 6.5 `signer.rs`: the wallet

Loads the encrypted keystore with the password. The `Debug` print is redacted, so the key never appears in logs. Only the RPC layer can sign.

### 6.6 `store.rs`: the database

- **`PgIntentStore`** reads signed intents for a borrower. It only reads. The gateway writes them.
- **`PgTxLog`** writes each transaction (`sent`) and updates its status (`set_status`). The table is `liquidator_txs`, and the gateway shows it on the frontend.

### 6.7 `rpc.rs` and `chain.rs`: talking to the chain

These implement the `TxChain` trait with real calls:

- **`send`** submits a signed transaction.
- **`status`** reads a receipt: confirmed, reverted, pending, or unknown.
- **`health_factor`, `bonus_bps`, `auction_start`, `nonce_used`** read the pool.
- **`block_number`, `block_timestamp`, `pending_nonce`, `latest_nonce`** read chain state.
- **`gas_price`** reads the current gas price.

Tests use a fake chain, so the executor logic is tested without a node.

### 6.8 `lib.rs`

Lists the modules, so tests and other crates can use them.

---

## Part 7: `gateway/`: the window for the frontend

**Job:** let the web app read the system's state and submit borrower-signed intents. It does not change anything on the chain itself.

### 7.1 `main.rs`: startup

1. **Read settings**: RPC URL, deployment file, database URL, NATS URL, listen address (default `127.0.0.1:3000`), and the frontend folder to serve (default `../web/dist`).
2. **Load the deployment file** and connect to the chain, Postgres and NATS.
3. **Build the routes** (the addresses the web app can call).
4. **Start the server.**

### 7.2 The routes

| Route | What it returns |
|---|---|
| `GET /api/stream` | A live feed (Server-Sent Events) of every NATS message: events, blocks, and signals. |
| `GET /api/config` | The deployment file, so the frontend knows the addresses. |
| `GET /api/protocol` | A snapshot of the chain: prices, totals, pause state, and oracle state. |
| `GET /api/positions` | All positions with debt. |
| `GET /api/positions/{addr}` | One position. |
| `GET /api/account/{addr}` | One account's supplies, collateral and debt. |
| `GET /api/liquidations` | Liquidator transactions. |
| `GET /api/intents` | Stored liquidation intents. |
| `POST /api/intents` | Accepts a signed intent (see 7.3). |
| `GET /api/health` | Counts for each stage, so you can see whether the pipeline is moving. |
| Everything else | Static files from `FRONTEND_DIR`, which is the built web app. |

### 7.3 Accepting an intent: `verify_and_store`

1. Parse the intent fields from the request.
2. Ask the contract for the digest of that intent (`hashIntent`). This is the same digest the borrower's wallet signed.
3. Recover the signer from the signature. It must match the borrower in the intent. If not, reject with an error.
4. Store the intent in the `intents` table. A duplicate (same borrower and nonce) is ignored.
5. Return the digest.

**Why verify here:** the Liquidator trusts whatever is in the table, so bad signatures must be rejected at the door.

### 7.4 `chain_snapshot` and `db_snapshot`

These collect the numbers the frontend shows: prices, totals, the last indexed block, how many events have been processed, how many positions have debt, how many intents exist, and how many liquidator transactions have been sent.

---

## Part 8: `scenario/`: the acceptance test

**Job:** run the whole system under stress and score it. This is the grading test described in the brief.

### 8.1 What it does, in order

1. **Phase 1, seeding.** Creates 100 borrowers with positions whose health factors range from 1.05 to 1.6. Each borrower signs 8 liquidation intents (`INTENTS_PER_BORROWER`), and they are stored in the database.
2. **Phase 2, the crisis.**
   - Starts the indexer, risk engine and liquidator as separate programs.
   - Drops the collateral prices over 15 blocks.
   - At block 8, injects a **2-block reorg** (the chain replaces its last two blocks).
   - At block 10, runs a **TWAP manipulation attack**: a fake price is pushed and used in the same block. The attack should not succeed.
   - At block 12, **kills the liquidator** and restarts it 3 blocks later. This tests crash recovery.
   - Keeps going for 30 more blocks.
   - Then mines `FLUSH_BLOCKS` (20) more blocks, and keeps mining until the mempool has been empty for `FLUSH_QUIET_BLOCKS` (5) blocks in a row (at most `FLUSH_MAX_BLOCKS`, 200). This lets the liquidator see its last transactions confirm before it is stopped.
   - After scoring, it turns automine back on, so the chain behaves normally afterwards.
3. **Phase 3, scoring.** Checks the results against the hard-zero criteria and writes `scenario-report.json`.

### 8.2 The checks (hard zero means the count must be 0)

- **`positions_still_liquidatable`**: borrowers still below health 1 at the end.
- **`nonce_gaps`**: holes in the liquidator's nonce sequence.
- **`duplicate_liquidations`**: the same liquidation executed twice.
- **`indexer_duplicates` and `indexer_gaps`**: events published twice, or missing.
- **`health_factor_divergences`**: the Risk Engine's health factor differs from the chain's.
- **`successful_manipulation_attacks`**: attacks that moved the price.

### 8.3 Helper functions

- **`Chain`**: a wrapper for sending transactions, mining blocks (`advance`), and setting prices.
- **`batch_as`** sends several calls and mines them in one block.
- **`inject_two_block_reorg`** performs the reorg with a snapshot and a revert.
- **`run_attacker`** performs the TWAP attack.
- **`parity_check`** compares the Rust maths with the chain's answers.
- **`indexer_check`** and **`nonce_gap_check`** compute those two checks from the database.

### 8.4 Settings you can change

At the top of `main.rs`: `BORROWERS` (100), `INTENTS_PER_BORROWER` (8), `FLUSH_BLOCKS` (20), `CRASH_STEPS` (15), `TAIL_BLOCKS` (30), `REORG_AT` (8), `ATTACK_AT` (10), `KILL_AT` (12), `RESTART_AFTER` (3).

---

## Part 9: Running the services

Start them in this order. Each needs the one before it.

```bash
# 1. Infrastructure
pg_ctl ... start            # Postgres on 5432
nats-server -js -p 4222     # NATS on 4222
anvil --port 8545           # chain

# 2. Contracts (writes contracts/deployments/31337.json)
cd contracts && forge script script/Deploy.s.sol --broadcast --rpc-url http://127.0.0.1:8545

# 3. Build the services
cd services && cargo build

# 4. Indexer
RPC_URL=http://127.0.0.1:8545 DEPLOYMENT_FILE=../contracts/deployments/31337.json \
  DATABASE_URL=postgres://vigil@127.0.0.1:5432/vigil ./target/debug/vigil-indexer

# 5. Risk engine (same variables, plus HTTP_ADDR if 8080 is in use)
DEPLOYMENT_FILE=... DATABASE_URL=... ./target/debug/vigil-risk-engine

# 6. Liquidator (needs the keystore password in the environment)
KEYSTORE_PASSWORD=vigil-dev-password RPC_URL=... DEPLOYMENT_FILE=... DATABASE_URL=... \
  ./target/debug/vigil-liquidator

# 7. Gateway (serves the web app on 127.0.0.1:3000)
DEPLOYMENT_FILE=... DATABASE_URL=... ./target/debug/vigil-gateway
```

To run the full acceptance test instead:

```bash
cd services
KEYSTORE_PASSWORD=vigil-dev-password BIN_DIR=./target/debug \
  DEPLOYMENT_FILE=../contracts/deployments/31337.json ./target/debug/vigil-scenario
```

Restart anvil and redeploy before each scenario run, because the scenario expects a fresh chain.

**Tests:**

```bash
cd services && cargo test
```

---

## Part 10: Things to know

- **Trace IDs** follow a liquidation through the logs. Search for one to see every step.
- **Idempotency** is the safety net. Events, signals and transactions can be delivered more than once, and the design makes that harmless.
- **Fail closed.** When the indexer can't tell what the chain looks like, it stops rather than guessing. When the oracle is frozen, the risk engine stays silent.
- **The keystore** must never be committed or shared. It is listed in `.gitignore`.
- **Open issues:** the `HealthDecreased` invariant (I3) can block some liquidations. The liquidator waits 10 blocks after a revert, but still sends some reverting transactions. This is kept by design.
