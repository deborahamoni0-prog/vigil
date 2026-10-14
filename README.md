# Vigil: a lending protocol that watches itself

Vigil is a small lending protocol plus the services that keep it safe. People deposit collateral and borrow a stablecoin against it. If a loan becomes too risky, the system detects it and anyone can liquidate it: they repay part of the debt and receive the collateral at a discount. The services are built to keep working when the chain reorganises, a service crashes, or prices are manipulated.

This README explains how the whole system fits together, and how to run it. Each part has its own step-by-step guide:

| Part | Language | Guide | Code |
|---|---|---|---|
| Smart contracts | Solidity | [contracts/README.md](contracts/README.md) | `contracts/` |
| Backend services | Rust | [services/README.md](services/README.md) | `services/` |
| Web app | TypeScript / React | [web/README.md](web/README.md) | `web/` |

If you have never seen any of this before, read this file first, then the contracts guide, then the services guide, then the web guide.

---

## 1. The big picture in one paragraph

Users interact with the **lending pool**, a set of contracts on the blockchain. The pool holds the money and enforces the rules. An **oracle** tells it what each collateral is worth. The **Indexer** (a Rust service) reads every new block and publishes what happened to a message queue. The **Risk Engine** (Rust) rebuilds every borrower's position from those messages, checks each one's health after every block, and raises a **signal** for each risky loan. The **Liquidator** (Rust) takes a signal, checks it is still valid, and sends the liquidation transaction using a permission the borrower signed earlier (an **intent**). The **Gateway** (Rust) serves the state to the **web app**, which lets people use the pool and sign intents.

---

## 2. The data flow, step by step

```
 1. A user calls the pool (deposit, borrow, repay...)     ── web app or a script
 2. The pool changes state and writes an event to the chain
 3. Indexer reads the new block, decodes the events, and publishes them to NATS
 4. Risk Engine applies each event to its copy of the positions (in Postgres)
 5. After each block, the Risk Engine checks every position's health factor
 6. An unhealthy position produces a liquidation signal on NATS
 7. Liquidator checks the signal, picks a signed intent, and sends a transaction
 8. The pool checks the intent, the health factor, and the price, then executes
 9. The new liquidation event flows back through steps 3–4
10. Gateway reads Postgres and the chain, streams updates to the web app
```

Every message carries a **trace ID** (built from the block hash and the log index), so one liquidation can be followed from start to finish in the logs.

---

## 3. The parts, and what each one is for

### 3.1 Smart contracts (`contracts/`)

- **`LendingPool.sol`** is the main contract. It handles deposits, withdrawals, borrowing, repaying, liquidation auctions, intents, and bad debt.
- **`OracleAdapter.sol`** combines a price feed with a time-averaged price from an exchange. If either is stale, out of range, or disagrees with the other, the whole oracle is *frozen*, and the pool refuses risky actions.
- **Libraries** (`FixedPoint`, `InterestMath`, `SafeTransfer`) provide exact decimal maths, the interest-rate model, and safe token transfers.
- **`Deploy.s.sol`** deploys everything and writes the addresses to `contracts/deployments/31337.json`, which every other part reads.

The most important rule: **a liquidation must never make a position less healthy.** If it would, the transaction reverts. This is invariant I3. It is the reason some liquidations are refused, and it is an open question for the project (see section 7).

### 3.2 Backend services (`services/`)

| Service | Job | Talks to |
|---|---|---|
| **Indexer** | Reads blocks and events, handles reorgs, publishes to NATS | chain, NATS, Postgres |
| **Risk Engine** | Keeps positions, finds unhealthy ones, emits signals | NATS, Postgres |
| **Liquidator** | Sends liquidation and auction-start transactions, tracks them until mined | NATS, chain, Postgres |
| **Gateway** | Serves the web app, its API, and a live stream | chain, NATS, Postgres |
| **Scenario** | The acceptance test: runs everything under stress and scores it | all of the above |
| **Common** | Shared data formats, maths, config and logging | used by all |

### 3.3 Web app (`web/`)

- **Markets** (the app): see markets and positions, supply and withdraw collateral, borrow and repay, and sign liquidation intents.
- **Live pipeline**: watch each service's status and the activity feed.
- **Landing page**: an introduction, with links to the app.

