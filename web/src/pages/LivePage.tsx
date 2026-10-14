// The live pipeline view: each service's stage, the activity feed and the liquidation log.
import { useEffect, useState } from "react";
import { useStream } from "../state/stream.tsx";
import { Header } from "../components/Header.tsx";
import { short } from "../lib/format.ts";

interface Liquidation { nonce: number; kind: string; status: string; sentBlock: number; borrower: string }

export function LivePage() {
  const stream = useStream();
  const [liquidations, setLiquidations] = useState<Liquidation[]>([]);

  useEffect(() => {
    const load = () => fetch("/api/liquidations").then((r) => r.json()).then(setLiquidations).catch(() => {});
    load();
    const t = setInterval(load, 2000);
    return () => clearInterval(t);
  }, []);

  const chain = stream.chain;
  const db = stream.db;
  const lag = chain && db?.indexerCursor != null ? Math.max(0, chain.head - db.indexerCursor) : null;

  const stages = [
    { id: "chain", title: "Chain", big: chain ? `block ${chain.head}` : "–", rows: [["Oracle", chain ? (chain.frozen ? "frozen" : "normal") : "–"]] },
    { id: "indexer", title: "Indexer", big: db?.indexerCursor != null ? `block ${db.indexerCursor}` : "–", rows: [["Lag (blocks)", lag ?? "–"]] },
    { id: "nats", title: "Messages", big: String(stream.counts.events + stream.counts.blocks + stream.counts.signals), rows: [["Events", stream.counts.events], ["Blocks", stream.counts.blocks], ["Signals", stream.counts.signals]] },
    { id: "risk", title: "Risk engine", big: db ? String(db.borrowingPositions) : "–", rows: [["Borrowing positions", db?.borrowingPositions ?? "–"], ["Processed events", db?.processedEvents ?? "–"]] },
    { id: "liquidator", title: "Liquidator", big: db ? String(db.liquidatorTxs) : "–", rows: [["Transactions sent", db?.liquidatorTxs ?? "–"], ["Intents on file", db?.intents ?? "–"]] },
  ];

  return (
    <>
      <Header active="live" connected={stream.connected} chain={chain} />
      <main className="wrap">
        <section className="section">
          <div className="section-head">
            <h2>Live pipeline</h2>
            <span className="muted">Each service, from the chain to the liquidator</span>
          </div>
          <div className="flow">
            {stages.map((s) => (
              <div className="stage" key={s.id}>
                <h3>{s.title}</h3>
                <div className="big">{s.big}</div>
                <dl>
                  {s.rows.map(([k, v]) => (
                    <div key={String(k)} style={{ display: "contents" }}><dt>{k}</dt><dd>{v}</dd></div>
                  ))}
                </dl>
              </div>
            ))}
          </div>
        </section>

        <section className="section split">
          <div className="card">
            <div className="card-head"><h3>Activity</h3><span className="muted" style={{ fontSize: 12.5 }}>Newest first</span></div>
            <div className="feed">
              {stream.activity.slice(0, 120).map((m, i) => (
                <div className="entry" key={i}>
                  <span className={`tag ${m.stage === "signals" ? "sig" : ""}`}>{m.stage === "signals" ? "signal" : m.data.event?.type ?? m.stage}</span>
                  {m.data.borrower ? short(m.data.borrower) : m.data.event?.user ? short(m.data.event.user) : ""}
                  <span className="muted"> block {m.data.block_number ?? ""}</span>
                </div>
              ))}
              {stream.activity.length === 0 && <div className="muted" style={{ padding: "16px 22px" }}>waiting for events…</div>}
            </div>
          </div>
          <div className="card">
            <div className="card-head"><h3>Liquidations</h3><span className="muted" style={{ fontSize: 12.5 }}>Latest transactions from the liquidator</span></div>
            <table>
              <thead><tr><th>Nonce</th><th>Kind</th><th>Status</th><th className="num">Sent block</th></tr></thead>
              <tbody>
                {liquidations.length === 0 && <tr><td colSpan={4} className="muted">none yet</td></tr>}
                {liquidations.slice(0, 20).map((r) => (
                  <tr key={r.nonce}><td>{r.nonce}</td><td>{r.kind}</td><td>{r.status}</td><td className="num">{r.sentBlock}</td></tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      </main>
    </>
  );
}
