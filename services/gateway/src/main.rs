//! Gateway: the live window onto the pipeline. It does three things.
//! 1. Streams every NATS message (events, blocks, signals) to browsers over Server-Sent Events.
//! 2. Polls the chain and Postgres, so each stage's state is visible even when quiet.
//! 3. Serves REST reads and accepts borrower-signed intents, verified on arrival.

use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, Signature, B256, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use futures::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use tokio::sync::broadcast;
use tower_http::services::ServeDir;
use vigil_common::config::Deployment;
use vigil_common::contracts::{ILendingPool, LiquidationIntent};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long, env = "RPC_URL", default_value = "http://127.0.0.1:8545")]
    rpc_url: String,
    #[arg(long, env = "DEPLOYMENT_FILE", default_value = "../contracts/deployments/31337.json")]
    deployment_file: String,
    #[arg(long, env = "DATABASE_URL", default_value = "postgres://vigil@127.0.0.1:5432/vigil")]
    database_url: String,
    #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,
    #[arg(long, env = "GATEWAY_ADDR", default_value = "127.0.0.1:3000")]
    addr: String,
    #[arg(long, env = "FRONTEND_DIR", default_value = "../web/dist")]
    frontend: String,
}

struct AppState {
    pool: PgPool,
    chain: DynProvider,
    deployment: Deployment,
    deployment_json: Value,
    /// Every live message, as JSON, for SSE subscribers.
    live: broadcast::Sender<String>,
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    vigil_common::telemetry::init("gateway");
    let args = Args::parse();
    let deployment_json: Value = serde_json::from_str(&std::fs::read_to_string(&args.deployment_file)?)?;
    let deployment = Deployment::load(&args.deployment_file)?;
    let pool = PgPoolOptions::new().max_connections(5).connect(&args.database_url).await?;
    let chain = ProviderBuilder::new().connect_http(args.rpc_url.parse()?).erased();
    let (live, _) = broadcast::channel::<String>(1024);

    let state = Arc::new(AppState { pool, chain, deployment, deployment_json, live: live.clone() });

    // Stage 1: NATS, every subject, forwarded as it arrives.
    let nats = async_nats::connect(&args.nats_url).await?;
    let nats_live = live.clone();
    tokio::spawn(async move {
        let Ok(mut sub) = nats.subscribe("vigil.>").await else {
            tracing::error!("cannot subscribe to vigil.>");
            return;
        };
        while let Some(msg) = sub.next().await {
            let data: Value = serde_json::from_slice(&msg.payload).unwrap_or(Value::Null);
            let stage = msg.subject.as_str().trim_start_matches("vigil.").to_string();
            let _ = nats_live.send(json!({ "kind": "nats", "stage": stage, "at": now_ms(), "data": data }).to_string());
        }
    });

    // Stage 0 and the database: chain head and protocol state, then indexer and risk-engine counts.
    let poll_state = state.clone();
    tokio::spawn(async move {
        loop {
            if let Ok(snapshot) = chain_snapshot(&poll_state).await {
                let _ = poll_state.live.send(json!({ "kind": "chain", "at": now_ms(), "data": snapshot }).to_string());
            }
            if let Ok(snapshot) = db_snapshot(&poll_state.pool).await {
                let _ = poll_state.live.send(json!({ "kind": "db", "at": now_ms(), "data": snapshot }).to_string());
            }
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        }
    });

