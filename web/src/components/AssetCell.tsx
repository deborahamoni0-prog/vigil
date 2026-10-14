// The round token badge with the name and ticker, used in every table that lists a collateral.
import { tokenInfo } from "../lib/tokens.ts";

export function assetCell(name: string) {
  const info = tokenInfo(name);
  return (
    <div className="asset" title={info.note}>
      <span className={`coin ${name}`}>{info.ticker.slice(0, 2)}</span>
      <div>
        <div className="name">{info.label}</div>
        <div className="ticker">{info.ticker}</div>
      </div>
    </div>
  );
}
