// =====================================================================
// Vigil lending page
//
// The page does two jobs:
//   1. LISTEN: the gateway sends live updates (prices, new blocks, activity).
//   2. SEND: when you press a button, we send a transaction to the pool.
//
// Money in the contracts is stored as whole numbers ("units").
// Example: 1 vUSD is 1000000 units, because vUSD has 6 decimals.
// We use BigInt (the "123n" numbers) so these numbers never lose precision.
// =====================================================================

const RPC_URL = "http://127.0.0.1:8545";
const ethers = window.ethers;
const provider = new ethers.JsonRpcProvider(RPC_URL, undefined, { staticNetwork: true });

// The contract uses 18 decimal places for prices. 1 USD is 10^18.
const ONE = 10n ** 18n;
// The largest possible number. Approving this means "allow any amount".
const MAX_UINT = ethers.MaxUint256;

// After a borrow or a withdraw we keep the health factor at 1.05 or more, not just 1.00.
// Prices move between reading the numbers and the transaction being mined, so this buffer
// stops a small price move from putting the position at risk straight away. Written in percent.
const TARGET_HEALTH_PERCENT = 105n;

// The functions on the pool and token contracts that this page calls.
const POOL_ABI = [
  "function depositCollateral(address asset, uint256 amount)",
  "function withdrawCollateral(address asset, uint256 amount)",
  "function borrow(uint256 amount)",
  "function repay(uint256 amount)",
  "function nonceUsed(address user, uint256 nonce) view returns (bool)",
];
const TOKEN_ABI = [
  "function balanceOf(address) view returns (uint256)",
  "function allowance(address owner, address spender) view returns (uint256)",
  "function approve(address spender, uint256 amount) returns (bool)",
  "function mint(address to, uint256 amount)",
];

// The shape of a liquidation intent. The signature covers exactly these fields.
const INTENT_TYPES = {
  LiquidationIntent: [
    { name: "borrower", type: "address" },
    { name: "collateralAsset", type: "address" },
    { name: "maxRepay", type: "uint256" },
    { name: "minCollateralOut", type: "uint256" },
    { name: "nonce", type: "uint256" },
    { name: "deadline", type: "uint256" },
  ],
};

// ---------------------------------------------------------------------
// Memory: things the page remembers while it runs
// ---------------------------------------------------------------------
let config = null;          // contract addresses and collateral settings, from the gateway
let pool = null;            // the lending pool contract
let debtToken = null;       // the vUSD token contract
let collaterals = [];       // one entry per collateral asset
let account = null;         // the latest position of the selected account
let busy = false;           // true while a transaction is running
let prices = {};            // latest price per collateral name (null when frozen)
let priceHistory = {};      // recent prices per collateral, for the small chart
let debtPrice = ONE;        // price of vUSD (about 1 USD)
const RAY = 10n ** 27n;
let market = null;          // pool totals and the interest rates derived from them

// ---------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------

// Shorthand for document.getElementById.
function $(id) {
  return document.getElementById(id);
}

// Turns a whole-number amount into a readable string.
// Example: fmt(1500000n, 6) gives "1.50".
function fmt(units, decimals, places = 2) {
  const number = Number(ethers.formatUnits(units, decimals));
  return number.toLocaleString("en-US", { maximumFractionDigits: places, minimumFractionDigits: 2 });
}

// Turns a price (18 decimals) into a dollar string. Example: "$1,593.55".
function dollars(price) {
  return "$" + fmt(price, 18, 2);
}

// Shortens an address for display: 0x1234…abcd
function short(address) {
  return address.slice(0, 6) + "…" + address.slice(-4);
}

// Shows a message under a button. kind is "good", "error" or "busy".
function showMessage(id, text, kind) {
  $(id).textContent = text;
  $(id).className = "msg " + (kind || "");
}

// Gets a short, human reason from a failed transaction.
function reasonOf(err) {
  const reason = err.reason || err.shortMessage || err.info?.error?.message || err.message || "something went wrong";
  // A revert usually means the price or the interest moved after we read the numbers.
  if (reason.includes("reverted")) {
    return "the contract refused it (the price or interest may have moved). Try a slightly smaller amount";
  }
  return reason;
}

// Turns text the user typed into units. Throws a clear error if the text is not a valid amount.
// allowZero: whether 0 is an acceptable amount.
function parseAmount(text, decimals, allowZero = false) {
  const typed = (text || "").trim();
  if (typed === "") throw new Error("enter an amount");
  if (!/^\d*\.?\d*$/.test(typed) || typed === ".") throw new Error("the amount must be a number");
  const decimalPart = typed.split(".")[1] || "";
  if (decimalPart.length > decimals) throw new Error("at most " + decimals + " decimal places");
  const units = ethers.parseUnits(typed, decimals);
  if (units === 0n && !allowZero) throw new Error("the amount must be above zero");
  return units;
}

// Sends a transaction from one account and waits until it is mined.
// The gas limit is the node's estimate plus 30%. The estimate can come out too low when interest
// accrues between estimating and mining, which made some repayments run out of gas.
async function sendTransaction(from, contract, method, args) {
  const signer = await provider.getSigner(from);
  const connected = contract.connect(signer);
  const gasLimit = (await connected[method].estimateGas(...args)) * 13n / 10n;
  const tx = await connected[method](...args, { gasLimit });
  return tx.wait();
}

