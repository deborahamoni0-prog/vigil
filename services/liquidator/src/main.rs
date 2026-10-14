use std::time::Duration;

use async_nats::jetstream::{
    self, consumer::pull, consumer::AckPolicy, consumer::DeliverPolicy, stream,
};
use clap::{Parser, ValueEnum};
use futures::StreamExt;
use sqlx::postgres::PgPoolOptions;
use vigil_common::config::Deployment;
use vigil_common::messages::{LiquidationSignal, STREAM_NAME, SUBJECT_SIGNALS_FILTER};
use vigil_liquidator::executor::{Executor, ExecutorConfig};
use vigil_liquidator::rpc::RpcTxChain;
use vigil_liquidator::signer::KeystoreSigner;
use vigil_liquidator::store::{PgIntentStore, PgTxLog};
use vigil_liquidator::strategy::{ImmediateStrike, Strategy, WaitForMargin};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum StrategyKind {
    Immediate,
    Margin,
}

#[derive(Parser, Debug)]
#[command(about = "Vigil Liquidator: strikes unhealthy positions with nonce-safe submission")]
struct Args {
    #[arg(long, env = "RPC_URL", default_value = "http://127.0.0.1:8545")]
    rpc_url: String,
    #[arg(long, env = "DEPLOYMENT_FILE")]
    deployment_file: String,
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
    #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,
    /// Encrypted keystore file. The password is read from KEYSTORE_PASSWORD, never from argv.
    #[arg(long, env = "KEYSTORE_PATH")]
    keystore: String,
    #[arg(long, value_enum, env = "STRATEGY", default_value = "margin")]
    strategy: StrategyKind,
    /// Required bonus coverage over gas cost, in bps (12000 = 1.2x).
    #[arg(long, env = "MARGIN_BPS", default_value_t = 12_000)]
    margin_bps: u64,
    /// Estimated gas cost of one liquidation, in debt-asset units (6 decimals).
    #[arg(long, env = "GAS_COST_DEBT", default_value_t = 1_000_000)]
    gas_cost_debt: u64,
    /// Blocks a transaction may stay unconfirmed before a fee-bumped replacement (RS-14).
    #[arg(long, env = "REPLACE_AFTER_BLOCKS", default_value_t = 3)]
    replace_after_blocks: u64,
    #[arg(long, env = "POLL_MS", default_value_t = 300)]
    poll_ms: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    vigil_common::telemetry::init("liquidator");
    let args = Args::parse();
    let deployment = Deployment::load(&args.deployment_file)?;

    let password = std::env::var("KEYSTORE_PASSWORD")
        .map_err(|_| anyhow::anyhow!("KEYSTORE_PASSWORD is not set"))?;
    let signer = KeystoreSigner::load(&args.keystore, &password)?;
    tracing::info!(?signer, "signer loaded");

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&args.database_url)
        .await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;

    let chain = RpcTxChain::connect(&args.rpc_url, &signer, deployment.pool).await?;
    let strategy: Box<dyn Strategy> = match args.strategy {
        StrategyKind::Immediate => Box::new(ImmediateStrike),
        StrategyKind::Margin => Box::new(WaitForMargin {
            margin_bps: args.margin_bps,
        }),
    };
    let cfg = ExecutorConfig {
        pool: deployment.pool,
        replace_after_blocks: args.replace_after_blocks,
        gas_limit: 400_000,
        gas_cost_debt: alloy::primitives::U256::from(args.gas_cost_debt),
    };
    let mut executor = Executor::new(chain, PgIntentStore::new(pool.clone()), strategy, cfg);
    executor.log = Box::new(PgTxLog::new(pool));
    executor.reconcile().await?;

    let nats = async_nats::connect(&args.nats_url).await?;
    let js = jetstream::new(nats);
    let stream = js
        .get_or_create_stream(stream::Config {
            name: STREAM_NAME.to_string(),
            subjects: vec!["vigil.>".to_string()],
            ..Default::default()
        })
        .await?;
    let consumer = stream
        .get_or_create_consumer(
            "liquidator-latest",
            pull::Config {
                durable_name: Some("liquidator-latest".to_string()),
                ack_policy: AckPolicy::Explicit,
                // Only the newest signal per borrower matters. Older ones for the same borrower are
                // already stale, so they are skipped rather than worked through one by one.
                deliver_policy: DeliverPolicy::LastPerSubject,
                filter_subject: SUBJECT_SIGNALS_FILTER.to_string(),
                ..Default::default()
            },
        )
        .await?;
    let mut signals = consumer.messages().await?;
    let mut ticker = tokio::time::interval(Duration::from_millis(args.poll_ms));
    let mut last_block = 0u64;
    tracing::info!("liquidator running");

    loop {
        tokio::select! {
            next = signals.next() => {
                let Some(next) = next else { anyhow::bail!("signal stream closed") };
                let msg = match next {
                    Ok(m) => m,
                    Err(e) => { tracing::warn!(error = %e, "signal consumer error"); continue; }
                };
                let outcome = async {
                    let signal: LiquidationSignal = serde_json::from_slice(&msg.payload)?;
                    executor.handle_signal(&signal).await.map(|o| (signal, o))
                }.await;
                match outcome {
                    Ok((signal, o)) => {
                        tracing::info!(borrower = %signal.borrower, trace_id = %signal.trace_id, outcome = ?o, "signal handled");
                        msg.ack().await.map_err(|e| anyhow::anyhow!("ack failed: {e}"))?;
                    }
                    // Not acked, so JetStream redelivers. Redelivery is safe (see the in-flight guard).
                    Err(e) => tracing::error!(error = %e, "signal not handled, will be redelivered"),
                }
            }
            _ = ticker.tick() => {
                match executor.chain_block().await {
                    Ok(b) if b > last_block => {
                        last_block = b;
                        if let Err(e) = executor.tick().await {
                            tracing::error!(error = %e, "tick failed");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "block poll failed"),
                }
            }
        }
    }
}