    let app = Router::new()
        .route("/api/stream", get(stream))
        .route("/api/config", get(config))
        .route("/api/protocol", get(protocol))
        .route("/api/positions", get(positions))
        .route("/api/positions/{addr}", get(position))
        .route("/api/account/{addr}", get(account))
        .route("/api/liquidations", get(liquidations))
        .route("/api/intents", get(intents).post(submit_intent))
        .route("/api/health", get(health))
        .fallback_service(ServeDir::new(&args.frontend))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.addr).await?;
    tracing::info!(addr = %args.addr, "gateway listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn chain_snapshot(s: &AppState) -> anyhow::Result<Value> {
    let pool = ILendingPool::new(s.deployment.pool, &s.chain);
    let oracle = vigil_common::contracts::IOracle::new(s.deployment.oracle, &s.chain);
    let head = s.chain.get_block_number().await?;
    let idx = pool.previewIndexes().call().await?;
    let totals = pool.totals().call().await?;
    let frozen = oracle.isFrozen().call().await?;
    let mut prices = Vec::new();
    for c in &s.deployment.collaterals {
        let p = oracle.getPrice(c.address).call().await.map(|v| v.to_string()).unwrap_or_else(|_| "frozen".into());
        prices.push(json!({ "name": c.name, "price_wad": p }));
    }
    let debt_price = oracle.getPrice(s.deployment.debt).call().await.map(|v| v.to_string()).unwrap_or_else(|_| "frozen".into());
    Ok(json!({
        "head": head,
        "frozen": frozen,
        "debtPriceWad": debt_price,
        "debtIndex": idx.debtIndex.to_string(),
        "supplyIndex": idx.supplyIndex.to_string(),
        "totalDebtScaled": totals.totalDebtScaled.to_string(),
        "totalSupplyScaled": totals.totalSupplyScaled.to_string(),
        "reserves": totals.reserves.to_string(),
        "deficit": totals.deficit.to_string(),
        "prices": prices,
    }))
}

async fn db_snapshot(pool: &PgPool) -> anyhow::Result<Value> {
    let cursor = sqlx::query("SELECT block_number FROM indexer_cursor WHERE id = 1").fetch_optional(pool).await?;
    let cursor: Option<i64> = cursor.map(|r| r.get("block_number"));
    let processed: i64 = sqlx::query("SELECT COUNT(*) AS n FROM processed_events").fetch_one(pool).await?.get("n");
    let positions: i64 = sqlx::query("SELECT COUNT(*) AS n FROM positions WHERE debt_scaled <> '0'").fetch_one(pool).await?.get("n");
    let intents: i64 = sqlx::query("SELECT COUNT(*) AS n FROM intents").fetch_one(pool).await?.get("n");
    let txs: i64 = sqlx::query("SELECT COUNT(*) AS n FROM liquidator_txs").fetch_one(pool).await?.get("n");
    Ok(json!({
        "indexerCursor": cursor,
        "processedEvents": processed,
        "borrowingPositions": positions,
        "intents": intents,
        "liquidatorTxs": txs,
    }))
}

async fn stream(State(s): State<Arc<AppState>>) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = s.live.subscribe();
    let events = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(msg) => return Some((Ok(Event::default().data(msg)), rx)),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return None,
            }
        }
    });
    Sse::new(events).keep_alive(KeepAlive::default())
}

async fn config(State(s): State<Arc<AppState>>) -> Json<Value> {
    Json(s.deployment_json.clone())
}

async fn protocol(State(s): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    chain_snapshot(&s).await.map(Json).map_err(err500)
}

