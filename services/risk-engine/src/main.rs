use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use async_nats::header::HeaderMap;
use async_nats::jetstream::{self, consumer::pull, consumer::AckPolicy, stream};
use clap::Parser;
use futures::StreamExt;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Mutex;
use vigil_common::config::Deployment;
use vigil_common::messages::{
    BlockMessage, EventMessage, LiquidationSignal, STREAM_NAME, SUBJECT_BLOCKS, SUBJECT_EVENTS,
    SUBJECT_SIGNALS,
};
use vigil_risk_engine::api::{self, ApiState};
use vigil_risk_engine::engine::RiskEngine;
use vigil_risk_engine::store::PgRiskStore;

#[derive(Parser, Debug)]
#[command(about = "Vigil Risk Engine: positions, exactly-once processing, liquidation signals")]
struct Args {
    #[arg(long, env = "DEPLOYMENT_FILE")]
    deployment_file: String,
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
    #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,
    #[arg(long, env = "HTTP_ADDR", default_value = "127.0.0.1:8080")]
    http_addr: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    vigil_common::telemetry::init("risk-engine");
    let args = Args::parse();
    let deployment = Deployment::load(&args.deployment_file)?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&args.database_url)
        .await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;

    let state = Arc::new(ApiState {
        engine: Mutex::new(RiskEngine::new(
            PgRiskStore::new(pool),
            deployment.pool_params(),
        )),
    });

    let addr = SocketAddr::from_str(&args.http_addr)?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let http_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, api::router(http_state)).await {
            tracing::error!(error = %e, "http server stopped");
        }
    });

    let nats = async_nats::connect(&args.nats_url).await?;
    let js = jetstream::new(nats);
    let stream = js
        .get_or_create_stream(stream::Config {
            name: STREAM_NAME.to_string(),
            subjects: vec!["vigil.>".to_string()],
            ..Default::default()
        })
        .await?;
    // Durable consumer: a restart resumes where it left off. Messages are acked only after the
    // state change commits, so an unprocessed message is redelivered.
    let consumer = stream
        .get_or_create_consumer(
            "risk-engine",
            pull::Config {
                durable_name: Some("risk-engine".to_string()),
                ack_policy: AckPolicy::Explicit,
                filter_subjects: vec![SUBJECT_EVENTS.to_string(), SUBJECT_BLOCKS.to_string()],
                ..Default::default()
            },
        )
        .await?;
    let mut messages = consumer.messages().await?;
    tracing::info!("risk engine consuming");

    while let Some(next) = messages.next().await {
        let msg = match next {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "consumer error, continuing");
                continue;
            }
        };
        let subject = msg.subject.to_string();
        let outcome: anyhow::Result<()> = async {
            if subject == SUBJECT_EVENTS {
                let ev: EventMessage = serde_json::from_slice(&msg.payload)?;
                let changed = state.engine.lock().await.handle_event(&ev).await?;
                tracing::debug!(trace_id = %ev.trace_id, changed, "event applied");
            } else if subject == SUBJECT_BLOCKS {
                let block: BlockMessage = serde_json::from_slice(&msg.payload)?;
                let signals = state.engine.lock().await.handle_block(block).await?;
                for s in signals {
                    publish_signal(&js, &s).await?;
                }
            }
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => msg
                .ack()
                .await
                .map_err(|e| anyhow::anyhow!("ack failed: {e}"))?,
            // Leave it unacked. JetStream redelivers after the ack wait, and the idempotency keys
            // stop a double-apply.
            Err(e) => {
                tracing::error!(error = %e, subject, "message not applied, will be redelivered")
            }
        }
    }
    Ok(())
}

async fn publish_signal(js: &jetstream::Context, s: &LiquidationSignal) -> anyhow::Result<()> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Nats-Msg-Id",
        format!("signal:{:#x}:{}", s.borrower, s.block_number).as_str(),
    );
    headers.insert("trace-id", s.trace_id.as_str());
    js.publish_with_headers(
        vigil_common::messages::signal_subject(s.borrower),
        headers,
        serde_json::to_vec(s)?.into(),
    )
    .await?
    .await?;
    tracing::info!(borrower = %s.borrower, block = s.block_number, trace_id = %s.trace_id, "liquidation signal");
    Ok(())
}
