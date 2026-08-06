//! Transaction Generator for NOVAI Load Testing
//!
//! Generates valid signed transactions at a configurable rate and submits them
//! to a NOVAI node for mempool ingestion and block inclusion.
//!
//! USAGE:
//!   tx-generator --tps 100 --senders 10 --duration 60 --endpoint http://localhost:3030

use anyhow::{Context, Result};
use clap::Parser;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};
use tx_generator::{generator, metrics, sender, submitter};

/// Transaction generator for load testing NOVAI nodes.
#[derive(Parser, Debug)]
#[command(name = "tx-generator")]
#[command(about = "Generate load transactions for NOVAI testnet", long_about = None)]
struct Args {
    /// Target transactions per second (TPS)
    #[arg(short, long, default_value_t = 100)]
    tps: u64,

    /// Number of sender accounts to use
    #[arg(short, long, default_value_t = 10)]
    senders: usize,

    /// Test duration in seconds (0 = run forever)
    #[arg(short, long, default_value_t = 60)]
    duration: u64,

    /// RPC endpoint URL
    #[arg(short, long, default_value = "http://localhost:3030")]
    endpoint: String,

    /// Transaction type: transfer, ai_register, ai_signal
    #[arg(long, default_value = "transfer")]
    tx_type: String,

    /// Fee per transaction (must exceed dynamic fee floor under congestion)
    #[arg(short, long, default_value_t = 1000)]
    fee: u64,

    /// Output format: text, json, csv
    #[arg(short, long, default_value = "text")]
    output: String,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Number of submitter worker threads
    #[arg(long, default_value_t = 4)]
    workers: usize,

    /// Enable confirmation tracking (polls for tx confirmation)
    #[arg(long)]
    track_confirmations: bool,

    /// Run continuously (equivalent to --duration 0). Overrides --duration.
    #[arg(long)]
    continuous: bool,

    /// Chain stall threshold in seconds. If the chain height does not
    /// advance for this duration the monitor logs a stall warning. Advisory
    /// only: it does not pause the generator. (It once did; the pause path
    /// was removed and this help text was left behind.)
    #[arg(long, default_value_t = 30)]
    stall_threshold_secs: u64,

    /// Chain monitor poll interval in seconds.
    #[arg(long, default_value_t = 5)]
    stall_poll_interval_secs: u64,

    /// Disable the chain progress monitor (pause only on MempoolFull).
    #[arg(long)]
    no_stall_monitor: bool,

    /// Period of the sender nonce reconciliation sweep, in seconds.
    ///
    /// Every sender's local nonce is compared against chain truth and
    /// corrected if it has drifted. Nonces are burned by every rejection and
    /// there is no safe rollback while workers claim concurrently, so drift
    /// is structural over a long run; this is what keeps a `--continuous`
    /// run healthy for days without operator action.
    #[arg(long, default_value_t = 60)]
    resync_interval_secs: u64,

    /// Disable the periodic nonce reconciliation sweep.
    #[arg(long)]
    no_resync_sweep: bool,

    /// Window in seconds over which the adaptive throttle judges the
    /// rejection rate before adjusting the offered rate.
    #[arg(long, default_value_t = 10)]
    throttle_window_secs: u64,

    /// Disable the adaptive throttle (always offer at full rate).
    #[arg(long)]
    no_throttle: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    init_tracing(args.verbose);

    // Validate parameters
    validate_args(&args)?;

    info!("=== Transaction Generator Starting ===");
    info!("Target TPS: {}", args.tps);
    info!("Senders: {}", args.senders);
    if args.continuous {
        info!("Duration: continuous (infinite)");
    } else {
        info!("Duration: {} seconds", args.duration);
    }
    info!("Endpoint: {}", args.endpoint);
    info!("Transaction Type: {}", args.tx_type);
    info!("Fee: {}", args.fee);
    info!("Workers: {}", args.workers);
    info!("Confirmation Tracking: {}", args.track_confirmations);

    // 1. Create sender pool
    let sender_pool = Arc::new(sender::SenderPool::new(args.senders));
    info!("Initialized {} sender accounts", sender_pool.len());

    // 1b. Resync every sender to its current on-chain nonce BEFORE any
    // load begins. Sender accounts are deterministic and long-lived, so
    // after prior runs their chain nonces are far above the 0 the pool
    // starts with; without this, every tx is rejected NonceTooLow.
    // Fails loud: no load starts if any sender's nonce is unknown.
    let resync_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("Failed to create HTTP client for startup nonce resync")?;
    submitter::resync_sender_nonces(&resync_client, &args.endpoint, &sender_pool)
        .await
        .context("startup nonce resync failed; not starting load with unknown nonces")?;

    // 2. Create channels (carries unsigned TxTemplates, not signed TxV1)
    let channel_capacity = (args.tps * 2) as usize;
    let (tx_sender, tx_receiver) = mpsc::channel(channel_capacity);
    let (metric_tx, metric_rx) = metrics::metric_channel();

    info!(
        "Created transaction channel (capacity: {})",
        channel_capacity
    );

