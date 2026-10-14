//! Position snapshot endpoint (RS-10). Read-only, served from the store and the last block.

use std::sync::Arc;

use alloy::primitives::Address;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::engine::{RiskEngine, RiskStore};

#[derive(Serialize)]
pub struct Snapshot {
    pub borrower: String,
    pub bitmap: u8,
    pub debt_scaled: String,
    pub amounts: [String; 3],
    pub health_factor: Option<String>,
    pub as_of_block: Option<u64>,
}

pub struct ApiState<S: RiskStore> {
    pub engine: Mutex<RiskEngine<S>>,
}

pub fn router<S: RiskStore + 'static>(state: Arc<ApiState<S>>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/positions/{borrower}", get(snapshot::<S>))
        .with_state(state)
}

async fn snapshot<S: RiskStore + 'static>(
    State(state): State<Arc<ApiState<S>>>,
    Path(borrower): Path<String>,
) -> Result<Json<Snapshot>, (StatusCode, String)> {
    let addr: Address = borrower
        .parse()
        .map_err(|_| (StatusCode::BAD_REQUEST, "bad address".to_string()))?;
    let engine = state.engine.lock().await;
    let p = engine
        .store
        .load_position(addr)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let hf = engine
        .health_factor_of(addr)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(Snapshot {
        borrower: format!("{addr:#x}"),
        bitmap: p.bitmap,
        debt_scaled: p.debt_scaled.to_string(),
        amounts: [
            p.amounts[0].to_string(),
            p.amounts[1].to_string(),
            p.amounts[2].to_string(),
        ],
        health_factor: hf.map(|(h, _)| h.to_string()),
        as_of_block: hf.map(|(_, b)| b),
    }))
}
