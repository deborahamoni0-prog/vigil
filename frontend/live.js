// Live pipeline view. Subscribes to /api/stream (Server-Sent Events from the gateway).
// Every number here comes from a message the services produced, or from a read of the chain or database.
const $ = (id) => document.getElementById(id);
const counts = { events: 0, blocks: 0, signals: 0, retracts: 0, reorgs: 0, acted: 0 };
const MAX_FEED = 200;
let lastStage = { chain: null, db: null };
let liqTxs = [];

function short(s) { return typeof s === "string" && s.length > 14 ? s.slice(0, 8) + "…" + s.slice(-4) : s; }

function flash(id) {
  const el = $(id);
  el.classList.add("flash");
  setTimeout(() => el.classList.remove("flash"), 350);
}

function addFeed(container, html) {
  const box = $(container);
  const row = document.createElement("div");
  row.className = "row";
  row.innerHTML = html;
  box.prepend(row);
  while (box.children.length > MAX_FEED) box.removeChild(box.lastChild);
}

function describe(stage, data) {
  switch (stage) {
    case "events": {
      const ev = data.event || {};
      const kind = data.kind === "retract" ? "RETRACT" : "log";
      const who = ev.user || ev.borrower || "";
      return `<span class="tag">${kind} · ${ev.type || "?"}</span> block ${data.block_number} idx ${data.log_index} ${who ? "· " + short(who) : ""}`;
    }
    case "blocks":
      return `<span class="tag">block</span> ${data.block_number} · frozen=${data.frozen} · debtIndex ${short(String(data.quote?.debt_index ?? ""))}`;
    case "signals":
      return `<span class="tag">SIGNAL</span> borrower ${short(data.borrower)} at block ${data.block_number} · HF ${short(String(data.health_factor))}`;
    default:
      return `<span class="tag">${stage}</span>`;
  }
}

function render(msg) {
  if (msg.kind === "chain") {
    const d = msg.data;
    lastStage.chain = d;
    $("head").textContent = `head ${d.head}`;
    $("frozenPill").textContent = d.frozen ? "oracle FROZEN" : "oracle normal";
    $("frozenPill").className = `pill ${d.frozen ? "bad" : "ok"}`;
    $("c-head").textContent = `block ${d.head}`;
    $("c-frozen").textContent = d.frozen ? "frozen" : "normal";
    $("c-idx").textContent = short(d.debtIndex);
    $("c-debt").textContent = short(d.totalDebtScaled);
    $("c-def").textContent = d.deficit;
    $("c-prices").innerHTML = d.prices.map((p) => `${p.name}: ${p.price_wad === "frozen" ? "frozen" : "$" + (Number(p.price_wad) / 1e18).toFixed(2)}`).join(" · ");
    flash("st-chain");
    const lag = d.head - (lastStage.db?.indexerCursor ?? d.head);
    $("i-lag").textContent = lastStage.db ? Math.max(0, lag) : "–";
    return;
  }
  if (msg.kind === "db") {
    const d = msg.data;
    lastStage.db = d;
    $("i-cursor").textContent = d.indexerCursor ?? "–";
    $("i-cur2").textContent = d.indexerCursor ?? "–";
    $("i-lag").textContent = lastStage.chain ? Math.max(0, lastStage.chain.head - (d.indexerCursor ?? 0)) : "–";
    $("r-pos").textContent = d.borrowingPositions;
    $("r-pos2").textContent = d.borrowingPositions;
    $("r-proc").textContent = d.processedEvents;
    $("l-txs").textContent = d.liquidatorTxs;
    $("l-sent").textContent = d.liquidatorTxs;
    $("l-int").textContent = d.intents;
    return;
  }
  if (msg.kind === "nats") {
    const stage = msg.stage;
    const data = msg.data || {};
    counts.events += stage === "events" ? 1 : 0;
    counts.blocks += stage === "blocks" ? 1 : 0;
    counts.signals += stage === "signals" ? 1 : 0;
    if (stage === "events" && data.kind === "retract") { counts.retracts += 1; $("i-retract").textContent = counts.retracts; }
    if (stage === "events" && data.kind !== "retract" && data.event?.type === "other") { /* not a position event */ }
    $("n-events").textContent = counts.events;
    $("n-blocks").textContent = counts.blocks;
    $("n-signals").textContent = counts.signals;
    $("n-total").textContent = counts.events + counts.blocks + counts.signals;
    $("r-sig").textContent = counts.signals;
    $("l-acted").textContent = counts.signals;
    if (stage === "events") { flash("st-indexer"); flash("st-risk"); }
    if (stage === "blocks") { flash("st-indexer"); }
    if (stage === "signals") { flash("st-risk"); flash("st-liq"); }
    flash("st-nats");
    addFeed("feed", `<span class="tag">${stage}</span> ${describe(stage, data)}`);
    if (stage === "events" || stage === "signals") {
      const who = (data.event && (data.event.user || data.event.borrower)) || data.borrower;
      const followed = $("who").value.trim().toLowerCase();
      if (followed && who && who.toLowerCase() === followed) {
        addFeed("trace", `<span class="tag">${stage}</span> ${describe(stage, data)}`);
      }
    }
  }
}

async function refreshFollowed() {
  const who = $("who").value.trim();
  if (!who) { $("whoPos").textContent = ""; return; }
  try {
    const p = await (await fetch(`/api/positions/${who}`)).json();
    if (p.empty) { $("whoPos").textContent = "no borrowing position"; return; }
    $("whoPos").innerHTML = `HF <b>${p.healthFactor}</b> wei · debt ${(Number(p.debt) / 1e6).toFixed(2)} vUSD · auction ${p.auctionStart === "0" ? "none" : "block " + p.auctionStart}`;
  } catch (e) { $("whoPos").textContent = "gateway unreachable"; }
}

async function refreshLiquidations() {
  try {
    const rows = await (await fetch("/api/liquidations")).json();
    $("liqRows").innerHTML = rows.slice(0, 20).map((r) => `<tr><td>${r.nonce}</td><td>${r.kind}</td><td>${r.status}</td><td>${r.sentBlock}</td></tr>`).join("");
  } catch (e) { /* gateway not up yet */ }
}

function connect() {
  const es = new EventSource("/api/stream");
  es.onopen = () => { $("conn").textContent = "live"; $("conn").className = "pill ok"; };
  es.onerror = () => { $("conn").textContent = "reconnecting…"; $("conn").className = "pill bad"; };
  es.onmessage = (ev) => {
    try { render(JSON.parse(ev.data)); } catch (e) { /* skip a malformed frame */ }
  };
}

$("who").addEventListener("change", refreshFollowed);
setInterval(refreshFollowed, 3000);
setInterval(refreshLiquidations, 2000);
connect();