// Approves a token for the pool, but only when the current approval is too small.
async function approveIfNeeded(from, token, amount) {
  const current = await token.allowance(from, config.pool);
  if (current < amount) {
    await sendTransaction(from, token, "approve", [config.pool, amount]);
  }
}

// Runs one button action with the same steps every time:
// lock the buttons, show "working", run the action, show the result, unlock, refresh.
async function runAction(messageId, label, action) {
  if (busy) return;
  busy = true;
  setButtonsDisabled(true);
  // Each message box sits under its button, e.g. msgDeposit belongs to btnDeposit.
  const button = $(messageId.replace("msg", "btn"));
  if (button) button.classList.add("busy");
  showMessage(messageId, label + "…", "busy");
  toast(label, "Waiting for the transaction to be mined…", "busy");
  try {
    const extra = await action();
    showMessage(messageId, label + " done." + (extra || ""), "good");
    toast(label + " complete", (extra || "").trim() || "Done.", "good");
  } catch (err) {
    showMessage(messageId, label + " failed: " + reasonOf(err), "error");
    toast(label + " failed", reasonOf(err), "error");
  }
  if (button) button.classList.remove("busy");
  busy = false;
  setButtonsDisabled(false);
  await loadAccount();
}

// A short pop-up message in the corner. Removes itself after a few seconds.
function toast(title, text, kind) {
  const box = $("toasts");
  if (!box) return;
  const item = document.createElement("div");
  item.className = "toast " + (kind || "");
  item.innerHTML = `<b></b><span></span>`;
  item.querySelector("b").textContent = title;
  item.querySelector("span").textContent = text;
  box.appendChild(item);
  while (box.children.length > 4) box.removeChild(box.firstChild);
  setTimeout(() => {
    item.classList.add("out");
    setTimeout(() => item.remove(), 320);
  }, kind === "busy" ? 4000 : 6000);
}

function setButtonsDisabled(disabled) {
  const buttons = ["btnMint", "btnDeposit", "btnBorrow", "btnRepay", "btnReturn", "btnWithdraw", "btnIntent"];
  for (const id of buttons) $(id).disabled = disabled;
}

// The account the user picked in the dropdown.
function selectedAccount() {
  return $("account").value;
}

// The collateral picked in a dropdown.
function collateralFrom(selectId) {
  return collaterals[Number($(selectId).value)];
}

// What each test token is for. Keys are the names in the deployment file, so the page
// can still find each token. The on-chain symbols are unchanged.
const TOKEN_INFO = {
  fee: { label: "Test Wrapped Ether", ticker: "tWETH", note: "Test token, 18 decimals. Takes a 1% fee on every transfer, so the pool must count what it actually receives." },
  six: { label: "Test USD Stable", ticker: "tUSDS", note: "Test token, 6 decimals, modelled on a stablecoin. Lowest volatility, highest liquidation threshold." },
  eight: { label: "Test Wrapped Bitcoin", ticker: "tWBTC", note: "Test token, 8 decimals, modelled on a wrapped bitcoin. Most volatile, highest liquidation bonus." },
};

// ---------------------------------------------------------------------
// Start-up
// ---------------------------------------------------------------------
async function start() {
  // 1. Ask the gateway for the contract addresses.
  try {
    config = await (await fetch("/api/config")).json();
  } catch (err) {
    showMessage("msgMint", "The gateway is not running. Start vigil-gateway and reload.", "error");
    return;
  }

  // 2. Make contract objects we can call.
  pool = new ethers.Contract(config.pool, POOL_ABI, provider);
  debtToken = new ethers.Contract(config.debt, TOKEN_ABI, provider);
  collaterals = config.collaterals.map((c) => {
    const info = TOKEN_INFO[c.name] || { label: c.name, ticker: c.name.toUpperCase(), note: "" };
    return { ...c, ...info, contract: new ethers.Contract(c.address, TOKEN_ABI, provider) };
  });

  // 3. Fill the dropdowns.
  const accounts = await provider.send("eth_accounts", []);
  $("account").innerHTML = accounts
    .map((address, i) => `<option value="${address}">${i === 0 ? "Deployer" : "Account " + i} · ${short(address)}</option>`)
    .join("");
  const assetOptions = collaterals.map((c, i) => `<option value="${i}">${c.label} (${c.ticker})</option>`).join("");
  $("depAsset").innerHTML = assetOptions;
  $("wdAsset").innerHTML = assetOptions;
  $("intAsset").innerHTML = assetOptions;

  // 4. Connect the buttons to their functions.
  $("btnMint").onclick = mintTestTokens;
  $("btnDeposit").onclick = deposit;
  $("btnBorrow").onclick = borrow;
  $("btnRepay").onclick = repay;
  $("btnReturn").onclick = repayAllAndReturn;
  $("btnWithdraw").onclick = withdraw;
  $("btnIntent").onclick = submitIntent;
  $("depMax").onclick = depositMax;
  $("borMax").onclick = borrowMax;
  $("repMax").onclick = repayMax;
  $("wdMax").onclick = withdrawMax;
  $("borAmount").oninput = updateBorrowPreview;
  $("depAmount").oninput = updateSupplyPreview;
  $("depAsset").onchange = updateSupplyPreview;
  $("account").onchange = () => { loadAccount(); loadIntents(); };

  // 5. Start listening for live updates and load the first data.
  connectToLiveUpdates();
  await loadAccount();
  await loadIntents();
  setInterval(loadAccount, 1500);
  setInterval(loadIntents, 5000);
}