fn err500(e: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn positions(State(s): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let rows = sqlx::query("SELECT borrower, bitmap, debt_scaled, amount_0, amount_1, amount_2 FROM positions WHERE debt_scaled <> '0' ORDER BY borrower")
        .fetch_all(&s.pool)
        .await
        .map_err(|e| err500(e.into()))?;
    let mut out = Vec::new();
    for r in rows {
        let borrower: String = r.get("borrower");
        let addr = Address::from_str(&borrower).map_err(|e| err500(e.into()))?;
        out.push(position_json(&s, addr, &r).await.map_err(err500)?);
    }
    Ok(Json(json!(out)))
}

async fn position(State(s): State<Arc<AppState>>, Path(addr): Path<String>) -> Result<Json<Value>, (StatusCode, String)> {
    let a = Address::from_str(&addr).map_err(|_| (StatusCode::BAD_REQUEST, "bad address".into()))?;
    let row = sqlx::query("SELECT borrower, bitmap, debt_scaled, amount_0, amount_1, amount_2 FROM positions WHERE borrower = $1")
        .bind(format!("{a:#x}"))
        .fetch_optional(&s.pool)
        .await
        .map_err(|e| err500(e.into()))?;
    let Some(row) = row else { return Ok(Json(json!({ "borrower": format!("{a:#x}"), "empty": true }))) };
    position_json(&s, a, &row).await.map(Json).map_err(err500)
}

/// Merges the Risk Engine's stored position with the health factor read live from the pool.
async fn position_json(s: &AppState, who: Address, row: &sqlx::postgres::PgRow) -> anyhow::Result<Value> {
    let pool = ILendingPool::new(s.deployment.pool, &s.chain);
    let hf = pool.getHealthFactor(who).call().await.map(|v| v.to_string()).unwrap_or_else(|_| "n/a".into());
    let auction = pool.auctionStartOf(who).call().await?;
    let debt = pool.debtOf(who).call().await?;
    let txt = |c: &str| -> String { row.try_get::<String, _>(c).unwrap_or_default() };
    Ok(json!({
        "borrower": format!("{who:#x}"),
        "bitmap": row.try_get::<i16, _>("bitmap").unwrap_or(0),
        "debtScaled": txt("debt_scaled"),
        "debt": debt.to_string(),
        "amounts": [txt("amount_0"), txt("amount_1"), txt("amount_2")],
        "healthFactor": hf,
        "auctionStart": auction.to_string(),
    }))
}

/// Live account state straight from the pool and oracle, so the page can show what the contract
/// will enforce. The Risk Engine's copy of the position is reported alongside, not used for the numbers.
async fn account(State(s): State<Arc<AppState>>, Path(addr): Path<String>) -> Result<Json<Value>, (StatusCode, String)> {
    let who = Address::from_str(&addr).map_err(|_| (StatusCode::BAD_REQUEST, "bad address".into()))?;
    account_json(&s, who).await.map(Json).map_err(err500)
}

async fn account_json(s: &AppState, who: Address) -> anyhow::Result<Value> {
    let pool = ILendingPool::new(s.deployment.pool, &s.chain);
    let oracle = vigil_common::contracts::IOracle::new(s.deployment.oracle, &s.chain);
    let debt = pool.debtOf(who).call().await?;
    let supplied = pool.supplyOf(who).call().await?;
    let auction = pool.auctionStartOf(who).call().await?;
    // Health factor reverts while the oracle is frozen, so it is reported as null rather than failing the page.
    let hf = if debt.is_zero() { None } else { pool.getHealthFactor(who).call().await.ok().map(|v| v.to_string()) };

    let mut collaterals = Vec::new();
    for c in &s.deployment.collaterals {
        let amount = pool.collateralOf(who, c.address).call().await?;
        let price = oracle.getPrice(c.address).call().await.ok().map(|v| v.to_string());
        collaterals.push(json!({
            "name": c.name,
            "address": format!("{:#x}", c.address),
            "decimals": c.decimals,
            "liqThresholdBps": c.liq_threshold_bps,
            "amount": amount.to_string(),
            "priceWad": price,
        }));
    }
    let debt_price = oracle.getPrice(s.deployment.debt).call().await.ok().map(|v| v.to_string());

    let tracked: i64 = sqlx::query("SELECT COUNT(*) AS n FROM positions WHERE borrower = $1 AND debt_scaled <> '0'")
        .bind(format!("{who:#x}"))
        .fetch_one(&s.pool)
        .await?
        .get("n");

    Ok(json!({
        "borrower": format!("{who:#x}"),
        "debt": debt.to_string(),
        "supplied": supplied.to_string(),
        "healthFactor": hf,
        "auctionStart": auction.to_string(),
        "debtPriceWad": debt_price,
        "debtDecimals": s.deployment.debt_decimals,
        "collaterals": collaterals,
        "riskEngineTracked": tracked > 0,
    }))
}

async fn liquidations(State(s): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let rows = sqlx::query("SELECT nonce, kind, borrower, tx_hash, max_fee_wei, sent_block, status, trace_id FROM liquidator_txs ORDER BY nonce DESC LIMIT 50")
        .fetch_all(&s.pool)
        .await
        .map_err(|e| err500(e.into()))?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "nonce": r.get::<i64, _>("nonce"),
                "kind": r.get::<String, _>("kind"),
                "borrower": r.get::<String, _>("borrower"),
                "txHash": r.get::<String, _>("tx_hash"),
                "maxFeeWei": r.get::<String, _>("max_fee_wei"),
                "sentBlock": r.get::<i64, _>("sent_block"),
                "status": r.get::<String, _>("status"),
                "traceId": r.get::<String, _>("trace_id"),
            })
        })
        .collect();
    Ok(Json(json!(out)))
}

