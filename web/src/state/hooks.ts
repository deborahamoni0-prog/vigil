// Data hooks: the deployment config, the selected account and its intents.
import { useCallback, useEffect, useMemo, useState } from "react";
import { ethers } from "ethers";
import { fetchAccount, fetchConfig, fetchIntents } from "../lib/api.ts";
import { provider, TOKEN_ABI, POOL_ABI } from "../lib/chain.ts";
import type { AccountView, Config, IntentRow } from "../lib/types.ts";
import { fetchProtocol } from "../lib/api.ts";
import type { ChainSnapshot } from "../lib/types.ts";

export interface Contracts {
  config: Config;
  pool: ethers.Contract;
  debtToken: ethers.Contract;
  tokens: Record<string, ethers.Contract>; // by collateral address (lower case)
}

// Loads the deployment config once and builds the contract objects.
export function useConfig(): Config | null {
  const [config, setConfig] = useState<Config | null>(null);
  useEffect(() => {
    fetchConfig().then(setConfig).catch(() => setConfig(null));
  }, []);
  return config;
}

export function useContracts(config: Config | null): Contracts | null {
  return useMemo(() => {
    if (!config) return null;
    const tokens: Record<string, ethers.Contract> = {};
    for (const c of config.collaterals) {
      tokens[c.address.toLowerCase()] = new ethers.Contract(c.address, TOKEN_ABI, provider);
    }
    return {
      config,
      pool: new ethers.Contract(config.pool, POOL_ABI, provider),
      debtToken: new ethers.Contract(config.debt, TOKEN_ABI, provider),
      tokens,
    };
  }, [config]);
}

export interface AccountState {
  account: AccountView | null;
  walletVusd: bigint;
  walletCollateral: Record<string, bigint>; // by collateral address (lower case)
  error: boolean;
  reload: () => Promise<void>;
}

// The selected account: its position from the gateway, and its wallet balances from the chain.
// Polled every 1.5 seconds, so the page follows each block.
export function useAccount(contracts: Contracts | null, who: string): AccountState {
  const [account, setAccount] = useState<AccountView | null>(null);
  const [walletVusd, setWalletVusd] = useState<bigint>(0n);
  const [walletCollateral, setWalletCollateral] = useState<Record<string, bigint>>({});
  const [error, setError] = useState(false);

  const reload = useCallback(async () => {
    if (!contracts || !who) return;
    try {
      const view = await fetchAccount(who);
      const vusd: bigint = await contracts.debtToken.balanceOf(who);
      const wallet: Record<string, bigint> = {};
      for (const c of contracts.config.collaterals) {
        const key = c.address.toLowerCase();
        wallet[key] = await contracts.tokens[key].balanceOf(who);
      }
      setAccount(view);
      setWalletVusd(vusd);
      setWalletCollateral(wallet);
      setError(false);
    } catch {
      setError(true);
    }
  }, [contracts, who]);

  useEffect(() => {
    reload();
    const timer = setInterval(reload, 1500);
    return () => clearInterval(timer);
  }, [reload]);

  return { account, walletVusd, walletCollateral, error, reload };
}

export interface IntentsState {
  rows: IntentRow[];          // this account's intents
  used: Record<string, boolean>; // nonce -> already used on chain
  reload: () => Promise<void>;
}

// This account's liquidation intents, and which nonces the pool has already used. Polled every 5 seconds.
export function useIntents(contracts: Contracts | null, who: string): IntentsState {
  const [rows, setRows] = useState<IntentRow[]>([]);
  const [used, setUsed] = useState<Record<string, boolean>>({});

  const reload = useCallback(async () => {
    if (!contracts || !who) return;
    const all = await fetchIntents();
    const mine = all.filter((i) => i.borrower.toLowerCase() === who.toLowerCase());
    const flags: Record<string, boolean> = {};
    for (const r of mine) {
      flags[r.nonce] = await contracts.pool.nonceUsed(who, BigInt(r.nonce));
    }
    setRows(mine);
    setUsed(flags);
  }, [contracts, who]);

  useEffect(() => {
    reload().catch(() => setRows([]));
    const timer = setInterval(() => reload().catch(() => {}), 5000);
    return () => clearInterval(timer);
  }, [reload]);

  return { rows, used, reload };
}

// Protocol-level data for the landing page's hero, read directly from the gateway.
export function useProtocolOnce(): ChainSnapshot | null {
  const [snapshot, setSnapshot] = useState<ChainSnapshot | null>(null);
  useEffect(() => {
    const load = () => fetchProtocol().then(setSnapshot).catch(() => {});
    load();
    const timer = setInterval(load, 3000);
    return () => clearInterval(timer);
  }, []);
  return snapshot;
}
