// The markets app: summary, markets, your supplies and borrows, intents, and activity.
import { useEffect, useMemo, useState } from "react";
import { provider } from "../lib/chain.ts";
import { calculatePosition, ONE, type Position } from "../lib/position.ts";
import { marketFrom } from "../lib/market.ts";
import { fmt } from "../lib/format.ts";
import * as actions from "../lib/actions.ts";
import type { ActionContext } from "../lib/actions.ts";
import { useConfig, useContracts, useAccount, useIntents } from "../state/hooks.ts";
import { useStream } from "../state/stream.tsx";
import { useToast } from "../state/toasts.tsx";
import { Header } from "../components/Header.tsx";
import { Summary } from "../components/Summary.tsx";
import { SupplyMarkets, BorrowMarkets, type Tab } from "../components/Markets.tsx";
import { Supplies, Borrows, AccountDetails } from "../components/AccountTables.tsx";
import { IntentsCard } from "../components/IntentsCard.tsx";
import { Activity } from "../components/Activity.tsx";
import { ActionDialog, type DialogStep } from "../components/ActionDialog.tsx";
import { reasonOf } from "../lib/chain.ts";
import type { Collateral } from "../lib/types.ts";
import type { ActionMessage } from "./messages.ts";

export function MarketsPage() {
  const config = useConfig();
  const contracts = useContracts(config);
  const stream = useStream();
  const toast = useToast();

  const [accounts, setAccounts] = useState<string[]>([]);
  const [selected, setSelected] = useState("");
  const [onlyMe, setOnlyMe] = useState(false);
  const [dialog, setDialog] = useState<{ open: boolean; tab: Tab; assetIndex: number }>({ open: false, tab: "deposit", assetIndex: 0 });
  const [messages, setMessages] = useState<Record<string, ActionMessage>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [steps, setSteps] = useState<DialogStep[]>([]);
  const [now, setNow] = useState(Date.now());
  const [tab, setTab] = useState<"markets" | "positions" | "intents" | "activity">("markets");
  const [query, setQuery] = useState("");

  // The accounts Hardhat/anvil provides, and the first one is selected.
  useEffect(() => {
    provider.send("eth_accounts", []).then((list: string[]) => {
      setAccounts(list);
      setSelected((s) => s || list[0] || "");
    });
  }, []);

  // Keeps the "updated N seconds ago" text moving between blocks.
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);

  const { account, walletVusd, walletCollateral, reload } = useAccount(contracts, selected);
  const intents = useIntents(contracts, selected);
  const market = marketFrom(stream.chain, config);

  // The position follows the gateway's account data. The fallback debt price is used only if it is missing.
  const position: Position | null = useMemo(() => {
    if (!account) return null;
    const fallback = stream.chain && stream.chain.debtPriceWad !== "frozen" ? BigInt(stream.chain.debtPriceWad) : ONE;
    return calculatePosition(account, fallback);
  }, [account, stream.chain]);

  // Runs one button action with the standard steps: lock the buttons, show progress, report, refresh.
  async function run(id: string, label: string, fn: (ctx: ActionContext) => Promise<string>) {
    if (busy || !contracts || !account || !position) return;
    setBusy(id);
    setMessages((m) => ({ ...m, [id]: { id, text: label + "…", kind: "busy" } }));
    toast.push(label, "Waiting for the transaction to be mined…", "busy");
    try {
      const ctx: ActionContext = {
        who: selected,
        contracts,
        account,
        walletVusd,
        walletCollateral,
        position,
        fallbackDebtPrice: stream.chain && stream.chain.debtPriceWad !== "frozen" ? BigInt(stream.chain.debtPriceWad) : ONE,
        reload,
      };
      const extra = await fn(ctx);
      setMessages((m) => ({ ...m, [id]: { id, text: label + " done." + extra, kind: "good" } }));
      toast.push(label + " complete", extra.trim() || "Done.", "good");
    } catch (err) {
      const text = reasonOf(err);
      setMessages((m) => ({ ...m, [id]: { id, text: label + " failed: " + text, kind: "error" } }));
      toast.push(label + " failed", text, "error");
    } finally {
      setBusy(null);
      await reload();
    }
  }

  function openDialog(assetIndex: number, tab: Tab) {
    setDialog({ open: true, tab, assetIndex });
  }

  let stepId = 0;
  const addStep = (text: string) => {
    const id = ++stepId;
    setSteps((s) => [...s, { id, text, done: false }]);
    return () => setSteps((s) => s.map((x) => (x.id === id ? { ...x, done: true } : x)));
  };

  const supplies: Collateral[] = config?.collaterals ?? [];
  const updated = stream.lastBlockAt ? Math.round((now - stream.lastBlockAt) / 1000) : null;
  const chain = stream.chain;

  return (
    <>
      <Header
        active="markets"
        connected={stream.connected}
        chain={chain}
        accounts={accounts}
        selected={selected}
        onSelect={(a) => setSelected(a)}
      />
      <main className="wrap">
        <div className="page-head">
          <div>
            <h1>Markets</h1>
            <p>Supply collateral, borrow vUSD, and repay to get your collateral back.</p>
          </div>
        </div>
        <Summary account={account} position={position} debtDecimals={config?.debtDecimals ?? 6} />

        <section className="totals">
          <span>Total borrowed <b>{chain && config ? fmt(BigInt(chain.totalDebtScaled) * BigInt(chain.debtIndex) / 10n ** 27n, config.debtDecimals) : "–"}</b></span>
          <span>Reserves <b>{chain && config ? fmt(BigInt(chain.reserves), config.debtDecimals) : "–"}</b></span>
          <span>Bad debt <b>{chain && config ? fmt(BigInt(chain.deficit), config.debtDecimals) : "–"}</b></span>
          <span>Debt index <b>{chain ? (Number(chain.debtIndex) / 1e27).toFixed(6) : "–"}</b></span>
          <span className="spacer" />
          <span className="updated">Updated {updated === null ? "–" : updated < 2 ? "just now" : `${updated}s ago`}</span>
        </section>

        <div className="tabbar" role="tablist">
          <button role="tab" aria-selected={tab === "markets"} onClick={() => setTab("markets")}>Markets</button>
          <button role="tab" aria-selected={tab === "positions"} onClick={() => setTab("positions")}>Your positions</button>
          <button role="tab" aria-selected={tab === "intents"} onClick={() => setTab("intents")}>Intents <span className="count">{intents.rows.length}</span></button>
          <button role="tab" aria-selected={tab === "activity"} onClick={() => setTab("activity")}>Activity</button>
        </div>

        {tab === "markets" && (
          <>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12, marginBottom: 12, flexWrap: "wrap" }}>
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search assets"
                aria-label="Search assets"
                style={{ maxWidth: 320 }}
              />
              <span className="muted" style={{ fontSize: 13 }}>Prices come from the protocol oracle, updated every block</span>
            </div>
            <SupplyMarkets
              config={config}
              chain={chain}
              history={stream.priceHistory}
              query={query}
              onOpen={(asset, t) => openDialog(config?.collaterals.indexOf(asset) ?? 0, t)}
            />
            <BorrowMarkets
              config={config}
              chain={chain}
              market={market}
              onBorrow={() => openDialog(0, "borrow")}
              onRepay={() => openDialog(0, "repay")}
            />
          </>
        )}

        {tab === "positions" && (
          <>
            <div className="split">
              <Supplies
                position={position}
                walletCollateral={walletCollateral}
                collaterals={supplies}
                onOpen={(asset, t) => openDialog(supplies.indexOf(asset), t)}
              />
              <Borrows
                account={account}
                position={position}
                market={market}
                debtDecimals={config?.debtDecimals ?? 6}
                onRepay={() => openDialog(0, "repay")}
              />
            </div>
            <AccountDetails
              account={account}
              position={position}
              walletVusd={walletVusd}
              debtDecimals={config?.debtDecimals ?? 6}
              riskTracked={account?.riskEngineTracked ?? false}
            />
          </>
        )}

        {tab === "intents" && (
          <IntentsCard
            who={selected}
            contracts={contracts}
            config={config}
            rows={intents.rows}
            used={intents.used}
            reload={intents.reload}
            onMessage={(m) => setMessages((x) => ({ ...x, [m.id]: m }))}
            busy={busy !== null}
            setBusy={(b) => setBusy(b ? "intent" : null)}
          />
        )}

        {tab === "activity" && (
          <Activity activity={stream.activity} config={config} onlyMe={onlyMe} setOnlyMe={setOnlyMe} selected={selected} />
        )}

        <footer>
          Vigil · local test network ·{" "}
          <button className="pill-btn" disabled={busy !== null} onClick={() => run("mint", "Get test tokens", (ctx) => actions.mintTestTokens(ctx))}>
            Get test tokens
          </button>
          {messages.mint && <div className={`msg ${messages.mint.kind === "good" ? "good" : messages.mint.kind === "error" ? "error" : "busy"}`}>{messages.mint.text}</div>}
        </footer>
      </main>

      <ActionDialog
        open={dialog.open}
        tab={dialog.tab}
        assetIndex={dialog.assetIndex}
        config={config}
        position={position}
        market={market}
        walletVusd={walletVusd}
        walletCollateral={walletCollateral}
        messages={messages}
        steps={steps}
        busy={busy}
        onTab={(tab) => setDialog((d) => ({ ...d, tab }))}
        onAsset={(assetIndex) => setDialog((d) => ({ ...d, assetIndex }))}
        onClose={() => setDialog((d) => ({ ...d, open: false }))}
        onSupply={(asset, text) => run("deposit", "Supply", (ctx) => actions.supply(ctx, asset, text))}
        onBorrow={(text) => run("borrow", "Borrow", (ctx) => actions.borrow(ctx, text))}
        onRepay={(text) => run("repay", "Repay", (ctx) => actions.repay(ctx, text))}
        onRepayAll={() => { setSteps([]); run("repayAll", "Repay and return", (ctx) => actions.repayAllAndReturn(ctx, addStep)); }}
        onWithdraw={(asset, text) => run("withdraw", "Withdraw", (ctx) => actions.withdraw(ctx, asset, text))}
      />
    </>
  );
}

