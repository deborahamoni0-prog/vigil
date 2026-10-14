import { test } from "node:test";
import assert from "node:assert/strict";
import { calculatePosition, borrowCapacity, maxWithdrawable, healthFactor, healthAfterBorrow } from "./position.ts";
import type { AccountView } from "./types.ts";

const E18 = 10n ** 18n;
const TOKEN = "0x1111111111111111111111111111111111111111";

// One token with 8 decimals, priced at $100, 75% liquidation threshold. vUSD has 6 decimals.
function account(amountUnits: bigint, debtUnits: bigint): AccountView {
  return {
    borrower: "0x0",
    debt: debtUnits.toString(),
    supplied: "0",
    healthFactor: null,
    auctionStart: "0",
    debtPriceWad: E18.toString(),
    debtDecimals: 6,
    riskEngineTracked: false,
    collaterals: [{
      name: "eight",
      address: TOKEN,
      decimals: 8,
      liqThresholdBps: 7500,
      amount: amountUnits.toString(),
      priceWad: (100n * E18).toString(),
    }],
  };
}

test("health factor is weighted collateral over debt", () => {
  // 1 token at $100, 75% threshold: weighted $75. Debt $50 gives a health factor of 1.5.
  const p = calculatePosition(account(10n ** 8n, 50n * 10n ** 6n), E18);
  assert.equal(p.weighted, 75n * E18);
  assert.equal(p.debtValue, 50n * E18);
  assert.equal(healthFactor(p), 1.5);
});

test("no debt means no health factor", () => {
  const p = calculatePosition(account(10n ** 8n, 0n), E18);
  assert.equal(healthFactor(p), null);
});

test("borrow capacity keeps the health factor at 1.05", () => {
  // Weighted $75. At 1.05 the most debt allowed is 75 / 1.05 = 71.43. Debt is $50, so $21.43 is left.
  const p = calculatePosition(account(10n ** 8n, 50n * 10n ** 6n), E18);
  assert.equal(borrowCapacity(p), 21428571n);
});

test("borrowing the full capacity leaves the health factor at 1.05 or above", () => {
  const p = calculatePosition(account(10n ** 8n, 50n * 10n ** 6n), E18);
  const cap = borrowCapacity(p);
  const hf = healthAfterBorrow(p, cap);
  assert.ok(hf !== null && hf >= 1.05 - 1e-6, `health factor after the full capacity was ${hf}`);
});

test("withdraw maximum keeps the health factor at 1.05", () => {
  // Required weighted is 50 × 1.05 = 52.5. Spare weighted is 75 − 52.5 = 22.5.
  // Removing 22.5 of weighted value at 75% threshold means removing $30 of collateral, which is 0.3 tokens.
  const p = calculatePosition(account(10n ** 8n, 50n * 10n ** 6n), E18);
  assert.equal(maxWithdrawable(p, TOKEN), 30000000n);
});

test("withdraw maximum is the whole amount when there is no debt", () => {
  const p = calculatePosition(account(10n ** 8n, 0n), E18);
  assert.equal(maxWithdrawable(p, TOKEN), 10n ** 8n);
});

test("a frozen price gives no capacity and no withdraw limit", () => {
  const acc = account(10n ** 8n, 50n * 10n ** 6n);
  acc.collaterals[0].priceWad = null;
  const p = calculatePosition(acc, E18);
  assert.equal(p.complete, false);
  assert.equal(borrowCapacity(p), 0n);
  assert.equal(maxWithdrawable(p, TOKEN), 0n);
});