    // 3. Shared pause flag: submitter sets true on MempoolFull, generator skips ticks
    let paused = Arc::new(AtomicBool::new(false));

    // 4. Start metrics collector
    let metrics_collector = metrics::MetricsCollector::new(args.track_confirmations);
    let metrics_handle = metrics_collector.start(metric_rx);
    info!("Started metrics collector");

    // 5. Start submitter workers (claims nonces and signs at submission time)
    let submitter_config = submitter::SubmitterConfig {
        endpoint: args.endpoint.clone(),
        worker_count: args.workers,
        track_confirmations: args.track_confirmations,
        ..Default::default()
    };
    let throttle = if args.no_throttle {
        info!("Adaptive throttle disabled (--no-throttle)");
        None
    } else {
        Some(Arc::new(tx_generator::throttle::Throttle::new()))
    };

    let mut submitter = submitter::Submitter::new(
        submitter_config,
        Arc::clone(&sender_pool),
        Arc::clone(&paused),
    );
    if let Some(t) = &throttle {
        submitter = submitter.with_throttle(Arc::clone(t));
    }
    let submitter_handle = submitter.start(tx_receiver, metric_tx);
    info!("Started {} submitter workers", args.workers);

    // 5b. Optionally start the chain progress monitor. It polls the
    // endpoint for chain height and engages the shared pause flag if
    // the chain stops advancing for `stall_threshold_secs`.
    let chain_monitor_handle = if args.no_stall_monitor {
        info!("Chain monitor disabled (--no-stall-monitor)");
        None
    } else {
        let monitor = submitter::ChainMonitor::new(
            args.endpoint.clone(),
            Duration::from_secs(args.stall_poll_interval_secs),
            Duration::from_secs(args.stall_threshold_secs),
        );
        Some(monitor.start())
    };

    // 5c. Start the periodic nonce reconciliation sweep. The reactive path
    // in the workers only fires after a run of rejections, and until the node
    // grew a nonce horizon the ahead-desync produced no rejection at all, so
    // a sender could sit wrong indefinitely. This sweep notices either
    // direction without waiting for the chain to complain.
    let resync_handle = if args.no_resync_sweep {
        info!("Nonce reconciliation sweep disabled (--no-resync-sweep)");
        None
    } else {
        let interval = Duration::from_secs(args.resync_interval_secs.max(1));
        let endpoint = args.endpoint.clone();
        let pool = Arc::clone(&sender_pool);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .context("failed to build reconciliation HTTP client")?;
        // Spread each sweep's queries across its own period so a large pool
        // trickles rather than bursting into the node's per-IP rate limit.
        let pace = interval / (pool.len().max(1) as u32);
        info!(
            interval_secs = args.resync_interval_secs,
            senders = pool.len(),
            pace_ms = pace.as_millis() as u64,
            "Started nonce reconciliation sweep"
        );
        Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                submitter::reconcile_sender_nonces(&client, &endpoint, &pool, pace).await;
            }
        }))
    };

    // 5d. Fold each window of submission outcomes into a throttle level.
    // Hysteresis in the controller keeps a steady rejection rate from
    // flapping the offered rate, and the level is capped so the generator
    // slows but never stops.
    let throttle_handle = throttle.as_ref().map(|t| {
        let t = Arc::clone(t);
        let window = Duration::from_secs(args.throttle_window_secs.max(1));
        info!(
            window_secs = args.throttle_window_secs,
            "Started adaptive throttle"
        );
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(window);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut previous = 0u32;
            loop {
                ticker.tick().await;
                let level = t.sample();
                if level != previous {
                    info!(
                        level,
                        multiplier = t.delay_multiplier(),
                        "Adaptive throttle changed"
                    );
                    previous = level;
                }
            }
        })
    });

    // 6. Start generator (produces unsigned templates)
    let generator_config = generator::GeneratorConfig {
        target_tps: args.tps,
        tx_type: generator::TxType::from_str(&args.tx_type).context("Invalid transaction type")?,
        fee: args.fee,
        max_duration: if args.continuous || args.duration == 0 {
            None
        } else {
            Some(Duration::from_secs(args.duration))
        },
    };
    let mut generator = generator::Generator::new(generator_config, sender_pool, paused);
    if let Some(t) = &throttle {
        generator = generator.with_throttle(Arc::clone(t));
    }
    let generator_handle = generator.start(tx_sender);
    info!("Started transaction generator");

    info!("=== Load test running ===");

    // 6b. Start periodic stats logger (every 60s in continuous mode)
    let stats_logger = if args.continuous || args.duration == 0 {
        let metrics_for_stats = metrics_handle.clone_state();
        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.tick().await; // skip first immediate tick
            loop {
                interval.tick().await;
                let snap = metrics_for_stats.read().await;
                let elapsed = snap.elapsed_secs();
                let accepted = snap.accepted_count();
                let rejected = snap.rejected_count();
                let tps = if elapsed > 0.0 {
                    accepted as f64 / elapsed
                } else {
                    0.0
                };
                info!(
                    accepted,
                    rejected,
                    tps = format!("{:.1}", tps),
                    elapsed_s = format!("{:.0}", elapsed),
                    "TXGEN_STATS"
                );
            }
        }))
    } else {
        None
    };

    // 7. Wait for generator to complete (duration or Ctrl+C)
    let generator_stats = tokio::select! {
        stats = generator_handle.wait() => {
            info!("Generator completed normally");
            stats
        }
        _ = tokio::signal::ctrl_c() => {
            warn!("Received Ctrl+C, aborting generator...");
            generator::GeneratorStats::default()
        }
    };

    // Stop periodic stats logger
    if let Some(handle) = stats_logger {
        handle.abort();
    }

    // Stop chain monitor
    if let Some(handle) = chain_monitor_handle {
        handle.abort();
    }

    // Stop the nonce reconciliation sweep
    if let Some(handle) = resync_handle {
        handle.abort();
    }

    // Stop the adaptive throttle sampler
    if let Some(handle) = throttle_handle {
        handle.abort();
    }

    info!(
        "Generator stats: generated={}, dropped={}, runtime={}ms, actual_tps={:.2}",
        generator_stats.generated_count,
        generator_stats.dropped_count,
        generator_stats.runtime_ms,
        generator_stats.actual_tps
    );

    // 8. Shutdown submitter and wait for pending submissions
    info!("Shutting down submitters, draining pending transactions...");
    submitter_handle.shutdown();
    let submitter_stats = submitter_handle.wait().await;

    info!(
        "Submitter stats: submitted={}, accepted={}, rejected={}, failed={}, retries={}",
        submitter_stats.total_submitted,
        submitter_stats.total_accepted,
        submitter_stats.total_rejected,
        submitter_stats.total_failed,
        submitter_stats.total_retries
    );

    // 9. Get final metrics and wait for collector
    info!("Collecting final metrics...");
    let final_metrics = metrics_handle.snapshot().await;
    metrics_handle.wait().await;

    // 10. Output results
    info!("=== Load test complete ===");
    match args.output.as_str() {
        "json" => println!("{}", final_metrics.to_json()),
        "csv" => println!("{}", final_metrics.to_csv(true)),
        _ => println!("{}", final_metrics.to_text()),
    }

    Ok(())
}

