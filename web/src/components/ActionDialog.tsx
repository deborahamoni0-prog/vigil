// The action dialog: supply, borrow, repay and withdraw. Opened from a market row or the buttons.
import { useEffect, useState } from "react";
import { fmt } from "../lib/format.ts";
import { borrowCapacity, healthAfterBorrow, afterSupply, maxWithdrawable, type Position } from "../lib/position.ts";
import { tokenInfo } from "../lib/tokens.ts";
import type { Collateral, Config } from "../lib/types.ts";
import type { MarketRates } from "../lib/rates.ts";
import type { Tab } from "./Markets.tsx";
import type { ActionMessage } from "../pages/messages.ts";

export interface DialogStep { id: number; text: string; done: boolean }

export interface DialogProps {
  open: boolean;
  tab: Tab;
  assetIndex: number;
  config: Config | null;
  position: Position | null;
  market: MarketRates | null;
  walletVusd: bigint;
  walletCollateral: Record<string, bigint>;
  messages: Record<string, ActionMessage>;
  steps: DialogStep[];
  busy: string | null;
  onTab: (tab: Tab) => void;
  onAsset: (index: number) => void;
  onClose: () => void;
  onSupply: (asset: Collateral, amount: string) => void;
  onBorrow: (amount: string) => void;
  onRepay: (amount: string) => void;
  onRepayAll: () => void;
  onWithdraw: (asset: Collateral, amount: string) => void;
}

const TABS: { key: Tab; label: string }[] = [
  { key: "deposit", label: "Supply" },
  { key: "borrow", label: "Borrow" },
  { key: "repay", label: "Repay" },
  { key: "withdraw", label: "Withdraw" },
];

