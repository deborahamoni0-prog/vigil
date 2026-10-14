# Vigil Web App: a step-by-step guide to the frontend

This guide explains the web app in `web/` in plain language. It assumes you know what a web page is, but not React, blockchains or wallets. It goes file by file, in the order the page starts up and then reacts to what the user does.

If you only read one thing: **the web app is a window onto the lending pool.** It shows the markets, lets a user deposit collateral, borrow, repay and withdraw, and lets them sign a permission that a liquidator can use. It reads prices and positions from the Rust gateway, and it sends transactions straight to the blockchain through the user's wallet.

---

## Part 0: Vocabulary

| Word | Meaning |
|---|---|
| **React** | A library for building web pages out of reusable pieces called components. |
| **Component** | A piece of the page, for example a table or a dialog. It is a function that returns what should be shown. |
| **State** | Data a component remembers and can change, for example "which tab is open". When state changes, React redraws the parts that depend on it. |
| **Hook** | A React function that gives a component state or runs code at certain moments. Names start with `use`. |
| **Context** | A way to share data with many components without passing it through every layer. |
| **Vite** | The tool that builds the app into plain files (`npm run build`) and runs a development server (`npm run dev`). |
| **TypeScript** | JavaScript with types added, so mistakes are caught before the page runs. |
| **ethers** | A library for talking to the blockchain from a browser: reading values, sending transactions, signing messages. |
| **Wallet / signer** | The thing that holds the user's keys and signs transactions. In local development, the browser provider is the local chain's accounts. |
| **Wei / WAD / RAY** | Scaled whole numbers (see the contracts guide). The frontend uses `BigInt` for these, so no precision is lost. |
| **SSE (Server-Sent Events)** | A connection where the server keeps sending messages to the browser as they happen. |
| **Intent** | A signed permission letting a liquidator act on the user's position under stated limits. |

---

## Part 1: The folder layout

```
web/
├── index.html              Landing page (entry point 1)
├── markets.html            The app: markets, positions, intents (entry point 2)
├── live.html               The live pipeline view (entry point 3)
├── package.json            Scripts and dependencies
├── vite.config.ts          Build settings; pages and the /api proxy
├── tsconfig.json           TypeScript settings
├── dist/                   The built files the gateway serves (generated)
├── node_modules/           Installed libraries (generated)
└── src/
    ├── entry-landing.tsx   Starts the landing page
    ├── entry-markets.tsx   Starts the app
    ├── entry-live.tsx      Starts the live view
    ├── theme.ts            Light and dark mode
    ├── lib/                Plain logic: no drawing, testable
    │   ├── chain.ts        Blockchain connection and transactions
    │   ├── api.ts          Calls to the Rust gateway
    │   ├── actions.ts      What each button does, step by step
    │   ├── position.ts     Health factor and borrowing limit maths
    │   ├── rates.ts        Interest rate maths
    │   ├── market.ts       Turns raw market data into rates for the table
    │   ├── format.ts       Turns numbers into readable text
    │   ├── tokens.ts       Names, tickers and symbols for each token
    │   ├── types.ts        The shapes of the data
    │   ├── position.test.ts and rates.test.ts   Automated tests
    ├── state/              Shared data and live updates
    │   ├── hooks.ts        Loads config, accounts, intents
    │   ├── stream.tsx      One live connection to the gateway
    │   └── toasts.tsx      Short popup messages
    ├── pages/              One component per page
    │   ├── MarketsPage.tsx The app
    │   ├── LandingPage.tsx The landing page
    │   ├── LivePage.tsx    The pipeline view
    │   └── messages.ts     The message shape shown to users
    ├── components/         Reusable pieces of the page
    └── styles/             CSS for each page
```

---

## Part 2: The three HTML pages

Each `.html` file is just a shell. It has a `<div id="root">` where React draws the page, and one script line that starts the matching entry file.

- **`index.html`** loads `entry-landing.tsx`. It is the public front page, with a link into the app.
- **`markets.html`** loads `entry-markets.tsx`. This is the real app.
- **`live.html`** loads `entry-live.tsx`. It shows each service's status in turn.

The page also loads fonts (Inter and JetBrains Mono) from Google Fonts.

---

## Part 3: `vite.config.ts`: build and development settings

1. **Plugin.** Enables React support.
2. **Development proxy.** Requests to `/api/...` are forwarded to the gateway at `http://127.0.0.1:3000`. This lets the development server (port 5173) and the gateway work together.
3. **Build inputs.** Tells the build about all three HTML pages, so each one is built and can be opened directly.
4. **Output.** The build goes to `dist/`. The gateway serves that folder (see the services guide).

---

## Part 4: The entry points

### 4.1 `entry-markets.tsx` (the app)

Runs in this order:

