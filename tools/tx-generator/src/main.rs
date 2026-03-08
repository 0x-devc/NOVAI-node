//! Transaction Generator for NOVAI Load Testing
//!
//! Generates valid signed transactions at a configurable rate and submits them
//! to a NOVAI node for mempool ingestion and block inclusion.
//!
//! USAGE:
//!   tx-generator --tps 100 --senders 10 --duration 60 --endpoint http://localhost:3030

mod generator;
mod metrics;
mod sender;
mod submitter;

use anyhow::{Context, Result};
use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};

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
    info!("Workers: {}", args.workers);
    info!("Confirmation Tracking: {}", args.track_confirmations);

    // 1. Create sender pool
    let sender_pool = Arc::new(sender::SenderPool::new(args.senders));
    info!("Initialized {} sender accounts", sender_pool.len());

    // 2. Create channels
    let channel_capacity = (args.tps * 2) as usize;
    let (tx_sender, tx_receiver) = mpsc::channel(channel_capacity);
    let (metric_tx, metric_rx) = metrics::metric_channel();

    info!(
        "Created transaction channel (capacity: {})",
        channel_capacity
    );

    // 3. Start metrics collector
    let metrics_collector = metrics::MetricsCollector::new(args.track_confirmations);
    let metrics_handle = metrics_collector.start(metric_rx);
    info!("Started metrics collector");

    // 4. Start submitter workers
    let submitter_config = submitter::SubmitterConfig {
        endpoint: args.endpoint.clone(),
        worker_count: args.workers,
        track_confirmations: args.track_confirmations,
        ..Default::default()
    };
    let submitter = submitter::Submitter::new(submitter_config);
    let submitter_handle = submitter.start(tx_receiver, metric_tx);
    info!("Started {} submitter workers", args.workers);

    // 5. Start generator
    let generator_config = generator::GeneratorConfig {
        target_tps: args.tps,
        tx_type: generator::TxType::from_str(&args.tx_type).context("Invalid transaction type")?,
        max_duration: if args.continuous || args.duration == 0 {
            None
        } else {
            Some(Duration::from_secs(args.duration))
        },
        ..Default::default()
    };
    let generator = generator::Generator::new(generator_config, sender_pool);
    let generator_handle = generator.start(tx_sender);
    info!("Started transaction generator");

    info!("=== Load test running ===");

    // 6. Wait for generator to complete (duration or Ctrl+C)
    let generator_stats = tokio::select! {
        stats = generator_handle.wait() => {
            info!("Generator completed normally");
            stats
        }
        _ = tokio::signal::ctrl_c() => {
            warn!("Received Ctrl+C, aborting generator...");
            // Note: generator_handle is consumed, so we can't call shutdown
            // It will be aborted when dropped
            generator::GeneratorStats::default()
        }
    };

    info!(
        "Generator stats: generated={}, dropped={}, runtime={}ms, actual_tps={:.2}",
        generator_stats.generated_count,
        generator_stats.dropped_count,
        generator_stats.runtime_ms,
        generator_stats.actual_tps
    );

    // 7. Shutdown submitter and wait for pending submissions
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

    // 8. Get final metrics and wait for collector
    info!("Collecting final metrics...");
    let final_metrics = metrics_handle.snapshot().await;
    metrics_handle.wait().await;

    // 9. Output results
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
            output: "text".to_string(),
            verbose: false,
            workers: 4,
            track_confirmations: false,
            continuous: false,
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
            output: "text".to_string(),
            verbose: false,
            workers: 4,
            track_confirmations: false,
            continuous: false,
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
            output: "text".to_string(),
            verbose: false,
            workers: 4,
            track_confirmations: false,
            continuous: false,
        };
        assert!(validate_args(&args).is_err());
    }
}
