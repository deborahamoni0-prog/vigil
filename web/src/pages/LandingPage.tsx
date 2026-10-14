// The homepage: hero, phone mockups, feature sections, steps and a call to action.
// Numbers are read live from the gateway.
import { useEffect, useState } from "react";
import { useProtocolOnce } from "../state/hooks.ts";
import { computeMarket } from "../lib/rates.ts";
import { useConfig } from "../state/hooks.ts";
import { fmt } from "../lib/format.ts";
import { useStream } from "../state/stream.tsx";

export function LandingPage() {
  const chain = useProtocolOnce();
  const config = useConfig();
  const stream = useStream();
  const [live, setLive] = useState<{ head: number } | null>(null);

  useEffect(() => { if (chain) setLive({ head: chain.head }); }, [chain]);

  const market = chain && config
    ? computeMarket({
        debtIndex: chain.debtIndex,
        supplyIndex: chain.supplyIndex,
        totalDebtScaled: chain.totalDebtScaled,
        totalSupplyScaled: chain.totalSupplyScaled,
        rates: config.rates,
        reserveFactor: config.reserveFactor,
      })
    : null;
  const priceOf = (name: string) => {
    const p = chain?.prices.find((x) => x.name === name);
    return p && p.price_wad !== "frozen" ? Number(p.price_wad) / 1e18 : null;
  };
  const btc = priceOf("eight");
  const eth = priceOf("fee");
  const usd = (n: number | null) => (n === null ? "–" : "$" + Math.round(n).toLocaleString("en-US"));
  const debt = market ? market.totalDebt : null;
  const debtNumber = debt === null || !config ? null : Number(debt) / 10 ** config.debtDecimals;
  const block = chain ? chain.head.toLocaleString("en-US") : "–";

  return (
    <>
      <header className="nav">
        <div className="wrap">
          <a className="brand" href="index.html"><span className="logo">V</span> Vigil</a>
          <nav className="links">
            <a href="#how">How it works</a>
            <a href="#features">Features</a>
            <a href="live.html">Live pipeline</a>
          </nav>
          <div className="spacer" />
          <span className="live-chip">{stream.connected ? `block ${block}` : "connecting…"}</span>
          <a className="pill solid" href="markets.html">Open app</a>
        </div>
      </header>

      <main>
        <section className="hero">
          <div className="wrap">
            <span className="eyebrow">Lending on a live chain</span>
            <h1>Borrow and earn with <em>every number in view.</em></h1>
            <p className="lead">Supply collateral, borrow vUSD against it, and repay to get your collateral back. Prices, health and rates update on every block.</p>
            <div className="cta-row">
              <a className="pill solid" href="markets.html">Start with markets</a>
              <a className="pill ghost" href="live.html">Watch the pipeline</a>
            </div>
            <div className="hero-stats">
              <div className="hero-stat"><div className="v">{debtNumber === null ? "–" : fmt(market!.totalDebt, config!.debtDecimals, 0)}</div><div className="l">vUSD borrowed</div></div>
              <div className="hero-stat"><div className="v">{block}</div><div className="l">Latest block</div></div>
              <div className="hero-stat"><div className="v">{usd(btc)}</div><div className="l">Bitcoin price, from the oracle</div></div>
            </div>

            <div className="phones">
              <div className="phone">
                <div className="screen">
                  <div className="notch" />
                  <div className="app-title">Supply <span>block {block}</span></div>
                  <div className="big">{usd(btc)}</div>
                  <span className="tag">Net worth</span>
                  <div className="card">
                    <div className="asset"><span className="dot" style={{ background: "#f08c2b" }}>WE</span>Wrapped Ether<span className="amt">{usd(eth)}</span></div>
                    <div className="asset"><span className="dot" style={{ background: "#5a7bd9" }}>WB</span>Wrapped Bitcoin<span className="amt">{usd(btc)}</span></div>
                  </div>
                  <div className="btn-ph">Supply</div>
                </div>
              </div>

              <div className="phone center">
                <div className="screen">
                  <div className="notch" />
                  <div className="app-title">Pool utilisation <span>live</span></div>
                  <div className="ring">
                    <svg viewBox="0 0 36 36">
                      <circle cx="18" cy="18" r="15.9" fill="none" stroke="#272731" strokeWidth="3.2" />
                      <circle cx="18" cy="18" r="15.9" fill="none" stroke="#6fd6e0" strokeWidth="3.2" strokeLinecap="round" strokeDasharray={`${market ? Math.min(Number(market.utilization), 100) : 0} 100`} />
                    </svg>
                    <div className="c">{market ? market.utilization.toFixed(1) + "%" : "–"}</div>
                  </div>
                  <div className="card"><div className="k">Borrow rate (APY)</div><div className="v">{market ? market.borrowApy.toFixed(2) + "%" : "–"}</div></div>
                  <div className="card"><div className="k">Supply APY</div><div className="v">{market ? market.supplyApy.toFixed(2) + "%" : "–"}</div></div>
                  <div className="btn-ph">Borrow vUSD</div>
                </div>
              </div>

              <div className="phone">
                <div className="screen">
                  <div className="notch" />
                  <div className="app-title">Activity <span>block {block}</span></div>
                  <div className="bars">
                    {[30, 55, 40, 70, 50, 85, 65].map((h, i) => <i key={i} style={{ height: `${h}%` }} />)}
                  </div>
                  <div className="card"><div className="k">Total borrowed</div><div className="v">{debtNumber === null ? "–" : debtNumber.toFixed(2) + " vUSD"}</div></div>
                  <div className="card"><div className="k">Oracle</div><div className="v">{chain ? (chain.frozen ? "Frozen" : "Normal") : "–"}</div></div>
                  <div className="btn-ph">Repay</div>
                </div>
              </div>
            </div>
          </div>
        </section>

        <section className="block" id="features">
          <div className="wrap split">
            <div className="copy">
              <span className="eyebrow">Supply</span>
              <h2>Put your collateral to work, and keep control of it.</h2>
              <p>Deposit assets into the pool. They stay there until you take them out, and they count toward what you can borrow.</p>
              <ul>
                <li>Collateral is valued in USD by the protocol's own oracle.</li>
                <li>Fee-on-transfer tokens are supported. The pool counts what it really receives.</li>
                <li>Withdraw any time your health factor allows it.</li>
              </ul>
              <a className="pill solid" href="markets.html">Supply collateral</a>
            </div>
            <div className="panel">
              <div className="kv"><span>Collateral assets</span><b>3</b></div>
              <div className="kv"><span>Oracle</span><b>{chain ? (chain.frozen ? "Frozen" : "Normal") : "–"}</b></div>
              <div className="kv"><span>Block</span><b>{block}</b></div>
            </div>
          </div>
        </section>

        <section className="block">
          <div className="wrap split flip">
            <div className="copy">
              <span className="eyebrow">Borrow</span>
              <h2>Borrow vUSD against what you hold, with a clear limit.</h2>
              <p>Your health factor tells you how safe you are. Above 1 is safe. At or below 1, the position can be liquidated.</p>
              <ul>
                <li>Rates rise as the pool fills up, so borrowing is cheap when there is spare liquidity.</li>
                <li>A safety buffer keeps small price moves from putting you at risk.</li>
                <li>See the health factor after your action before you confirm it.</li>
              </ul>
              <a className="pill ghost" href="markets.html">See borrow markets</a>
            </div>
            <div className="panel">
              <div className="kv"><span>Borrow APY</span><b>{market ? market.borrowApy.toFixed(2) + "%" : "–"}</b></div>
              <div className="kv"><span>Supply APY</span><b>{market ? market.supplyApy.toFixed(2) + "%" : "–"}</b></div>
              <div className="kv"><span>Pool utilisation</span><b>{market ? market.utilization.toFixed(1) + "%" : "–"}</b></div>
              <div className="meter"><div style={{ width: `${market ? Math.min(Number(market.utilization), 100) : 0}%` }} /></div>
              <div className="label-row"><span>Share of supplied vUSD borrowed</span><span>{market ? market.utilization.toFixed(1) + "%" : "–"}</span></div>
            </div>
          </div>
        </section>

        <section className="block" id="how">
          <div className="wrap">
            <span className="eyebrow">How it works</span>
            <h2 className="heading">Three steps from wallet to borrowing.</h2>
            <div className="steps">
              <div className="step"><div className="n">1</div><h3>Supply collateral</h3><p>Deposit an asset into the pool. It counts toward your borrowing power as soon as it lands.</p></div>
              <div className="step"><div className="n">2</div><h3>Borrow against it</h3><p>Borrow vUSD up to your limit. Your health factor updates with every block.</p></div>
              <div className="step"><div className="n">3</div><h3>Repay and get it back</h3><p>Repay everything, including interest, and every asset you supplied comes back to your wallet.</p></div>
            </div>
          </div>
        </section>

        <section style={{ padding: "0 0 150px" }}>
          <div className="wrap">
            <div className="band">
              <div><div className="v">{debtNumber === null ? "–" : fmt(market!.totalDebt, config!.debtDecimals, 0)}</div><div className="l">vUSD borrowed</div></div>
              <div><div className="v">{chain && config ? fmt(BigInt(chain.reserves), config.debtDecimals) : "–"}</div><div className="l">Reserves</div></div>
              <div><div className="v">{chain && config ? fmt(BigInt(chain.deficit), config.debtDecimals) : "–"}</div><div className="l">Bad debt</div></div>
              <div><div className="v">{block}</div><div className="l">Latest block</div></div>
            </div>
          </div>
        </section>

        <section className="final">
          <div className="wrap">
            <h2>Open the markets and see your position update live.</h2>
            <p>This is a local test network. The tokens and money are for testing only.</p>
            <div className="cta-row">
              <a className="pill solid" href="markets.html">Open app</a>
              <a className="pill ghost" href="live.html">Live pipeline</a>
            </div>
          </div>
        </section>
      </main>

      <footer>
        <div className="wrap">
          <span>Vigil · local test network</span>
          <nav>
            <a href="markets.html">Markets</a>
            <a href="live.html">Live pipeline</a>
            <a href="#how">How it works</a>
          </nav>
        </div>
      </footer>
    </>
  );
}
