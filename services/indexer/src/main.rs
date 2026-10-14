use std::time::Duration;

use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder};
use clap::Parser;
use sqlx::postgres::PgPoolOptions;
use vigil_common::config::Deployment;
use vigil_common::model::MAX_COLLATERAL;
use vigil_indexer::chain::RpcChain;
use vigil_indexer::engine::Indexer;
use vigil_indexer::sink::NatsSink;
use vigil_indexer::store::PgIndexerStore;

#[derive(Parser, Debug)]
#[command(about = "Vigil Indexer: publishes pool events, retractions and block state")]
struct Args {
    #[arg(long, env = "RPC_URL", default_value = "http://127.0.0.1:8545")]
    rpc_url: String,
    #[arg(long, env = "DEPLOYMENT_FILE")]
    deployment_file: String,
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
    #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,
    /// First block to index when no cursor exists.
    #[arg(long, env = "START_BLOCK", default_value_t = 0)]
    start_block: u64,
    #[arg(long, env = "POLL_MS", default_value_t = 200)]
    poll_ms: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    vigil_common::telemetry::init("indexer");
    let args = Args::parse();
    let deployment = Deployment::load(&args.deployment_file)?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&args.database_url)
        .await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;

    let nats = async_nats::connect(&args.nats_url).await?;
    let sink = NatsSink::connect(nats).await?;

    let provider = ProviderBuilder::new()
        .connect_http(args.rpc_url.parse()?)
        .erased();
    let mut by_bit: [Option<Address>; MAX_COLLATERAL] = [None; MAX_COLLATERAL];
    for c in &deployment.collaterals {
        by_bit[c.bit_index as usize] = Some(c.address);
    }
    let chain = RpcChain::new(
        provider,
        deployment.pool,
        deployment.oracle,
        deployment.debt,
        by_bit,
    );

    let store = PgIndexerStore::new(pool);
    let mut indexer = Indexer::resume(chain, sink, store, args.start_block).await?;
    tracing::info!(next = indexer.next, "indexer started");

    loop {
        match indexer.catch_up().await {
            Ok(0) => {}
            Ok(n) => tracing::info!(blocks = n, next = indexer.next, "indexed"),
            // A failed read is retried. A reorg deeper than the ring is fatal, so exit (fail closed).
            Err(e) if e.to_string().contains("deeper than") => return Err(e),
            Err(e) => tracing::warn!(error = %e, "indexer step failed, retrying"),
        }
        tokio::time::sleep(Duration::from_millis(args.poll_ms)).await;
    }
}
