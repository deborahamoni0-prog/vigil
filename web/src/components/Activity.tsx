// Pool events and liquidation signals from the pipeline, newest first.
import { short } from "../lib/format.ts";
import { fmt } from "../lib/format.ts";
import type { ActivityMessage, Config } from "../lib/types.ts";

export function Activity({ activity, config, onlyMe, setOnlyMe, selected }: {
  activity: ActivityMessage[];
  config: Config | null;
  onlyMe: boolean;
  setOnlyMe: (v: boolean) => void;
  selected: string;
}) {
  const rows = activity.filter((m) => {
    if (!onlyMe) return true;
    const who = m.data.event?.user ?? m.data.event?.borrower ?? m.data.borrower ?? "";
    return who.toLowerCase() === selected.toLowerCase();
  });

  return (
    <div className="card">
      <div className="card-head">
        <h3>Activity</h3>
        <label className="toggle">
          <input type="checkbox" checked={onlyMe} onChange={(e) => setOnlyMe(e.target.checked)} /> Only this account
        </label>
      </div>
      <div className="feed">
        {rows.length === 0 && <div className="muted" style={{ padding: "16px 22px" }}>waiting for events…</div>}
        {rows.slice(0, 80).map((m, i) => {
          const who = m.data.event?.user ?? m.data.event?.borrower ?? m.data.borrower ?? "";
          if (m.stage === "signals") {
            return (
              <div className="entry" key={i}>
                <span className="tag sig">liquidation signal</span> {short(m.data.borrower ?? "")}
                <span className="muted"> block {m.data.block_number}</span>
              </div>
            );
          }
          const type = m.data.event?.type ?? "event";
          const debt = type === "borrowed" || type === "repaid";
          const amount = debt && m.data.event?.amount && config
            ? " · " + fmt(BigInt(m.data.event.amount), config.debtDecimals) + " vUSD"
            : "";
          const retracted = m.data.kind === "retract" ? " (retracted)" : "";
          return (
            <div className="entry" key={i}>
              <span className={`tag ${m.data.kind === "retract" ? "retract" : ""}`}>{type}</span>
              {retracted} {who ? short(who) : ""}{amount}
              <span className="muted"> block {m.data.block_number}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
