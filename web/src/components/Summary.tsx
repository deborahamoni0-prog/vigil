// The four cards at the top: net worth, debt, the health factor bar, and borrow power used.
import { dollars, fmt } from "../lib/format.ts";
import { borrowCapacity, limitUsedPercent, type Position } from "../lib/position.ts";
import type { AccountView } from "../lib/types.ts";

export function Summary({ account, position, debtDecimals }: { account: AccountView | null; position: Position | null; debtDecimals: number }) {
  if (!account || !position) {
    return (
      <section className="summary">
        <div className="stat"><div className="label">Net worth</div><div className="value">–</div></div>
        <div className="stat"><div className="label">Debt</div><div className="value">–</div></div>
        <div className="stat"><div className="label">Health factor</div><div className="value">–</div></div>
        <div className="stat"><div className="label">Borrow power used</div><div className="value">–</div></div>
      </section>
    );
  }

  const netWorth = position.collateralTotal - position.debtValue;
  const used = limitUsedPercent(position);
  const capacity = borrowCapacity(position);

  // The gateway reads the health factor from the contract, so it is the official number.
  let hfText = "No debt";
  let hfNumber = Infinity;
  let colour = "var(--ok)";
  if (position.debt > 0n) {
    if (account.healthFactor === null) {
      hfText = "Unavailable";
      colour = "var(--warn)";
    } else {
      hfNumber = Number(account.healthFactor) / 1e18;
      hfText = hfNumber.toFixed(2);
      colour = hfNumber >= 1.2 ? "var(--ok)" : hfNumber >= 1 ? "var(--warn)" : "var(--bad)";
    }
  }
  // Bar runs from 0 to 2. Infinity (no debt) fills it.
  const dash = hfNumber === Infinity ? 100 : Math.min(hfNumber / 2, 1) * 100;

  return (
    <section className="summary">
      <div className="stat">
        <div className="label">Net worth <span className="info" title="Collateral value minus debt, in USD">i</span></div>
        <div className="value">{dollars(netWorth)}</div>
        <div className="sub">Collateral {dollars(position.collateralTotal)} − debt {dollars(position.debtValue)}</div>
      </div>
      <div className="stat">
        <div className="label">Debt <span className="info" title="vUSD you owe, including interest">i</span></div>
        <div className="value">{fmt(position.debt, debtDecimals)} vUSD</div>
        <div className="sub">{Number(account.auctionStart) > 0 ? `Auction started at block ${account.auctionStart}` : ""}</div>
      </div>
      <div className="stat hf-card">
        <div className="label">Health factor <span className="info" title="Above 1 is safe. At or below 1 the position can be liquidated.">i</span></div>
        <div className="value" style={{ color: colour }}>{hfText}</div>
        <div className="hf-bar" aria-hidden="true">
          <div className="hf-fill" style={{ width: `${dash}%`, background: colour }} />
          <div className="hf-line" title="1.00 liquidation line" />
        </div>
        <div className="hf-scale"><span>0</span><span>1.00 liquidation</span><span>2+</span></div>
        <div className="sub">{hfNumber >= 1 || hfNumber === Infinity ? "Liquidation starts below 1.00" : "Below 1: can be liquidated"}</div>
      </div>
      <div className="stat">
        <div className="label">Borrow power used <span className="info" title="Share of your borrowing power in use. 100% is the limit.">i</span></div>
        <div className="value">{used.toFixed(1)}%</div>
        <div className="meter"><div style={{ width: `${Math.min(used, 100)}%`, background: used < 70 ? "var(--primary)" : used < 90 ? "var(--warn)" : "var(--bad)" }} /></div>
        <div className="meter-row"><span>Borrow limit left</span><span>{fmt(capacity, debtDecimals)} vUSD</span></div>
      </div>
    </section>
  );
}
