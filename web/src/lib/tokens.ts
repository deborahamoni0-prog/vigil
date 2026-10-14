// Display names for the test collaterals. The keys are the names in the deployment file.
// The on-chain symbols are unchanged (FEE, SIX, EIGHT).
export interface TokenInfo {
  label: string;
  ticker: string;
  note: string;
  color: string;
}

export const TOKEN_INFO: Record<string, TokenInfo> = {
  fee: {
    label: "Wrapped Ether",
    ticker: "WETH",
    note: "Test token, 18 decimals. Takes a 1% fee on every transfer, so the pool must count what it actually receives.",
    color: "#f08c2b",
  },
  six: {
    label: "USD Stablecoin",
    ticker: "USDX",
    note: "Test token, 6 decimals, modelled on a stablecoin. Lowest volatility, highest liquidation threshold.",
    color: "#3fa86b",
  },
  eight: {
    label: "Wrapped Bitcoin",
    ticker: "WBTC",
    note: "Test token, 8 decimals, modelled on a wrapped bitcoin. Most volatile, highest liquidation bonus.",
    color: "#5a7bd9",
  },
};

export function tokenInfo(name: string): TokenInfo {
  return TOKEN_INFO[name] ?? { label: name, ticker: name.toUpperCase(), note: "", color: "#5b5f73" };
}