The web app talks to the chain directly for transactions (through the user's wallet), and to the gateway for everything it reads in bulk.

### 3.4 Infrastructure

| Piece | What it does | Port |
|---|---|---|
| **anvil** | A local blockchain for development | 8545 |
| **PostgreSQL** | Stores the services' state | 5432 |
| **NATS JetStream** | The message queue between services | 4222 |
| **Gateway** | Web app and API | 3000 |
| **Risk Engine HTTP** | Position lookup (default) | 8080 |

`docker-compose.yml` describes all of this in containers, but Docker is not installed on the development machine, so it has not been tested.

---

## 4. Folder layout

```
vigil-project/
├── README.md               This file
├── contracts/              Solidity contracts, deploy script, tests
│   └── README.md           Contracts guide
├── services/               Rust services and the acceptance scenario
│   └── README.md           Services guide
├── web/                    The web app
│   └── README.md           Web app guide
├── migrations/             Database tables (run by the services on start)
├── scripts/                Price feed and liquidation check (Node.js)
├── docs/                   The project brief
├── frontend/               Old static version of the web app (kept for reference)
├── docker-compose.yml      Container setup (not tested locally)
└── .gitignore              What not to commit
```

---

## 5. Running everything locally

### 5.1 Requirements

- Rust and Cargo (for `services/`)
- Foundry: `forge` and `anvil` (for `contracts/`)
- Node.js and npm (for `web/` and `scripts/`)
- PostgreSQL 16 and NATS server 2.10 (installed locally, see the environment notes)

### 5.2 Start in this order

```bash
# 1. Start the infrastructure
pg_ctl -D <pgdata> -l pg.log start -w       # PostgreSQL
nats-server -js -sd natsdata -p 4222 &       # NATS
anvil --port 8545 &                          # the local chain

# 2. Deploy the contracts (writes contracts/deployments/31337.json)
cd contracts
forge script script/Deploy.s.sol --broadcast --rpc-url http://127.0.0.1:8545

# 3. Build the services and the web app
cd ../services && cargo build
cd ../web && npm install && npm run build

# 4. Start the services (each in its own terminal; see services/README.md for the variables)
./target/debug/vigil-indexer
./target/debug/vigil-risk-engine
KEYSTORE_PASSWORD=vigil-dev-password ./target/debug/vigil-liquidator
./target/debug/vigil-gateway                  # serves the web app on port 3000

# 5. Open the app
#    http://127.0.0.1:3000/markets.html       (the app)
#    http://127.0.0.1:3000/live.html          (the pipeline view)
```

**Price feed (optional, for real prices):**

```bash
PRICE_MODE=live node scripts/price-feed.mjs
```

It fetches prices from CoinGecko and writes them to the oracle every 30 seconds. In live mode it stops if the fetch fails. Set `PRICE_MODE=walk` only if you want simulated prices.

### 5.3 Run the acceptance test

```bash
# Restart anvil and redeploy first, so the scenario starts from a clean chain
cd services
KEYSTORE_PASSWORD=vigil-dev-password BIN_DIR=./target/debug \
  DEPLOYMENT_FILE=../contracts/deployments/31337.json ./target/debug/vigil-scenario
```

The result is written to `services/scenario-report.json`. Every hard-zero criterion must be 0 for a pass.

### 5.4 Run the tests

```bash
cd contracts && forge test        # contracts
cd services && cargo test         # Rust services
cd web && npm test                # web app maths
```

---

## 6. How the pieces prove they work

- **Maths parity.** The Rust maths and the frontend maths follow the Solidity step by step, so they agree bit for bit. Tests check this.
- **Reorg safety.** The Indexer publishes a retraction for every event on a replaced block, before publishing the new chain.
- **Exactly-once effects.** Every event has a unique key (block hash and log index). Services ignore keys they've already applied, so a replay after a crash changes nothing.
- **Nonce safety.** The Liquidator tracks its own nonces, re-syncs from the chain after a rejected send, and fills any gap so no nonce is left empty.
- **Oracle manipulation.** A price must agree with the time-averaged price before it is trusted. A price pushed and used in the same block carries no weight.
- **Signed permissions.** Intents are checked on arrival (by the gateway) and again on-chain (by the pool). Each nonce can be used once.
- **Acceptance scenario.** A single run crashes prices, reorgs the chain, attacks the oracle, and kills the Liquidator, then scores the result.

---

## 7. Status and open issues

**Working:**
- Contract tests pass (53 of 53), including invariant tests.
- Rust tests pass for all services.
- The web app builds and its maths tests pass.
- The gateway serves the app and its API.
- The acceptance scenario runs end to end. The liquidator's replacement-error problem is fixed. Most stale-confirmation problems are fixed; the last run still left a few (see below).

**Open:**
- **The acceptance scenario still fails** on its most recent run, with 1 nonce gap and 1 position still liquidatable. The cause is a race at the end of the run: the last liquidation is sent after the final blocks are mined, so it isn't confirmed when the run is scored. Some liquidator database rows also stay `pending` after the liquidator restarts, even though they were mined.
- **`HealthDecreased` (invariant I3)** blocks some liquidations. The liquidator waits 10 blocks after a revert, but it still sends some reverting transactions. The invariant is kept by design; whether to relax it is a project decision.
- **The web app defaults to localhost.** Set `VITE_RPC_URL` and `VITE_GATEWAY_URL` to point it elsewhere.
- **Browser testing is incomplete.** The app has not been clicked through in a browser. The intent signature digest matches the contract's, so a browser signature should verify.
- **The "supply" button deposits collateral**, not the debt token. Lenders cannot yet supply the debt token from the app.
- **Security-sensitive files.** `services/keystore/` holds an encrypted key. It must not be shared in the repository.

---

## 8. Where to look when something breaks

| Symptom | Look at |
|---|---|
| Web app shows no prices | Price feed running? `scripts/price-feed.mjs` log. Gateway running? |
| Web app shows empty tables | Indexer, Risk Engine and Liquidator running? Postgres reachable? |
| Liquidation reverts with `HealthDecreased` | Expected for some borrowers. See contracts guide, section 7.12 |
| Transaction rejected as "underpriced" | Liquidator log: the nonce book is re-synced automatically |
| Scenario fails | `services/scenario-report.json` and `services/logs/` |
| Contract build fails | `forge build` output, and `contracts/lib` (dependencies) |

Logs are JSON, one line per event. Search them for a trace ID to follow one liquidation across services.

---

## 9. Glossary

- **Position**: one borrower's collateral and debt.
- **Health factor**: collateral value (weighted by threshold) divided by debt value. Below 1 means liquidatable.
- **Liquidation**: repaying part of an unsafe loan and taking the collateral at a discount.
- **Auction**: the liquidation bonus starts at zero and grows over 10 blocks, so the liquidator waits for a better deal.
- **Intent**: a signed permission letting a liquidator act under stated limits, until a deadline, with a one-time nonce.
- **Oracle**: the source of prices. Frozen means the prices are not trusted, so risky actions are blocked.
- **Reorg**: the chain replaces its most recent blocks. Events from the replaced blocks must be undone.
- **Trace ID**: an identifier that follows one piece of work through every service's logs.
- **Hard-zero criterion**: a scenario check that must be exactly 0 for a pass.
