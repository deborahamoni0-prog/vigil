// Shapes of the data the gateway sends. They mirror services/gateway/src/main.rs.

export interface Rates {
  baseRate: string;
  slope1: string;
  slope2: string;
  optimalUtil: string;
}

export interface Collateral {
  name: string;
  address: string;
  decimals: number;
  bitIndex: number;
  liqThresholdBps: number;
  liqBonusBps: number;
  feed: string;
  amm: string;
}

export interface Config {
  chainId: number;
  pool: string;
  oracle: string;
  debt: string;
  debtDecimals: number;
  reserveFactor: string;
  rates: Rates;
  collaterals: Collateral[];
}

export interface PriceRow {
  name: string;
  price_wad: string; // "frozen" when the oracle cannot give a price
}

// The chain snapshot pushed every block over /api/stream (kind "chain").
export interface ChainSnapshot {
  head: number;
  frozen: boolean;
  debtIndex: string;
  supplyIndex: string;
  totalDebtScaled: string;
  totalSupplyScaled: string;
  reserves: string;
  deficit: string;
  debtPriceWad: string;
  prices: PriceRow[];
}

export interface AccountCollateral {
  name: string;
  address: string;
  decimals: number;
  liqThresholdBps: number;
  amount: string;
  priceWad: string | null;
}

// One account, from /api/account/{address}.
export interface AccountView {
  borrower: string;
  debt: string;
  supplied: string;
  healthFactor: string | null;
  auctionStart: string;
  debtPriceWad: string | null;
  debtDecimals: number;
  collaterals: AccountCollateral[];
  riskEngineTracked: boolean;
}

export interface IntentRow {
  borrower: string;
  nonce: string;
  collateralAsset: string;
  maxRepay: string;
  minCollateralOut: string;
  deadline: number;
}

// One message from the activity stream (kind "nats").
export interface ActivityMessage {
  stage: string; // "events", "blocks", "signals", ...
  data: {
    kind?: string;
    block_number?: number;
    log_index?: number;
    borrower?: string;
    event?: { type?: string; user?: string; borrower?: string; amount?: string };
  };
}