async fn intents(State(s): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let rows = sqlx::query("SELECT borrower, nonce, collateral_asset, max_repay, min_collateral_out, deadline FROM intents ORDER BY borrower, nonce")
        .fetch_all(&s.pool)
        .await
        .map_err(|e| err500(e.into()))?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "borrower": r.get::<String, _>("borrower"),
                "nonce": r.get::<String, _>("nonce"),
                "collateralAsset": r.get::<String, _>("collateral_asset"),
                "maxRepay": r.get::<String, _>("max_repay"),
                "minCollateralOut": r.get::<String, _>("min_collateral_out"),
                "deadline": r.get::<i64, _>("deadline"),
            })
        })
        .collect();
    Ok(Json(json!(out)))
}

#[derive(Deserialize)]
struct IntentBody {
    borrower: String,
    #[serde(rename = "collateralAsset")]
    collateral_asset: String,
    #[serde(rename = "maxRepay")]
    max_repay: String,
    #[serde(rename = "minCollateralOut")]
    min_collateral_out: String,
    nonce: String,
    deadline: String,
    signature: String,
}

/// Accepts a borrower-signed intent only if the signature recovers to the borrower, over the same
/// EIP-712 digest the contract computes. Anything else is rejected before it reaches the database.
async fn submit_intent(State(s): State<Arc<AppState>>, Json(b): Json<IntentBody>) -> impl IntoResponse {
    match verify_and_store(&s, b).await {
        Ok(hash) => (StatusCode::CREATED, Json(json!({ "accepted": true, "digest": hash }))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "accepted": false, "error": e.to_string() }))),
    }
}

async fn verify_and_store(s: &AppState, b: IntentBody) -> anyhow::Result<String> {
    let borrower = Address::from_str(&b.borrower)?;
    let intent = LiquidationIntent {
        borrower,
        collateralAsset: Address::from_str(&b.collateral_asset)?,
        maxRepay: U256::from_str(&b.max_repay)?,
        minCollateralOut: U256::from_str(&b.min_collateral_out)?,
        nonce: U256::from_str(&b.nonce)?,
        deadline: U256::from_str(&b.deadline)?,
    };
    let pool = ILendingPool::new(s.deployment.pool, &s.chain);
    let digest: B256 = pool.hashIntent(intent.clone()).call().await?;
    let sig_bytes = alloy::hex::decode(b.signature.trim_start_matches("0x"))?;
    let sig = Signature::try_from(sig_bytes.as_slice())?;
    let signer = sig.recover_address_from_prehash(&digest)?;
    anyhow::ensure!(signer == borrower, "signature recovers to {signer:#x}, not the borrower");

    sqlx::query(
        "INSERT INTO intents (borrower, nonce, collateral_asset, max_repay, min_collateral_out, deadline, signature)
         VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (borrower, nonce) DO NOTHING",
    )
    .bind(format!("{borrower:#x}"))
    .bind(b.nonce)
    .bind(format!("{:#x}", intent.collateralAsset))
    .bind(b.max_repay)
    .bind(b.min_collateral_out)
    .bind(i64::try_from(intent.deadline.to::<u64>()).unwrap_or(i64::MAX))
    .bind(format!("0x{}", alloy::hex::encode(&sig_bytes)))
    .execute(&s.pool)
    .await?;
    Ok(format!("{digest:#x}"))
}

async fn health(State(s): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let chain = chain_snapshot(&s).await.map_err(err500)?;
    let db = db_snapshot(&s.pool).await.map_err(err500)?;
    let head = chain["head"].as_u64().unwrap_or(0);
    let cursor = db["indexerCursor"].as_i64().unwrap_or(0) as u64;
    Ok(Json(json!({
        "chainHead": head,
        "indexerCursor": cursor,
        "indexerLagBlocks": head.saturating_sub(cursor),
        "frozen": chain["frozen"],
    })))
}
