// The position maths, mirroring LendingPool._healthFactorPx. Pure functions, no browser or chain code,
// so they can be tested directly (see position.test.ts).
//
//   value of a collateral    = amount × price
//   weighted value           = sum of value × liquidation threshold
//   debt value               = debt × vUSD price
//   health factor           = weighted value ÷ debt value      (1 or more is safe)
//
// The page keeps a safety target of 1.05 when it offers borrow capacity and withdraw limits.

import type { AccountView } from "./types.ts";

export const ONE = 10n ** 18n;          // one whole unit of a 18-decimal price or value
export const TARGET_HEALTH_PERCENT = 105n;

export interface AssetPosition {
  name: string;
  address: string;
  decimals: number;
  liqThresholdBps: number;
  amount: bigint;
  price: bigint | null; // 18-decimal USD price, or null when the oracle is frozen
  value: bigint;        // 18-decimal USD value
}

export interface Position {
  debt: bigint;           // vUSD units
  debtDecimals: number;
  debtPrice: bigint;      // 18-decimal
  debtValue: bigint;      // 18-decimal USD
  weighted: bigint;       // 18-decimal USD, weighted by liquidation threshold
  complete: boolean;      // false when a price is missing, so limits cannot be worked out
  assets: AssetPosition[];
  collateralTotal: bigint;
}

export function calculatePosition(account: AccountView, fallbackDebtPrice: bigint): Position {
  const debt = BigInt(account.debt);
  const debtDecimals = account.debtDecimals;
  const debtPrice = account.debtPriceWad ? BigInt(account.debtPriceWad) : fallbackDebtPrice;
  let complete = account.debtPriceWad !== null;

  let weighted = 0n;
  let collateralTotal = 0n;
  const assets: AssetPosition[] = account.collaterals.map((c) => {
    const amount = BigInt(c.amount);
    const price = c.priceWad ? BigInt(c.priceWad) : null;
    if (price === null) complete = false;
    // Bring the amount up to 18 decimals so it can be multiplied by the 18-decimal price.
    const value = price === null ? 0n : (amount * 10n ** BigInt(18 - c.decimals) * price) / ONE;
    weighted += (value * BigInt(c.liqThresholdBps)) / 10000n;
    collateralTotal += value;
    return { name: c.name, address: c.address, decimals: c.decimals, liqThresholdBps: c.liqThresholdBps, amount, price, value };
  });

  const debtValue = (debt * 10n ** BigInt(18 - debtDecimals) * debtPrice) / ONE;
  return { debt, debtDecimals, debtPrice, debtValue, weighted, complete, assets, collateralTotal };
}

// Health factor as a number. null when there is no debt (nothing to liquidate) or the price is missing.
export function healthFactor(p: Position): number | null {
  if (p.debt === 0n || !p.complete) return null;
  return Number((p.weighted * ONE) / p.debtValue) / 1e18;
}

// Share of the borrowing power in use, in percent. 100 means the limit.
export function limitUsedPercent(p: Position): number {
  if (p.debt === 0n) return 0;
  if (p.weighted === 0n) return 100;
  return Math.min(Number((p.debtValue * 10000n) / p.weighted) / 100, 999);
}

// How much more vUSD (in vUSD units) can be borrowed and keep the health factor at the target.
export function borrowCapacity(p: Position): bigint {
  if (!p.complete || p.debtPrice === 0n) return 0n;
  const maxDebtValue = (p.weighted * 100n) / TARGET_HEALTH_PERCENT;
  const spare = maxDebtValue > p.debtValue ? maxDebtValue - p.debtValue : 0n;
  return (spare * 10n ** BigInt(p.debtDecimals)) / p.debtPrice;
}

// The most of one collateral that can be taken out and keep the health factor at the target.
export function maxWithdrawable(p: Position, address: string): bigint {
  const asset = p.assets.find((a) => a.address.toLowerCase() === address.toLowerCase());
  if (!asset || asset.amount === 0n) return 0n;
  if (p.debt === 0n) return asset.amount;
  if (asset.price === null || !p.complete) return 0n;

  // Taking collateral out lowers the weighted value, which must stay above debt × 1.05.
  const requiredWeighted = (p.debtValue * TARGET_HEALTH_PERCENT) / 100n;
  const spare = p.weighted > requiredWeighted ? p.weighted - requiredWeighted : 0n;
  const removableValue = (spare * 10000n) / BigInt(asset.liqThresholdBps);
  const units = (removableValue * 10n ** BigInt(asset.decimals)) / asset.price;
  return units < asset.amount ? units : asset.amount;
}

// Health factor after borrowing `amount` more vUSD (vUSD units). null when there would be no debt.
export function healthAfterBorrow(p: Position, amount: bigint): number | null {
  const newDebtValue = ((p.debt + amount) * 10n ** BigInt(18 - p.debtDecimals) * p.debtPrice) / ONE;
  if (newDebtValue === 0n) return null;
  return Number((p.weighted * ONE) / newDebtValue) / 1e18;
}

// Health factor and remaining borrow capacity after supplying `amount` of a collateral.
export function afterSupply(
  p: Position,
  asset: { decimals: number; liqThresholdBps: number; price: bigint },
  amount: bigint,
): { health: number | null; capacity: bigint } {
  const value = (amount * 10n ** BigInt(18 - asset.decimals) * asset.price) / ONE;
  const weighted = p.weighted + (value * BigInt(asset.liqThresholdBps)) / 10000n;
  const health = p.debt === 0n ? null : Number((weighted * ONE) / p.debtValue) / 1e18;
  const spare = weighted > p.debtValue ? weighted - p.debtValue : 0n;
  const capacity = (spare * 10n ** BigInt(p.debtDecimals)) / p.debtPrice;
  return { health, capacity };
}
