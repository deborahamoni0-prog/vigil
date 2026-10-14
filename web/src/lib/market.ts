// Pool-level figures for the markets page, from the chain snapshot and the deployment's rate settings.
import { computeMarket, type MarketRates } from "./rates.ts";
import type { ChainSnapshot, Config } from "./types.ts";

export function marketFrom(chain: ChainSnapshot | null, config: Config | null): MarketRates | null {
  if (!chain || !config) return null;
  return computeMarket({
    debtIndex: chain.debtIndex,
    supplyIndex: chain.supplyIndex,
    totalDebtScaled: chain.totalDebtScaled,
    totalSupplyScaled: chain.totalSupplyScaled,
    rates: config.rates,
    reserveFactor: config.reserveFactor,
  });
}
