import { test } from "node:test";
import assert from "node:assert/strict";
import { annualBorrowRate, computeMarket, RAY } from "./rates.ts";

const ray = (x: number) => (BigInt(Math.round(x * 1e6)) * RAY) / 1000000n;
const rates = { baseRate: ray(0.02).toString(), slope1: ray(0.04).toString(), slope2: ray(0.6).toString(), optimalUtil: ray(0.8).toString() };

test("borrow rate rises linearly up to the optimal utilisation", () => {
  // At 50% utilisation: 2% + 4% × 0.5 / 0.8 = 4.5%.
  const r = annualBorrowRate(ray(0.5), rates);
  assert.ok(Math.abs(Number(r) / 1e27 - 0.045) < 1e-9);
});

test("borrow rate rises steeply above the optimal utilisation", () => {
  // At 90%: 2% + 4% + 60% × 0.1 / 0.2 = 36%.
  const r = annualBorrowRate(ray(0.9), rates);
  assert.ok(Math.abs(Number(r) / 1e27 - 0.36) < 1e-6);
});

test("market figures at 50% utilisation", () => {
  // Debt 50 and supply 100, both at index 1, with a 10% reserve cut.
  const m = computeMarket({
    debtIndex: RAY.toString(),
    supplyIndex: RAY.toString(),
    totalDebtScaled: (50n * 10n ** 6n * 10n ** 0n).toString() + "000000000000000000000000000",
    totalSupplyScaled: (100n * 10n ** 6n).toString() + "000000000000000000000000000",
    rates,
    reserveFactor: ray(0.1).toString(),
  });
  assert.ok(Math.abs(m.utilization - 50) < 1e-6);
  // Borrow APY = expm1(4.5%) = 4.6%. Supply APY = expm1(4.5% × 0.5 × 0.9) = 2.0%.
  assert.ok(Math.abs(m.borrowApy - 4.6) < 0.05, `borrow APY ${m.borrowApy}`);
  assert.ok(Math.abs(m.supplyApy - 2.04) < 0.05, `supply APY ${m.supplyApy}`);
});