// ---------------------------------------------------------------------
// Live updates from the gateway (Server-Sent Events)
// ---------------------------------------------------------------------
function connectToLiveUpdates() {
  const stream = new EventSource("/api/stream");
  stream.onopen = () => setPill("conn", "live", "ok");
  stream.onerror = () => setPill("conn", "reconnecting…", "bad");
  stream.onmessage = (event) => {
    const msg = JSON.parse(event.data);
    if (msg.kind === "chain") showChainState(msg.data);
    if (msg.kind === "nats") addActivity(msg.stage, msg.data);
  };
}

function setPill(id, text, kind) {
  $(id).textContent = text;
  $(id).className = "pill " + (kind || "");
}

// Called on every new block: prices, the block number and the pool totals.
function showChainState(data) {
  setPill("block", "block " + data.head);
  setPill("oracle", data.frozen ? "oracle FROZEN" : "oracle normal", data.frozen ? "bad" : "ok");

  if (data.debtPriceWad && data.debtPriceWad !== "frozen") {
    debtPrice = BigInt(data.debtPriceWad);
  }

  // Pool totals. The stored debt is "scaled", so multiply by the debt index to get real vUSD.
  const index = BigInt(data.debtIndex);
  const totalDebt = (BigInt(data.totalDebtScaled) * index) / 10n ** 27n;
  $("totalDebt").textContent = fmt(totalDebt, config.debtDecimals);
  $("reserves").textContent = fmt(BigInt(data.reserves), config.debtDecimals);
  $("deficit").textContent = fmt(BigInt(data.deficit), config.debtDecimals);
  $("debtIndex").textContent = (Number(index) / 1e27).toFixed(6);

  // Prices: remember the last 60, so the chart can draw them.
  for (const p of data.prices) {
    const price = p.price_wad === "frozen" ? null : BigInt(p.price_wad);
    prices[p.name] = price;
    if (price !== null) {
      const history = priceHistory[p.name] || [];
      history.push(price);
      if (history.length > 60) history.shift();
      priceHistory[p.name] = history;
    }
  }
  updateMarket(data);
  showMarkets();
  showBorrowMarkets();
  showHoldings();
  showBorrows();
}

// Interest rates, the same formula as the contract (InterestMath.annualBorrowRate).
// Borrow rate rises with utilisation. Supply rate is the borrow rate × utilisation × (1 − reserve cut).
function updateMarket(data) {
  const index = BigInt(data.debtIndex);
  const supplyIndex = BigInt(data.supplyIndex);
  const totalDebt = (BigInt(data.totalDebtScaled) * index) / RAY;
  const totalSupply = (BigInt(data.totalSupplyScaled) * supplyIndex) / RAY;
  const utilization = totalSupply === 0n ? 0n : (totalDebt * RAY) / totalSupply;
  const u = utilization > RAY ? RAY : utilization;

  const rates = config.rates;
  const base = BigInt(rates.baseRate);
  const slope1 = BigInt(rates.slope1);
  const slope2 = BigInt(rates.slope2);
  const optimal = BigInt(rates.optimalUtil);
  const borrowRate = u <= optimal
    ? base + (slope1 * u) / optimal
    : base + slope1 + (slope2 * (u - optimal)) / (RAY - optimal);
  const reserveFactor = BigInt(config.reserveFactor);
  const supplyRate = (borrowRate * u * (RAY - reserveFactor)) / (RAY * RAY);

  // Annual rates as APY (compounded per second, as the contract does).
  const apy = (rate) => (Math.expm1(Number(rate) / 1e27) * 100);
  market = {
    totalDebt,
    totalSupply,
    utilization: Number(u) / 1e27 * 100,
    borrowApy: apy(borrowRate),
    supplyApy: apy(supplyRate),
  };
}

// The vUSD borrow market, with Borrow and Repay buttons.
function showBorrowMarkets() {
  if (!market || !config) return;
  $("borrowMarkets").innerHTML = `
    <tr>
      <td><div class="asset"><span class="coin vusd">$</span><div><div class="name">vUSD</div><div class="ticker">Vigil USD</div></div></div></td>
      <td class="num">${dollars(debtPrice)}</td>
      <td class="num">${fmt(market.totalDebt, config.debtDecimals)}</td>
      <td class="num ${market.borrowApy > 0 ? "" : "muted"}">${market.borrowApy.toFixed(2)}%</td>
      <td class="num">${market.supplyApy.toFixed(2)}%</td>
      <td class="num">${market.utilization.toFixed(1)}%</td>
      <td><div class="row-actions">
        <button class="btn-sm" onclick="openActionDialog(-1, 'borrow')">Borrow</button>
        <button class="btn-sm alt" onclick="openActionDialog(-1, 'repay')">Repay</button>
      </div></td>
    </tr>`;
}

