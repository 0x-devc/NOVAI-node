use mempool::{NonceProvider, TxMempool};
use novai_ai_service::{
    AiServiceConfig, AiServiceRunner, AiTriggerCallback, AnthropicClient, FeatureFlags,
};
use novai_codec::txid_v1;
use novai_copilot::congestion_forecaster::CongestionForecaster;
use novai_copilot::congestion_responder::CongestionResponder;
use novai_copilot::congestion_stats::CongestionStats;
use novai_copilot::observer::{AnomalyCallback, ChainObserver, ObservableState, ObserverConfig};
use novai_copilot::spam_detector::SpamDetector;
use novai_copilot::spam_stats::SpamStats;
use novai_crypto::{address_from_pubkey, generate_keypair, sign_tx_v1};
use novai_node::consensus_node::{ConsensusNode, Storage};
use novai_node::metrics;
use novai_node::rpc;
use novai_node::MutexExt;
use novai_state::{
    account_key, encode_account_v1, AccountStateV1, Kv, KvBatch, MemKv, RocksKv, WriteOp,
    KEY_COMMITTED_HEIGHT,
};
use novai_types::{Address, TxId, TxV1, TxVersion};
use std::collections::HashMap;
use std::env;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn usage() {
    eprintln!(
        "usage:
  novai-node run --port <port> --genesis <path> [--peer <addr>]... [--seed <host:port>]... [--key-file <path>] [--metrics-port <port>] [--base-timeout <ms>] [--proposal-interval <ms>] [--storage <rocksdb|memory>] [--data-dir <path>] [--no-encryption]
  novai-node run --port <port> --dev-keys --allow-insecure-dev-keys --validator <index> [--peer <addr>]... [--seed <host:port>]... [--metrics-port <port>] [--base-timeout <ms>] [--proposal-interval <ms>] [--storage <rocksdb|memory>] [--data-dir <path>] [--no-encryption]
  novai-node generate-key [--output <path>]
  novai-node submit-tx <payload> [--nonce <u64>] [--fee <u64>] [--min-fee <u64>] [--cap <u64>]
  novai-node drain-mempool <payload> [<payload> ...] [--max <u64>] [--min-fee <u64>] [--cap <u64>]

examples:
  novai-node generate-key --output ~/.novai/data/validator.key
  novai-node run --port 9000 --genesis testnet/genesis.json
  novai-node run --port 9000 --genesis testnet/genesis.json --key-file ~/.novai/data/validator.key
  novai-node run --port 9000 --dev-keys --validator 0
  novai-node run --port 9001 --peer 127.0.0.1:9000 --dev-keys --validator 1 --metrics-port 8081
  novai-node submit-tx hello
  novai-node drain-mempool a b c
"
    );
}

/// Print an error message to stderr and exit with code 1.
/// Used for user-facing errors (bad CLI args, missing files, etc.)
/// so users see clean text instead of Rust backtraces.
fn fatal(msg: impl std::fmt::Display) -> ! {
    eprintln!("Error: {msg}");
    std::process::exit(1);
}

fn parse_u64(opt: Option<String>, what: &str) -> u64 {
    let Some(s) = opt else {
        fatal(format!("missing value for {what}"));
    };
    s.parse::<u64>()
        .unwrap_or_else(|_| fatal(format!("invalid {what}: {s}")))
}

struct InMemoryNonceProvider {
    /// In-memory nonce cache. Seeded from DB at startup, then advanced by
    /// committed blocks. Never acquires the DB Mutex — eliminates deadlock
    /// when called from `drain_ready` (which runs under the DB lock).
    expected: Mutex<HashMap<Address, u64>>,
}

impl InMemoryNonceProvider {
    fn new() -> Self {
        Self {
            expected: Mutex::new(HashMap::new()),
        }
    }

    /// Create a standalone provider for CLI commands.
    fn standalone() -> Self {
        Self::new()
    }

    /// Seed nonce cache from the 100 dev-genesis accounts.
    ///
    /// Reads current nonces directly from storage (caller holds no Mutex).
    /// Handles both fresh start (nonce=0) and restart (nonce=N from prior
    /// commits).
    fn seed_dev_accounts(&self, storage: &Storage) {
        const FUNDED_ACCOUNTS: usize = 100;
        let mut map = self.expected.lock().unwrap_or_else(|p| p.into_inner());
        for index in 0..FUNDED_ACCOUNTS {
            // Same key derivation as apply_dev_genesis / SenderAccount::from_index
            let seed_byte = (index % 256) as u8;
            let mut seed = [seed_byte; 32];
            let index_bytes = index.to_le_bytes();
            for (j, &b) in index_bytes.iter().enumerate() {
                seed[j] ^= b;
            }
            let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
            let addr = address_from_pubkey(&sk.verifying_key());

            let nonce = match storage.get(&account_key(&addr)) {
                Ok(Some(bytes)) => novai_state::decode_account_v1(&bytes)
                    .map(|a| a.nonce)
                    .unwrap_or(0),
                _ => 0,
            };
            map.insert(addr, nonce);
        }
        tracing::info!(
            accounts = FUNDED_ACCOUNTS,
            sample_nonce = map.values().next().copied().unwrap_or(0),
            "Nonce provider seeded"
        );
    }

    /// Set a specific expected nonce (used by CLI commands).
    fn set(&self, from: Address, nonce: u64) {
        let mut map = self.expected.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(from, nonce);
    }

    /// Advance nonces for all senders in committed blocks.
    ///
    /// Called after execution, regardless of individual tx success — a
    /// consensus-committed tx occupies its nonce slot forever.
    fn advance_nonces_for_blocks(&self, blocks: &[novai_consensus_types::Block]) {
        let mut map = self.expected.lock().unwrap_or_else(|p| p.into_inner());
        for block in blocks {
            for tx in &block.txs {
                let entry = map.entry(tx.from).or_insert(tx.nonce);
                if tx.nonce >= *entry {
                    *entry = tx.nonce + 1;
                }
            }
        }
    }
}

impl NonceProvider for InMemoryNonceProvider {
    fn expected_nonce(&self, from: &Address) -> u64 {
        let map = self.expected.lock().unwrap_or_else(|p| p.into_inner());
        map.get(from).copied().unwrap_or(0)
    }
}

