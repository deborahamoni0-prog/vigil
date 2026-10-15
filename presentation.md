# Vigil: Frontend Walkthrough

A guided tour of the Vigil web app. Allow about 10 minutes.

**What Vigil is:** a lending pool. You supply collateral, borrow **vUSD** against it, and repay to get your collateral back. A set of backend services watches every position and liquidates the ones that become unsafe.

**Before you start**
- Everything runs on a local test network. The tokens have no value.
- The app needs the backend running: the gateway, the chain (anvil), the indexer, the risk engine, the liquidator and the price feed. If a page shows a connection error, the backend is down.
- The app has three pages: **Landing**, **Markets** and **Live pipeline**. The header links between them, and it has a theme button for light and dark mode.

---

## 1. Landing page (`/`)

**Say:** "This is the front door. It explains the product in three ideas."

Point out, top to bottom:
1. **Hero:** "Borrow and earn..." with two buttons, *Start with markets* and *Watch the pipeline*.
2. **Features:**
   - *Put your collateral to work, and keep control of it.* Assets sit in the pool until you withdraw them.
   - *Borrow vUSD against what you hold, with a clear limit.* The health factor shows how safe you are. Above 1 is safe. At or below 1, the position can be liquidated.
3. **How it works:** three steps: *Supply collateral*, *Borrow against it*, *Repay and get it back*.
4. **Footer note:** "This is a local test network. The tokens and money are for testing only."

**Click:** *Open app* to go to Markets.

---

## 2. Markets page (`/markets.html`)

**Say:** "This is the working app. Everything updates on every block, so you don't need to refresh."

### 2.1 Pick an account
The app has no wallet pop-up. It uses the test accounts the local chain provides, and it selects the first one. Use the account selector to switch. Each account has its own position.

### 2.2 Summary strip
Three numbers describe the selected account:
- **Net worth**
- **Health factor**, with the note "1.00 liquidation". Below 1.00, the position can be liquidated.
- **Borrow power used**, with **Borrow limit left**

Before anything is supplied, these show an empty position.

### 2.3 Tabs
The page has four tabs: **Markets**, **Positions**, **Intents** and **Activity**.

**Markets tab**
- *Supply markets* lists the three test collateral assets (`fee`, `six`, `eight`). They differ in decimals and in liquidation threshold, which is the point of having three.
- *Borrow markets* shows the vUSD market and its current rates.
- The search box filters the list.

### 2.4 Live demo: supply, borrow, repay
Do these in order and watch the summary strip change after each one.

1. **Get test tokens.** Use the mint button for test tokens. Without it the wallet is empty.
2. **Supply.** Click *Supply* on an asset, enter an amount and confirm. The dialog shows its steps and a toast says "Waiting for the transaction to be mined…". Your *Borrow power* rises.
3. **Borrow.** Open the *Borrow* tab in the dialog and borrow some vUSD. Watch the **health factor** drop and **Borrow power used** rise.
4. **Repay.** Repay part of the debt. The health factor recovers.
5. **Repay all and withdraw.** Repaying everything returns every supplied asset to the wallet.

**Say:** "The app won't let you borrow past your limit. The limit comes from each asset's liquidation threshold."

### 2.5 Positions tab
Shows **Supplies** and **Borrows** for the selected account, plus an account details panel. Use it to confirm what the demo just did.

### 2.6 Intents tab: liquidation intents
**Say:** "An intent is a signed instruction that lets a liquidator act for you if your health factor drops."

The form asks for:
- **Collateral** they may take
- **Max repay**: the most vUSD they may repay for you
- **Min out**: the least collateral you accept
- **Valid for (days)**

Submit one, and it appears in the table below with its **Expires** date and **Status**. For another account, the table shows "no intents for this account".

### 2.7 Activity tab
A feed of recent events for the account: deposits, borrows, repayments.

---

## 3. Live pipeline (`/live.html`)

**Say:** "This page shows the backend working. It is the part of the product users don't normally see."

### 3.1 The five stages
Left to right, each stage is a card with one big number:

| Stage | What it shows |
|---|---|
| **Chain** | The current block, and whether the price oracle is frozen or normal |
| **Indexer** | The block it has processed, and its lag behind the chain |
| **Messages** | How many events, blocks and signals have passed through the message bus |
| **Risk engine** | How many positions are currently borrowing |
| **Liquidator** | How many transactions it has sent |

**Say:** "A healthy system has the indexer lag near zero and a block number that keeps moving."

### 3.2 Activity feed
Newest first. Each entry has a tag (the event type, or **signal** for a liquidation signal), a short address and a block number. Do a borrow on the Markets page in another tab and watch it appear here.

### 3.3 Liquidations table
Columns: **Nonce**, **Kind**, **Status**, **Sent block**. It reads "none yet" until a liquidation happens.

---

## 4. Optional: show a liquidation end to end
1. On Markets, supply collateral and borrow close to the limit.
2. Move the price down. The price feed script drives prices, and the scenario run does this automatically.
3. Watch the health factor fall below 1.00 on Markets.
4. Switch to Live pipeline: a **signal** appears in the feed, the Risk engine count is non-zero, and a row appears in the Liquidations table.

---

## 5. Key points to leave people with
- Supply, borrow and repay in three clicks, with a live health factor.
- The limit is clear before you act, and it updates every block.
- Liquidation is automated and visible: the Live pipeline page shows each step from chain event to liquidator transaction.
- Users stay in control through intents: they choose what a liquidator may take and what they will accept.

## Troubleshooting during a demo
| Symptom | Likely cause |
|---|---|
| Every page shows a connection error | Gateway not running (port 3000) |
| Prices show "frozen" | Price feed script not running |
| Transactions stay on "waiting to be mined" | Anvil automine is off. Run `cast rpc anvil_setAutomine true --rpc-url http://127.0.0.1:8545` |
| Live pipeline numbers are stuck at 0 | Indexer not running, or started at the wrong block (use the pool's deploy block) |