// Your borrows: the vUSD debt of the selected account.
function showBorrows() {
  if (!account || !market) return;
  const p = calculatePosition();
  if (p.debt === 0n) {
    $("borrows").innerHTML = `<tr><td colspan="4" class="muted">No borrows yet. Borrow against your collateral from the markets above.</td></tr>`;
    return;
  }
  $("borrows").innerHTML = `
    <tr>
      <td><div class="asset"><span class="coin vusd">$</span><div><div class="name">vUSD</div><div class="ticker">Vigil USD</div></div></div></td>
      <td class="num">${fmt(p.debt, account.debtDecimals)}</td>
      <td class="num">${market.borrowApy.toFixed(2)}%</td>
      <td><div class="row-actions"><button class="btn-sm alt" onclick="openActionDialog(-1, 'repay')">Repay</button></div></td>
    </tr>`;
}

// The market table: one row per collateral, with its price and a small chart.
let lastShownPrice = {};   // the price each row showed last time, to detect a move

function showMarkets() {
  if (collaterals.length === 0) return;
  $("markets").innerHTML = collaterals
    .map((c, index) => {
      const price = prices[c.name];
      const history = priceHistory[c.name] || [];
      const first = history[0];
      const last = history[history.length - 1];
      const direction = first !== undefined && last !== undefined && last < first ? "down" : "up";
      const change = first && last ? (Number(last - first) / Number(first) * 100).toFixed(2) + "%" : "";

      // Flash the price green or red when it moves since the last block.
      const before = lastShownPrice[c.name];
      let flash = "";
      if (before && price && price !== before) flash = price > before ? "tick-up" : "tick-down";
      if (price) lastShownPrice[c.name] = price;

      return `<tr>
        <td>${assetBadge(c)}</td>
        <td class="num price-cell ${flash}">${price === null || price === undefined ? "frozen" : dollars(price)}</td>
        <td style="width:40%">${sparkline(history, direction)}<span class="chg ${direction}">${change}</span></td>
        <td class="num">${(c.liqThresholdBps / 100).toFixed(0)}%</td>
        <td class="num">${(c.liqBonusBps / 100).toFixed(0)}%</td>
        <td><div class="row-actions">
          <button class="btn-sm" onclick="openActionDialog(${index}, 'deposit')">Supply</button>
          <button class="btn-sm alt" onclick="openActionDialog(${index}, 'withdraw')">Withdraw</button>
        </div></td>
      </tr>`;
    })
    .join("");
}

// A round badge with the asset's first letter, next to its name.
function assetBadge(c) {
  return `<div class="asset" title="${c.note}"><span class="coin ${c.name}">${c.ticker.slice(1, 3)}</span><div><div class="name">${c.label}</div><div class="ticker">${c.ticker}</div></div></div>`;
}

// A tiny line chart drawn as SVG. Each point is one recent price.
function sparkline(values, direction) {
  if (values.length < 2) return `<svg class="spark"></svg>`;
  const low = values.reduce((a, b) => (b < a ? b : a));
  const high = values.reduce((a, b) => (b > a ? b : a));
  const range = high - low || 1n;
  const width = 120;
  const height = 32;
  const points = values
    .map((v, i) => {
      const x = (i / (values.length - 1)) * width;
      const y = height - (Number((v - low) * 1000n / range) / 1000) * height;
      return x.toFixed(1) + "," + y.toFixed(1);
    })
    .join(" ");
  const color = direction === "down" ? "var(--bad)" : "var(--ok)";
  return `<svg class="spark" viewBox="0 0 ${width} ${height}" preserveAspectRatio="none"><polyline fill="none" stroke="${color}" stroke-width="1.5" points="${points}"/></svg>`;
}

// Activity feed: pool events and liquidation signals from the pipeline.
function addActivity(stage, data) {
  if (stage !== "events" && stage !== "signals") return;

  // Who the entry is about, so the "only the selected account" filter can work.
  const who = (data.event && (data.event.user || data.event.borrower)) || data.borrower || "";
  if ($("onlyMe").checked && who.toLowerCase() !== selectedAccount().toLowerCase()) return;

  let html;
  if (stage === "signals") {
    html = `<span class="tag sig">liquidation signal</span> ${short(data.borrower)} at block ${data.block_number}`;
  } else {
    const event = data.event || {};
    const retracted = data.kind === "retract" ? " (retracted)" : "";
    // Only borrow and repay amounts are in vUSD. Collateral amounts use the asset's own decimals.
    const isDebtEvent = event.type === "borrowed" || event.type === "repaid";
    const amount = isDebtEvent && event.amount ? " · " + fmt(BigInt(event.amount), config.debtDecimals) + " vUSD" : "";
    html = `<span class="tag">${event.type || "event"}</span>${retracted} ${who ? short(who) : ""}${amount} <span class="muted">block ${data.block_number}</span>`;
  }

  const row = document.createElement("div");
  row.className = "entry";
  row.innerHTML = html;
  const feed = $("feed");
  if (feed.firstElementChild && feed.firstElementChild.classList.contains("muted")) feed.innerHTML = "";
  feed.prepend(row);
  while (feed.children.length > 200) feed.removeChild(feed.lastChild);
}