1. `applySavedTheme()`: sets light or dark mode from the user's last choice.
2. `createRoot(...).render(...)`: attaches React to the `root` element.
3. Wraps the page in two providers, from the outside in:
   - **`StreamProvider`**: the live connection to the gateway.
   - **`ToastProvider`**: the popup messages.
4. Draws **`MarketsPage`** inside them.

The providers wrap the page, so any component inside can read the live data and show messages without passing them down by hand.

### 4.2 `entry-landing.tsx` and `entry-live.tsx`

The same pattern, drawing `LandingPage` or `LivePage`.

---

## Part 5: `src/lib/chain.ts`: the blockchain connection

**Job:** everything that talks to the chain directly.

1. **`RPC_URL`** is `VITE_RPC_URL` if it is set at build time, and `http://127.0.0.1:8545` (the local chain) otherwise. Set `VITE_RPC_URL` to point the app at another chain.
2. **`provider`** is the connection to the node. `staticNetwork: true` tells ethers not to re-check the network on every call.
3. **`POOL_ABI` and `TOKEN_ABI`** list the functions the app calls, written in the Solidity signature style. For example, `"function borrow(uint256 amount)"`. These match the contracts.
4. **`INTENT_TYPES`** describes the shape of a liquidation intent for the wallet's typed-data signing. It must match the contract's `LIQUIDATION_INTENT_TYPEHASH` exactly, or signatures won't verify.
5. **`sendTransaction(from, contract, method, args)`**:
   1. Get a signer for the account.
   2. Estimate the gas needed, then add 30%. The estimate can be too low when interest accrues before the transaction is mined. This fixed a real out-of-gas failure on repay.
   3. Send the transaction, then wait until it is mined.
6. **`approveIfNeeded(from, token, spender, amount)`**: a token must be approved before the pool can take it. This checks the current allowance and only sends an approval if it's too small.
7. **`parseAmount(text, decimals, allowZero)`**: turns what the user typed (like `12.5`) into the smallest units (like `12500000` for a 6-decimal token). It rejects empty, negative and zero amounts unless allowed, and rejects too many decimal places.
8. **`reasonOf(err)`**: finds a short readable reason in an error from the wallet or the chain, so the user sees something useful instead of raw code.

---

## Part 6: `src/lib/api.ts`: calls to the gateway

**Job:** talk to the Rust gateway over HTTP.

1. **`getJson(path)`**: fetches a path and parses JSON. If the response isn't OK, it throws the text from the server.
2. **`fetchConfig`**: `/api/config`, the deployment addresses.
3. **`fetchProtocol`**: `/api/protocol`, the chain snapshot (prices, totals, pause state).
4. **`fetchAccount(who)`**: `/api/account/{address}`, one account's position.
5. **`fetchIntents`**: `/api/intents`, all stored intents.
6. **`submitIntent(body)`**: posts a signed intent to `/api/intents`. It throws if the gateway rejects it, passing the reason along.

---

## Part 7: `src/lib/actions.ts`: what each button does

Every function here is one user action. Each one checks the numbers first and throws a plain message if something is wrong. Then it sends the transaction.

1. **`ActionContext`** bundles what an action needs: the account, the contracts, the balances, the position, and a reload function.
2. **`mintTestTokens`**: for local testing only. Mints 1000 of each collateral and 100 vUSD to the account.
3. **`supply(asset, text)`**: deposits collateral.
   - Parses the amount, refuses if the wallet doesn't have enough.
   - Approves the token for the pool if needed.
   - Calls `depositCollateral`.
   
   **Note:** the button labelled "supply" deposits *collateral*. It does not call the pool's lender `supply` function, so this page cannot be used to lend the debt token.
4. **`borrow(text)`**: checks the amount against the borrow capacity, then calls `borrow`. Capacity is worked out so the health factor stays at least 1.05 after borrowing.
5. **`repay(text)`**: approves the debt token if needed, then calls `repay`.
6. **`withdraw(asset, text)`**: checks the amount against the most that can be taken out while keeping the health factor at 1.05, then calls `withdrawCollateral`.
7. **`repayAllAndReturn(ctx, step)`**: the "close everything" flow:
   1. If there is debt, check the wallet has enough vUSD to cover it. Interest grows every block, so this is checked against the latest debt.
   2. Approve the maximum amount, then repay the full debt. Using the maximum amount repays everything, including interest added since the check.
   3. Read the position again, to confirm the debt is zero.
   4. Withdraw each collateral the pool holds, one transaction each.
   
   The `step` function adds a line to a progress list in the dialog, so the user sees each stage.
8. **`positionFor`**: builds a position from fresh account data.

---

## Part 8: `src/lib/position.ts`: the health maths

**Job:** the same maths as the contract's health factor, so the page can show the numbers before a transaction is sent. It is pure functions with no browser code, which makes it testable.

