use mempool::{NonceProvider, TxMempool};
use novai_codec::txid_v1;
use novai_copilot::observer::{AnomalyCallback, ChainObserver, ObservableState, ObserverConfig};
use novai_crypto::{address_from_pubkey, generate_keypair, sign_tx_v1};
use novai_node::consensus_node::{ConsensusNode, Storage};
use novai_node::metrics;
use novai_state::{MemKv, RocksKv};
use novai_types::{Address, TxId, TxV1, TxVersion};
use std::collections::HashMap;
use std::env;
use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn usage() {
    eprintln!(
        "usage:
  novai-node run --port <port> --genesis <path> [--peer <addr>]... [--key-file <path>] [--metrics-port <port>] [--base-timeout <ms>] [--storage <rocksdb|memory>] [--data-dir <path>] [--no-encryption]
  novai-node run --port <port> --dev-keys --validator <index> [--peer <addr>]... [--metrics-port <port>] [--base-timeout <ms>] [--storage <rocksdb|memory>] [--data-dir <path>] [--no-encryption]
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

fn parse_u64(opt: Option<String>, what: &str) -> u64 {
    let Some(s) = opt else {
        panic!("missing value for {what}");
    };
    s.parse::<u64>()
        .unwrap_or_else(|_| panic!("invalid {what}: {s}"))
}

#[derive(Default)]
struct InMemoryNonceProvider {
    expected: HashMap<Address, u64>,
}

impl InMemoryNonceProvider {
    fn set(&mut self, from: Address, nonce: u64) {
        self.expected.insert(from, nonce);
    }
}

impl NonceProvider for InMemoryNonceProvider {
    fn expected_nonce(&self, from: &Address) -> u64 {
        *self.expected.get(from).unwrap_or(&0)
    }
}

/// Wrapper for node state that implements ObservableState.
struct NodeObservableState {
    node: Arc<ConsensusNode>,
    mempool: Arc<Mutex<TxMempool>>,
}

impl ObservableState for NodeObservableState {
    fn committed_height(&self) -> u64 {
        self.node.state.lock().unwrap().committed_height
    }

    fn current_round(&self) -> u64 {
        self.node.state.lock().unwrap().round
    }

    fn peer_count(&self) -> u64 {
        self.node.peer_manager.peer_count() as u64
    }

    fn mempool_size(&self) -> u64 {
        self.mempool.lock().unwrap().len() as u64
    }

    fn view_changes_total(&self) -> u64 {
        self.node.state.lock().unwrap().view_changes_total
    }

    fn validator_set(&self) -> Vec<Address> {
        self.node.validator_set.clone()
    }

    fn expected_leader(&self, height: u64, round: u64) -> Option<Address> {
        let validators = &self.node.validator_set;
        if validators.is_empty() {
            return None;
        }
        let idx = ((height + round) as usize) % validators.len();
        Some(validators[idx])
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
        println!(
            "🚨 ANOMALY: height={} confidence={} type={:?}",
            signal.height, signal.confidence, signal.signal_type
        );
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
    let bytes =
        std::fs::read(path).unwrap_or_else(|e| panic!("Failed to read key file {}: {}", path, e));
    if bytes.len() != 32 {
        panic!(
            "Key file {} must be exactly 32 bytes (got {} bytes)",
            path,
            bytes.len()
        );
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    ed25519_dalek::SigningKey::from_bytes(&seed)
}

/// Save a 32-byte Ed25519 seed to a file with 0600 permissions.
fn save_key_file(path: &str, seed: &[u8; 32]) {
    // Create parent directories if needed
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("Failed to create directory {}: {}", parent.display(), e));
    }

    let mut file = std::fs::File::create(path)
        .unwrap_or_else(|e| panic!("Failed to create key file {}: {}", path, e));

    // Set 0600 permissions before writing
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        file.set_permissions(perms)
            .unwrap_or_else(|e| panic!("Failed to set permissions on {}: {}", path, e));
    }

    file.write_all(seed)
        .unwrap_or_else(|e| panic!("Failed to write key file {}: {}", path, e));
}

/// Parse genesis.json and extract validator set (pubkeys + addresses).
fn parse_genesis_validator_set(
    genesis_path: &str,
) -> (Vec<Address>, HashMap<Address, ed25519_dalek::VerifyingKey>) {
    let json = std::fs::read_to_string(genesis_path)
        .unwrap_or_else(|e| panic!("Failed to read genesis file {}: {}", genesis_path, e));
    let parsed: serde_json::Value = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("Failed to parse genesis JSON {}: {}", genesis_path, e));

    let validators = parsed["validators"]
        .as_array()
        .unwrap_or_else(|| panic!("genesis.json missing 'validators' array"));

    let mut validator_set = Vec::new();
    let mut validator_pubkeys = HashMap::new();

    for (i, v) in validators.iter().enumerate() {
        let pubkey_hex = v["pubkey"]
            .as_str()
            .unwrap_or_else(|| panic!("Validator {} missing 'pubkey' field", i));

        let pubkey_bytes = hex::decode(pubkey_hex)
            .unwrap_or_else(|e| panic!("Validator {} pubkey invalid hex: {}", i, e));

        if pubkey_bytes.len() != 32 {
            panic!(
                "Validator {} pubkey must be 32 bytes (got {})",
                i,
                pubkey_bytes.len()
            );
        }

        let mut pk_array = [0u8; 32];
        pk_array.copy_from_slice(&pubkey_bytes);

        let vk = novai_crypto::pubkey_from_bytes(&pk_array)
            .unwrap_or_else(|e| panic!("Validator {} pubkey invalid Ed25519: {:?}", i, e));

        let addr = address_from_pubkey(&vk);
        validator_set.push(addr);
        validator_pubkeys.insert(addr, vk);
    }

    (validator_set, validator_pubkeys)
}

