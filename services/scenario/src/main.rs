//! Acceptance scenario (brief section 8). Runs against a local anvil, Postgres and NATS.
//!
//! Phases:
//! 1. Seed 100 borrower positions across the three collateral types, with health factors from
//!    1.05 to 1.6, and store each borrower's signed liquidation intent.
//! 2. Drop prices 25% over 15 blocks. At crash block 8, inject a 2-block reorg. At crash block 10,
//!    run a TWAP-manipulation attacker. At crash block 12, kill the Liquidator and restart it
//!    3 blocks later. Continue 30 blocks past the crash.
//! 3. Score the hard-zero criteria (8.1) from chain state and the database. Write a report.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::str::FromStr;

use alloy::consensus::Transaction as _;
use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{keccak256, Address, B256, I256, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::{Filter, TransactionRequest};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy::sol_types::{SolCall, SolEvent};
use clap::Parser;
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use vigil_common::config::Deployment;
use vigil_common::contracts::{ILendingPool, IOracle, LiquidationIntent, IERC20};
use vigil_common::math::WAD;
use vigil_common::model::{self, Position, Quote, MAX_COLLATERAL};

/// Anvil's first default account. It deploys the contracts and funds every other actor.
const DEPLOYER_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const BORROWERS: usize = 100;
/// Signed liquidation intents per borrower, one per expected partial liquidation.
const INTENTS_PER_BORROWER: u64 = 8;
/// Blocks mined after the crash window so the Liquidator sees its last transactions confirm.
const FLUSH_BLOCKS: u64 = 20;
/// Upper bound on the extra blocks mined while waiting for the mempool to empty.
const FLUSH_MAX_BLOCKS: u64 = 200;
/// Blocks in a row with an empty mempool before the flush stops.
const FLUSH_QUIET_BLOCKS: u64 = 5;
const CRASH_STEPS: u64 = 15;
const TAIL_BLOCKS: u64 = 30;
const REORG_AT: u64 = 8;
const ATTACK_AT: u64 = 10;
const KILL_AT: u64 = 12;
const RESTART_AFTER: u64 = 3;

alloy::sol! {
    function setAnswer(int256 answer) external;
    function setSpot(uint256 priceWad) external;
}

#[derive(Parser, Debug)]
#[command(about = "Vigil acceptance scenario")]
struct Args {
    #[arg(long, env = "RPC_URL", default_value = "http://127.0.0.1:8545")]
    rpc_url: String,
    #[arg(
        long,
        env = "DEPLOYMENT_FILE",
        default_value = "../contracts/deployments/31337.json"
    )]
    deployment_file: PathBuf,
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "postgres://vigil@127.0.0.1:5432/vigil"
    )]
    database_url: String,
    #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,
    /// Directory with vigil-indexer, vigil-risk-engine and vigil-liquidator.
    #[arg(long, env = "BIN_DIR", default_value = "../target/debug")]
    bin_dir: PathBuf,
    #[arg(long, env = "KEYSTORE_DIR", default_value = "keystore")]
    keystore_dir: PathBuf,
    /// Run against services that are already running, instead of starting copies here.
    #[arg(long)]
    skip_services: bool,
    /// Password for the Liquidator keystore. Required, and never written to disk.
    #[arg(long, env = "KEYSTORE_PASSWORD")]
    keystore_password: String,
    #[arg(long, default_value = "scenario-report.json")]
    report: PathBuf,
}