1. **`calculatePosition(account, fallbackPrice)`**:
   - For each collateral, convert its amount to 18 decimals, multiply by its price to get a dollar value, then multiply by its liquidation threshold to get a *weighted* value.
   - Convert the debt to dollars using the debt price.
   - Mark the position as incomplete if any price is missing, so no limit is shown as if it were known.
2. **`healthFactor(p)`**: weighted value divided by debt value. `null` when there is no debt or a price is missing.
3. **`limitUsedPercent(p)`**: how much of the borrowing power is in use, as a percent.
4. **`borrowCapacity(p)`**: the extra vUSD that can be borrowed while keeping the health factor at the 1.05 target.
5. **`maxWithdrawable(p, asset)`**: the most of one collateral that can be taken out at the same target.
6. **`healthAfterBorrow(p, amount)`** and **`afterSupply(p, asset, amount)`**: preview what the health factor will be after a borrow or deposit, before the user confirms.

**Why 1.05 and not 1.0:** the page keeps a small buffer, so a small price move doesn't put the position straight into liquidation.

---

## Part 9: `src/lib/rates.ts` and `src/lib/market.ts`: interest maths

1. **`annualBorrowRate(u, rates)`**: the same kinked model as the contract. Below the optimal utilization it rises gently, above it steeply.
2. **`toApy(rate)`**: converts the yearly rate to the annual yield with per-second compounding, matching the contract.
3. **`computeMarket(input)`**:
   - Total debt and total supply are scaled balances times their indexes.
   - Utilization is debt divided by supply, capped at 100%.
   - The borrow rate comes from the model. The supply rate is the borrow rate on the borrowed share, minus the reserve cut.
4. **`marketFrom(chain, config)`** in `market.ts` gathers the snapshot data, and calls `computeMarket` for the table.

---

## Part 10: `src/lib/format.ts`, `tokens.ts`, `types.ts`

- **`format.ts`**: `fmt(units, decimals, places)` turns a raw amount into readable text with commas. `dollars(wad)` formats an 18-decimal dollar value. `short(address)` shortens an address to `0x1234…abcd`.
- **`tokens.ts`**: `TOKEN_INFO` maps each collateral's internal name to a readable label and ticker. `tokenInfo(name)` looks one up.
- **`types.ts`**: the shapes of the data coming from the gateway: `Config`, `ChainSnapshot`, `AccountView`, `IntentRow`, `ActivityMessage`, and so on. These keep the rest of the code honest about what fields exist.

---

## Part 11: `src/state/`: shared data and live updates

### 11.1 `stream.tsx`: one live connection

1. **`StreamProvider`** opens an `EventSource` to `/api/stream` once, when the app starts.
2. **Each message** is parsed and sorted by kind:
   - **`chain`**: a new chain snapshot. It updates the prices and keeps the last 60 prices per collateral for the small charts.
   - **`db`**: the counts from the database (indexer position, events processed, positions, intents, transactions).
   - **`nats`**: a message from the queue (an event, a block, a signal or a retraction). It is added to the activity feed (newest first, at most 200) and the counts go up.
3. **`connected`** tracks whether the connection is open.
4. **`useStream()`** lets any component read this state.

### 11.2 `hooks.ts`: the data hooks

- **`useConfig()`** loads `/api/config` once and returns `null` until it arrives.
- **`useContracts(config)`** builds the ethers contract objects for the pool, the debt token and each collateral token. It runs again only when the config changes.
- **`useAccount(contracts, who)`** loads the selected account's position from the gateway, and its wallet balances from the chain. It refreshes every 1.5 seconds, so the page follows each block. It also exposes `reload` for the actions to call after a transaction.
- **`useIntents(contracts, who)`** loads this account's intents and asks the chain which nonces are already used. It refreshes every 5 seconds.
- **`useProtocolOnce()`** loads the protocol snapshot for the landing page every 3 seconds.

### 11.3 `toasts.tsx`

A small provider for short popup messages, used by the app to confirm or report actions.

---

## Part 12: `src/pages/MarketsPage.tsx`: the app

**Job:** the main screen. It brings everything together.

1. **Load data.** Config, the protocol snapshot, the selected account and the intents, using the hooks above.
2. **Choose an account.** The page lists known accounts. "Only mine" filters to the selected one.
3. **Show the totals**: total supplied, total borrowed, utilization and rates.
4. **Four tabs:**
   - **Markets** (`Markets.tsx`): one row per collateral, with price, rates and the user's amount. Each collateral has Supply and Withdraw buttons, and the debt token has Borrow and Repay buttons. Each opens the action dialog on the matching tab.
   - **Positions** (`AccountTables.tsx`): the account's collateral and debt, with its health factor and limit use.
   - **Intents** (`IntentsCard.tsx`): create and view liquidation intents.
   - **Activity** (`Activity.tsx`): the live feed from the stream.
