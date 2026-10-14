// Liquidation intents: a signed permission for a liquidator to act if the health factor drops.
import { useState } from "react";
import { provider, INTENT_TYPES, parseAmount, reasonOf } from "../lib/chain.ts";
import { submitIntent } from "../lib/api.ts";
import { fmt, short } from "../lib/format.ts";
import { tokenInfo } from "../lib/tokens.ts";
import type { Collateral, Config, IntentRow } from "../lib/types.ts";
import type { Contracts } from "../state/hooks.ts";
import type { ActionMessage } from "../pages/messages.ts";

export function IntentsCard(props: {
  who: string;
  contracts: Contracts | null;
  config: Config | null;
  rows: IntentRow[];
  used: Record<string, boolean>;
  reload: () => Promise<void>;
  onMessage: (m: ActionMessage) => void;
  busy: boolean;
  setBusy: (b: boolean) => void;
}) {
  const { who, contracts, config, rows, used, reload, onMessage, busy, setBusy } = props;
  const [assetIndex, setAssetIndex] = useState(0);
  const [maxRepay, setMaxRepay] = useState("");
  const [minOut, setMinOut] = useState("");
  const [days, setDays] = useState("30");

  async function sign() {
    if (!contracts || !config) return;
    setBusy(true);
    onMessage({ id: "intent", text: "Intent…", kind: "busy" });
    try {
      const c: Collateral = config.collaterals[assetIndex];
      const repayUnits = parseAmount(maxRepay, config.debtDecimals);
      const minUnits = minOut.trim() === "" ? 0n : parseAmount(minOut, c.decimals, true);
      const dayCount = Number(days);
      if (!(dayCount > 0)) throw new Error("validity must be a positive number of days");

      // The deadline is in chain time, so use the time of the latest block.
      const latest = await provider.getBlock("latest");
      if (!latest) throw new Error("no latest block");
      const deadline = BigInt(latest.timestamp) + BigInt(Math.round(dayCount * 86400));

      // The lowest nonce that is neither used on chain nor already submitted.
      const taken = new Set(rows.map((r) => BigInt(r.nonce)));
      let nonce = 1n;
      while (taken.has(nonce) || (await contracts.pool.nonceUsed(who, nonce))) nonce += 1n;

      const domain = { name: "Vigil LendingPool", version: "1", chainId: config.chainId, verifyingContract: config.pool };
      const value = { borrower: who, collateralAsset: c.address, maxRepay: repayUnits, minCollateralOut: minUnits, nonce, deadline };
      // Signing costs no gas. It only proves that this account agrees to these limits.
      const signer = await provider.getSigner(who);
      const signature = await signer.signTypedData(domain, INTENT_TYPES, value);

      await submitIntent({
        borrower: who,
        collateralAsset: c.address,
        maxRepay: repayUnits.toString(),
        minCollateralOut: minUnits.toString(),
        nonce: nonce.toString(),
        deadline: deadline.toString(),
        signature,
      });
      await reload();
      onMessage({
        id: "intent",
        text: `Intent ${nonce} is live until ${new Date(Number(deadline) * 1000).toLocaleDateString("en-US")}.`,
        kind: "good",
      });
    } catch (err) {
      onMessage({ id: "intent", text: "Intent failed: " + reasonOf(err), kind: "error" });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card">
      <div className="card-head">
        <h3>Liquidation intents</h3>
        <span className="muted" style={{ fontSize: 12.5 }}>Let a liquidator act if your health factor drops</span>
      </div>
      <p className="hint" style={{ padding: "14px 22px 0", margin: 0 }}>
        Signing is free. The pool still enforces the limits below, and the intent stops working after its expiry.
      </p>
      <div className="intent-form">
        <div>
          <div className="field-label"><span>Collateral they may take</span></div>
          <select value={assetIndex} onChange={(e) => setAssetIndex(Number(e.target.value))}>
            {config?.collaterals.map((c, i) => <option key={c.name} value={i}>{tokenInfo(c.name).label}</option>)}
          </select>
        </div>
        <div>
          <div className="field-label"><span>Most vUSD they may repay</span></div>
          <input value={maxRepay} onChange={(e) => setMaxRepay(e.target.value)} placeholder="e.g. 500" inputMode="decimal" />
        </div>
        <div>
          <div className="field-label"><span>Least collateral you accept</span></div>
          <input value={minOut} onChange={(e) => setMinOut(e.target.value)} placeholder="0" inputMode="decimal" />
        </div>
        <div>
          <div className="field-label"><span>Valid for (days)</span></div>
          <input value={days} onChange={(e) => setDays(e.target.value)} inputMode="decimal" />
        </div>
      </div>
      <button className="btn-sm" disabled={busy || !contracts} onClick={sign} style={{ margin: "6px 22px 0", background: "var(--primary)", color: "#fff" }}>
        Sign intent
      </button>
      <table className="intent-table" style={{ marginTop: 12 }}>
        <thead><tr><th>Nonce</th><th>Collateral</th><th className="num">Max repay</th><th className="num">Min out</th><th>Expires</th><th>Status</th></tr></thead>
        <tbody>
          {rows.length === 0 && <tr><td colSpan={6} className="muted">no intents for this account</td></tr>}
          {rows.map((r) => {
            const c = config?.collaterals.find((x) => x.address.toLowerCase() === r.collateralAsset.toLowerCase());
            const expired = r.deadline < Math.floor(Date.now() / 1000);
            const status = used[r.nonce] ? "used" : expired ? "expired" : "active";
            return (
              <tr key={r.nonce}>
                <td>{r.nonce}</td>
                <td>{c ? tokenInfo(c.name).ticker : short(r.collateralAsset)}</td>
                <td className="num">{config ? fmt(BigInt(r.maxRepay), config.debtDecimals) : r.maxRepay} vUSD</td>
                <td className="num">{c ? fmt(BigInt(r.minCollateralOut), c.decimals, 4) : "–"}</td>
                <td>{new Date(r.deadline * 1000).toLocaleDateString("en-US")}</td>
                <td className={status === "active" ? "ok" : "muted"}>{status}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