// ---------------------------------------------------------------------
// The account: balances and position
// ---------------------------------------------------------------------

// Reads the selected account from the gateway, and its wallet balances from the chain.
async function loadAccount() {
  if (!pool) return;
  const who = selectedAccount();
  try {
    const response = await fetch("/api/account/" + who);
    if (!response.ok) throw new Error(await response.text());
    account = await response.json();

    // Wallet balances come straight from the token contracts.
    account.walletVusd = await debtToken.balanceOf(who);
    account.walletCollateral = {};
    for (const c of collaterals) {
      account.walletCollateral[c.address.toLowerCase()] = await c.contract.balanceOf(who);
    }
    showAccount();
  } catch (err) {
    account = null;
    $("hf").innerHTML = `<span class="bad">gateway not reachable</span>`;
  }
}

// The position math. It follows the contract's own health-factor rule:
//
//   value of each collateral = amount × price
//   weighted value           = sum of (value × liquidation threshold)
//   debt value               = debt × vUSD price
//   health factor           = weighted value ÷ debt value
//
// A health factor of 1 or more means the position is safe.
function calculatePosition() {
  const debt = BigInt(account.debt);
  const currentDebtPrice = account.debtPriceWad ? BigInt(account.debtPriceWad) : debtPrice;

  let weighted = 0n;
  const assets = account.collaterals.map((c) => {
    const amount = BigInt(c.amount);
    const price = c.priceWad ? BigInt(c.priceWad) : null;
    // Bring the amount up to 18 decimals so it can be multiplied by the 18-decimal price.
    const value = price === null ? 0n : (amount * 10n ** BigInt(18 - c.decimals) * price) / ONE;
    weighted += (value * BigInt(c.liqThresholdBps)) / 10000n;
    return { ...c, ...(TOKEN_INFO[c.name] || {}), amount, price, value };
  });

  const debtValue = (debt * 10n ** BigInt(18 - account.debtDecimals) * currentDebtPrice) / ONE;
  return { debt, debtValue, weighted, assets, debtPrice: currentDebtPrice };
}

// How much more vUSD can be borrowed, in vUSD units, keeping the health factor at the target.
// Health factor = weighted ÷ debt, so the most debt allowed is weighted ÷ 1.05.
function borrowCapacity() {
  if (!account) return 0n;
  const p = calculatePosition();
  if (p.debtPrice === 0n) return 0n;
  const maxDebtValue = (p.weighted * 100n) / TARGET_HEALTH_PERCENT;
  const spare = maxDebtValue > p.debtValue ? maxDebtValue - p.debtValue : 0n;
  return (spare * 10n ** BigInt(account.debtDecimals)) / p.debtPrice;
}

// The most of one collateral that can be taken out and still keep the health factor at the target.
function maxWithdrawable(collateral) {
  if (!account) return 0n;
  const p = calculatePosition();
  const asset = p.assets.find((a) => a.address.toLowerCase() === collateral.address.toLowerCase());
  if (!asset || asset.amount === 0n) return 0n;
  if (p.debt === 0n) return asset.amount;
  if (asset.price === null) return 0n;

  // Taking out collateral lowers the weighted value. The weighted value must stay above debt × 1.05.
  const requiredWeighted = (p.debtValue * TARGET_HEALTH_PERCENT) / 100n;
  const spare = p.weighted > requiredWeighted ? p.weighted - requiredWeighted : 0n;
  const removableValue = (spare * 10000n) / BigInt(asset.liqThresholdBps);
  const units = (removableValue * 10n ** BigInt(asset.decimals)) / asset.price;
  return units < asset.amount ? units : asset.amount;
}

// The vUSD needed to repay all the debt, or the wallet balance if that is smaller.
function repayableNow() {
  const debt = BigInt(account.debt);
  return debt < account.walletVusd ? debt : account.walletVusd;
}

// Shows everything about the selected account on the page.
function showAccount() {
  const p = calculatePosition();
  const dec = account.debtDecimals;

  showHealth(p);
  const collateralTotal = p.assets.reduce((sum, a) => sum + a.value, 0n);
  const netWorth = collateralTotal - p.debtValue;
  $("collateralValue").textContent = dollars(netWorth);
  $("collateralSub").textContent = "Collateral " + dollars(collateralTotal) + " − debt " + dollars(p.debtValue);
  $("myDebtBig").textContent = fmt(p.debt, dec) + " vUSD";
  $("capacity").textContent = fmt(borrowCapacity(), dec);
  $("auction").textContent = account.auctionStart !== "0" ? "Auction started at block " + account.auctionStart : "";
  $("riskTracked").textContent = account.riskEngineTracked ? "Risk engine is watching this position" : "";
  $("walletVusd").textContent = fmt(account.walletVusd, dec);
  $("supplied").textContent = "Lender deposits: " + fmt(BigInt(account.supplied), dec);
  $("fullNote").textContent = p.debt === 0n
    ? "You have no debt. This returns your collateral."
    : `You owe ${fmt(p.debt, dec, 6)} vUSD. Your wallet has ${fmt(account.walletVusd, dec, 6)} vUSD.`;

  const hasSomething = p.debt > 0n || p.assets.some((a) => a.amount > 0n);
  $("btnReturn").disabled = busy || !hasSomething;

  showHoldings();
  showBorrows();
  updateBorrowPreview();
}