/// Post-commit callback: executes transactions, advances nonces, and updates the blockchain index.
struct ExecutionCommitCallback {
    nonce_provider: Arc<InMemoryNonceProvider>,
    blockchain_index: Arc<Mutex<rpc::BlockchainIndex>>,
}

impl novai_node::consensus_node::CommitCallback for ExecutionCommitCallback {
    fn on_commit(&self, db: &mut Storage, blocks: &[novai_consensus_types::Block]) {
        let total_txs: usize = blocks.iter().map(|b| b.txs.len()).sum();
        tracing::debug!(block_count = blocks.len(), total_txs, "on_commit executing");
        for block in blocks {
            for tx in &block.txs {
                match novai_execution::dispatch_tx(db, tx, block.height) {
                    Ok(()) => {
                        tracing::debug!(
                            height = block.height,
                            from = ?&tx.from[..4],
                            nonce = tx.nonce,
                            "Executed tx"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            height = block.height,
                            from = ?&tx.from[..4],
                            nonce = tx.nonce,
                            ?e,
                            "Tx execution failed (committed, skipping)"
                        );
                    }
                }
            }
        }
        self.nonce_provider.advance_nonces_for_blocks(blocks);

        // Update blockchain index for block explorer queries
        if let Ok(mut idx) = self.blockchain_index.lock() {
            for block in blocks {
                // Index block hash
                if let Ok(hash) = novai_consensus_types::codec::hash_block_v1(block) {
                    idx.block_hashes.insert(hash, block.height);
                }
                // Index transaction receipts
                for (tx_index, tx) in block.txs.iter().enumerate() {
                    if let Ok(txid) = novai_codec::txid_v1(tx) {
                        idx.tx_receipts.insert(txid, (block.height, tx_index));
                    }
                }
                idx.committed_height = block.height;
            }
        }
    }
}

/// Snapshot of node state that implements ObservableState without holding locks.
///
/// Created by `NodeObservableState::snapshot()` which acquires each lock once,
/// reads all values, and releases immediately. The observer then uses this
/// snapshot for the entire observation cycle — zero lock contention.
struct ObservableStateSnapshot {
    committed_height: u64,
    current_round: u64,
    peer_count: u64,
    mempool_size: u64,
    view_changes_total: u64,
    validator_set: Vec<Address>,
}

impl ObservableState for ObservableStateSnapshot {
    fn committed_height(&self) -> u64 {
        self.committed_height
    }

    fn current_round(&self) -> u64 {
        self.current_round
    }

    fn peer_count(&self) -> u64 {
        self.peer_count
    }

    fn mempool_size(&self) -> u64 {
        self.mempool_size
    }

    fn view_changes_total(&self) -> u64 {
        self.view_changes_total
    }

    fn validator_set(&self) -> Vec<Address> {
        self.validator_set.clone()
    }

    fn expected_leader(&self, height: u64, round: u64) -> Option<Address> {
        if self.validator_set.is_empty() {
            return None;
        }
        let idx = ((height + round) as usize) % self.validator_set.len();
        Some(self.validator_set[idx])
    }
}

/// Creates a snapshot of node state with minimal lock holding.
///
/// Acquires each lock once, reads all values, and returns a lock-free snapshot.
fn snapshot_observable_state(
    node: &ConsensusNode,
    mempool: &Mutex<TxMempool>,
) -> ObservableStateSnapshot {
    let (committed_height, current_round, view_changes_total) = {
        let state = node.state.lock_or_recover();
        (
            state.committed_height,
            state.round,
            state.view_changes_total,
        )
    };
    let peer_count = node.peer_manager.peer_count() as u64;
    let mempool_size = mempool.lock_or_recover().len() as u64;
    ObservableStateSnapshot {
        committed_height,
        current_round,
        peer_count,
        mempool_size,
        view_changes_total,
        validator_set: node.validator_set.clone(),
    }
}

/// Callback that logs anomalies (signal tx submission to be added later).
struct LoggingAnomalyCallback;

impl AnomalyCallback for LoggingAnomalyCallback {
    fn on_anomaly(
        &self,
        _payload: novai_ai_entities::SignalPayload,
        signal: novai_ai_entities::AiSignalV1,
    ) {
        tracing::warn!(
            height = signal.height,
            confidence = signal.confidence,
            signal_type = ?signal.signal_type,
            "ANOMALY"
        );
    }
}

/// Dispatches anomalies to either the logging callback or the AI service.
enum AnomalyHandler {
    Logging(LoggingAnomalyCallback),
    AiService(AiTriggerCallback),
}

impl AnomalyCallback for AnomalyHandler {
    fn on_anomaly(
        &self,
        payload: novai_ai_entities::SignalPayload,
        signal: novai_ai_entities::AiSignalV1,
    ) {
        match self {
            Self::Logging(cb) => cb.on_anomaly(payload, signal),
            Self::AiService(cb) => cb.on_anomaly(payload, signal),
        }
    }
}

fn build_tx(from: Address, pubkey: [u8; 32], nonce: u64, fee: u64, payload: String) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from,
        pubkey,
        nonce,
        fee,
        payload: payload.into_bytes(),
        sig: [0u8; 64],
    }
}

fn short_id(id: &TxId) -> String {
    // print first 8 bytes as hex for readability
    let mut s = String::new();
    for b in &id[..8] {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Load an Ed25519 signing key from a 32-byte seed file.
fn load_key_file(path: &str) -> ed25519_dalek::SigningKey {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| fatal(format!("Failed to read key file {}: {}", path, e)));
    if bytes.len() != 32 {
        fatal(format!(
            "Key file {} must be exactly 32 bytes (got {} bytes)",
            path,
            bytes.len()
        ));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    ed25519_dalek::SigningKey::from_bytes(&seed)
}

/// Save a 32-byte Ed25519 seed to a file with 0600 permissions.
fn save_key_file(path: &str, seed: &[u8; 32]) {
    // Create parent directories if needed
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| {
            fatal(format!(
                "Failed to create directory {}: {}",
                parent.display(),
                e
            ))
        });
    }

    let mut file = std::fs::File::create(path)
        .unwrap_or_else(|e| fatal(format!("Failed to create key file {}: {}", path, e)));

    // Set 0600 permissions before writing
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        file.set_permissions(perms)
            .unwrap_or_else(|e| fatal(format!("Failed to set permissions on {}: {}", path, e)));
    }

    file.write_all(seed)
        .unwrap_or_else(|e| fatal(format!("Failed to write key file {}: {}", path, e)));
}

