// The top bar: logo, navigation, live status, the account selector and the theme switch.
import { ThemeButton } from "./ThemeButton.tsx";
import { short } from "../lib/format.ts";
import type { ChainSnapshot } from "../lib/types.ts";

export function Header(props: {
  active: "markets" | "live";
  connected: boolean;
  chain: ChainSnapshot | null;
  accounts?: string[];
  selected?: string;
  onSelect?: (address: string) => void;
}) {
  const { active, connected, chain, accounts, selected, onSelect } = props;
  return (
    <header className="topbar">
      <div className="wrap">
        <a className="brand" href="index.html" style={{ textDecoration: "none", color: "inherit" }}>
          <span className="logo" aria-hidden="true" />
          <span className="wordmark">Vigil</span>
        </a>
        <nav className="main">
          <a className={active === "markets" ? "active" : ""} href="markets.html">Markets</a>
          <a className={active === "live" ? "active" : ""} href="live.html">Live pipeline</a>
        </nav>
        <div className="spacer" />
        <div className="statusbar">
          <div className={`stat-item ${connected ? "is-ok" : "is-wait"}`} title="Live event stream from the gateway">
            <span className="pulse" />
            <span className="stat-label">{connected ? "Stream connected" : "Reconnecting"}</span>
          </div>
          <BlockCounter head={chain ? chain.head : null} />
          <div className={`stat-item ${!chain ? "is-wait" : chain.frozen ? "is-bad" : "is-ok"}`} title="State of the price oracle">
            <span className="stat-label">Oracle</span>
            <span className="stat-value">{chain ? (chain.frozen ? "Frozen" : "Normal") : "–"}</span>
          </div>
        </div>
        {accounts && onSelect && (
          <select id="account" aria-label="Account" value={selected} onChange={(e) => onSelect(e.target.value)}>
            {accounts.map((a, i) => (
              <option key={a} value={a}>
                {i === 0 ? "Deployer" : `Account ${i}`} · {short(a)}
              </option>
            ))}
          </select>
        )}
        <ThemeButton />
      </div>
    </header>
  );
}

// Shows the latest block number.
function BlockCounter({ head }: { head: number | null }) {
  return (
    <div className="stat-item is-ok" title="Latest block seen by the indexer">
      <span className="stat-label">Block</span>
      <span className="stat-value block-num">{head ?? "–"}</span>
    </div>
  );
}
