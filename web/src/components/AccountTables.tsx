// Your supplies, your borrows, and account details for the selected account.
import { assetCell } from "./AssetCell.tsx";
import { dollars, fmt } from "../lib/format.ts";
import type { Position } from "../lib/position.ts";
import type { AccountView, Collateral } from "../lib/types.ts";
import type { MarketRates } from "../lib/rates.ts";
import type { Tab } from "./Markets.tsx";

export function Supplies({ position, walletCollateral, collaterals, onOpen }: {
  position: Position | null;
  walletCollateral: Record<string, bigint>;
  collaterals: Collateral[];
  onOpen: (asset: Collateral, tab: Tab) => void;
}) {
  return (
    <div className="card">
      <div className="card-head">
        <h3>Your supplies</h3>
        <span className="muted" style={{ fontSize: 12.5 }}>Collateral in your wallet and in the pool</span>
      </div>
      <table>
        <thead><tr><th>Asset</th><th className="num">In wallet</th><th className="num">In pool</th><th className="num">Value</th></tr></thead>
        <tbody>
          {!position && <tr><td colSpan={4} className="muted">loading…</td></tr>}
          {position && collaterals.map((c) => {
            const pos = position.assets.find((a) => a.address.toLowerCase() === c.address.toLowerCase());
            const wallet = walletCollateral[c.address.toLowerCase()] ?? 0n;
            return (
              <tr key={c.name} onClick={() => onOpen(c, "deposit")}>
                <td>{assetCell(c.name)}</td>
                <td className="num">{fmt(wallet, c.decimals, 4)}</td>
                <td className="num">{fmt(pos?.amount ?? 0n, c.decimals, 4)}</td>
                <td className="num">{pos?.price === null ? "frozen" : dollars(pos?.value ?? 0n)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export function Borrows({ account, position, market, debtDecimals, onRepay }: {
  account: AccountView | null;
  position: Position | null;
  market: MarketRates | null;
  debtDecimals: number;
  onRepay: () => void;
}) {
  return (
    <div className="card">
      <div className="card-head">
        <h3>Your borrows</h3>
        <span className="muted" style={{ fontSize: 12.5 }}>vUSD you owe</span>
      </div>
      <table>
        <thead><tr><th>Asset</th><th className="num">Debt</th><th className="num">Borrow APY</th><th></th></tr></thead>
        <tbody>
          {!account || !position ? (
            <tr><td colSpan={4} className="muted">loading…</td></tr>
          ) : position.debt === 0n ? (
            <tr><td colSpan={4} className="muted">No borrows yet. Borrow against your collateral from the markets above.</td></tr>
          ) : (
            <tr>
              <td>
                <div className="asset">
                  <span className="coin vusd">$</span>
                  <div><div className="name">vUSD</div><div className="ticker">Vigil USD</div></div>
                </div>
              </td>
              <td className="num">{fmt(position.debt, debtDecimals)}</td>
              <td className="num">{market ? market.borrowApy.toFixed(2) + "%" : "–"}</td>
              <td><div className="row-actions"><button className="btn-sm alt" onClick={onRepay}>Repay</button></div></td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}

export function AccountDetails({ account, position, walletVusd, debtDecimals, riskTracked }: {
  account: AccountView | null;
  position: Position | null;
  walletVusd: bigint;
  debtDecimals: number;
  riskTracked: boolean;
}) {
  const debt = position?.debt ?? 0n;
  return (
    <div className="card">
      <div className="card-head">
        <h3>Account details</h3>
        <span className="muted" style={{ fontSize: 12.5 }}>Selected account</span>
      </div>
      <div className="card-pad kv">
        <span className="k">Wallet vUSD</span><span className="v">{fmt(walletVusd, debtDecimals)}</span>
        <span className="k">Lender deposits</span><span className="v">{account ? fmt(BigInt(account.supplied), debtDecimals) : "–"}</span>
        <span className="k">Risk engine</span><span className="v">{riskTracked ? "watching this position" : "not tracking"}</span>
      </div>
      <div className="card-pad" style={{ paddingTop: 0 }}>
        <div className="muted" style={{ fontSize: 12.5 }}>
          {debt === 0n
            ? "You have no debt. Repaying returns your collateral."
            : `You owe ${fmt(debt, debtDecimals, 6)} vUSD. Your wallet has ${fmt(walletVusd, debtDecimals, 6)} vUSD.`}
        </div>
      </div>
    </div>
  );
}