/// Parse genesis.json and extract validator set (pubkeys + addresses).
fn parse_genesis_validator_set(
    genesis_path: &str,
) -> (Vec<Address>, HashMap<Address, ed25519_dalek::VerifyingKey>) {
    let json = std::fs::read_to_string(genesis_path).unwrap_or_else(|e| {
        fatal(format!(
            "Failed to read genesis file {}: {}",
            genesis_path, e
        ))
    });
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap_or_else(|e| {
        fatal(format!(
            "Failed to parse genesis JSON {}: {}",
            genesis_path, e
        ))
    });

    let validators = parsed["validators"]
        .as_array()
        .unwrap_or_else(|| fatal("genesis.json missing 'validators' array"));

    let mut validator_set = Vec::new();
    let mut validator_pubkeys = HashMap::new();

    for (i, v) in validators.iter().enumerate() {
        let pubkey_hex = v["pubkey"]
            .as_str()
            .unwrap_or_else(|| fatal(format!("Validator {} missing 'pubkey' field", i)));

        let pubkey_bytes = hex::decode(pubkey_hex)
            .unwrap_or_else(|e| fatal(format!("Validator {} pubkey invalid hex: {}", i, e)));

        if pubkey_bytes.len() != 32 {
            fatal(format!(
                "Validator {} pubkey must be 32 bytes (got {})",
                i,
                pubkey_bytes.len()
            ));
        }

        let mut pk_array = [0u8; 32];
        pk_array.copy_from_slice(&pubkey_bytes);

        let vk = novai_crypto::pubkey_from_bytes(&pk_array)
            .unwrap_or_else(|e| fatal(format!("Validator {} pubkey invalid Ed25519: {:?}", i, e)));

        let addr = address_from_pubkey(&vk);
        validator_set.push(addr);
        validator_pubkeys.insert(addr, vk);
    }

    (validator_set, validator_pubkeys)
}

/// Apply dev-mode genesis: fund the first 100 tx-generator sender addresses.
///
/// Uses the same deterministic key derivation as `tools/tx-generator/src/sender.rs`
/// (`SenderAccount::from_index`). Only runs on fresh storage (no committed height).
fn apply_dev_genesis(storage: &mut Storage) {
    // Skip if DB already has state (restart)
    if storage.get(KEY_COMMITTED_HEIGHT).ok().flatten().is_some() {
        return;
    }

    const FUNDED_ACCOUNTS: usize = 100;
    const INITIAL_BALANCE: u128 = 1_000_000_000;

    let mut ops = Vec::with_capacity(FUNDED_ACCOUNTS);

    for index in 0..FUNDED_ACCOUNTS {
        // Replicate sender.rs SenderAccount::from_index key derivation
        let seed_byte = (index % 256) as u8;
        let mut seed = [seed_byte; 32];
        let index_bytes = index.to_le_bytes();
        for (j, &b) in index_bytes.iter().enumerate() {
            seed[j] ^= b;
        }

        let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
        let addr = address_from_pubkey(&sk.verifying_key());

        let account = AccountStateV1 {
            balance: INITIAL_BALANCE,
            nonce: 0,
        };
        ops.push(WriteOp::Put(
            account_key(&addr),
            encode_account_v1(&account).to_vec(),
        ));
    }

    storage.apply_batch(&ops).expect("dev genesis write failed");
    tracing::info!(
        accounts = FUNDED_ACCOUNTS,
        balance = INITIAL_BALANCE,
        "Dev genesis: funded tx-generator sender accounts"
    );
}