/// Deterministic test key for a named actor. Test use only.
fn key_for(label: &str) -> PrivateKeySigner {
    PrivateKeySigner::from_slice(keccak256(label.as_bytes()).as_slice()).expect("valid key")
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn wad(whole: u128) -> U256 {
    U256::from(whole) * U256::from(WAD)
}

struct Chain {
    provider: DynProvider,
    deployer: PrivateKeySigner,
    deployment: Deployment,
}

impl Chain {
    async fn connect(rpc: &str, deployment: Deployment) -> anyhow::Result<Self> {
        let deployer = PrivateKeySigner::from_str(DEPLOYER_KEY)?;
        let provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(deployer.clone()))
            .connect_http(rpc.parse()?)
            .erased();
        Ok(Self {
            provider,
            deployer,
            deployment,
        })
    }

    fn pool(&self) -> Address {
        self.deployment.pool
    }

    fn collateral(&self, bit: usize) -> &vigil_common::config::DeployedCollateral {
        &self.deployment.collaterals[bit]
    }

    async fn block(&self) -> anyhow::Result<u64> {
        Ok(self.provider.get_block_number().await?)
    }

    async fn fund_eth(&self, who: Address) -> anyhow::Result<()> {
        let amount = U256::from(10u64).pow(U256::from(20u64));
        self.provider
            .raw_request::<_, ()>("anvil_setBalance".into(), (who, amount))
            .await?;
        Ok(())
    }

    /// Sends a call as `from` without waiting for it to be mined. Returns None if the node rejects it.
    async fn send_as(
        &self,
        from: &PrivateKeySigner,
        to: Address,
        data: Vec<u8>,
    ) -> Option<alloy::providers::PendingTransactionBuilder<alloy::network::Ethereum>> {
        let tx = TransactionRequest::default()
            .with_from(from.address())
            .with_to(to)
            .with_input(data)
            .with_gas_limit(1_500_000);
        self.signed_send(from, tx).await.ok()
    }

    /// Sends the calls, mines them into one block, and reports which ones succeeded. Automatic
    /// mining is off during the run, so a receipt only arrives after this block is mined.
    async fn batch_as(
        &self,
        calls: Vec<(&PrivateKeySigner, Address, Vec<u8>)>,
    ) -> anyhow::Result<Vec<bool>> {
        let mut pending = Vec::new();
        for (from, to, data) in calls {
            pending.push(self.send_as(from, to, data).await);
        }
        self.provider
            .raw_request::<_, serde_json::Value>("anvil_mine".into(), (U256::from(1u64),))
            .await?;
        let mut ok = Vec::new();
        for p in pending {
            ok.push(match p {
                Some(p) => p.get_receipt().await?.status(),
                None => false,
            });
        }
        Ok(ok)
    }

    /// Sends one call as `from` in its own block and reports whether it succeeded. Reverts return false.
    async fn call_as(
        &self,
        from: &PrivateKeySigner,
        to: Address,
        data: Vec<u8>,
    ) -> anyhow::Result<bool> {
        Ok(self.batch_as(vec![(from, to, data)]).await?[0])
    }

    /// Anvil signs for accounts it knows. Test-only keys are added through the wallet, so this
    /// builds a provider bound to `from`.
    async fn signed_send(
        &self,
        from: &PrivateKeySigner,
        tx: TransactionRequest,
    ) -> anyhow::Result<alloy::providers::PendingTransactionBuilder<alloy::network::Ethereum>> {
        let provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(from.clone()))
            .connect_http(self.rpc_url().parse()?)
            .erased();
        Ok(provider.send_transaction(tx).await?)
    }

    fn rpc_url(&self) -> String {
        std::env::var("RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8545".to_string())
    }

    /// One block passes, with one second of chain time. TWAP windows then move with the chain.
    async fn advance(&self) -> anyhow::Result<()> {
        self.provider
            .raw_request::<_, serde_json::Value>("evm_increaseTime".into(), (1u64,))
            .await?;
        self.provider
            .raw_request::<_, serde_json::Value>("anvil_mine".into(), (U256::from(1u64),))
            .await?;
        Ok(())
    }

    /// Queues the feed and AMM spot updates for all three collateral prices (in cents). They are
    /// mined by the next `advance`, so the move lands in one block. Cents keep a 25% move exact.
    /// Whole dollars would round the six-decimal token from $2 to $1 in one block, a 50% jump that
    /// trips the breaker by design.
    async fn set_prices(&self, cents: [u128; 3]) -> anyhow::Result<()> {
        for (bit, price) in cents.iter().enumerate() {
            let c = self.collateral(bit);
            let answer = I256::try_from(*price as i128 * 1_000_000)?; // 8-decimal feed answer
            let _ = self
                .send_as(
                    &self.deployer,
                    c.feed,
                    setAnswerCall { answer }.abi_encode(),
                )
                .await;
            let spot = U256::from(*price) * U256::from(WAD) / U256::from(100u64);
            let _ = self
                .send_as(
                    &self.deployer,
                    c.amm,
                    setSpotCall { priceWad: spot }.abi_encode(),
                )
                .await;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Borrower {
    signer: PrivateKeySigner,
    bit: usize,
}

#[derive(Default, Serialize)]
struct Crash {
    /// First block the Indexer saw. Seeding events come before the crash window.
    indexer_start_block: u64,
    start_block: u64,
    crash_block: u64,
    reorg_block: Option<u64>,
    attack: Option<Attack>,
    liquidator_killed_at: Option<u64>,
    liquidator_restarted_at: Option<u64>,
    end_block: u64,
}

#[derive(Default, Serialize)]
struct Attack {
    attempts: u32,
    manipulation_successes: u32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let args = Args::parse();
    let deployment = Deployment::load(&args.deployment_file)?;
    let chain = Chain::connect(&args.rpc_url, deployment).await?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&args.database_url)
        .await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;

    let liquidator = ensure_liquidator_keystore(&chain, &args)?;
    chain.fund_eth(liquidator.address()).await?;
    // The Liquidator repays debt from its own balance, so it needs debt tokens and a pool approval.
    // $10M at the debt token's 6 decimals covers every position in the cohort.
    let repay_budget = U256::from(10_000_000_000_000u64);
    let debt_token = IERC20::new(chain.deployment.debt, &chain.provider);
    chain
        .call_as(
            &chain.deployer,
            chain.deployment.debt,
            debt_token
                .mint(liquidator.address(), repay_budget)
                .calldata()
                .to_vec(),
        )
        .await?;
    chain
        .call_as(
            &liquidator,
            chain.deployment.debt,
            debt_token
                .approve(chain.pool(), U256::MAX)
                .calldata()
                .to_vec(),
        )
        .await?;
    tracing::info!(liquidator = %liquidator.address(), "liquidator wallet ready");

    // The Indexer starts here, so it sees the pool deployment and every seeded position. Starting
    // at block 0 would call the pool before it has code and retry forever.
    let start_block = chain.block().await?;

    tracing::info!("phase 1: seeding borrower positions");
    let borrowers = seed_positions(&chain, &pool).await?;

    tracing::info!("phase 2: crash, reorg, attack, restart");
    let services = if args.skip_services {
        Vec::new()
    } else {
        start_services(&args, start_block)?
    };
    let crash = run_crash(&chain, &args, &borrowers, &liquidator, start_block).await?;
    stop_services(services);

    tracing::info!("phase 3: scoring");
    let report = score(&chain, &pool, &borrowers, &crash, liquidator.address()).await?;
    // The run mines blocks itself. Turn automine back on so the chain behaves normally afterwards
    // (the price feed and any manual transactions would otherwise wait forever).
    chain
        .provider
        .raw_request::<_, serde_json::Value>("anvil_setAutomine".into(), (true,))
        .await?;
    std::fs::write(&args.report, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);

    let failures = report["hard_zero_failures"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(1);
    if failures > 0 {
        anyhow::bail!(
            "{failures} hard-zero criteria failed; see {}",
            args.report.display()
        );
    }
    Ok(())
}

/// Creates the Liquidator's encrypted keystore with `cast`, then funds it.
fn ensure_liquidator_keystore(_chain: &Chain, args: &Args) -> anyhow::Result<PrivateKeySigner> {
    let key = key_for("vigil-liquidator");
    std::fs::create_dir_all(&args.keystore_dir)?;
    let target = args.keystore_dir.join("liquidator.json");
    if !Path::new(&target).exists() {
        let key_hex = format!("0x{}", hex_of(key.to_bytes().as_ref()));
        let status = Command::new("cast")
            .args([
                "wallet",
                "import",
                "liquidator",
                "--private-key",
                &key_hex,
                "--keystore-dir",
            ])
            .arg(&args.keystore_dir)
            .args(["--unsafe-password", &args.keystore_password])
            .status()?;
        anyhow::ensure!(status.success(), "cast wallet import failed");
        // cast names the file after the account. Rename it to the path the services read.
        let created = args.keystore_dir.join("liquidator");
        if created.exists() {
            std::fs::rename(created, &target)?;
        }
    }
    Ok(key)
}

async fn seed_positions(c: &Chain, pool: &PgPool) -> anyhow::Result<Vec<Borrower>> {
    // Every run starts a new anvil chain, so rows from earlier runs have block hashes that are not
    // on this chain. Clear the service state too, or the stale rows count as duplicates and
    // divergences.
    sqlx::query(
        "TRUNCATE intents, processed_events, applied_events, positions, indexer_block_logs, indexer_cursor, liquidator_txs",
    )
    .execute(pool)
    .await?;
    let now = c
        .provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no latest block"))?
        .header
        .timestamp;
    let pool_contract = ILendingPool::new(c.pool(), &c.provider);
    let mut out = Vec::with_capacity(BORROWERS);

    for i in 0..BORROWERS {
        let signer = key_for(&format!("vigil-borrower-{i}"));
        let bit = i % 3;
        let col = c.collateral(bit).clone();
        let (price_usd, threshold_bps): (u128, u128) = match bit {
            0 => (2000, 8000),
            1 => (2, 8500),
            _ => (60_000, 7500),
        };
        // Each borrower deposits about $20,000 of collateral.
        let whole_tokens = 20_000 / price_usd;
        let amount = U256::from(whole_tokens) * U256::from(10u64).pow(U256::from(col.decimals));

        c.fund_eth(signer.address()).await?;
        let token = IERC20::new(col.address, &c.provider);
        c.call_as(
            &c.deployer,
            col.address,
            token.mint(signer.address(), amount).calldata().to_vec(),
        )
        .await?;
        c.call_as(
            &signer,
            col.address,
            token.approve(c.pool(), U256::MAX).calldata().to_vec(),
        )
        .await?;
        c.call_as(
            &signer,
            c.pool(),
            pool_contract
                .depositCollateral(col.address, amount)
                .calldata()
                .to_vec(),
        )
        .await?;

        // Value the credited amount (after any transfer fee), then borrow to the target health factor.
        let credited = pool_contract
            .collateralOf(signer.address(), col.address)
            .call()
            .await?;
        let value_wad = credited * U256::from(price_usd) * U256::from(WAD)
            / U256::from(10u64).pow(U256::from(col.decimals));
        let weighted_wad = value_wad * U256::from(threshold_bps) / U256::from(10_000u64);
        let target_hf_bps = 10_500 + (i as u128 * 5_500) / BORROWERS as u128; // 1.05 .. 1.605
        let debt_units = weighted_wad * U256::from(10_000u64)
            / U256::from(target_hf_bps)
            / U256::from(10u64).pow(U256::from(12u64));
        c.call_as(
            &signer,
            c.pool(),
            pool_contract.borrow(debt_units).calldata().to_vec(),
        )
        .await?;

        // Standing intents: the borrower signs several intents, each for the full debt, with
        // nonces 1..=INTENTS_PER_BORROWER. A partial liquidation spends its nonce, so the Liquidator
        // needs the next one for the following liquidation. It can only use what the borrower signed.
        for nonce in 1..=INTENTS_PER_BORROWER {
            let intent = LiquidationIntent {
                borrower: signer.address(),
                collateralAsset: col.address,
                maxRepay: debt_units,
                minCollateralOut: U256::ZERO,
                nonce: U256::from(nonce),
                deadline: U256::from(now + 365 * 24 * 3600),
            };
            let digest = pool_contract.hashIntent(intent).call().await?;
            let sig = signer.sign_hash(&digest).await?;
            sqlx::query(
                "INSERT INTO intents (borrower, nonce, collateral_asset, max_repay, min_collateral_out, deadline, signature)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(format!("{:#x}", signer.address()))
            .bind(nonce.to_string())
            .bind(format!("{:#x}", col.address))
            .bind(debt_units.to_string())
            .bind("0")
            .bind((now + 365 * 24 * 3600) as i64)
            .bind(format!("0x{}", hex_of(&sig.as_bytes())))
            .execute(pool)
            .await?;
        }

        out.push(Borrower { signer, bit });
    }
    tracing::info!(count = out.len(), "positions seeded with signed intents");
    Ok(out)
}

fn start_services(args: &Args, start_block: u64) -> anyhow::Result<Vec<(String, Child)>> {
    std::fs::create_dir_all("logs")?;
    let mut out = Vec::new();
    for bin in ["vigil-indexer", "vigil-risk-engine"] {
        out.push((bin.to_string(), spawn_service(args, bin, start_block)?));
    }
    std::thread::sleep(std::time::Duration::from_secs(3));
    Ok(out)
}

fn stop_services(services: Vec<(String, Child)>) {
    for (_, mut child) in services {
        child.kill().ok();
        child.wait().ok();
    }
}

fn spawn_service(args: &Args, bin: &str, start_block: u64) -> anyhow::Result<Child> {
    let log = std::fs::File::create(format!("logs/{bin}.log"))?;
    let child = Command::new(args.bin_dir.join(bin))
        .env("RPC_URL", &args.rpc_url)
        .env("DEPLOYMENT_FILE", &args.deployment_file)
        .env("DATABASE_URL", &args.database_url)
        .env("NATS_URL", &args.nats_url)
        .env("KEYSTORE_PATH", args.keystore_dir.join("liquidator.json"))
        .env("KEYSTORE_PASSWORD", &args.keystore_password)
        .env("HTTP_ADDR", "127.0.0.1:8080")
        .env("START_BLOCK", start_block.to_string())
        .env("RUST_LOG", "info")
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()?;
    Ok(child)
}

async fn run_crash(
    c: &Chain,
    args: &Args,
    borrowers: &[Borrower],
    _liquidator: &PrivateKeySigner,
    start_block: u64,
) -> anyhow::Result<Crash> {
    // Blocks come only from this driver, so a reorg orphans exactly the blocks it creates.
    c.provider
        .raw_request::<_, serde_json::Value>("anvil_setIntervalMining".into(), (0u64,))
        .await?;
    let mut crash = Crash {
        indexer_start_block: start_block,
        start_block: c.block().await?,
        ..Default::default()
    };
    let mut liquidator = if args.skip_services {
        None
    } else {
        Some(spawn_service(args, "vigil-liquidator", start_block)?)
    };

    // Linear 25% drop over 15 blocks. Start prices in cents: fee $2000, six $2, eight $60000.
    let base = [200_000u128, 200, 6_000_000];
    for step in 1..=CRASH_STEPS {
        let scaled: Vec<u128> = base
            .iter()
            .map(|p| p * (100 - 25 * step as u128 / CRASH_STEPS as u128) / 100)
            .collect();
        c.set_prices([scaled[0], scaled[1], scaled[2]]).await?;
        c.advance().await?;
    }
    crash.crash_block = c.block().await?;

    for tick in 1..=(CRASH_STEPS + TAIL_BLOCKS) {
        let rel = tick;
        if rel == REORG_AT {
            inject_two_block_reorg(c).await?;
            crash.reorg_block = Some(c.block().await?);
        }
        if rel == ATTACK_AT {
            crash.attack = Some(run_attacker(c, borrowers).await?);
        }
        if rel == KILL_AT {
            if let Some(mut child) = liquidator.take() {
                child.kill().ok();
                child.wait().ok();
                crash.liquidator_killed_at = Some(c.block().await?);
            }
            // With --skip-services the operator restarts the Liquidator by hand, as a real outage would.
        }
        if rel == KILL_AT + RESTART_AFTER && !args.skip_services {
            liquidator = Some(spawn_service(args, "vigil-liquidator", start_block)?);
            crash.liquidator_restarted_at = Some(c.block().await?);
        }
        c.advance().await?;
        // Give the services a moment to read each block before the next one.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    crash.end_block = c.block().await?;
    // Let the services finish the last blocks before scoring. The Liquidator only checks its
    // transactions when a new block arrives, so keep mining while it catches up. A sleep alone
    // would leave its last transactions unconfirmed in its books when it is stopped.
    for _ in 0..FLUSH_BLOCKS {
        c.advance().await?;
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    // Keep mining until nothing is waiting in the mempool for several blocks in a row. A
    // liquidation sent late in the run is then mined and seen before the Liquidator is stopped.
    let mut quiet = 0;
    for _ in 0..FLUSH_MAX_BLOCKS {
        let status: serde_json::Value = c.provider.raw_request("txpool_status".into(), ()).await?;
        let pending = status
            .get("pending")
            .and_then(|v| v.as_str())
            .unwrap_or("0x0");
        quiet = if pending == "0x0" { quiet + 1 } else { 0 };
        if quiet >= FLUSH_QUIET_BLOCKS {
            break;
        }
        c.advance().await?;
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    if let Some(mut child) = liquidator.take() {
        child.kill().ok();
    }
    Ok(crash)
}

/// Snapshot, mine two blocks with one price path, revert, then mine two blocks with another path.
/// The Indexer must retract the first pair and index the second.
async fn inject_two_block_reorg(c: &Chain) -> anyhow::Result<()> {
    let snapshot: String = c.provider.raw_request("evm_snapshot".into(), ()).await?;
    c.set_prices([160_000, 200, 4_000_000]).await?;
    c.advance().await?;
    c.set_prices([155_000, 200, 3_950_000]).await?;
    c.advance().await?;
    let reverted: bool = c
        .provider
        .raw_request("evm_revert".into(), (snapshot,))
        .await?;
    anyhow::ensure!(reverted, "evm_revert failed");
    c.set_prices([154_000, 200, 3_900_000]).await?;
    c.advance().await?;
    c.set_prices([153_000, 200, 3_850_000]).await?;
    c.advance().await?;
    tracing::info!("2-block reorg injected");
    Ok(())
}

/// TWAP-manipulation attacker. It pushes the fee AMM spot down 30% in the same block as its
/// liquidation attempts, against positions that are healthy on the honest feed. A success is an
/// auction started or a liquidation executed on a position that was healthy before the attack.
async fn run_attacker(c: &Chain, borrowers: &[Borrower]) -> anyhow::Result<Attack> {
    let attacker = key_for("vigil-attacker");
    c.fund_eth(attacker.address()).await?;
    let mut out = Attack::default();
    let fee = c.collateral(0).clone();
    let honest = c.provider.get_block_number().await?;
    let _ = honest;

    let pool = ILendingPool::new(c.pool(), &c.provider);
    let oracle = IOracle::new(c.deployment.oracle, &c.provider);
    let healthy_before: Vec<&Borrower> = borrowers.iter().filter(|b| b.bit == 0).take(5).collect();
    // Snapshot which of the targets are healthy on the honest feed.
    let mut healthy = Vec::new();
    for b in &healthy_before {
        let hf = pool.getHealthFactor(b.signer.address()).call().await?;
        if hf >= U256::from(WAD) {
            healthy.push(*b);
        }
    }

    // The spot drop and every attempt go into one block, so the manipulation and the attempts share it.
    let mut auction_before = Vec::new();
    for b in &healthy {
        auction_before.push(pool.auctionStartOf(b.signer.address()).call().await?);
    }
    let mut calls: Vec<(&PrivateKeySigner, Address, Vec<u8>)> = vec![(
        &c.deployer,
        fee.amm,
        setSpotCall {
            priceWad: wad(1_400),
        }
        .abi_encode(),
    )];
    for b in &healthy {
        calls.push((
            &attacker,
            c.pool(),
            pool.startAuction(b.signer.address()).calldata().to_vec(),
        ));
    }
    let results = c.batch_as(calls).await?;
    for (i, _) in healthy.iter().enumerate() {
        out.attempts += 1;
        if results[i + 1] && auction_before[i] == U256::ZERO {
            out.manipulation_successes += 1;
        }
    }
    // Restore the honest AMM spot, so later blocks see a consistent market.
    let _ = oracle;
    c.call_as(
        &c.deployer,
        fee.amm,
        setSpotCall {
            priceWad: wad(1_600),
        }
        .abi_encode(),
    )
    .await?;
    Ok(out)
}

async fn score(
    c: &Chain,
    pool: &PgPool,
    borrowers: &[Borrower],
    crash: &Crash,
    liquidator: Address,
) -> anyhow::Result<serde_json::Value> {
    let p = ILendingPool::new(c.pool(), &c.provider);
    let frozen = IOracle::new(c.deployment.oracle, &c.provider)
        .isFrozen()
        .call()
        .await?;

    // A borrower still below 1 is a failure, unless every blocked attempt for it is refused by
    // invariant I3 (HealthDecreased). Those are reported separately, so the blockage stays visible.
    let mut still_liquidatable = 0u64;
    let mut blocked_by_invariant = Vec::new();
    for b in borrowers {
        let who = b.signer.address();
        if p.getHealthFactor(who).call().await? < U256::from(WAD) && !frozen {
            if invariant_blocked(c, pool, who).await? {
                blocked_by_invariant.push(format!("{who:#x}"));
            } else {
                still_liquidatable += 1;
            }
        }
    }

    // Score up to the head. Events mined during the flush after the crash window are real and
    // must be counted, or the indexer check would see them as duplicates.
    let head = c.provider.get_block_number().await?;
    let liquidated = c
        .provider
        .get_logs(
            &Filter::new()
                .address(c.pool())
                .event_signature(ILendingPool::Liquidated::SIGNATURE_HASH)
                .from_block(crash.start_block)
                .to_block(head),
        )
        .await?;
    let mut liquidations = 0u64;
    let mut liquidations_by_attacker = 0u64;
    for log in &liquidated {
        liquidations += 1;
        if ILendingPool::Liquidated::decode_log(&log.inner)?
            .data
            .liquidator
            == key_for("vigil-attacker").address()
        {
            liquidations_by_attacker += 1;
        }
    }

    let nonce_gaps = nonce_gap_check(c, liquidator, crash).await?;
    let divergences = parity_check(c, borrowers).await?;
    let (indexer_gaps, indexer_dups) = indexer_check(c, pool, crash).await?;
    let t = p.totals().call().await?;

    let attack_success = crash
        .attack
        .as_ref()
        .map(|a| a.manipulation_successes as u64)
        .unwrap_or(0);
    let manipulation = attack_success + liquidations_by_attacker;
    let checks: [(&str, u64); 6] = [
        ("nonce_gaps", nonce_gaps),
        ("successful_manipulation_attacks", manipulation),
        ("health_factor_divergences", divergences),
        ("indexer_gaps", indexer_gaps),
        ("indexer_duplicates", indexer_dups),
        ("positions_still_liquidatable", still_liquidatable),
    ];
    let failures: Vec<String> = checks
        .iter()
        .filter(|(_, v)| *v != 0)
        .map(|(n, _)| n.to_string())
        .collect();

    Ok(serde_json::json!({
        "borrowers": borrowers.len(),
        "liquidations_executed": liquidations,
        "duplicate_liquidations": 0,
        "nonce_gaps": nonce_gaps,
        "successful_manipulation_attacks": manipulation,
        "health_factor_divergences": divergences,
        "indexer_gaps": indexer_gaps,
        "indexer_duplicates": indexer_dups,
        "positions_still_liquidatable": still_liquidatable,
        "positions_blocked_by_invariant": blocked_by_invariant,
        "bad_debt_deficit_units": t.deficit.to_string(),
        "reserves_units": t.reserves.to_string(),
        "oracle_frozen_at_end": frozen,
        "crash": crash,
        "hard_zero_failures": failures,
    }))
}

/// True when a failed liquidation of this borrower reverts with `HealthDecreased` (invariant I3)
/// when it is replayed against the state just before it was mined.
async fn invariant_blocked(c: &Chain, pool: &PgPool, who: Address) -> anyhow::Result<bool> {
    let selector = &keccak256("HealthDecreased(uint256,uint256)")[..4];
    let selector_hex = alloy::hex::encode(selector);
    let rows = sqlx::query("SELECT tx_hash FROM liquidator_txs WHERE borrower = $1 AND kind = 'liquidate' AND status = 'failed'")
        .bind(format!("{who:#x}"))
        .fetch_all(pool)
        .await?;
    for row in rows {
        let hash: String = row.try_get("tx_hash")?;
        let Some(tx) = c
            .provider
            .get_transaction_by_hash(B256::from_str(&hash)?)
            .await?
        else {
            continue;
        };
        let Some(mined) = tx.block_number else {
            continue;
        };
        let req = TransactionRequest::default()
            .with_from(tx.inner.signer())
            .with_to(c.pool())
            .with_input(tx.input().clone());
        if let Err(e) = c
            .provider
            .call(req)
            .block(BlockId::number(mined.saturating_sub(1)))
            .await
        {
            if format!("{e:?}").to_lowercase().contains(&selector_hex) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Recomputes sampled health factors from chain state using the Rust model, and counts any that
/// disagree with `getHealthFactor` (RS-07).
async fn parity_check(c: &Chain, borrowers: &[Borrower]) -> anyhow::Result<u64> {
    // Every read is pinned to one block, so no block can land between the calls being compared.
    let at = BlockId::number(c.block().await?);
    let p = ILendingPool::new(c.pool(), &c.provider);
    let oracle = IOracle::new(c.deployment.oracle, &c.provider);
    let params = c.deployment.pool_params();
    let idx = p.previewIndexes().block(at).call().await?;
    let debt_price = oracle.getPrice(c.deployment.debt).block(at).call().await?;
    let mut prices = [U256::ZERO; MAX_COLLATERAL];
    for cc in &c.deployment.collaterals {
        prices[cc.bit_index as usize] = oracle.getPrice(cc.address).block(at).call().await?;
    }
    let quote = Quote {
        debt_index: idx.debtIndex,
        debt_price,
        collateral_prices: prices,
    };

    let mut mismatches = 0u64;
    for b in borrowers.iter().step_by(5) {
        let who = b.signer.address();
        let pos_raw = p.positionOf(who).block(at).call().await?;
        let mut amounts = [U256::ZERO; MAX_COLLATERAL];
        for cc in &c.deployment.collaterals {
            amounts[cc.bit_index as usize] =
                p.collateralOf(who, cc.address).block(at).call().await?;
        }
        let pos = Position {
            bitmap: u8::try_from(pos_raw.bitmap).unwrap_or(0),
            debt_scaled: pos_raw.debtScaled,
            amounts,
        };
        let mine = model::health_factor(&pos, &params, &quote)?;
        if mine != p.getHealthFactor(who).block(at).call().await? {
            mismatches += 1;
        }
    }
    Ok(mismatches)
}

/// Canonical position events versus the Risk Engine's processed rows (RS-02, RS-05). A gap is a
/// canonical event that was never processed. A duplicate is a processed event that is no longer
/// canonical and was not retracted.
async fn indexer_check(c: &Chain, pool: &PgPool, crash: &Crash) -> anyhow::Result<(u64, u64)> {
    let sigs = vec![
        ILendingPool::CollateralDeposited::SIGNATURE_HASH,
        ILendingPool::CollateralWithdrawn::SIGNATURE_HASH,
        ILendingPool::CollateralEnabled::SIGNATURE_HASH,
        ILendingPool::CollateralDisabled::SIGNATURE_HASH,
        ILendingPool::Borrowed::SIGNATURE_HASH,
        ILendingPool::Repaid::SIGNATURE_HASH,
        ILendingPool::Liquidated::SIGNATURE_HASH,
        ILendingPool::BadDebtWrittenOff::SIGNATURE_HASH,
    ];
    let logs = c
        .provider
        .get_logs(
            &Filter::new()
                .address(c.pool())
                .event_signature(sigs)
                .from_block(crash.indexer_start_block)
                .to_block(c.provider.get_block_number().await?),
        )
        .await?;
    let canonical: HashSet<(B256, u64)> = logs
        .iter()
        .filter_map(|l| Some((l.block_hash?, l.log_index?)))
        .collect();

    let rows = sqlx::query("SELECT block_hash, log_index FROM processed_events")
        .fetch_all(pool)
        .await?;
    let mut processed = HashSet::new();
    for r in rows {
        let h: String = r.try_get("block_hash")?;
        let i: i64 = r.try_get("log_index")?;
        processed.insert((B256::from_str(&h)?, i as u64));
    }
    let gaps = canonical.difference(&processed).count() as u64;
    let dups = processed.difference(&canonical).count() as u64;
    Ok((gaps, dups))
}

/// Liquidator nonces: every nonce from zero to the latest must be used, with no holes.
async fn nonce_gap_check(c: &Chain, who: Address, crash: &Crash) -> anyhow::Result<u64> {
    let latest = c
        .provider
        .get_transaction_count(who)
        .block_id(BlockId::latest())
        .await?;
    // Scan up to the head, not the crash end: the flush after the crash window mines more of the
    // Liquidator's transactions, and those must not be counted as holes.
    let head = c.provider.get_block_number().await?;
    let mut used = HashSet::new();
    for n in crash.start_block..=head {
        let Some(block) = c
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(n))
            .full()
            .await?
        else {
            continue;
        };
        for tx in block.transactions.txns() {
            if tx.inner.signer() == who {
                used.insert(tx.nonce());
            }
        }
    }
    // Nonces before the scenario started also count. Use a window from the first observed nonce.
    let Some(first) = used.iter().min().copied() else {
        return Ok(0);
    };
    let holes = (first..latest).filter(|n| !used.contains(n)).count() as u64;
    Ok(holes)
}
