//! Genesis Generator Tool for NOVAI Mainnet
//!
//! PURPOSE:
//! Deterministic genesis state generator. Given identical input config,
//! multiple independent parties MUST produce identical state roots.
//!
//! INVARIANTS:
//! - Same config JSON → same state root (byte-for-byte)
//! - Validator set sorted by address (lexicographic ascending)
//! - Genesis block: height=0, round=0, `parent_hash`=\[0;32\], empty txs
//!
//! FAILURE MODES:
//! - Invalid config JSON → `ValidationError` with details
//! - I/O errors → `IoError` with path
//! - State root mismatch in verify mode → exits with code 1
//!
//! USAGE:
//! ```text
//!   # Generate genesis state
//!   genesis-generator --config mainnet_config.json --output-dir ./genesis-out
//!
//!   # Verify against expected state root
//!   genesis-generator --config mainnet_config.json --verify <expected_hex>
//!
//!   # Print state root only (for piping)
//!   genesis-generator --config mainnet_config.json --state-root-only
//! ```

use anyhow::{Context, Result};
use clap::Parser;
use genesis::{GenesisConfig, GenesisGenerator, GenesisState};
use novai_consensus_types::codec::encode_block_v1;
use novai_state::MemKv;
use std::fs;
use std::path::PathBuf;
use tracing::{error, info};

/// Deterministic genesis state generator for NOVAI mainnet.
///
/// Generates genesis block, state root, and validator set from a config file.
/// Multiple parties running this tool with the same config MUST produce
/// identical state roots.
#[derive(Parser, Debug)]
#[command(name = "genesis-generator")]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to genesis config JSON file
    #[arg(short, long)]
    config: PathBuf,

    /// Output directory for generated files (genesis.json, genesis\_block.bin, etc.)
    #[arg(short, long)]
    output_dir: Option<PathBuf>,

    /// Verify mode: compare generated state root against expected value (64 hex chars)
    #[arg(long)]
    verify: Option<String>,

    /// Print only the state root hex (for scripting/piping)
    #[arg(long)]
    state_root_only: bool,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging (unless state-root-only mode)
    if !args.state_root_only {
        init_tracing(args.verbose);
    }

    // 1. Load and validate config
    if !args.state_root_only {
        info!("Loading config from: {}", args.config.display());
    }

    let config = GenesisConfig::from_file(&args.config)
        .with_context(|| format!("Failed to load config from {}", args.config.display()))?;

    if !args.state_root_only {
        info!("Config validated successfully");
        info!("  Chain ID: {}", config.chain_id);
        info!("  Protocol version: {}", config.protocol_version);
        info!("  Validators: {}", config.validators.len());
        info!("  Accounts: {}", config.accounts.len());
        info!("  AI entities: {}", config.ai_entities.len());
        info!("  Approval gates: {}", config.approval_gates.len());
    }

    // 2. Generate genesis state (deterministic)
    if !args.state_root_only {
        info!("Generating genesis state...");
    }

    let generator = GenesisGenerator::new(config.clone());
    let mut db = MemKv::new();
    let genesis_state = generator
        .generate(&mut db)
        .context("Failed to generate genesis state")?;

    let state_root_hex = hex::encode(genesis_state.state_root);

    // 3. Handle state-root-only mode
    if args.state_root_only {
        println!("{state_root_hex}");
        return Ok(());
    }

    info!("Genesis state generated successfully");
    info!("  State root: {state_root_hex}");
    info!("  Validators in set: {}", genesis_state.validator_set.len());

    // 4. Handle verify mode
    if let Some(expected) = &args.verify {
        return handle_verify_mode(&state_root_hex, expected);
    }

    // 5. Handle output mode
    if let Some(output_dir) = &args.output_dir {
        return handle_output_mode(output_dir, &config, &genesis_state, &state_root_hex);
    }

    // 6. Default: print summary
    print_summary(&config, &genesis_state, &state_root_hex);

    Ok(())
}

/// Initialize tracing subscriber for logging.
fn init_tracing(verbose: bool) {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = if verbose {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("genesis_generator=debug,info"))
    } else {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("genesis_generator=info"))
    };

    fmt().with_env_filter(filter).with_target(false).init();
}

