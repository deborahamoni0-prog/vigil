// The interest model, the same as the contract (contracts/src/libs/InterestMath.sol).
// Rates are fixed-point with 27 decimals (RAY). Utilisation is debt divided by supply.

export const RAY = 10n ** 27n;

export interface RateInputs {
  debtIndex: string;
  supplyIndex: string;
  totalDebtScaled: string;
  totalSupplyScaled: string;
  rates: { baseRate: string; slope1: string; slope2: string; optimalUtil: string };
  reserveFactor: string;
}

export interface MarketRates {
  totalDebt: bigint;     // vUSD units (6 decimals)
  totalSupply: bigint;   // vUSD units
  utilization: number;   // percent
  borrowApy: number;     // percent
  supplyApy: number;     // percent
}

// Annual borrow rate for a utilisation level (both RAY). Rises gently, then steeply past the optimum.
export function annualBorrowRate(u: bigint, rates: RateInputs["rates"]): bigint {
  const base = BigInt(rates.baseRate);
  const slope1 = BigInt(rates.slope1);
  const slope2 = BigInt(rates.slope2);
  const optimal = BigInt(rates.optimalUtil);
  if (u <= optimal) return base + (slope1 * u) / optimal;
  return base + slope1 + (slope2 * (u - optimal)) / (RAY - optimal);
}

// Converts an annual rate (RAY) to APY, compounded per second as the contract does.
export function toApy(rate: bigint): number {
  return Math.expm1(Number(rate) / 1e27) * 100;
}

export function computeMarket(input: RateInputs): MarketRates {
  const totalDebt = (BigInt(input.totalDebtScaled) * BigInt(input.debtIndex)) / RAY;
  const totalSupply = (BigInt(input.totalSupplyScaled) * BigInt(input.supplyIndex)) / RAY;
  const raw = totalSupply === 0n ? 0n : (totalDebt * RAY) / totalSupply;
  const u = raw > RAY ? RAY : raw;

  const borrowRate = annualBorrowRate(u, input.rates);
  // Suppliers earn the borrow rate on the borrowed share, after the reserve cut.
  const supplyRate = (borrowRate * u * (RAY - BigInt(input.reserveFactor))) / (RAY * RAY);

  return {
    totalDebt,
    totalSupply,
    utilization: Number(u) / 1e25,
    borrowApy: toApy(borrowRate),
    supplyApy: toApy(supplyRate),
  };
}