// The health-factor ring and the borrow-limit meter.
function showHealth(p) {
  const ring = $("hfRing");
  if (p.debt === 0n) {
    ring.style.stroke = "var(--ok)";
    ring.setAttribute("stroke-dasharray", "100 100");
    $("hfCenter").textContent = "∞";
    $("hf").innerHTML = `<span class="ok">No debt</span>`;
    $("hfNote").textContent = "Nothing can be liquidated";
    $("limitUsed").textContent = "0%";
    $("hfBar").style.width = "0%";
    return;
  }
  if (account.healthFactor === null) {
    $("hf").innerHTML = `<span class="warn">Unavailable</span>`;
    $("hfNote").textContent = "The oracle is frozen. Repaying still works.";
    return;
  }

  const hf = Number(ethers.formatUnits(account.healthFactor, 18));
  const colour = hf >= 1.2 ? "var(--ok)" : hf >= 1 ? "var(--warn)" : "var(--bad)";
  ring.style.stroke = colour;
  ring.setAttribute("stroke-dasharray", Math.min(hf / 2, 1) * 100 + " 100");
  $("hfCenter").textContent = hf >= 100 ? ">100" : hf.toFixed(2);
  $("hf").innerHTML = `<span style="color:${colour}">${hf.toFixed(2)}</span>`;
  $("hfNote").textContent = hf >= 1 ? "Liquidation starts below 1.00" : "Below 1: can be liquidated";

  // Borrow limit used: debt value as a share of the weighted collateral value (the limit at HF 1).
  const used = p.weighted > 0n ? Math.min(Number((p.debtValue * 10000n) / p.weighted) / 100, 999) : 100;
  $("limitUsed").textContent = used.toFixed(1) + "%";
  $("hfBar").style.width = Math.min(used, 100) + "%";
  $("hfBar").style.background = used < 70 ? "var(--accent)" : used < 90 ? "var(--warn)" : "var(--bad)";
}

// Shows the health factor and borrow limit that supplying the typed amount would give.
function updateSupplyPreview() {
  if (!account) return;
  const p = calculatePosition();
  const c = collateralFrom("depAsset");
  let hfText = "–", limitText = "–";
  try {
    const amount = parseAmount($("depAmount").value, c.decimals);
    const price = c.priceWad ? BigInt(c.priceWad) : null;
    if (price !== null) {
      const value = (amount * 10n ** BigInt(18 - c.decimals) * price) / ONE;
      const weighted = p.weighted + (value * BigInt(c.liqThresholdBps)) / 10000n;
      const newHf = p.debt === 0n ? null : Number((weighted * ONE) / p.debtValue) / 1e18;
      hfText = newHf === null ? "No debt" : newHf.toFixed(2);
      limitText = fmt(((weighted > p.debtValue ? weighted - p.debtValue : 0n) * 10n ** BigInt(account.debtDecimals)) / p.debtPrice, account.debtDecimals) + " vUSD";
    }
  } catch (err) {
    // Nothing typed yet. Keep the placeholders.
  }
  $("depHf").textContent = hfText;
  $("depLimit").textContent = limitText;
  $("depLt").textContent = c.liqThresholdBps / 100 + "%";
}

// The table of collateral: what is in the wallet and what is in the pool.
function showHoldings() {
  if (!account) return;
  const p = calculatePosition();
  $("holdings").innerHTML = p.assets
    .map((c) => {
      const wallet = account.walletCollateral[c.address.toLowerCase()] ?? 0n;
      const value = c.price === null ? "frozen" : dollars(c.value);
      return `<tr>
        <td>${assetBadge(c)}</td>
        <td class="num">${fmt(wallet, c.decimals, 4)}</td>
        <td class="num">${fmt(c.amount, c.decimals, 4)}</td>
        <td class="num">${value}</td>
      </tr>`;
    })
    .join("");
}