/// Handle verify mode: compare state root against expected value.
#[allow(clippy::unnecessary_wraps)] // Consistent API with other handlers; exit(1) path never returns
fn handle_verify_mode(actual: &str, expected: &str) -> Result<()> {
    let expected_normalized = expected.to_lowercase();

    if actual == expected_normalized {
        info!("✅ VERIFICATION PASSED");
        info!("   State root matches expected value");
        Ok(())
    } else {
        error!("❌ VERIFICATION FAILED");
        error!("   Expected: {expected_normalized}");
        error!("   Actual:   {actual}");
        std::process::exit(1);
    }
}

/// Handle output mode: write files to output directory.
fn handle_output_mode(
    output_dir: &PathBuf,
    config: &GenesisConfig,
    genesis_state: &GenesisState,
    state_root_hex: &str,
) -> Result<()> {
    // Create output directory
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    info!("Writing genesis files to: {}", output_dir.display());

    // 1. Write canonical config (for reproducibility)
    let config_path = output_dir.join("genesis_config.json");
    let canonical_json = config
        .to_canonical_json()
        .context("Failed to serialize config")?;
    fs::write(&config_path, &canonical_json)
        .with_context(|| format!("Failed to write {}", config_path.display()))?;
    info!(
        "  Written: genesis_config.json ({} bytes)",
        canonical_json.len()
    );

    // 2. Write genesis block (binary)
    let block_path = output_dir.join("genesis_block.bin");
    let block_bytes = encode_block_v1(&genesis_state.genesis_block)
        .map_err(|e| anyhow::anyhow!("Failed to encode genesis block: {e:?}"))?;
    fs::write(&block_path, &block_bytes)
        .with_context(|| format!("Failed to write {}", block_path.display()))?;
    info!("  Written: genesis_block.bin ({} bytes)", block_bytes.len());

    // 3. Write state root (hex)
    let root_path = output_dir.join("state_root.hex");
    fs::write(&root_path, state_root_hex)
        .with_context(|| format!("Failed to write {}", root_path.display()))?;
    info!("  Written: state_root.hex");

    // 4. Write validator set (JSON)
    let validators_path = output_dir.join("validator_set.json");
    let validators_json: Vec<String> = genesis_state
        .validator_set
        .iter()
        .map(hex::encode)
        .collect();
    let validators_pretty = serde_json::to_string_pretty(&validators_json)
        .context("Failed to serialize validator set")?;
    fs::write(&validators_path, &validators_pretty)
        .with_context(|| format!("Failed to write {}", validators_path.display()))?;
    info!(
        "  Written: validator_set.json ({} validators)",
        genesis_state.validator_set.len()
    );

    // 5. Write summary (human-readable)
    let summary_path = output_dir.join("genesis_summary.txt");
    let summary = format_summary(config, genesis_state, state_root_hex);
    fs::write(&summary_path, &summary)
        .with_context(|| format!("Failed to write {}", summary_path.display()))?;
    info!("  Written: genesis_summary.txt");

    info!("✅ Genesis files written successfully");
    info!("");
    info!("To verify, another party should run:");
    info!(
        "  genesis-generator --config {} --verify {}",
        output_dir.join("genesis_config.json").display(),
        state_root_hex
    );

    Ok(())
}