fn main() {
    // Install panic hook BEFORE tracing so panics are always visible.
    // Tracing may not work if the panic happened while holding a tracing lock.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        eprintln!("NOVAI PANIC in thread '{name}': {info}");
        default_hook(info);
    }));

    // Initialize structured logging (controlled via RUST_LOG env var)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("novai=info".parse().unwrap()),
        )
        .init();

    let mut args = env::args().skip(1);
    let Some(cmd) = args.next() else {
        usage();
        return;
    };

    match cmd.as_str() {
        "generate-key" => {
            let rest: Vec<String> = args.collect();
            let mut output_path: Option<String> = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--output" => {
                        output_path =
                            Some(rest.get(i + 1).cloned().expect("missing --output value"));
                        i += 2;
                    }
                    other => {
                        fatal(format!("unknown flag: {other}"));
                    }
                }
            }

            let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
            let path = output_path.unwrap_or_else(|| format!("{}/.novai/data/validator.key", home));

            // Check if file already exists to avoid accidental overwrite
            if std::path::Path::new(&path).exists() {
                tracing::error!(path = %path, "Key file already exists — remove it first to generate a new key");
                std::process::exit(1);
            }

            let (sk, pk) = generate_keypair();
            let seed = sk.to_bytes();
            save_key_file(&path, &seed);

            let pubkey_hex = hex::encode(pk.as_bytes());
            let addr = address_from_pubkey(&pk);
            let addr_hex = hex::encode(addr);

            println!("{}", pubkey_hex);
            tracing::info!(path = %path, pubkey = %pubkey_hex, address = %addr_hex, "Key generated");
        }

        "run" => {
            // Parse flags
            let mut port: Option<u16> = None;
            let mut peers: Vec<String> = Vec::new();
            let mut seeds: Vec<String> = Vec::new();
            let mut validator_idx: Option<usize> = None;
            let mut metrics_port: Option<u16> = None;
            let mut rpc_port: Option<u16> = None;
            let mut base_timeout_ms: u64 = novai_consensus::BASE_TIMEOUT_MS;
            let mut storage_backend: String = "rocksdb".to_string();
            let mut data_dir: Option<String> = None;
            let mut key_file: Option<String> = None;
            let mut genesis_path: Option<String> = None;
            let mut dev_keys = false;
            let mut no_encryption = false;
            let mut allow_insecure_dev_keys = false;
            let mut proposal_interval_ms: u64 = 100; // Default: 100ms

            let rest: Vec<String> = args.collect();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--port" => {
                        port = Some(parse_u64(rest.get(i + 1).cloned(), "--port") as u16);
                        i += 2;
                    }
                    "--peer" => {
                        peers.push(rest.get(i + 1).cloned().expect("missing --peer value"));
                        i += 2;
                    }
                    "--seed" => {
                        seeds.push(rest.get(i + 1).cloned().expect("missing --seed value"));
                        i += 2;
                    }
                    "--validator" => {
                        validator_idx =
                            Some(parse_u64(rest.get(i + 1).cloned(), "--validator") as usize);
                        i += 2;
                    }
                    "--metrics-port" => {
                        metrics_port =
                            Some(parse_u64(rest.get(i + 1).cloned(), "--metrics-port") as u16);
                        i += 2;
                    }
                    "--rpc-port" => {
                        rpc_port = Some(parse_u64(rest.get(i + 1).cloned(), "--rpc-port") as u16);
                        i += 2;
                    }
                    "--base-timeout" => {
                        base_timeout_ms = parse_u64(rest.get(i + 1).cloned(), "--base-timeout");
                        i += 2;
                    }
                    "--proposal-interval" => {
                        proposal_interval_ms =
                            parse_u64(rest.get(i + 1).cloned(), "--proposal-interval");
                        if proposal_interval_ms < 5 {
                            eprintln!("--proposal-interval must be >= 5ms");
                            std::process::exit(1);
                        }
                        i += 2;
                    }
                    "--storage" => {
                        storage_backend =
                            rest.get(i + 1).cloned().expect("missing --storage value");
                        i += 2;
                    }
                    "--data-dir" => {
                        data_dir =
                            Some(rest.get(i + 1).cloned().expect("missing --data-dir value"));
                        i += 2;
                    }
                    "--key-file" => {
                        key_file =
                            Some(rest.get(i + 1).cloned().expect("missing --key-file value"));
                        i += 2;
                    }
                    "--genesis" => {
                        genesis_path =
                            Some(rest.get(i + 1).cloned().expect("missing --genesis value"));
                        i += 2;
                    }
                    "--dev-keys" => {
                        dev_keys = true;
                        i += 1;
                    }
                    "--no-encryption" => {
                        no_encryption = true;
                        i += 1;
                    }
                    "--allow-insecure-dev-keys" => {
                        allow_insecure_dev_keys = true;
                        i += 1;
                    }
                    other => {
                        fatal(format!("unknown flag: {other}"));
                    }
                }
            }

            let port = port.expect("--port required");
            let metrics_port = metrics_port.unwrap_or(8080);
            let rpc_port = rpc_port.unwrap_or(3030);

            // C-05: Block dev-keys without explicit acknowledgment.
            if dev_keys && !allow_insecure_dev_keys {
                eprintln!(
                    "SECURITY ERROR: --dev-keys generates DETERMINISTIC keys known to EVERYONE."
                );
                eprintln!("Seeds: [0;32], [1;32], [2;32], [3;32] — any attacker can derive them.");
                eprintln!();
                eprintln!("To proceed, add: --allow-insecure-dev-keys");
                fatal("Refusing to start with --dev-keys without explicit acknowledgment");
            }
            if dev_keys && allow_insecure_dev_keys {
                eprintln!("WARNING: Running with DETERMINISTIC dev keys — NOT SAFE FOR PRODUCTION");
            }

            // Resolve key + validator set based on mode
            // Returns: (signing_key, validator_set, pubkeys, ed25519_seed, known_noise_keys)
            #[allow(clippy::type_complexity)]
            let (our_key, validator_set, validator_pubkeys, our_seed, known_noise_keys): (
                ed25519_dalek::SigningKey,
                Vec<Address>,
                HashMap<Address, ed25519_dalek::VerifyingKey>,
                [u8; 32],
                Vec<[u8; 32]>,
            ) = if dev_keys {
                // ── Dev-keys mode ──────────────────────────────────────
                tracing::warn!("Using deterministic dev keys — NOT for production");
                let idx = validator_idx.expect("--validator <index> required with --dev-keys");

                let dev_seeds: Vec<[u8; 32]> = (0..4).map(|i| [i as u8; 32]).collect();
                let dev_validator_keys: Vec<ed25519_dalek::SigningKey> = dev_seeds
                    .iter()
                    .map(ed25519_dalek::SigningKey::from_bytes)
                    .collect();

                let dev_validator_set: Vec<Address> = dev_validator_keys
                    .iter()
                    .map(|sk| address_from_pubkey(&sk.verifying_key()))
                    .collect();

                let dev_validator_pubkeys: HashMap<Address, ed25519_dalek::VerifyingKey> =
                    dev_validator_keys
                        .iter()
                        .map(|sk| {
                            let pk = sk.verifying_key();
                            (address_from_pubkey(&pk), pk)
                        })
                        .collect();

                if idx >= dev_validator_keys.len() {
                    fatal(format!(
                        "--validator {} out of range (dev-keys supports 0..{})",
                        idx,
                        dev_validator_keys.len() - 1
                    ));
                }

                // Precompute X25519 noise PUBLIC keys for all dev validators
                let noise_keys: Vec<[u8; 32]> = dev_seeds
                    .iter()
                    .map(novai_p2p::noise::noise_pubkey_from_seed)
                    .collect();

                let key = dev_validator_keys[idx].clone();
                let seed = dev_seeds[idx];
                (key, dev_validator_set, dev_validator_pubkeys, seed, noise_keys)
            } else {
                // ── Production mode ────────────────────────────────────
                let gp = genesis_path.expect("--genesis <path> required (or use --dev-keys)");

                let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
                let base = data_dir
                    .as_deref()
                    .map(String::from)
                    .unwrap_or_else(|| format!("{}/.novai/data", home));

                let kf = key_file.unwrap_or_else(|| format!("{}/validator.key", base));

                let our_key = load_key_file(&kf);
                let seed = our_key.to_bytes();
                let our_pk = our_key.verifying_key();
                let our_addr = address_from_pubkey(&our_pk);

                let (vs, vp) = parse_genesis_validator_set(&gp);

                if !vs.contains(&our_addr) {
                    let our_pubkey_hex = hex::encode(our_pk.as_bytes());
                    fatal(format!(
                        "Our public key {} is not in the genesis validator set at {}",
                        our_pubkey_hex, gp
                    ));
                }

                tracing::info!(key_file = %kf, "Key loaded");
                // Production mode: no precomputed noise keys (peer verification skipped
                // until a mechanism for distributing noise pubkeys is implemented)
                (our_key, vs, vp, seed, Vec::new())
            };

            let our_addr = address_from_pubkey(&our_key.verifying_key());

            // Build storage backend
            let storage = match storage_backend.as_str() {
                "rocksdb" => {
                    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
                    let base = data_dir
                        .as_deref()
                        .map(String::from)
                        .unwrap_or_else(|| format!("{}/.novai/data", home));
                    // Use validator index for dev-keys, address prefix for production
                    let db_subdir = if dev_keys {
                        format!(
                            "validator-{}",
                            validator_idx.expect("validator_idx set in dev-keys")
                        )
                    } else {
                        format!("validator-{}", &hex::encode(our_addr)[..16])
                    };
                    let db_path = format!("{}/{}", base, db_subdir);

                    std::fs::create_dir_all(&db_path).unwrap_or_else(|e| {
                        fatal(format!("Failed to create data dir {}: {}", db_path, e))
                    });

                    tracing::info!(backend = "RocksDB", path = %db_path, "Storage initialized");

                    let rocks = RocksKv::open(&db_path).unwrap_or_else(|e| {
                        fatal(format!("Failed to open RocksDB at {}: {}", db_path, e))
                    });
                    Storage::Rocks(rocks)
                }
                "memory" => {
                    tracing::warn!("Storage: MEMORY (volatile — state lost on restart!)");
                    Storage::Memory(MemKv::new())
                }
                other => {
                    fatal(format!(
                        "unknown --storage value: {} (expected: rocksdb | memory)",
                        other
                    ));
                }
            };

            // Fund tx-generator sender accounts on first start (dev-keys only)
            let mut storage = storage;
            if dev_keys {
                apply_dev_genesis(&mut storage);
            }

            let encryption_enabled = !no_encryption;
            if encryption_enabled {
                tracing::info!("Transport: encrypted (Noise_XX_25519_ChaChaPoly_SHA256)");
            } else {
                tracing::warn!("Transport: PLAINTEXT (--no-encryption)");
            }

            tracing::info!(
                port,
                metrics_port,
                address = %&hex::encode(our_addr)[..16],
                peers = ?peers,
                base_timeout_ms,
                proposal_interval_ms,
                "Starting consensus node"
            );

            // Create node (clone our_key since we need it for copilot observer too)
            let observer_key = our_key.clone();
            let ed25519_seed = if encryption_enabled {
                Some(our_seed)
            } else {
                None
            };
            let mut node = ConsensusNode::new_with_storage(
                our_key,
                validator_set.clone(),
                validator_pubkeys,
                base_timeout_ms,
                storage,
                ed25519_seed,
            );

            // Set known noise keys for peer identity verification
            let has_noise_keys = !known_noise_keys.is_empty();
            if encryption_enabled && has_noise_keys {
                node.set_known_noise_keys(known_noise_keys);
            }

            // C-02: Refuse to start without peer authentication when encryption
            // is enabled and we're not the only validator. Dev-keys mode always
            // populates known_noise_keys, so this only fires in production.
            if encryption_enabled && !has_noise_keys && validator_set.len() > 1 {
                eprintln!("SECURITY ERROR: Noise encryption enabled but no known validator keys configured.");
                eprintln!(
                    "Without peer authentication, any attacker can connect and eclipse this node."
                );
                eprintln!();
                eprintln!("Options:");
                eprintln!("  1. Use --dev-keys for testing (NOT for production)");
                eprintln!("  2. Use --no-encryption to disable Noise (reduced security)");
                eprintln!("  3. Configure validator noise pubkeys via genesis (future feature)");
                fatal(
                    "Refusing to start: peer authentication required for multi-validator network",
                );
            }

            // Create nonce provider and seed from DB (lock held briefly, single-threaded)
            let nonce_provider = Arc::new(InMemoryNonceProvider::new());
            {
                let db_guard = node.db.lock().unwrap_or_else(|p| p.into_inner());
                nonce_provider.seed_dev_accounts(&db_guard);
            }

            // Shared blockchain index for block explorer RPC endpoints
            let blockchain_index = Arc::new(Mutex::new(rpc::BlockchainIndex::new()));

            // Wire execution commit callback
            let commit_callback = Arc::new(ExecutionCommitCallback {
                nonce_provider: Arc::clone(&nonce_provider),
                blockchain_index: Arc::clone(&blockchain_index),
            });
            node.set_commit_callback(commit_callback);

            // Create mempool early so we can wire gossip before Arc-wrapping the node
            let mempool = Arc::new(Mutex::new(TxMempool::new(1, 1000)));

            // Wire gossip: allows peers to insert received txs into our mempool
            node.set_gossip_mempool(
                Arc::clone(&mempool),
                Arc::clone(&nonce_provider) as Arc<dyn NonceProvider + Send + Sync>,
            );

            let node = Arc::new(node);

            // Start listener
            let bind_addr = format!("127.0.0.1:{}", port)
                .parse()
                .expect("parse bind addr");
            node.start_listener(bind_addr).expect("start listener");

            // Connect to peers (with retry)
            std::thread::sleep(Duration::from_millis(200)); // Brief pause for listener to start
            for peer in &peers {
                let peer_addr = peer.parse().expect("parse peer addr");
                match node.connect_to_peer(peer_addr) {
                    Ok(_) => tracing::info!(peer = %peer, "Connected to peer"),
                    Err(e) => tracing::warn!(peer = %peer, %e, "Failed to connect to peer"),
                }
            }

            // Resolve and connect to DNS seed nodes
            for seed in &seeds {
                use std::net::ToSocketAddrs;
                match seed.to_socket_addrs() {
                    Ok(addrs) => {
                        let mut connected = false;
                        for addr in addrs {
                            match node.connect_to_peer(addr) {
                                Ok(_) => {
                                    tracing::info!(seed = %seed, addr = %addr, "Connected to seed node");
                                    connected = true;
                                    break;
                                }
                                Err(e) => {
                                    tracing::debug!(seed = %seed, addr = %addr, %e, "Seed address failed, trying next");
                                }
                            }
                        }
                        if !connected {
                            tracing::warn!(seed = %seed, "Failed to connect to any address for seed node");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(seed = %seed, %e, "Failed to resolve seed node DNS");
                    }
                }
            }

            tracing::info!("Node started, waiting for peers...");
            std::thread::sleep(Duration::from_millis(500));

            // Graceful shutdown flag — created early so AI service can share it
            let shutdown = Arc::new(AtomicBool::new(false));

            // Create copilot observer
            let observer_config = ObserverConfig::default();
            let observer = ChainObserver::new(observer_key, observer_config);
            let observer_metrics = observer.metrics();
            let observer = Arc::new(Mutex::new(observer));

            // AI service: conditionally start if NOVAI_AI_API_KEY is set
            let ai_api_key = env::var("NOVAI_AI_API_KEY").ok().filter(|k| !k.is_empty());

            let callback: AnomalyHandler = if let Some(ref api_key) = ai_api_key {
                let config = AiServiceConfig {
                    enabled: true,
                    api_key: Some(api_key.clone()),
                    ..AiServiceConfig::default()
                };

                match AnthropicClient::new(config) {
                    Ok(client) => {
                        let client = Arc::new(client);
                        let (trigger_tx, trigger_rx) = tokio::sync::mpsc::channel(32);
                        let ai_shutdown = Arc::clone(&shutdown);
                        let features = FeatureFlags::default();
                        let mut runner =
                            AiServiceRunner::new(client, trigger_rx, ai_shutdown, features);

                        // Allow the consensus loop to update height for the runner
                        let _ai_height_handle = runner.height_handle();

                        // Spawn AI service thread (Thread 3) with its own tokio runtime
                        std::thread::Builder::new()
                            .name("ai-service".into())
                            .spawn(move || {
                                let rt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .expect("AI service tokio runtime");
                                rt.block_on(runner.run());
                                tracing::info!("AI service thread exited");
                            })
                            .expect("spawn AI service thread");

                        tracing::info!("AI service enabled — wired to copilot anomaly triggers");
                        AnomalyHandler::AiService(AiTriggerCallback::new(trigger_tx))
                    }
                    Err(e) => {
                        tracing::warn!(
                            %e,
                            "AI service client creation failed — falling back to logging"
                        );
                        AnomalyHandler::Logging(LoggingAnomalyCallback)
                    }
                }
            } else {
                tracing::info!(
                    "NOVAI_AI_API_KEY not set — AI service disabled, using log-only callback"
                );
                AnomalyHandler::Logging(LoggingAnomalyCallback)
            };

            // Start copilot observer background thread (with congestion response)
            {
                let observer = Arc::clone(&observer);
                let observer_node = Arc::clone(&node);
                let observer_mempool = Arc::clone(&mempool);

                // Wire dynamic fee floor from mempool to congestion responder
                let dynamic_fee_floor = mempool.lock_or_recover().dynamic_fee_floor();
                let mut congestion_responder = CongestionResponder::new(dynamic_fee_floor);
                let mut congestion_stats = CongestionStats::new(100);
                let congestion_forecaster = CongestionForecaster::new();

                // Wire threat scores from spam detector to mempool
                let (threat_scores_shared, threat_scores_empty) =
                    mempool.lock_or_recover().threat_scores();
                let spam_detector = SpamDetector::new();
                let mut spam_stats = SpamStats::new(100);
                let mut decay_counter: u64 = 0;

                std::thread::spawn(move || {
                    tracing::info!("Copilot observer started (with congestion + threat response)");
                    loop {
                        // 5s interval — observer is advisory, doesn't need high frequency
                        std::thread::sleep(Duration::from_millis(5000));

                        // Snapshot all state with 2 lock acquisitions (state + mempool),
                        // then use the lock-free snapshot for the entire cycle.
                        let snapshot = snapshot_observable_state(&observer_node, &observer_mempool);
                        let height = snapshot.committed_height;
                        let mempool_size = snapshot.mempool_size;

                        // Run observer using snapshot (zero additional lock acquisitions)
                        {
                            let mut obs = observer.lock_or_recover();
                            let anomalies = obs.observe(&snapshot, &callback);
                            if !anomalies.is_empty() {
                                tracing::debug!(
                                    count = anomalies.len(),
                                    "Detected anomalies this cycle"
                                );
                            }
                        }

                        // Congestion response (no shared locks needed — uses atomic store)
                        congestion_stats.record_block(
                            height,
                            mempool_size,
                            0,   // block_tx_count: TODO wire to actual block commit events
                            100, // max_block_txs
                            0,   // pending_total_value
                            0,   // avg_fee
                        );
                        if congestion_stats.has_sufficient_data() {
                            if let Some(forecast) =
                                congestion_forecaster.forecast(&congestion_stats)
                            {
                                congestion_responder.respond(&forecast);
                            }
                        }

                        // Threat deprioritization (brief lock on threat_scores only)
                        spam_stats.record_mempool_size(mempool_size);
                        let patterns = spam_detector.detect(&spam_stats, height);
                        if !patterns.is_empty() {
                            let scores = spam_detector.compute_threat_scores(&patterns);
                            if let Ok(mut map) = threat_scores_shared.lock() {
                                for (addr, score) in scores {
                                    let entry = map.entry(addr).or_insert(0);
                                    *entry = (*entry).max(score);
                                }
                                threat_scores_empty.store(map.is_empty(), Ordering::Relaxed);
                            }
                            tracing::debug!(
                                patterns = patterns.len(),
                                "Spam patterns detected, threat scores updated"
                            );
                        }

                        // Decay threat scores every 10 cycles (~50s at 5s interval)
                        decay_counter += 1;
                        if decay_counter >= 10 {
                            decay_counter = 0;
                            if let Ok(mut map) = threat_scores_shared.lock() {
                                map.retain(|_, score| {
                                    *score = score.saturating_sub(5);
                                    *score > 0
                                });
                                threat_scores_empty.store(map.is_empty(), Ordering::Relaxed);
                            }
                        }
                    }
                });
            }

            // Start metrics server
            let metrics_collect = {
                let state = Arc::clone(&node.state);
                let peer_manager = Arc::clone(&node.peer_manager);
                let mempool = Arc::clone(&mempool);
                let observer_metrics = Arc::clone(&observer_metrics);
                move || {
                    // Acquire state lock once for all state fields
                    let (committed_height, current_round, view_changes_total) = {
                        let s = state.lock_or_recover();
                        (s.committed_height, s.round, s.view_changes_total)
                    };
                    metrics::MetricsSnapshot {
                        committed_height,
                        current_round,
                        peer_count: peer_manager.peer_count() as u64,
                        mempool_size: mempool.lock_or_recover().len() as u64,
                        view_changes_total,
                        block_tx_count: 0, // TODO: Wire to actual block commit events
                        total_txs_committed: 0, // TODO: Accumulate from block commits
                        // Copilot metrics from observer
                        copilot_observations_total: observer_metrics
                            .observations
                            .load(Ordering::Relaxed),
                        anomaly_signals_total: observer_metrics
                            .anomalies_detected
                            .load(Ordering::Relaxed),
                        anomaly_signals_published: observer_metrics
                            .signals_published
                            .load(Ordering::Relaxed),
                        anomaly_last_confidence: observer_metrics
                            .last_confidence
                            .load(Ordering::Relaxed),
                    }
                }
            };

            let metrics_addr = format!("0.0.0.0:{}", metrics_port);
            if let Err(e) = metrics::start_metrics_server(&metrics_addr, metrics_collect) {
                tracing::error!(%e, "Failed to start metrics server");
            }

            // Start RPC server with state access (transaction submission + queries)
            let rpc_addr = format!("0.0.0.0:{}", rpc_port);
            if let Err(e) = rpc::start_rpc_server_with_state(
                &rpc_addr,
                Arc::clone(&mempool),
                Arc::clone(&nonce_provider) as Arc<dyn NonceProvider + Send + Sync>,
                Arc::clone(&node.db),
                dev_keys,
                Arc::clone(&blockchain_index),
            ) {
                tracing::error!(%e, "Failed to start RPC server");
            }

            // Signal handler — reuses the shutdown flag created earlier
            {
                let shutdown_flag = shutdown.clone();
                std::thread::Builder::new()
                    .name("signal-handler".into())
                    .spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("signal handler runtime");
                        rt.block_on(async {
                            #[cfg(unix)]
                            {
                                use tokio::signal::unix::{signal, SignalKind};
                                let mut sigterm = signal(SignalKind::terminate()).unwrap();
                                let mut sighup = signal(SignalKind::hangup()).unwrap();
                                loop {
                                    tokio::select! {
                                        _ = sigterm.recv() => {
                                            tracing::info!("Received SIGTERM");
                                            break;
                                        }
                                        _ = sighup.recv() => {
                                            tracing::info!("Received SIGHUP (ignored)");
                                            // Daemon ignores SIGHUP — continue waiting
                                        }
                                        _ = tokio::signal::ctrl_c() => {
                                            tracing::info!("Received SIGINT");
                                            break;
                                        }
                                    }
                                }
                            }
                            #[cfg(not(unix))]
                            {
                                let _ = tokio::signal::ctrl_c().await;
                                tracing::info!("Received SIGINT");
                            }
                            shutdown_flag.store(true, Ordering::Relaxed);
                        });
                    })
                    .expect("spawn signal handler");
            }

            // Simple consensus loop with timeout checking
            // NOTE: 5ms sleep allows 200 iterations/sec for responsive consensus
            let mut last_proposal_attempt = std::time::Instant::now();
            let mut last_status_log = std::time::Instant::now();
            let mut last_sync_check = std::time::Instant::now();
            let mut last_sync_trigger = std::time::Instant::now();
            let mut last_resource_log = std::time::Instant::now();
            let mut last_mempool_purge = std::time::Instant::now();
            loop {
                if shutdown.load(Ordering::Relaxed) {
                    tracing::info!("Shutting down gracefully...");
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));

                // Check for timeout
                if let Some(timeout) = node.check_timeout() {
                    tracing::debug!("Broadcasting timeout...");
                    if let Err(e) =
                        node.broadcast(novai_p2p::NetworkMessage::Timeout(timeout.clone()))
                    {
                        tracing::error!(%e, "Timeout broadcast failed");
                    }
                    // Handle our own timeout
                    if let Err(e) = node.handle_timeout(timeout) {
                        tracing::error!(%e, "Handle timeout failed");
                    }
                }

                // Check for sync timeout every 500ms (not every loop iteration)
                if last_sync_check.elapsed() >= Duration::from_millis(500) {
                    last_sync_check = std::time::Instant::now();
                    let mut pending = node.pending_sync_request.lock_or_recover();
                    if let Some(ref request) = *pending {
                        if request.request_time.elapsed() >= Duration::from_secs(5) {
                            tracing::warn!(
                                peer = ?&request.peer[..4],
                                start_height = request.start_height,
                                end_height = request.end_height,
                                "Sync request timed out"
                            );
                            *pending = None;
                        }
                    }
                }

                // Periodic sync: detect when committed_height is behind highest_qc
                // and trigger block requests every 2 seconds
                if last_sync_trigger.elapsed() >= Duration::from_secs(2) {
                    last_sync_trigger = std::time::Instant::now();
                    node.try_request_missing_blocks();
                }

                // Periodic resource monitoring (every 60 seconds)
                if last_resource_log.elapsed() >= Duration::from_secs(60) {
                    last_resource_log = std::time::Instant::now();
                    let state = node.state.lock_or_recover();
                    let qc_bc = node.qc_broadcasted.lock_or_recover();
                    let nonce_map = nonce_provider
                        .expected
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    tracing::info!(
                        committed_height = state.committed_height,
                        round = state.round,
                        block_cache_len = state.block_cache.len(),
                        block_cache_cap = state.block_cache.capacity(),
                        block_by_hash_len = state.block_by_hash.len(),
                        block_by_hash_cap = state.block_by_hash.capacity(),
                        qc_cache_len = state.qc_cache.len(),
                        qc_cache_cap = state.qc_cache.capacity(),
                        pending_votes_len = state.pending_votes.len(),
                        pending_votes_cap = state.pending_votes.capacity(),
                        pending_timeouts_len = state.pending_timeouts.len(),
                        pending_timeouts_cap = state.pending_timeouts.capacity(),
                        voted_in_round_len = state.voted_in_round.len(),
                        voted_in_round_cap = state.voted_in_round.capacity(),
                        timed_out_in_round_len = state.timed_out_in_round.len(),
                        qc_broadcast_len = qc_bc.len(),
                        qc_broadcast_cap = qc_bc.capacity(),
                        nonce_map_len = nonce_map.len(),
                        peers = node.peer_manager.peer_count(),
                        view_changes = state.view_changes_total,
                        "RESOURCE_MONITOR"
                    );
                    drop(nonce_map);
                    drop(qc_bc);
                    drop(state);
                }

                // Purge stale mempool transactions every 30 seconds.
                // Prevents future-nonce orphans from permanently filling capacity.
                if last_mempool_purge.elapsed() >= Duration::from_secs(30) {
                    last_mempool_purge = std::time::Instant::now();
                    let mut mp = mempool.lock_or_recover();
                    let purged = mp.purge_stale(Duration::from_secs(120));
                    if purged > 0 {
                        tracing::info!(purged, "Purged stale transactions from mempool");
                    }
                }

                // Propose every proposal_interval_ms (must be less than BASE_TIMEOUT_MS for consensus to work)
                if last_proposal_attempt.elapsed() >= Duration::from_millis(proposal_interval_ms) {
                    last_proposal_attempt = std::time::Instant::now();

                    // Recover txs from abandoned proposals (round changed before
                    // our block was committed). Nonce check filters out txs that
                    // were already committed via a different block.
                    let recovered = node.recover_abandoned_txs();
                    if !recovered.is_empty() {
                        let mut mp = mempool.lock_or_recover();
                        let mut reinserted = 0usize;
                        for tx in recovered {
                            if tx.nonce >= nonce_provider.expected_nonce(&tx.from)
                                && mp.reinsert_unchecked(tx).is_ok()
                            {
                                reinserted += 1;
                            }
                        }
                        if reinserted > 0 {
                            tracing::warn!(reinserted, "Recovered abandoned txs to mempool");
                        }
                    }

                    let mut mempool_guard = mempool.lock_or_recover();
                    match node.try_propose_block(&mut mempool_guard, &*nonce_provider) {
                        Ok(true) => tracing::info!("Proposed block successfully"),
                        Ok(false) => {
                            // Only log status every 5 seconds to reduce noise
                            if last_status_log.elapsed() >= Duration::from_secs(5) {
                                last_status_log = std::time::Instant::now();
                                let peer_count = node.peer_manager.peer_count();
                                let state = node.state.lock_or_recover();
                                tracing::debug!(
                                    height = state.committed_height,
                                    round = state.round,
                                    peers = peer_count,
                                    "Status"
                                );
                            }
                        }
                        Err(e) => tracing::error!(%e, "Propose failed"),
                    }
                }
            }
            tracing::info!("Node stopped");
        }

        "submit-tx" => {
            let Some(payload) = args.next() else {
                usage();
                return;
            };

            // defaults
            let mut nonce: u64 = 0;
            let mut fee: u64 = 1;
            let mut min_fee: u64 = 1;
            let mut cap: usize = 1000;

            // parse simple flags
            let rest: Vec<String> = args.collect();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--nonce" => {
                        nonce = parse_u64(rest.get(i + 1).cloned(), "--nonce");
                        i += 2;
                    }
                    "--fee" => {
                        fee = parse_u64(rest.get(i + 1).cloned(), "--fee");
                        i += 2;
                    }
                    "--min-fee" => {
                        min_fee = parse_u64(rest.get(i + 1).cloned(), "--min-fee");
                        i += 2;
                    }
                    "--cap" => {
                        cap = parse_u64(rest.get(i + 1).cloned(), "--cap") as usize;
                        i += 2;
                    }
                    other => {
                        fatal(format!("unknown flag: {other}"));
                    }
                }
            }

            // Real Week2 mempool (policy-enforcing)
            let mut mp = TxMempool::new(min_fee, cap);

            // Dev keypair per run
            let (sk, pk) = generate_keypair();
            let from = address_from_pubkey(&pk);

            let nonce_provider = InMemoryNonceProvider::standalone();
            nonce_provider.set(from, nonce);

            let mut tx = build_tx(from, pk.to_bytes(), nonce, fee, payload);
            sign_tx_v1(&sk, &mut tx).expect("sign tx");

            let id = mp.insert(tx, &nonce_provider).expect("mempool insert");
            println!(
                "submitted tx id={} (mempool size={})",
                short_id(&id),
                mp.len()
            );
        }

        "drain-mempool" => {
            // collect payloads until flags begin
            let mut payloads: Vec<String> = Vec::new();
            let mut rest: Vec<String> = Vec::new();

            let all: Vec<String> = args.collect();
            let mut seen_flag = false;
            for a in all {
                if a.starts_with("--") {
                    seen_flag = true;
                }
                if seen_flag {
                    rest.push(a);
                } else {
                    payloads.push(a);
                }
            }

            if payloads.is_empty() {
                usage();
                return;
            }

            // defaults
            let mut max: usize = 100;
            let mut min_fee: u64 = 1;
            let mut cap: usize = 1000;

            // parse flags
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--max" => {
                        max = parse_u64(rest.get(i + 1).cloned(), "--max") as usize;
                        i += 2;
                    }
                    "--min-fee" => {
                        min_fee = parse_u64(rest.get(i + 1).cloned(), "--min-fee");
                        i += 2;
                    }
                    "--cap" => {
                        cap = parse_u64(rest.get(i + 1).cloned(), "--cap") as usize;
                        i += 2;
                    }
                    other => {
                        fatal(format!("unknown flag: {other}"));
                    }
                }
            }

            let mut mp = TxMempool::new(min_fee, cap);
            let nonce_provider = InMemoryNonceProvider::standalone();

            // Insert txs with increasing fees so drain shows fee-priority deterministically.
            let (sk, pk) = generate_keypair();
            let from = address_from_pubkey(&pk);
            nonce_provider.set(from, 0);

            for (idx, payload) in payloads.into_iter().enumerate() {
                let fee = (idx as u64) + 1;
                let mut tx = build_tx(from, pk.to_bytes(), 0, fee, payload);
                sign_tx_v1(&sk, &mut tx).expect("sign tx");

                mp.insert(tx, &nonce_provider).expect("mempool insert");
            }

            // ✅ MISSING IN YOUR BROKEN FILE: define these before using them
            let before = mp.len();
            let drained = mp.drain_ready(max, &nonce_provider);
            let after = mp.len();

            let lines: Vec<String> = drained
                .iter()
                .map(|tx| {
                    let id_bytes = txid_v1(tx).expect("txid");
                    let id: TxId = id_bytes;
                    let payload = String::from_utf8_lossy(&tx.payload);
                    format!("fee={} payload={} id={}", tx.fee, payload, short_id(&id))
                })
                .collect();

            println!(
                "drained {} txs (before={} after={})\n{}",
                drained.len(),
                before,
                after,
                lines.join("\n")
            );
        }

        _ => {
            usage();
        }
    }
}