/// Initialize tracing subscriber for logging.
fn init_tracing(verbose: bool) {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = if verbose {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("tx_generator=debug,info"))
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("tx_generator=info"))
    };

    fmt().with_env_filter(filter).with_target(false).init();
}

/// Validate command-line arguments.
fn validate_args(args: &Args) -> Result<()> {
    if args.tps == 0 {
        anyhow::bail!("TPS must be greater than 0");
    }

    if args.senders == 0 {
        anyhow::bail!("Number of senders must be greater than 0");
    }

    if !["transfer", "ai_register", "ai_signal"].contains(&args.tx_type.as_str()) {
        anyhow::bail!("Invalid transaction type: {}", args.tx_type);
    }

    if !["text", "json", "csv"].contains(&args.output.as_str()) {
        anyhow::bail!("Invalid output format: {}", args.output);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_args_valid() {
        let args = Args {
            tps: 100,
            senders: 10,
            duration: 60,
            endpoint: "http://localhost:3030".to_string(),
            tx_type: "transfer".to_string(),
            fee: 1000,
            output: "text".to_string(),
            verbose: false,
            workers: 4,
            track_confirmations: false,
            continuous: false,
            stall_threshold_secs: 30,
            stall_poll_interval_secs: 5,
            no_stall_monitor: false,
            resync_interval_secs: 60,
            no_resync_sweep: false,
            throttle_window_secs: 10,
            no_throttle: false,
        };
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_zero_tps() {
        let args = Args {
            tps: 0,
            senders: 10,
            duration: 60,
            endpoint: "http://localhost:3030".to_string(),
            tx_type: "transfer".to_string(),
            fee: 1000,
            output: "text".to_string(),
            verbose: false,
            workers: 4,
            track_confirmations: false,
            continuous: false,
            stall_threshold_secs: 30,
            stall_poll_interval_secs: 5,
            no_stall_monitor: false,
            resync_interval_secs: 60,
            no_resync_sweep: false,
            throttle_window_secs: 10,
            no_throttle: false,
        };
        assert!(validate_args(&args).is_err());
    }

    #[test]
    fn test_validate_args_invalid_tx_type() {
        let args = Args {
            tps: 100,
            senders: 10,
            duration: 60,
            endpoint: "http://localhost:3030".to_string(),
            tx_type: "invalid".to_string(),
            fee: 1000,
            output: "text".to_string(),
            verbose: false,
            workers: 4,
            track_confirmations: false,
            continuous: false,
            stall_threshold_secs: 30,
            stall_poll_interval_secs: 5,
            no_stall_monitor: false,
            resync_interval_secs: 60,
            no_resync_sweep: false,
            throttle_window_secs: 10,
            no_throttle: false,
        };
        assert!(validate_args(&args).is_err());
    }
}
