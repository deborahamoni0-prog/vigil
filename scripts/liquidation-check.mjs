// End-to-end liquidation check for the local stack (dev only, uses anvil's published keys).
// 1. Borrower mints, deposits "eight", borrows close to the limit.
// 2. Borrower signs a liquidation intent and submits it to the gateway, as the page does.
// 3. The price of "eight" falls 8%, which pushes the health factor below 1.
// 4. We watch the gateway until the debt falls, which means the Liquidator acted on the intent.
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const { ethers } = { ethers: require(resolve(here, "../frontend/vendor/ethers.umd.min.js")) };

const RPC = "http://127.0.0.1:8545";
const GATEWAY = "http://127.0.0.1:3000";
const BORROWER_KEY = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"; // anvil account 1
const DEPLOYER_KEY = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"; // anvil account 0

const dep = JSON.parse(readFileSync(resolve(here, "../contracts/deployments/31337.json"), "utf8"));
const provider = new ethers.JsonRpcProvider(RPC, undefined, { staticNetwork: true });
const borrower = new ethers.Wallet(BORROWER_KEY, provider);
const deployer = new ethers.Wallet(DEPLOYER_KEY, provider);
const eight = dep.collaterals.find((c) => c.name === "eight");

const POOL = ["function depositCollateral(address,uint256)", "function borrow(uint256)", "function nonceUsed(address,uint256) view returns (bool)"];
const TOKEN = ["function mint(address,uint256)", "function approve(address,uint256) returns (bool)"];
const AGG = ["function setAnswer(int256)"];
const AMM = ["function setSpot(uint256)"];
const pool = new ethers.Contract(dep.pool, POOL, borrower);
const debtToken = new ethers.Contract(dep.debt, TOKEN, borrower);
const colToken = new ethers.Contract(eight.address, TOKEN, borrower);

const log = (...a) => console.log(new Date().toISOString().slice(11, 19), ...a);
const account = (who) => fetch(`${GATEWAY}/api/account/${who}`).then((r) => r.json());
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function main() {
  const me = borrower.address;
  log("borrower", me);

  // 1. Position
  await (await colToken.mint(me, ethers.parseUnits("1000", eight.decimals))).wait();
  await (await debtToken.mint(me, ethers.parseUnits("100", dep.debtDecimals))).wait();
  await (await colToken.approve(dep.pool, ethers.MaxUint256)).wait();
  const deposit = ethers.parseUnits("0.1", eight.decimals);
  await (await pool.depositCollateral(eight.address, deposit)).wait();
  // 0.1 eight at $38,500 with 75% threshold = $2,887.5 weighted. Borrow 2,700: health factor about 1.07.
  const debt = ethers.parseUnits("2700", dep.debtDecimals);
  await (await pool.borrow(debt)).wait();
  let a = await account(me);
  log("after borrow: debt", a.debt, "hf", ethers.formatUnits(a.healthFactor, 18));

  // 2. Intent, signed the same way the page does it
  const latest = await provider.getBlock("latest");
  const deadline = BigInt(latest.timestamp) + 30n * 86400n;
  const intent = {
    borrower: me,
    collateralAsset: eight.address,
    maxRepay: ethers.parseUnits("500", dep.debtDecimals),
    minCollateralOut: 0n,
    nonce: 1n,
    deadline,
  };
  const domain = { name: "Vigil LendingPool", version: "1", chainId: dep.chainId, verifyingContract: dep.pool };
  const types = {
    LiquidationIntent: [
      { name: "borrower", type: "address" }, { name: "collateralAsset", type: "address" },
      { name: "maxRepay", type: "uint256" }, { name: "minCollateralOut", type: "uint256" },
      { name: "nonce", type: "uint256" }, { name: "deadline", type: "uint256" },
    ],
  };
  const signature = await borrower.signTypedData(domain, types, intent);
  const res = await fetch(`${GATEWAY}/api/intents`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      borrower: me, collateralAsset: eight.address, maxRepay: intent.maxRepay.toString(),
      minCollateralOut: "0", nonce: "1", deadline: deadline.toString(), signature,
    }),
  });
  log("intent submitted:", res.status, JSON.stringify(await res.json()));

  // 3. Price drop of 8% on "eight": AMM spot first, then the feed, so the oracle's deviation check passes.
  const spotNow = await new ethers.Contract(eight.amm, ["function spot() view returns (uint256)"], provider).spot();
  const newSpot = (spotNow * 92n) / 100n;
  const feedNow = await new ethers.Contract(eight.feed, ["function answer() view returns (int256)"], provider).answer();
  await (await new ethers.Contract(eight.amm, AMM, deployer).setSpot(newSpot)).wait();
  await (await new ethers.Contract(eight.feed, AGG, deployer).setAnswer((feedNow * 92n) / 100n)).wait();
  log("eight price moved to", ethers.formatUnits(newSpot, 18));

  // 4. Watch for the Liquidator acting on the intent
  const before = BigInt(a.debt);
  for (let i = 0; i < 60; i++) {
    await sleep(3000);
    a = await account(me);
    const hf = a.healthFactor ? ethers.formatUnits(a.healthFactor, 18) : "n/a";
    if (i % 5 === 0) log("hf", hf, "debt", ethers.formatUnits(BigInt(a.debt), dep.debtDecimals), "auction", a.auctionStart);
    if (BigInt(a.debt) < before) {
      log("LIQUIDATED: debt fell by", ethers.formatUnits(before - BigInt(a.debt), dep.debtDecimals), "vUSD");
      log("nonce 1 used on chain:", await pool.nonceUsed(me, 1n));
      return;
    }
  }
  log("no liquidation within 3 minutes");
}

main().catch((e) => { console.error("FAILED:", e.shortMessage ?? e.message); process.exit(1); });