export function ActionDialog(p: DialogProps) {
  const [dialog, setDialog] = useState<HTMLDialogElement | null>(null);
  const [amount, setAmount] = useState("");

  // Keep the native <dialog> in step with `open`.
  useEffect(() => {
    if (!dialog) return;
    if (p.open && !dialog.open) dialog.showModal();
    if (!p.open && dialog.open) dialog.close();
  }, [p.open, dialog]);

  // A new tab or asset starts with an empty amount.
  useEffect(() => setAmount(""), [p.tab, p.assetIndex]);

  const collateral = p.config?.collaterals[p.assetIndex];
  const decimals = p.config?.debtDecimals ?? 6;
  const msg = (id: string) => p.messages[id];
  const MessageLine = ({ id }: { id: string }) => {
    const m = msg(id);
    return m ? <div className={`msg ${m.kind === "good" ? "good" : m.kind === "error" ? "error" : "busy"}`}>{m.text}</div> : null;
  };

  // The overview numbers for the current tab.
  let overview: { label: string; value: string }[] = [];
  let maxText = "";
  if (p.position && p.config) {
    const pos = p.position;
    if (p.tab === "deposit" && collateral) {
      const wallet = p.walletCollateral[collateral.address.toLowerCase()] ?? 0n;
      maxText = fmt(wallet, collateral.decimals, 6);
      let after = { health: null as number | null, capacity: 0n };
      try {
        const units = amount ? parseFree(amount, collateral.decimals) : 0n;
        const price = pos.assets.find((a) => a.address.toLowerCase() === collateral.address.toLowerCase())?.price ?? null;
        if (price !== null && units > 0n) after = afterSupply(pos, { decimals: collateral.decimals, liqThresholdBps: collateral.liqThresholdBps, price }, units);
      } catch { /* nothing valid typed */ }
      overview = [
        { label: "Health factor after", value: after.health === null ? "–" : after.health.toFixed(2) },
        { label: "Liquidation threshold", value: (collateral.liqThresholdBps / 100).toFixed(0) + "%" },
        { label: "Borrow limit after", value: fmt(after.capacity, decimals) + " vUSD" },
      ];
    } else if (p.tab === "borrow") {
      maxText = fmt(borrowCapacity(pos), decimals, 6);
      let hf: number | null = null;
      let left = borrowCapacity(pos);
      try {
        const units = amount ? parseFree(amount, decimals) : 0n;
        hf = healthAfterBorrow(pos, units);
        left = borrowCapacity(pos) > units ? borrowCapacity(pos) - units : 0n;
      } catch { /* nothing valid typed */ }
      overview = [
        { label: "Health factor after", value: hf === null ? "–" : hf.toFixed(2) },
        { label: "Borrow APY", value: p.market ? p.market.borrowApy.toFixed(2) + "%" : "–" },
        { label: "Borrow limit left", value: fmt(left, decimals) + " vUSD" },
      ];
    } else if (p.tab === "repay") {
      const debt = pos.debt;
      maxText = fmt(debt < p.walletVusd ? debt : p.walletVusd, decimals, 6);
      overview = [
        { label: "You owe", value: fmt(debt, decimals, 4) + " vUSD" },
        { label: "Wallet vUSD", value: fmt(p.walletVusd, decimals, 4) + " vUSD" },
      ];
    } else if (p.tab === "withdraw" && collateral) {
      maxText = fmt(maxWithdrawable(pos, collateral.address), collateral.decimals, 6);
      overview = [
        { label: "Keeps health factor at", value: "1.05 or more" },
        { label: "Liquidation threshold", value: (collateral.liqThresholdBps / 100).toFixed(0) + "%" },
      ];
    }
  }

  const title = TABS.find((t) => t.key === p.tab)?.label ?? "";
  const assetInfo = collateral ? tokenInfo(collateral.name) : null;

  return (
    <dialog ref={setDialog} onClose={p.onClose} onClick={(e) => { if (e.target === e.currentTarget) p.onClose(); }}>
      <div className="dlg-head">
        <h3>{title}</h3>
        <button className="close" aria-label="Close" onClick={p.onClose}>×</button>
      </div>
      <div className="dlg-body">
        <div className="tabs" role="tablist">
          {TABS.map((t) => (
            <button key={t.key} role="tab" aria-selected={p.tab === t.key} onClick={() => p.onTab(t.key)}>{t.label}</button>
          ))}
        </div>

        {(p.tab === "deposit" || p.tab === "withdraw") && (
          <>
            <div className="field-label"><span>Asset</span></div>
            <select value={p.assetIndex} onChange={(e) => p.onAsset(Number(e.target.value))}>
              {p.config?.collaterals.map((c, i) => <option key={c.name} value={i}>{tokenInfo(c.name).label} ({tokenInfo(c.name).ticker})</option>)}
            </select>
          </>
        )}

        <div className="field-label">
          <span>Amount</span>
          <span className="bal">{p.tab === "deposit" || p.tab === "withdraw" ? assetInfo?.ticker : "vUSD"} · max {maxText || "–"}</span>
        </div>
        <div className="amount-row">
          <input value={amount} onChange={(e) => setAmount(e.target.value)} placeholder="0.00" inputMode="decimal" />
          <button className="pill-btn" onClick={() => setAmount(maxText)}>Max</button>
        </div>

        <div className="overview">
          {overview.map((o) => (
            <div className="row" key={o.label}><span>{o.label}</span><b>{o.value}</b></div>
          ))}
        </div>

        {p.tab === "deposit" && collateral && (
          <>
            <button className="btn" disabled={p.busy !== null} onClick={() => p.onSupply(collateral, amount)}>
              {p.busy === "deposit" ? "Supplying…" : "Supply"}
            </button>
            <MessageLine id="deposit" />
          </>
        )}
        {p.tab === "borrow" && (
          <>
            <div className="preview">{p.position && p.position.debt > 0n ? "Keep your health factor at 1.05 or above." : "Borrowing against your collateral."}</div>
            <button className="btn" disabled={p.busy !== null} onClick={() => p.onBorrow(amount)}>
              {p.busy === "borrow" ? "Borrowing…" : "Borrow"}
            </button>
            <MessageLine id="borrow" />
          </>
        )}
        {p.tab === "repay" && (
          <>
            <button className="btn" disabled={p.busy !== null} onClick={() => p.onRepay(amount)}>
              {p.busy === "repay" ? "Repaying…" : "Repay"}
            </button>
            <MessageLine id="repay" />
            <div className="divider">
              <div className="muted" style={{ fontSize: 13, marginBottom: 10 }}>
                Repay everything, then take all your collateral back.
              </div>
              <button className="btn ghost" style={{ marginTop: 0 }} disabled={p.busy !== null} onClick={p.onRepayAll}>
                Repay all and withdraw
              </button>
              {p.steps.length > 0 && (
                <div className="steps">
                  {p.steps.map((s) => <div key={s.id}>{s.text}{s.done ? " ✓" : ""}</div>)}
                </div>
              )}
              <MessageLine id="repayAll" />
            </div>
          </>
        )}
        {p.tab === "withdraw" && collateral && (
          <>
            <button className="btn" disabled={p.busy !== null} onClick={() => p.onWithdraw(collateral, amount)}>
              {p.busy === "withdraw" ? "Withdrawing…" : "Withdraw"}
            </button>
            <MessageLine id="withdraw" />
          </>
        )}
      </div>
    </dialog>
  );
}

// Parses typed text without throwing, for live previews.
function parseFree(text: string, decimals: number): bigint {
  const t = text.trim();
  if (!/^\d*\.?\d*$/.test(t) || t === "." || t === "") return 0n;
  const frac = t.split(".")[1] ?? "";
  if (frac.length > decimals) return 0n;
  const [whole, fractional = ""] = t.split(".");
  return BigInt((whole || "0") + fractional.padEnd(decimals, "0"));
}