// As you type a borrow amount, show what the health factor would become.
function updateBorrowPreview() {
  if (!account) return;
  const p = calculatePosition();
  let text = "";
  try {
    const amount = parseAmount($("borAmount").value, account.debtDecimals);
    const newDebtValue = ((p.debt + amount) * 10n ** BigInt(18 - account.debtDecimals) * p.debtPrice) / ONE;
    const hf = newDebtValue === 0n ? null : Number((p.weighted * ONE) / newDebtValue) / 1e18;
    const capacity = borrowCapacity();
    $("borHf").textContent = hf === null ? "–" : hf.toFixed(2);
    $("borApy").textContent = market ? market.borrowApy.toFixed(2) + "%" : "–";
    $("borLeft").textContent = fmt(capacity > amount ? capacity - amount : 0n, account.debtDecimals) + " vUSD";
    if (amount > capacity) {
      text = `<span class="bad">Above your capacity of ${fmt(capacity, account.debtDecimals)} vUSD.</span>`;
    } else {
      text = `After this borrow the health factor would be <b>${hf === null ? "–" : hf.toFixed(3)}</b>.`;
    }
  } catch (err) {
    // Nothing typed yet, or not a valid number. Show nothing.
  }
  $("borProjected").innerHTML = text;
  updateSupplyPreview();
}

// ---------------------------------------------------------------------
// Button actions
// ---------------------------------------------------------------------

// Test tokens only. The vUSD is for paying interest, which grows every block.
async function mintTestTokens() {
  const who = selectedAccount();
  await runAction("msgMint", "Minting", async () => {
    for (const c of collaterals) {
      await sendTransaction(who, c.contract, "mint", [who, ethers.parseUnits("1000", c.decimals)]);
    }
    await sendTransaction(who, debtToken, "mint", [who, ethers.parseUnits("100", config.debtDecimals)]);
    return " 1000 of each collateral and 100 vUSD minted.";
  });
}

async function deposit() {
  const who = selectedAccount();
  const c = collateralFrom("depAsset");
  await runAction("msgDeposit", "Deposit", async () => {
    const amount = parseAmount($("depAmount").value, c.decimals);
    if (amount > (account.walletCollateral[c.address.toLowerCase()] ?? 0n)) {
      throw new Error("you do not have that much " + c.label + " in your wallet");
    }
    await approveIfNeeded(who, c.contract, amount);
    await sendTransaction(who, pool, "depositCollateral", [c.address, amount]);
    return " " + c.label + " is now held by the pool.";
  });
}

async function borrow() {
  const who = selectedAccount();
  await runAction("msgBorrow", "Borrow", async () => {
    const amount = parseAmount($("borAmount").value, config.debtDecimals);
    const capacity = borrowCapacity();
    if (capacity === 0n) {
      throw new Error("you cannot borrow more right now. Add collateral or repay some debt first");
    }
    if (amount > capacity) {
      throw new Error("that is above your capacity of " + fmt(capacity, config.debtDecimals) + " vUSD");
    }
    await sendTransaction(who, pool, "borrow", [amount]);
    return " The vUSD is in your wallet.";
  });
}

async function repay() {
  const who = selectedAccount();
  await runAction("msgRepay", "Repay", async () => {
    const amount = parseAmount($("repAmount").value, config.debtDecimals);
    await approveIfNeeded(who, debtToken, amount);
    await sendTransaction(who, pool, "repay", [amount]);
    return "";
  });
}

async function withdraw() {
  const who = selectedAccount();
  const c = collateralFrom("wdAsset");
  await runAction("msgWithdraw", "Withdraw", async () => {
    const amount = parseAmount($("wdAmount").value, c.decimals);
    const most = maxWithdrawable(c);
    if (most === 0n) {
      throw new Error("nothing can be taken out right now without the health factor dropping below 1.05. Repay some debt first");
    }
    if (amount > most) {
      throw new Error("the most you can take out is " + fmt(most, c.decimals, 4) + " " + c.ticker);
    }
    await sendTransaction(who, pool, "withdrawCollateral", [c.address, amount]);
    return " " + c.label + " is back in your wallet.";
  });
}

// Repays the whole debt, then gives back every collateral the pool holds for this account.
async function repayAllAndReturn() {
  const who = selectedAccount();
  const steps = $("returnSteps");
  steps.hidden = false;
  steps.innerHTML = "";
  const addStep = (text) => {
    const line = document.createElement("div");
    line.textContent = text;
    steps.appendChild(line);
    return line;
  };

  await runAction("msgReturn", "Repay and return", async () => {
    const p = calculatePosition();

    if (p.debt > 0n) {
      if (account.walletVusd < p.debt) {
        const missing = fmt(p.debt - account.walletVusd, account.debtDecimals, 6);
        throw new Error("you need " + missing + " more vUSD. Interest grows every block, so get the difference from the Mint button or another account");
      }
      const approveLine = addStep("Approving vUSD for the full debt");
      await approveIfNeeded(who, debtToken, MAX_UINT);
      approveLine.textContent += " ✓";

      const repayLine = addStep("Repaying the full debt, with interest");
      await sendTransaction(who, pool, "repay", [MAX_UINT]);
      repayLine.textContent += " ✓";
    }

    // Read the position again, now that the debt is gone.
    await loadAccount();
    const after = calculatePosition();
    if (after.debt > 0n) throw new Error("some debt is still open, try again");

    const returned = after.assets.filter((a) => a.amount > 0n);
    for (const c of returned) {
      const line = addStep(`Returning ${fmt(c.amount, c.decimals, 4)} ${c.ticker} to your wallet`);
      await sendTransaction(who, pool, "withdrawCollateral", [c.address, c.amount]);
      line.textContent += " ✓";
    }
    return returned.length ? " " + returned.length + " collateral asset(s) returned." : " Nothing left to return.";
  });
}