/// Format summary for display or file output.
fn format_summary(
    config: &GenesisConfig,
    genesis_state: &GenesisState,
    state_root_hex: &str,
) -> String {
    let mut lines = Vec::new();

    lines.push("═══════════════════════════════════════════════════════════════".to_string());
    lines.push("NOVAI Genesis Summary".to_string());
    lines.push("═══════════════════════════════════════════════════════════════".to_string());
    lines.push(String::new());

    lines.push(format!("Chain ID:         {}", config.chain_id));
    lines.push(format!("Protocol Version: {}", config.protocol_version));
    lines.push(format!("Timestamp:        {}", config.timestamp));
    lines.push(String::new());

    lines.push("Genesis Block:".to_string());
    lines.push(format!(
        "  Height:      {}",
        genesis_state.genesis_block.height
    ));
    lines.push(format!(
        "  Round:       {}",
        genesis_state.genesis_block.round
    ));
    lines.push(format!(
        "  Parent Hash: {}",
        hex::encode(genesis_state.genesis_block.parent_hash)
    ));
    lines.push(format!("  State Root:  {state_root_hex}"));
    lines.push(format!(
        "  Transactions: {}",
        genesis_state.genesis_block.txs.len()
    ));
    lines.push(String::new());

    lines.push(format!("Validators: {} total", config.validators.len()));
    for (i, v) in config.validators.iter().enumerate() {
        let name = v.name.as_deref().unwrap_or("(unnamed)");
        lines.push(format!("  [{i}] {name}"));
        lines.push(format!("      Pubkey: {}", v.pubkey));
        lines.push(format!("      Stake:  {}", v.initial_stake));
    }
    lines.push(String::new());

    lines.push("Validator Set (sorted by address):".to_string());
    for (i, addr) in genesis_state.validator_set.iter().enumerate() {
        lines.push(format!("  [{i}] {}", hex::encode(addr)));
    }
    lines.push(String::new());

    lines.push(format!("Accounts: {} total", config.accounts.len()));
    let mut total_balance: u128 = 0;
    for (addr, balance) in &config.accounts {
        if let Ok(b) = balance.parse::<u64>() {
            total_balance = total_balance.saturating_add(u128::from(b));
        }
        lines.push(format!("  {addr}: {balance}"));
    }
    lines.push(format!("  Total: {total_balance}"));
    lines.push(String::new());

    if !config.ai_entities.is_empty() {
        lines.push(format!("AI Entities: {} total", config.ai_entities.len()));
        for (i, entity) in config.ai_entities.iter().enumerate() {
            lines.push(format!(
                "  [{i}] {} ({})",
                entity.name, entity.autonomy_mode
            ));
        }
        lines.push(String::new());
    }

    if !config.approval_gates.is_empty() {
        lines.push(format!(
            "Approval Gates: {} total",
            config.approval_gates.len()
        ));
        for (i, gate) in config.approval_gates.iter().enumerate() {
            lines.push(format!(
                "  [{i}] {} (threshold={}, timelock={})",
                gate.gate_type, gate.threshold, gate.timelock_blocks
            ));
        }
        lines.push(String::new());
    }

    lines.push("═══════════════════════════════════════════════════════════════".to_string());
    lines.push(format!("STATE ROOT: {state_root_hex}"));
    lines.push("═══════════════════════════════════════════════════════════════".to_string());
    lines.push(String::new());
    lines.push("Verification: Multiple parties must independently generate".to_string());
    lines.push("this state root from the same genesis_config.json to confirm".to_string());
    lines.push("determinism before mainnet launch.".to_string());

    lines.join("\n")
}

/// Print summary to stdout.
fn print_summary(config: &GenesisConfig, genesis_state: &GenesisState, state_root_hex: &str) {
    println!("{}", format_summary(config, genesis_state, state_root_hex));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_test_config() -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        let config = r#"{
            "chain_id": "novai-test",
            "protocol_version": 1,
            "timestamp": "2026-02-03T00:00:00Z",
            "validators": [
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000000",
                    "initial_stake": "1000000",
                    "name": "test-validator"
                }
            ],
            "accounts": {
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": "1000000000"
            },
            "ai_entities": [],
            "approval_gates": []
        }"#;
        file.write_all(config.as_bytes()).unwrap();
        file
    }

    #[test]
    fn test_deterministic_generation() {
        let config_file = create_test_config();
        let config = GenesisConfig::from_file(config_file.path()).unwrap();

        let generator = GenesisGenerator::new(config);

        // Generate twice
        let mut db1 = MemKv::new();
        let state1 = generator.generate(&mut db1).unwrap();

        let mut db2 = MemKv::new();
        let state2 = generator.generate(&mut db2).unwrap();

        // Must be identical
        assert_eq!(state1.state_root, state2.state_root);
        assert_eq!(state1.validator_set, state2.validator_set);
    }

    #[test]
    fn test_state_root_format() {
        let config_file = create_test_config();
        let config = GenesisConfig::from_file(config_file.path()).unwrap();
        let generator = GenesisGenerator::new(config);
        let mut db = MemKv::new();
        let state = generator.generate(&mut db).unwrap();

        let hex_root = hex::encode(state.state_root);
        assert_eq!(hex_root.len(), 64); // 32 bytes = 64 hex chars
    }
}
