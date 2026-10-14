// Price feed for the local test chain.
//
// The protocol reads each collateral's price from two sources on the chain: a price feed (the
// aggregator) and an AMM spot price. The oracle only accepts a price when both agree within its
// deviation band, so this script always writes both, AMM first.
//
// Two modes:
//   live (default)  Real prices from CoinGecko (public API, no key). Each collateral follows one coin:
//                   fee -> Ethereum, six -> USD Coin, eight -> Bitcoin.
//   walk            A small random walk around the starting price. Used when the API is unreachable.
//
// Usage: node scripts/price-feed.mjs
// Env:   PRICE_MODE (live | walk), POLL_MS (live refresh, default 30000), RPC_URL, DEPLOYMENT_FILE,
//        FEED_KEY (default: anvil account 0, a published test key, local use only).

import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const { ethers } = { ethers: require(resolve(here, "../frontend/vendor/ethers.umd.min.js")) };

const RPC_URL = process.env.RPC_URL ?? "http://127.0.0.1:8545";
const DEPLOYMENT_FILE = process.env.DEPLOYMENT_FILE ?? resolve(here, "../contracts/deployments/31337.json");
const FEED_KEY = process.env.FEED_KEY ?? "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const MODE = process.env.PRICE_MODE ?? "live";
const POLL_MS = Number(process.env.POLL_MS ?? 30000);
const WALK_TICK_MS = 2000;
const WALK_STEP_BPS = 20;
const WALK_BAND_BPS = 3000;

// Which real coin each test collateral follows, by its name in the deployment file.
const COIN_FOR = { fee: "ethereum", six: "usd-coin", eight: "bitcoin" };
const PRICE_URL = "https://api.coingecko.com/api/v3/simple/price?vs_currencies=usd&ids=" + Object.values(COIN_FOR).join(",");

const AGGREGATOR_ABI = ["function setAnswer(int256 answer)", "function answer() view returns (int256)"];
const AMM_ABI = ["function setSpot(uint256 priceWad)", "function spot() view returns (uint256)"];

const deployment = JSON.parse(readFileSync(DEPLOYMENT_FILE, "utf8"));
const provider = new ethers.JsonRpcProvider(RPC_URL, undefined, { staticNetwork: true });
// NonceManager reads the pending nonce from the node and tracks it, so a stale nonce cannot be reused.
const wallet = new ethers.NonceManager(new ethers.Wallet(FEED_KEY, provider));

const markets = await Promise.all(
  deployment.collaterals.map(async (c) => {
    const amm = new ethers.Contract(c.amm, AMM_ABI, wallet);
    const aggregator = new ethers.Contract(c.feed, AGGREGATOR_ABI, wallet);
    const start = await amm.spot();
    return { name: c.name, amm, aggregator, start, price: start, coin: COIN_FOR[c.name] };
  }),
);

// Write one price: the AMM first, then the feed, so the feed never runs ahead of the TWAP.
// priceWad is 18 decimals; the feed uses 8.
async function writePrice(m, priceWad) {
  await (await m.amm.setSpot(priceWad)).wait();
  await (await m.aggregator.setAnswer(priceWad / 10n ** 10n)).wait();
  m.price = priceWad;
}

// ---------------- live mode ----------------

async function fetchRealPrices() {
  const response = await fetch(PRICE_URL, { signal: AbortSignal.timeout(10000) });
  if (!response.ok) throw new Error("price API returned " + response.status);
  const data = await response.json();
  const out = {};
  for (const m of markets) {
    const usd = data[m.coin]?.usd;
    if (typeof usd !== "number") throw new Error("no price for " + m.coin);
    // Six decimals of USD precision, then scaled to 18 decimals.
    out[m.name] = BigInt(Math.round(usd * 1e6)) * 10n ** 12n;
  }
  return out;
}

async function liveTick() {
  const prices = await fetchRealPrices();
  for (const m of markets) {
    if (prices[m.name] !== m.price) await writePrice(m, prices[m.name]);
  }
  const summary = markets.map((m) => `${m.coin} $${ethers.formatUnits(m.price, 18)}`).join(", ");
  console.log(new Date().toISOString(), "live:", summary);
}

// ---------------- walk mode ----------------

function walkNext(m) {
  // Random step, plus a pull of 5% of the gap back toward the start, then clamped to the band.
  const step = BigInt(Math.round((Math.random() * 2 - 1) * WALK_STEP_BPS));
  const pull = (m.start - m.price) / 20n;
  const moved = m.price + (m.price * step) / 10000n + pull;
  const lo = (m.start * BigInt(10000 - WALK_BAND_BPS)) / 10000n;
  const hi = (m.start * BigInt(10000 + WALK_BAND_BPS)) / 10000n;
  return moved < lo ? lo : moved > hi ? hi : moved;
}

async function walkTick() {
  for (const m of markets) await writePrice(m, walkNext(m));
}

// ---------------- main loop ----------------

let running = true;
process.on("SIGINT", () => { running = false; });
process.on("SIGTERM", () => { running = false; });

let mode = MODE;
console.log(`price feed starting: mode=${mode}, RPC ${RPC_URL}`);
if (mode === "live") {
  // Stop rather than fall back to simulated prices: the risk engine and the frontend would
  // act on random-walk numbers with nothing to show it. Walk mode runs only when asked for.
  // A few retries first, because the price API can fail for a moment at start-up.
  for (let attempt = 1; ; attempt++) {
    try {
      await liveTick();
      break;
    } catch (err) {
      if (attempt >= 3) {
        console.error("live prices unavailable, exiting (set PRICE_MODE=walk for simulated prices):", err.shortMessage ?? err.message);
        process.exit(1);
      }
      console.error(`live price attempt ${attempt} failed, retrying in 5s:`, err.shortMessage ?? err.message);
      await new Promise((r) => setTimeout(r, 5000));
    }
  }
}

while (running) {
  try {
    if (mode === "live") {
      await liveTick();
      await new Promise((r) => setTimeout(r, POLL_MS));
    } else {
      await walkTick();
      await new Promise((r) => setTimeout(r, WALK_TICK_MS));
    }
  } catch (err) {
    console.error("tick failed:", err.shortMessage ?? err.message);
    await new Promise((r) => setTimeout(r, 5000));
  }
}
console.log("price feed stopped");