// ---------------------------------------------------------------------
// The "Max" buttons fill in the biggest sensible amount
// ---------------------------------------------------------------------
function depositMax() {
  const c = collateralFrom("depAsset");
  $("depAmount").value = ethers.formatUnits(account.walletCollateral[c.address.toLowerCase()] ?? 0n, c.decimals);
}

function borrowMax() {
  $("borAmount").value = ethers.formatUnits(borrowCapacity(), config.debtDecimals);
  updateBorrowPreview();
}

function repayMax() {
  $("repAmount").value = ethers.formatUnits(repayableNow(), config.debtDecimals);
}

function withdrawMax() {
  const c = collateralFrom("wdAsset");
  $("wdAmount").value = ethers.formatUnits(maxWithdrawable(c), c.decimals);
}

// ---------------------------------------------------------------------
// Liquidation intents: a signed permission for a liquidator to act if your health factor drops
// ---------------------------------------------------------------------

// The intents the gateway holds for the selected account.
async function myIntents() {
  const all = await (await fetch("/api/intents")).json();
  const who = selectedAccount().toLowerCase();
  return all.filter((i) => i.borrower.toLowerCase() === who);
}

// The lowest nonce that is neither used on chain nor already submitted.
async function nextFreeNonce(who, existing) {
  const taken = new Set(existing.map((i) => BigInt(i.nonce)));
  let nonce = 1n;
  while (taken.has(nonce) || (await pool.nonceUsed(who, nonce))) nonce += 1n;
  return nonce;
}

async function submitIntent() {
  const who = selectedAccount();
  await runAction("msgIntent", "Intent", async () => {
    const c = collateralFrom("intAsset");
    const maxRepay = parseAmount($("intMaxRepay").value, config.debtDecimals);
    const minOutText = ($("intMinOut").value || "").trim();
    const minOut = minOutText === "" ? 0n : parseAmount(minOutText, c.decimals, true);
    const days = Number($("intDays").value);
    if (!(days > 0)) throw new Error("validity must be a positive number of days");

    // The deadline is in chain time, so use the time of the latest block.
    const latest = await provider.getBlock("latest");
    const deadline = BigInt(latest.timestamp) + BigInt(Math.round(days * 86400));
    const nonce = await nextFreeNonce(who, await myIntents());

    const intent = {
      borrower: who,
      collateralAsset: c.address,
      maxRepay,
      minCollateralOut: minOut,
      nonce,
      deadline,
    };
    // The domain names the contract. The gateway checks the signature against the same values.
    const domain = { name: "Vigil LendingPool", version: "1", chainId: config.chainId, verifyingContract: config.pool };

    // Signing costs no gas. It only proves that this account agrees to these limits.
    const signer = await provider.getSigner(who);
    const signature = await signer.signTypedData(domain, INTENT_TYPES, intent);

    const response = await fetch("/api/intents", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        borrower: who,
        collateralAsset: c.address,
        maxRepay: maxRepay.toString(),
        minCollateralOut: minOut.toString(),
        nonce: nonce.toString(),
        deadline: deadline.toString(),
        signature,
      }),
    });
    const result = await response.json();
    if (!response.ok || !result.accepted) throw new Error(result.error || "the gateway rejected the intent");

    await loadIntents();
    return " Intent " + nonce + " is live until " + new Date(Number(deadline) * 1000).toLocaleDateString("en-US") + ".";
  });
}

// Shows the selected account's intents, and whether each one can still be used.
async function loadIntents() {
  if (!pool) return;
  const who = selectedAccount();
  try {
    const rows = await myIntents();
    if (rows.length === 0) {
      $("intents").innerHTML = `<tr><td colspan="6" class="muted">no intents for this account</td></tr>`;
      return;
    }

    const latest = await provider.getBlock("latest");
    const now = BigInt(latest.timestamp);
    const lines = [];
    for (const r of rows) {
      const c = collaterals.find((x) => x.address.toLowerCase() === r.collateralAsset.toLowerCase());
      const used = await pool.nonceUsed(who, BigInt(r.nonce));
      const expired = BigInt(r.deadline) < now;
      const status = used ? `<span class="muted">used</span>` : expired ? `<span class="muted">expired</span>` : `<span class="ok">active</span>`;
      lines.push(`<tr>
        <td>${r.nonce}</td>
        <td>${c ? c.ticker : short(r.collateralAsset)}</td>
        <td class="num">${fmt(BigInt(r.maxRepay), config.debtDecimals)} vUSD</td>
        <td class="num">${c ? fmt(BigInt(r.minCollateralOut ?? "0"), c.decimals, 4) : "–"}</td>
        <td>${new Date(Number(r.deadline) * 1000).toLocaleDateString("en-US")}</td>
        <td>${status}</td>
      </tr>`);
    }
    $("intents").innerHTML = lines.join("");
  } catch (err) {
    $("intents").innerHTML = `<tr><td colspan="6" class="muted">could not load intents</td></tr>`;
  }
}

// Start the page.
start();