fn main() {
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
                        panic!("unknown flag: {other}");
                    }
                }
            }

            let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
            let path = output_path.unwrap_or_else(|| format!("{}/.novai/data/validator.key", home));

            // Check if file already exists to avoid accidental overwrite
            if std::path::Path::new(&path).exists() {
                eprintln!("ERROR: Key file already exists at {}", path);
                eprintln!("Remove it first if you want to generate a new key.");
                std::process::exit(1);
            }

            let (sk, pk) = generate_keypair();
            let seed = sk.to_bytes();
            save_key_file(&path, &seed);

            let pubkey_hex = hex::encode(pk.as_bytes());
            let addr = address_from_pubkey(&pk);
            let addr_hex = hex::encode(addr);

            println!("{}", pubkey_hex);
            eprintln!("Key written to: {}", path);
            eprintln!("Public key:     {}", pubkey_hex);
            eprintln!("Address:        {}", addr_hex);
        }

        "run" => {
            // Parse flags
            let mut port: Option<u16> = None;
            let mut peers: Vec<String> = Vec::new();
            let mut validator_idx: Option<usize> = None;
            let mut metrics_port: Option<u16> = None;
            let mut base_timeout_ms: u64 = novai_consensus::BASE_TIMEOUT_MS;
            let mut storage_backend: String = "rocksdb".to_string();
            let mut data_dir: Option<String> = None;
            let mut key_file: Option<String> = None;
            let mut genesis_path: Option<String> = None;
            let mut dev_keys = false;
            let mut no_encryption = false;

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
                    "--base-timeout" => {
                        base_timeout_ms = parse_u64(rest.get(i + 1).cloned(), "--base-timeout");
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
                    other => {
                        panic!("unknown flag: {other}");
                    }
                }
            }

            let port = port.expect("--port required");
            let metrics_port = metrics_port.unwrap_or(8080);

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
                eprintln!("WARNING: using deterministic dev keys — NOT for production");
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
                    panic!(
                        "--validator {} out of range (dev-keys supports 0..{})",
                        idx,
                        dev_validator_keys.len() - 1
                    );
                }

                // Precompute X25519 noise keys for all dev validators
                let noise_keys: Vec<[u8; 32]> = dev_seeds
                    .iter()
                    .map(novai_p2p::noise::noise_keypair_from_seed)
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
                    panic!(
                        "Our public key {} is not in the genesis validator set at {}",
                        our_pubkey_hex, gp
                    );
                }

                println!("🔑 Key loaded from: {}", kf);
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

                    std::fs::create_dir_all(&db_path)
                        .unwrap_or_else(|e| panic!("Failed to create data dir {}: {}", db_path, e));

                    println!("💾 Storage: RocksDB");
                    println!("   Path: {}", db_path);

                    let rocks = RocksKv::open(&db_path)
                        .unwrap_or_else(|e| panic!("Failed to open RocksDB at {}: {}", db_path, e));
                    Storage::Rocks(rocks)
                }
                "memory" => {
                    println!("⚠️  Storage: MEMORY (volatile — state lost on restart!)");
                    Storage::Memory(MemKv::new())
                }
                other => {
                    panic!(
                        "unknown --storage value: {} (expected: rocksdb | memory)",
                        other
                    );
                }
            };

            let encryption_enabled = !no_encryption;
            if encryption_enabled {
                println!("🔒 Transport: encrypted (Noise_XX_25519_ChaChaPoly_SHA256)");
            } else {
                println!("⚠️  Transport: PLAINTEXT (--no-encryption)");
            }

            println!("🚀 Starting consensus node");
            println!("   Port: {}", port);
            println!("   Metrics port: {}", metrics_port);
            println!("   Address: {}", &hex::encode(our_addr)[..16]);
            println!("   Peers: {:?}", peers);
            println!("   Base timeout: {}ms", base_timeout_ms);

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
            if encryption_enabled && !known_noise_keys.is_empty() {
                node.set_known_noise_keys(known_noise_keys);
            }

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
                    Ok(_) => println!("✅ Connected to peer {}", peer),
                    Err(e) => println!("⚠️  Failed to connect to {}: {}", peer, e),
                }
            }

            println!("✅ Node started, waiting for peers...");
            std::thread::sleep(Duration::from_millis(500));

            // Create dummy mempool and nonce provider for Week 6
            let mempool = Arc::new(Mutex::new(TxMempool::new(1, 1000)));
            let nonce_provider = InMemoryNonceProvider::default();

            // Create copilot observer
            let observer_config = ObserverConfig::default();
            let observer = ChainObserver::new(observer_key, observer_config);
            let observer_metrics = observer.metrics();
            let observer = Arc::new(Mutex::new(observer));

            // Start copilot observer background thread
            {
                let observer = Arc::clone(&observer);
                let observable_state = NodeObservableState {
                    node: Arc::clone(&node),
                    mempool: Arc::clone(&mempool),
                };
                let callback = LoggingAnomalyCallback;

                std::thread::spawn(move || {
                    println!("🤖 Copilot observer started");
                    loop {
                        std::thread::sleep(Duration::from_millis(500));
                        let mut obs = observer.lock().unwrap();
                        let anomalies = obs.observe(&observable_state, &callback);
                        if !anomalies.is_empty() {
                            println!("   Detected {} anomalies this cycle", anomalies.len());
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
                move || metrics::MetricsSnapshot {
                    committed_height: state.lock().unwrap().committed_height,
                    current_round: state.lock().unwrap().round,
                    peer_count: peer_manager.peer_count() as u64,
                    mempool_size: mempool.lock().unwrap().len() as u64,
                    view_changes_total: state.lock().unwrap().view_changes_total,
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
            };

            let metrics_addr = format!("0.0.0.0:{}", metrics_port);
            if let Err(e) = metrics::start_metrics_server(&metrics_addr, metrics_collect) {
                eprintln!("❌ Failed to start metrics server: {}", e);
            }

            // Simple consensus loop with timeout checking
            // NOTE: 5ms sleep allows 200 iterations/sec for responsive consensus
            let mut last_proposal_attempt = std::time::Instant::now();
            let mut last_status_log = std::time::Instant::now();
            let mut last_sync_check = std::time::Instant::now();
            let mut last_sync_trigger = std::time::Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(5));

                // Check for timeout
                if let Some(timeout) = node.check_timeout() {
                    println!("⏰ Broadcasting timeout...");
                    if let Err(e) =
                        node.broadcast(novai_p2p::NetworkMessage::Timeout(timeout.clone()))
                    {
                        println!("❌ Timeout broadcast failed: {}", e);
                    }
                    // Handle our own timeout
                    if let Err(e) = node.handle_timeout(timeout) {
                        println!("❌ Handle timeout failed: {}", e);
                    }
                }

                // Check for sync timeout every 500ms (not every loop iteration)
                if last_sync_check.elapsed() >= Duration::from_millis(500) {
                    last_sync_check = std::time::Instant::now();
                    let mut pending = node.pending_sync_request.lock().unwrap();
                    if let Some(ref request) = *pending {
                        if request.request_time.elapsed() >= Duration::from_secs(5) {
                            println!(
                                "⏰ Sync request timed out (peer {:?}, blocks {}-{})",
                                &request.peer[..4],
                                request.start_height,
                                request.end_height
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

                // Propose every 100ms (must be less than BASE_TIMEOUT_MS for consensus to work)
                // This allows up to 10 proposals/sec - balanced for stability
                if last_proposal_attempt.elapsed() >= Duration::from_millis(100) {
                    last_proposal_attempt = std::time::Instant::now();

                    let mut mempool_guard = mempool.lock().unwrap();
                    match node.try_propose_block(&mut mempool_guard, &nonce_provider) {
                        Ok(true) => println!("👑 Proposed block successfully"),
                        Ok(false) => {
                            // Only log status every 5 seconds to reduce noise
                            if last_status_log.elapsed() >= Duration::from_secs(5) {
                                last_status_log = std::time::Instant::now();
                                let peer_count = node.peer_manager.peer_count();
                                let state = node.state.lock().unwrap();
                                println!(
                                    "📊 Status: height={} round={} peers={}",
                                    state.committed_height, state.round, peer_count
                                );
                            }
                        }
                        Err(e) => println!("❌ Propose failed: {}", e),
                    }
                }
            }
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
                        panic!("unknown flag: {other}");
                    }
                }
            }

            // Real Week2 mempool (policy-enforcing)
            let mut mp = TxMempool::new(min_fee, cap);

            // Dev keypair per run
            let (sk, pk) = generate_keypair();
            let from = address_from_pubkey(&pk);

            let mut nonce_provider = InMemoryNonceProvider::default();
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
                        panic!("unknown flag: {other}");
                    }
                }
            }

            let mut mp = TxMempool::new(min_fee, cap);
            let mut nonce_provider = InMemoryNonceProvider::default();

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