5. **Action dialog** (`ActionDialog.tsx`): opens for deposit, borrow, repay or withdraw. It shows the preview from `position.ts` before the user confirms, and runs the matching action from `actions.ts`.
6. **Busy state.** While an action runs, the other buttons are disabled, so two transactions aren't started at once.
7. **Messages.** Each action reports success or failure in the message area, using the `messages.ts` shape.

---

## Part 13: `src/components/IntentsCard.tsx`: signing an intent

**Job:** let the user create a liquidation intent. This is the most security-sensitive part of the page.

**What happens when the user clicks "sign":**

1. Read the form: which collateral, the maximum to repay, the minimum collateral to receive (optional), and how many days it stays valid (default 30).
2. Get the latest block's timestamp, and set the deadline to that plus the number of days. The deadline uses chain time, not the computer's clock.
3. Pick the **lowest nonce** that is neither already used on-chain nor already in the list. Nonces are used once each, so reusing one would be rejected.
4. Build the typed data: the domain (`Vigil LendingPool`, version 1, the chain ID, the pool's address) and the six intent fields.
5. Ask the wallet to sign the typed data. This costs no gas. It only proves the account agrees to the limits.
6. Send the intent and signature to `/api/intents`. The gateway checks the signature before storing it.
7. Reload the list and show "Intent N is live until …", or the reason it failed.

**What the signature means:** anyone holding it can liquidate this account's position up to `maxRepay`, take at least `minCollateralOut`, and only until the deadline. Nothing more.

---

## Part 14: `src/pages/LivePage.tsx`: the pipeline view

**Job:** show each stage of the system as a card, with its key number.

- **Chain**: the head block, and whether the oracle is frozen.
- **Indexer**: the last indexed block, and how far behind the head it is (the lag).
- **Messages**: how many events, blocks and signals have passed through NATS.
- **Risk engine**: how many positions have debt, and how many events were processed.
- **Liquidator**: how many transactions were sent, and how many intents are on file.

Below the cards: the activity feed, and a log of liquidator transactions.

---

## Part 15: `src/pages/LandingPage.tsx`

A marketing-style page, with the "how it works" and "features" sections and links to the app and the live view. It shows a small amount of live protocol data from `useProtocolOnce`.

---

## Part 16: `src/components/` (the smaller pieces)

- **`Header.tsx`**: top bar with links, the theme button, and the connection status.
- **`Summary.tsx`**: the totals row on the app.
- **`Markets.tsx`**: the markets table.
- **`AssetCell.tsx`**: a token's icon and name.
- **`Sparkline.tsx`**: a tiny price chart, drawn as SVG from the price history.
- **`AccountTables.tsx`**: the positions and balances tables.
- **`Activity.tsx`**: the activity feed.
- **`ActionDialog.tsx`**: the form and preview dialog for each action.
- **`ThemeButton.tsx`**: switches light and dark mode.
- **`IntentsCard.tsx`**: described above.

---

## Part 17: `theme.ts`

Remembers the user's light or dark choice, and applies it on start-up. Colours are set as CSS variables, so every page follows the same theme.

---

## Part 18: Tests

- **`position.test.ts`**: checks the health factor, borrow capacity and withdraw limits on fixed examples.
- **`rates.test.ts`**: checks the interest rate and APY maths.

Run them with:

```bash
cd web
npm test
```

---

## Part 19: Running the web app

```bash
cd web
npm install        # first time only

# Option A: development server with hot reload (needs the gateway on port 3000)
npm run dev        # open http://localhost:5173/markets.html

# Option B: build, then let the gateway serve it
npm run build      # writes dist/
# open http://127.0.0.1:3000/markets.html
```

The build runs the TypeScript check first (`tsc --noEmit`), so type errors stop the build.

---

## Part 20: Things to know

- **Localhost by default.** The chain address comes from `VITE_RPC_URL` (default `http://127.0.0.1:8545`), and the development proxy to the gateway comes from `VITE_GATEWAY_URL` (default `http://127.0.0.1:3000`). Set them when you build or run the app against other machines.
- **The app only shows real data when the services are running.** The positions and intents come from the gateway, which reads the database. Without the indexer, risk engine and liquidator, the tables can be empty or stale.
- **Prices come from the chain.** The app reads the oracle through the gateway's snapshot. If the price feed stops, the oracle goes stale and the app shows frozen prices.
- **The "supply" button deposits collateral**, not debt tokens. Lenders can't use this page to supply the debt token yet.
- **Intent signing is checked for the digest, not in a browser.** The typed data the browser signs produces the same digest as the contract's `hashIntent` for the same values. A browser click-through has not been run.
