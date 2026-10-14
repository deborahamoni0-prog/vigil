// Supply markets (collateral assets) and the borrow market (vUSD).
import { Sparkline } from "./Sparkline.tsx";
import { assetCell } from "./AssetCell.tsx";
import { dollars, fmt } from "../lib/format.ts";
import { tokenInfo } from "../lib/tokens.ts";
import type { MarketRates } from "../lib/rates.ts";
import type { ChainSnapshot, Collateral, Config } from "../lib/types.ts";
import type { StreamState } from "../state/stream.tsx";

export type Tab = "deposit" | "borrow" | "repay" | "withdraw";

export function SupplyMarkets({ config, chain, history, onOpen, query }: {
  config: Config | null;
  chain: ChainSnapshot | null;
  history: StreamState["priceHistory"];
  onOpen: (asset: Collateral, tab: Tab) => void;
  query: string;
}) {
  const q = query.trim().toLowerCase();
  const shown = config?.collaterals.filter((c) => {
    if (!q) return true;
    const info = tokenInfo(c.name);
    return info.label.toLowerCase().includes(q) || info.ticker.toLowerCase().includes(q);
  });
  return (
    <section>
      <div className="card table-card">
        <table>
          <thead>
            <tr>
              <th>Asset</th>
              <th className="num">Price</th>
              <th>Last prices</th>
              <th className="num">Liq. threshold</th>
              <th className="num">Liq. bonus</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {!config && <tr><td colSpan={6} className="muted">loading…</td></tr>}
            {shown?.length === 0 && <tr><td colSpan={6} className="empty">No assets match “{query}”.</td></tr>}
            {shown?.map((c) => {
              const price = chain?.prices.find((p) => p.name === c.name);
              const hist = history[c.name] ?? [];
              const dir = hist.length > 1 && hist[hist.length - 1] < hist[0] ? "down" : "up";
              const change = hist.length > 1 && hist[0] !== 0n ? (Number(hist[hist.length - 1] - hist[0]) / Number(hist[0]) * 100).toFixed(2) + "%" : "";
              return (
                <tr key={c.name}>
                  <td>{assetCell(c.name)}</td>
                  <td className="num price-cell">{!price || price.price_wad === "frozen" ? "frozen" : dollars(BigInt(price.price_wad))}</td>
                  <td style={{ width: "40%" }}>
                    <Sparkline values={hist} />
                    <span className={`chg ${dir}`}>{change}</span>
                  </td>
                  <td className="num">{(c.liqThresholdBps / 100).toFixed(0)}%</td>
                  <td className="num">{(c.liqBonusBps / 100).toFixed(0)}%</td>
                  <td>
                    <div className="row-actions">
                      <button className="btn-sm" onClick={(e) => { e.stopPropagation(); onOpen(c, "deposit"); }}>Supply</button>
                      <button className="btn-sm alt" onClick={(e) => { e.stopPropagation(); onOpen(c, "withdraw"); }}>Withdraw</button>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </section>
  );
}

export function BorrowMarkets({ config, chain, market, onBorrow, onRepay }: {
  config: Config | null;
  chain: ChainSnapshot | null;
  market: MarketRates | null;
  onBorrow: () => void;
  onRepay: () => void;
}) {
  const debtPrice = chain && chain.debtPriceWad !== "frozen" ? BigInt(chain.debtPriceWad) : null;
  return (
    <section style={{ marginTop: 16 }}>
      <div className="card table-card">
        <table>
          <thead>
            <tr>
              <th>Asset</th>
              <th className="num">Price</th>
              <th className="num">Total borrowed</th>
              <th className="num" title="What a borrower pays per year, including compounding">Borrow APY</th>
              <th className="num" title="What a supplier of vUSD earns per year, after the reserve cut">Supply APY</th>
              <th className="num" title="Share of the pool that is borrowed">Utilization</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {!config || !market ? (
              <tr><td colSpan={7} className="muted">loading…</td></tr>
            ) : (
              <tr>
                <td>
                  <div className="asset">
                    <span className="coin vusd">$</span>
                    <div><div className="name">vUSD</div><div className="ticker">Vigil USD</div></div>
                  </div>
                </td>
                <td className="num">{debtPrice === null ? "frozen" : dollars(debtPrice)}</td>
                <td className="num">{fmt(market.totalDebt, config.debtDecimals)}</td>
                <td className="num">{market.borrowApy.toFixed(2)}%</td>
                <td className="num">{market.supplyApy.toFixed(2)}%</td>
                <td className="num">{market.utilization.toFixed(1)}%</td>
                <td>
                  <div className="row-actions">
                    <button className="btn-sm" onClick={onBorrow}>Borrow</button>
                    <button className="btn-sm alt" onClick={onRepay}>Repay</button>
                  </div>
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </section>
  );
}
