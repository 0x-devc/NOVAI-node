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
    account_key, block_key, encode_account_v1, qc_key, AccountStateV1, Kv, KvBatch, MemKv, RocksKv,
    WriteOp, KEY_COMMITTED_HEIGHT, KEY_EXECUTED_HEIGHT,
};
use novai_types::{Address, TxId, TxV1, TxVersion};
use std::collections::{BTreeSet, HashMap};
use std::env;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

    /// Seed the nonce cache from persisted state.
    ///
    /// Scans every account row and every entry of the address-to-entity
    /// reverse index so the expected view matches what execution will accept
    /// next for every sender with on-chain history, not only the 100
    /// dev-genesis accounts. Reads storage directly (caller holds no Mutex);
    /// runs single-threaded at boot before RPC, gossip, and consensus start.
    ///
    /// Entity entries overwrite account entries at the same address:
    /// execution resolves senders entity-first (check_ai_entity_sender) and
    /// rejects account-only tx types from entity addresses outright, so the
    /// entity nonce is the only nonce execution can accept there.
    ///
    /// Fails closed: an unreadable or malformed row is a boot error, because
    /// silently skipping a sender would re-create the admitted-but-never-
    /// proposable strand this seeding removes.
    fn seed_from_state(&self, storage: &Storage) -> Result<(usize, usize), String> {
        let account_rows = storage
            .scan_prefix(novai_state::KEY_PREFIX_ACCOUNTS)
            .map_err(|e| format!("account scan failed: {e}"))?;
        let mut account_nonces = Vec::with_capacity(account_rows.len());
        for (key, value) in &account_rows {
            let addr: Address = key[novai_state::KEY_PREFIX_ACCOUNTS.len()..]
                .try_into()
                .map_err(|_| format!("malformed account key ({} bytes)", key.len()))?;
            let account = novai_state::decode_account_v1(value)
                .map_err(|e| format!("undecodable account row for {:02x?}: {e:?}", &addr[..4]))?;
            account_nonces.push((addr, account.nonce));
        }

        let index_rows = storage
            .scan_prefix(novai_state::KEY_PREFIX_AI_ENTITY_BY_ADDR)
            .map_err(|e| format!("entity index scan failed: {e}"))?;
        let mut entity_nonces = Vec::with_capacity(index_rows.len());
        for (key, value) in &index_rows {
            let addr: Address = key[novai_state::KEY_PREFIX_AI_ENTITY_BY_ADDR.len()..]
                .try_into()
                .map_err(|_| format!("malformed entity index key ({} bytes)", key.len()))?;
            let entity_id: [u8; 32] = value.as_slice().try_into().map_err(|_| {
                format!(
                    "malformed entity id for {:02x?} ({} bytes)",
                    &addr[..4],
                    value.len()
                )
            })?;
            let entity = novai_execution::read_ai_entity(storage, &entity_id)
                .map_err(|e| format!("entity read failed for {:02x?}: {e:?}", &addr[..4]))?
                .ok_or_else(|| format!("dangling entity index for {:02x?}", &addr[..4]))?;
            entity_nonces.push((addr, entity.nonce));
        }

        let accounts = account_nonces.len();
        let entity_signers = entity_nonces.len();
        let mut map = self
            .expected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Accounts first, then entities, so an entity entry wins at a
        // shared address.
        for (addr, nonce) in account_nonces {
            map.insert(addr, nonce);
        }
        for (addr, nonce) in entity_nonces {
            map.insert(addr, nonce);
        }
        tracing::info!(accounts, entity_signers, "Nonce provider seeded from state");
        Ok((accounts, entity_signers))
    }

    /// Set a specific expected nonce (used by CLI commands).
    fn set(&self, from: Address, nonce: u64) {
        let mut map = self
            .expected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.insert(from, nonce);
    }
}

impl NonceProvider for InMemoryNonceProvider {
    fn expected_nonce(&self, from: &Address) -> u64 {
        let map = self
            .expected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.get(from).copied().unwrap_or(0)
    }
}

/// Shared atomic accumulators for the Prometheus commit metrics. on_commit
/// writes here on every commit; the /metrics collect closure reads on each
/// scrape. Backs novai_block_tx_count (gauge of last block's tx count) and
/// novai_total_txs_committed (monotonic counter).
#[derive(Default)]
struct CommitMetrics {
    block_tx_count: AtomicU64,
    total_txs_committed: AtomicU64,
}

/// Post-commit callback: executes transactions, advances nonces, and updates the blockchain index.
struct ExecutionCommitCallback {
    nonce_provider: Arc<InMemoryNonceProvider>,
    blockchain_index: Arc<Mutex<rpc::BlockchainIndex>>,
    /// Queue of committed txids drained by the propose loop and removed from
    /// the mempool. Cannot remove inline: on_commit fires from peer-connection
    /// threads holding the db lock, while the propose loop holds the mempool
    /// lock and acquires the db lock inside try_propose_block. Taking the
    /// mempool lock here would close an AB-BA cycle.
    pending_mempool_removals: Arc<Mutex<Vec<(TxId, Address)>>>,
    /// Shared with the /metrics collect closure so on_commit can publish
    /// novai_block_tx_count and novai_total_txs_committed without going
    /// through the consensus state lock.
    commit_metrics: Arc<CommitMetrics>,
    /// Gate F5 Stage 2: the demand-driven snapshot producer. Its commit-path
    /// hook does at most a RocksDB checkpoint (flush plus hard links) and never
    /// scans, audits or rebuilds; all of that runs on the background thread
    /// against the created checkpoint. With no peer asking, the hook returns
    /// before doing any work at all.
    snapshot_producer: Arc<novai_node::snapshot::producer::SnapshotProducer>,
}

impl novai_node::consensus_node::CommitCallback for ExecutionCommitCallback {
    fn on_commit(
        &self,
        db: &mut Storage,
        block: &novai_consensus_types::Block,
        cached: Option<novai_node::exec_apply::CachedExec>,
    ) -> Result<(), String> {
        let total_txs: usize = block.txs.len();
        let cached_hit = cached.is_some();
        tracing::debug!(
            height = block.height,
            total_txs,
            cached_hit,
            "on_commit executing"
        );

        // Resolve, bind, apply (gate ACCEL Stage B): the block's state change
        // lands as ONE atomic batch (rows, SMT nodes, root, executed cursor)
        // through the single choke point. A cached hit applies the vote-time
        // write set; a miss re-executes once in the overlay and refuses
        // BEFORE applying if the computed root does not match the header.
        let outcomes = novai_node::exec_apply::resolve_and_apply_block(db, block, cached)?;

        // Per-tx outcome logs, at the per-tx executor's levels.
        for (tx, outcome) in block.txs.iter().zip(outcomes.iter()) {
            match outcome {
                novai_execution::TxOutcome::Applied => tracing::debug!(
                    height = block.height,
                    from = ?&tx.from[..4],
                    nonce = tx.nonce,
                    "Executed tx"
                ),
                novai_execution::TxOutcome::Skipped => tracing::warn!(
                    height = block.height,
                    from = ?&tx.from[..4],
                    nonce = tx.nonce,
                    "Tx execution skipped root-neutrally (committed)"
                ),
            }
        }

        // Advance nonces for ALL committed transactions, regardless of
        // execution success. A consensus-committed tx permanently occupies
        // its nonce slot — if we only advanced on success, a single failed
        // tx would stall drain_ready (which requires nonce == expected)
        // and block all future txs from that sender indefinitely.
        {
            let mut map = self
                .nonce_provider
                .expected
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for tx in &block.txs {
                let entry = map.entry(tx.from).or_insert(tx.nonce);
                if tx.nonce >= *entry {
                    *entry = tx.nonce + 1;
                }
            }
        }

        // Queue committed txs for deferred mempool removal. The propose loop
        // drains this at the top of each tick (~100 ms typical). Even in the
        // gap before drain runs, drain_ready's stale-evict path at
        // crates/mempool/src/lib.rs prevents reselection because the
        // nonce-advance block above has already moved expected past every
        // committed tx's nonce. The append is done under a tiny critical
        // section that does not hold any other lock, so it cannot participate
        // in the propose loop's mempool->state->db ordering.
        //
        // Gate SOAK A2: each entry now carries the sender alongside the txid.
        // The nonce-advance block above just moved this sender's expected
        // nonce, which is the only moment a pooled transaction can BECOME
        // dead-past, so the drain side uses the sender to run the dead-past
        // eviction at exactly that moment. This is deliberately a wider
        // element in the SAME queue rather than a second queue: it adds no
        // lock, no allocation beyond the tuple, and no write of any kind to
        // the commit path.
        {
            let mut pending = self
                .pending_mempool_removals
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for tx in &block.txs {
                if let Ok(id) = txid_v1(tx) {
                    pending.push((id, tx.from));
                }
            }
        }

        // Publish commit metrics for /metrics scrapes. block_tx_count is a
        // gauge of the last committed block's tx count; total_txs_committed is
        // a monotonic counter across all commits. Relaxed ordering: the
        // /metrics reader does not synchronize with consensus state, so a
        // briefly-stale scrape is acceptable.
        self.commit_metrics
            .block_tx_count
            .store(block.txs.len() as u64, Ordering::Relaxed);
        self.commit_metrics
            .total_txs_committed
            .fetch_add(total_txs as u64, Ordering::Relaxed);

        // H-07 (periodic purge of expired governance proposals) is intentionally
        // not wired here: purge_expired_proposals deletes rows the SMT root still
        // commits to (raw delete, no root update), and its commit-batch-boundary
        // trigger is not deterministic across nodes or startup replay. Proposal
        // expiry must return as a deterministic state transition that keeps rows
        // and root consistent and restores the H-07 growth bound. See the doc
        // comment on novai_execution::purge_expired_proposals.

        // Update blockchain index for block explorer queries.
        // Cap at 100K entries to prevent unbounded memory growth
        // (2.5M blocks × ~120 bytes/entry = 300MB without cap).
        const MAX_INDEX_ENTRIES: usize = 100_000;
        if let Ok(mut idx) = self.blockchain_index.lock() {
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

            // Evict old entries when index exceeds cap
            if idx.block_hashes.len() > MAX_INDEX_ENTRIES {
                let cutoff = idx
                    .committed_height
                    .saturating_sub(MAX_INDEX_ENTRIES as u64);
                idx.block_hashes.retain(|_, &mut h| h > cutoff);
                idx.tx_receipts.retain(|_, &mut (h, _)| h > cutoff);
            }
        }

        // CRASH-SAFE COMMIT/EXECUTE: the executed_height cursor rides the
        // block's atomic execution batch inside resolve_and_apply_block (gate
        // ACCEL Stage B), so rows, SMT nodes, root, and cursor move together
        // or not at all. On startup, if executed_height is behind
        // committed_height, the missing blocks are replayed before the node
        // rejoins consensus. Without this cursor, a crash between
        // persist_commit_atomic (which advances committed_height) and the
        // execution batch leaves account/SMT state behind committed state
        // forever, producing permanent state-root divergence.

        // Force compaction over the pruned block/QC range every COMPACT_INTERVAL
        // heights. persist_commit_atomic writes Delete tombstones for blocks and
        // QCs older than PRUNE_RETAIN_BLOCKS, but RocksDB only frees the
        // underlying SST bytes when a compaction visits the range. Without this,
        // disk usage grows far beyond the retention window (observed: 6.3GB per
        // node at 4M blocks, expected ~88MB).
        const COMPACT_INTERVAL: u64 = 5_000;
        const SAFETY_MARGIN: u64 = 100; // never compact too close to the live tail
        if block.height % COMPACT_INTERVAL == 0
            && block.height > novai_consensus::PRUNE_RETAIN_BLOCKS + SAFETY_MARGIN
        {
            let prune_below = block.height - novai_consensus::PRUNE_RETAIN_BLOCKS - SAFETY_MARGIN;
            let block_start = block_key(0);
            let block_end = block_key(prune_below);
            let qc_start = qc_key(0);
            let qc_end = qc_key(prune_below);
            // Bug 1 latent concern B (docs/gate3-bug1-diagnosis.md Risk 2):
            // synchronously flush the default-CF memtable to L0 BEFORE the
            // compaction runs. RocksDB's WAL fsync is bandwidth-triggered
            // (set_bytes_per_sync / set_wal_bytes_per_sync at
            // crates/state/src/rocksdb_kv.rs), so without this flush a
            // crash between the executor's apply_batch and the next
            // bandwidth-triggered fsync could lose ops still resident in
            // the memtable, and the subsequent compaction would not see
            // them either. A flush failure here is logged but not fatal;
            // the compaction can still proceed over whatever is durable.
            if let Err(e) = db.flush_default() {
                tracing::warn!(
                    height = block.height,
                    error = %e,
                    "Pre-compaction flush failed; proceeding with compaction anyway"
                );
            }
            db.compact_range_default(Some(&block_start), Some(&block_end));
            db.compact_range_default(Some(&qc_start), Some(&qc_end));
            tracing::info!(
                height = block.height,
                prune_below,
                "Forced compaction on pruned block/QC range"
            );
        }

        // Gate F5 Stage 2: the snapshot checkpoint hook. Deliberately the LAST
        // thing on this path and deliberately tiny. It returns immediately
        // unless a snapshot has been asked for and no fresh one is cached, and
        // even then its only work is a RocksDB checkpoint: a memtable flush
        // plus hard links, cost independent of database size. The audit, the
        // key scan, the leaf extraction and the SMT rebuild all happen on the
        // background thread against the created checkpoint, which is reachable
        // by path alone and holds no handle to this database.
        self.snapshot_producer.on_commit(db, block.height);

        Ok(())
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
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Load an Ed25519 signing key from a 32-byte seed file.
fn load_key_file(path: &str) -> ed25519_dalek::SigningKey {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| fatal(format!("Failed to read key file {path}: {e}")));
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
        .unwrap_or_else(|e| fatal(format!("Failed to create key file {path}: {e}")));

    // Set 0600 permissions before writing
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        file.set_permissions(perms)
            .unwrap_or_else(|e| fatal(format!("Failed to set permissions on {path}: {e}")));
    }

    file.write_all(seed)
        .unwrap_or_else(|e| fatal(format!("Failed to write key file {path}: {e}")));
}

/// Parse genesis.json and extract validator set (pubkeys + addresses).
fn parse_genesis_validator_set(
    genesis_path: &str,
) -> (Vec<Address>, HashMap<Address, ed25519_dalek::VerifyingKey>) {
    let json = std::fs::read_to_string(genesis_path)
        .unwrap_or_else(|e| fatal(format!("Failed to read genesis file {genesis_path}: {e}")));
    let parsed: serde_json::Value = serde_json::from_str(&json)
        .unwrap_or_else(|e| fatal(format!("Failed to parse genesis JSON {genesis_path}: {e}")));

    let validators = parsed["validators"]
        .as_array()
        .unwrap_or_else(|| fatal("genesis.json missing 'validators' array"));

    let mut validator_set = Vec::new();
    let mut validator_pubkeys = HashMap::new();

    for (i, v) in validators.iter().enumerate() {
        let pubkey_hex = v["pubkey"]
            .as_str()
            .unwrap_or_else(|| fatal(format!("Validator {i} missing 'pubkey' field")));

        let pubkey_bytes = hex::decode(pubkey_hex)
            .unwrap_or_else(|e| fatal(format!("Validator {i} pubkey invalid hex: {e}")));

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
            .unwrap_or_else(|e| fatal(format!("Validator {i} pubkey invalid Ed25519: {e:?}")));

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
///
/// **Updates the SMT** so the genesis state_root reflects the funded accounts.
/// Without this, every node has the funded accounts in its DB but NOT in its
/// SMT — the SMT root only authenticates txs that touched the account, not the
/// genesis balance. While this is consistent across validators (they all do the
/// same wrong thing), it breaks any external state-root verification and is a
/// latent determinism risk if the on-chain state ever has to be reconstructed
/// from canonical genesis data.
fn apply_dev_genesis(storage: &mut Storage) {
    // Skip if DB already has state (restart)
    if storage.get(KEY_COMMITTED_HEIGHT).ok().flatten().is_some() {
        return;
    }

    const FUNDED_ACCOUNTS: usize = 100;
    const INITIAL_BALANCE: u128 = 1_000_000_000;

    let mut state_ops = Vec::with_capacity(FUNDED_ACCOUNTS);

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
        state_ops.push(WriteOp::Put(
            account_key(&addr),
            encode_account_v1(&account).to_vec(),
        ));
    }

    // Compute SMT root over the genesis accounts and bundle SMT node writes
    // with the state writes in a single atomic batch. This makes the SMT
    // consistent with the DB from block 0.
    let mut all_ops = state_ops.clone();
    let state_root =
        novai_execution::append_smt_ops_for_state_ops(&*storage, &state_ops, &mut all_ops)
            .expect("dev genesis SMT computation failed");

    storage
        .apply_batch(&all_ops)
        .expect("dev genesis write failed");
    tracing::info!(
        accounts = FUNDED_ACCOUNTS,
        balance = INITIAL_BALANCE,
        state_root = ?&state_root[..4],
        "Dev genesis: funded tx-generator sender accounts (with SMT root)"
    );
}

/// Replay any committed-but-unexecuted blocks before joining consensus.
///
/// CRASH-SAFE COMMIT/EXECUTE INVARIANT:
/// - `KEY_COMMITTED_HEIGHT` (consensus layer) is durably bumped by
///   `persist_commit_atomic` BEFORE `execute_committed_blocks` dispatches the
///   txs. A crash in that window leaves committed_height ahead of executed
///   state, producing permanent state-root divergence on restart.
/// - `KEY_EXECUTED_HEIGHT` (execution layer) is bumped at the END of
///   on_commit, after every tx in every block has been dispatched.
///
/// On startup, if executed < committed, we reload the missing blocks from
/// disk and feed them through `dispatch_tx`. Replay is naturally idempotent:
/// a tx whose effects were already persisted will fail nonce/balance/exists
/// checks and be silently skipped (same code path as the existing on_commit
/// "Tx execution failed (committed, skipping)" branch).
///
/// Returns the post-replay executed_height.
fn replay_unexecuted_blocks(storage: &mut Storage) -> u64 {
    let committed_height = match storage.get(KEY_COMMITTED_HEIGHT) {
        Ok(Some(bytes)) if bytes.len() == 8 => {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&bytes);
            u64::from_be_bytes(arr)
        }
        _ => return 0, // No committed state → nothing to replay
    };

    let executed_height = match storage.get(KEY_EXECUTED_HEIGHT) {
        Ok(Some(bytes)) if bytes.len() == 8 => {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&bytes);
            u64::from_be_bytes(arr)
        }
        // Missing cursor = legacy DB or fresh DB. Treat as "everything up to
        // committed_height has already been executed" — we can't reconstruct
        // earlier intent, and the dispatch_tx replay is idempotent anyway,
        // so on a legacy DB we simply mark the cursor up-to-date and proceed.
        _ => {
            if committed_height > 0 {
                tracing::info!(
                    committed_height,
                    "No executed_height cursor found — initializing to committed_height \
                     (assuming no prior crash). Future commits will track this cursor."
                );
                let _ = storage.put(KEY_EXECUTED_HEIGHT, &committed_height.to_be_bytes());
            }
            return committed_height;
        }
    };

    if executed_height >= committed_height {
        return executed_height;
    }

    let count = committed_height - executed_height;
    tracing::warn!(
        executed_height,
        committed_height,
        count,
        "REPLAY: executed_height behind committed_height — replaying missing blocks"
    );

    if let Err(msg) = replay_range(storage, executed_height, committed_height) {
        fatal(msg);
    }

    tracing::info!(
        replayed_blocks = count,
        new_executed_height = committed_height,
        "REPLAY complete"
    );

    committed_height
}

/// Replay blocks `(executed_height + 1)..=committed_height` through the Stage
/// B applier: re-execute each block in the non-persisting overlay over
/// current state, REFUSE BEFORE APPLYING on a header mismatch, then land
/// rows, SMT nodes, root, and the executed cursor for that height as ONE
/// atomic batch (per-block cursor granularity, strictly finer than the old
/// end-of-loop cursor put). A block already applied by an earlier run
/// re-executes with every tx skipping on nonce checks, yielding an empty
/// write set and an unchanged root that equals the header, so the replay
/// stays idempotent.
///
/// Any error is a `REPLAY FAILED` message the caller escalates to `fatal`;
/// nothing is applied for a block whose replayed root does not match its
/// post-state header, so the failure path leaves state untouched.
fn replay_range(
    storage: &mut Storage,
    executed_height: u64,
    committed_height: u64,
) -> Result<(), String> {
    for height in (executed_height + 1)..=committed_height {
        let block = match novai_consensus::ConsensusState::load_block(&*storage, height) {
            Ok(Some(b)) => b,
            Ok(None) => {
                return Err(format!(
                    "REPLAY FAILED: block at height {height} missing from disk \
                     (committed_height={committed_height}, executed_height={executed_height}). \
                     This usually means the DB was wiped while committed_height was retained. \
                     Wipe the data dir and resync from peers."
                ));
            }
            Err(e) => {
                return Err(format!(
                    "REPLAY FAILED: load_block({height}) returned error: {e:?}"
                ));
            }
        };

        let exec = novai_execution::execute_block_to_root(&*storage, &block.txs, height)
            .map_err(|e| format!("REPLAY FAILED: re-execution at height {height}: {e:?}"))?;
        for (tx, outcome) in block.txs.iter().zip(exec.outcomes.iter()) {
            if *outcome == novai_execution::TxOutcome::Skipped {
                tracing::debug!(
                    height,
                    from = ?&tx.from[..4],
                    nonce = tx.nonce,
                    "REPLAY: tx skipped (likely already applied)"
                );
            }
        }

        // Pre-apply refusal (gate wedge-276272 check, moved ahead of the
        // apply by gate ACCEL Stage B): the replayed root must equal the
        // block's post-state header BEFORE anything lands, or boot execution
        // has diverged and nothing may be applied.
        if exec.post_root != block.state_root {
            return Err(format!(
                "REPLAY FAILED: post-execution state root mismatch at height {height} \
                 (executed={:02x?}, header={:02x?}). Local execution diverged from the committed \
                 header. Wipe the data dir and resync from peers.",
                &exec.post_root[..8],
                &block.state_root[..8],
            ));
        }

        novai_node::exec_apply::apply_block_execution(storage, height, exec.write_ops())
            .map_err(|e| format!("REPLAY FAILED: {e}"))?;
    }
    Ok(())
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
            let path = output_path.unwrap_or_else(|| format!("{home}/.novai/data/validator.key"));

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

            println!("{pubkey_hex}");
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
            let mut rpc_bind: Option<String> = None;
            let mut faucet_key_path: Option<String> = None;
            // Repeatable --faucet-trusted-proxy <CIDR> flag. Empty by default,
            // which keeps the safe behavior of ignoring X-Forwarded-For.
            let mut faucet_trusted_proxies_raw: Vec<String> = Vec::new();
            let mut base_timeout_ms: u64 = novai_consensus::BASE_TIMEOUT_MS;
            let mut storage_backend: String = "rocksdb".to_string();
            let mut data_dir: Option<String> = None;
            let mut key_file: Option<String> = None;
            let mut genesis_path: Option<String> = None;
            let mut dev_keys = false;
            let mut no_encryption = false;
            let mut allow_insecure_dev_keys = false;
            let mut proposal_interval_ms: u64 = 100; // Default: 100ms
            let mut _max_timeout_ms: u64 = novai_consensus::MAX_TIMEOUT_MS;
            // F3 runtime wire send cap: 2 MiB default (Phase A); the
            // Phase B deploy raises it to the 16 MiB receive cap by
            // restarting with the flag. Parsed once; SIGHUP is ignored.
            let mut wire_send_cap_bytes: u32 = novai_p2p::MAX_WIRE_MSG_BYTES;
            // Gate F5 Stage 4: sending snapshot messages is OFF unless asked.
            let mut snapshot_send_enabled = false;

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
                    "--rpc-bind" => {
                        rpc_bind =
                            Some(rest.get(i + 1).cloned().expect("missing --rpc-bind value"));
                        i += 2;
                    }
                    "--faucet-key" => {
                        faucet_key_path = Some(
                            rest.get(i + 1)
                                .cloned()
                                .expect("missing --faucet-key value"),
                        );
                        i += 2;
                    }
                    "--faucet-trusted-proxy" => {
                        faucet_trusted_proxies_raw.push(
                            rest.get(i + 1)
                                .cloned()
                                .expect("missing --faucet-trusted-proxy value"),
                        );
                        i += 2;
                    }
                    "--base-timeout" => {
                        base_timeout_ms = parse_u64(rest.get(i + 1).cloned(), "--base-timeout");
                        i += 2;
                    }
                    "--wire-send-cap-bytes" => {
                        let raw = parse_u64(rest.get(i + 1).cloned(), "--wire-send-cap-bytes");
                        wire_send_cap_bytes = u32::try_from(raw).unwrap_or_else(|_| {
                            fatal(format!("--wire-send-cap-bytes {raw} does not fit in u32"))
                        });
                        if let Err(e) =
                            novai_node::consensus_node::validate_wire_send_cap(wire_send_cap_bytes)
                        {
                            fatal(e);
                        }
                        i += 2;
                    }
                    // Gate F5 Stage 4, Phase B. Gates SENDING snapshot messages
                    // only; receiving and serving are on as soon as the binary
                    // is deployed. Default off, because a node that sends one of
                    // the new wire kinds to a peer running an older binary
                    // disconnects that peer.
                    "--snapshot-sync" => {
                        snapshot_send_enabled = true;
                        i += 1;
                    }
                    "--max-timeout" => {
                        // L-04: Parsed for future use. The configurable cap function
                        // timeout_for_round_capped() exists but wiring it through
                        // ConsensusNode requires a config struct refactor (future work).
                        _max_timeout_ms = parse_u64(rest.get(i + 1).cloned(), "--max-timeout");
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

            // M-09: Validate proposal interval vs timeout to prevent consensus deadloop
            if proposal_interval_ms >= base_timeout_ms {
                fatal(format!(
                    "--proposal-interval ({proposal_interval_ms}ms) must be less than \
                     --base-timeout ({base_timeout_ms}ms). A proposal interval >= timeout \
                     causes rounds to expire before votes can return."
                ));
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
                    .unwrap_or_else(|| format!("{home}/.novai/data"));

                let kf = key_file.unwrap_or_else(|| format!("{base}/validator.key"));

                let our_key = load_key_file(&kf);
                let seed = our_key.to_bytes();
                let our_pk = our_key.verifying_key();
                let our_addr = address_from_pubkey(&our_pk);

                let (vs, vp) = parse_genesis_validator_set(&gp);

                if !vs.contains(&our_addr) {
                    let our_pubkey_hex = hex::encode(our_pk.as_bytes());
                    fatal(format!(
                        "Our public key {our_pubkey_hex} is not in the genesis validator set at {gp}"
                    ));
                }

                tracing::info!(key_file = %kf, "Key loaded");
                // Production mode: no precomputed noise keys (peer verification skipped
                // until a mechanism for distributing noise pubkeys is implemented)
                (our_key, vs, vp, seed, Vec::new())
            };

            let our_addr = address_from_pubkey(&our_key.verifying_key());

            // Gate F5 Stage 2: where transient snapshot checkpoints live. Set
            // beside the database directory by the rocksdb arm below; empty for
            // the in-memory backend, which cannot checkpoint at all.
            let mut snapshot_work_dir = String::new();

            // Build storage backend
            let storage = match storage_backend.as_str() {
                "rocksdb" => {
                    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
                    let base = data_dir
                        .as_deref()
                        .map(String::from)
                        .unwrap_or_else(|| format!("{home}/.novai/data"));
                    // Use validator index for dev-keys, address prefix for production
                    let db_subdir = if dev_keys {
                        format!(
                            "validator-{}",
                            validator_idx.expect("validator_idx set in dev-keys")
                        )
                    } else {
                        format!("validator-{}", &hex::encode(our_addr)[..16])
                    };
                    let db_path = format!("{base}/{db_subdir}");
                    // BESIDE the database directory, never inside it: a
                    // checkpoint nested under the live database would be walked
                    // by anything that scans the data directory and could be
                    // mistaken for chain state.
                    snapshot_work_dir = format!("{base}/snapshot-work");

                    // Gate F5 Stage 3: complete a staged snapshot install
                    // BEFORE the database is opened and before the directory
                    // is created, single threaded. The boot path re-runs the
                    // full audit against the exact bytes it is about to
                    // install, merges this node's own vote and lock marks so
                    // neither can regress, and commits with one rename. It is
                    // idempotent under a crash at any step and never deletes
                    // anything.
                    match novai_node::snapshot::install::complete_install_at_boot(
                        std::path::Path::new(&base),
                        &db_subdir,
                    ) {
                        Ok(outcome) => match outcome {
                            novai_node::snapshot::install::InstallOutcome::Nothing => {}
                            other => tracing::warn!(?other, "Snapshot install boot path"),
                        },
                        // A refused rename is the one thing that must not be
                        // papered over: it would leave the node booting from a
                        // directory whose identity is now ambiguous.
                        Err(e) => fatal(format!("Snapshot install failed at boot: {e}")),
                    }

                    std::fs::create_dir_all(&db_path).unwrap_or_else(|e| {
                        fatal(format!("Failed to create data dir {db_path}: {e}"))
                    });

                    tracing::info!(backend = "RocksDB", path = %db_path, "Storage initialized");

                    let rocks = RocksKv::open(&db_path).unwrap_or_else(|e| {
                        fatal(format!("Failed to open RocksDB at {db_path}: {e}"))
                    });
                    Storage::Rocks(rocks)
                }
                "memory" => {
                    tracing::warn!("Storage: MEMORY (volatile — state lost on restart!)");
                    Storage::Memory(MemKv::new())
                }
                other => {
                    fatal(format!(
                        "unknown --storage value: {other} (expected: rocksdb | memory)"
                    ));
                }
            };

            // Fund tx-generator sender accounts on first start (dev-keys only)
            let mut storage = storage;
            if dev_keys {
                apply_dev_genesis(&mut storage);
            }

            // Replay any committed-but-unexecuted blocks before joining
            // consensus. Closes the persist_commit_atomic + execute_committed_blocks
            // crash window that produces permanent state-root divergence.
            replay_unexecuted_blocks(&mut storage);

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

            // F3: apply the runtime wire send cap (validated at parse
            // time; re-validated by the setter). The value lands on the
            // PeerManager the encoder reads, which is the same value the
            // proposer guard and the responder budget read.
            if let Err(e) = node.set_wire_send_cap(wire_send_cap_bytes) {
                fatal(e);
            }
            if wire_send_cap_bytes != novai_p2p::MAX_WIRE_MSG_BYTES {
                tracing::info!(
                    wire_send_cap_bytes,
                    "wire send cap raised above the default (F3 Phase B)"
                );
            }

            // Gate F5 Stage 4. Receiving and serving are already live; this
            // decides only whether snapshot messages LEAVE this node. Logged
            // either way, because "which phase is this node in" is the first
            // question anyone asks during the two-phase deploy.
            node.set_snapshot_send_enabled(snapshot_send_enabled);
            if snapshot_send_enabled {
                tracing::warn!(
                    "snapshot sync SENDING enabled (F5 Phase B); every peer must already \
                     be running a binary that can receive the snapshot wire kinds"
                );
            } else {
                tracing::info!(
                    "snapshot sync sending disabled (F5 Phase A: receive and serve only)"
                );
            }

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
                let db_guard = node
                    .db
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Err(e) = nonce_provider.seed_from_state(&db_guard) {
                    fatal(format!("nonce seed-from-state failed: {e}"));
                }
            }

            // Shared blockchain index for block explorer RPC endpoints
            let blockchain_index = Arc::new(Mutex::new(rpc::BlockchainIndex::new()));

            // Create mempool early so we can wire gossip before Arc-wrapping the node.
            // Moved above the commit callback so the deferred-removal queue (below)
            // can be shared by both the callback (append on commit) and the propose
            // loop (drain under the mempool lock).
            let mempool = Arc::new(Mutex::new(TxMempool::new(1, 1000)));

            // Deferred queue for committed txids. on_commit cannot remove from
            // the mempool inline (AB-BA with the propose loop's mempool->db
            // ordering), so it appends here and the propose loop drains.
            let pending_mempool_removals: Arc<Mutex<Vec<(TxId, Address)>>> =
            Arc::new(Mutex::new(Vec::new()));

            // Shared atomic accumulators for the Prometheus commit metrics.
            // on_commit publishes the per-commit values; the /metrics collect
            // closure reads on each scrape.
            let commit_metrics = Arc::new(CommitMetrics::default());

            // Gate F5 Stage 2: transient checkpoints live BESIDE the database
            // directory, never inside it, so a checkpoint can never be mistaken
            // for chain state and RocksDB never sees a nested database.
            let snapshot_producer = Arc::new(
                novai_node::snapshot::producer::SnapshotProducer::new(
                    std::path::PathBuf::from(&snapshot_work_dir),
                ),
            );

            // Wire execution commit callback
            let commit_callback = Arc::new(ExecutionCommitCallback {
                nonce_provider: Arc::clone(&nonce_provider),
                blockchain_index: Arc::clone(&blockchain_index),
                pending_mempool_removals: Arc::clone(&pending_mempool_removals),
                commit_metrics: Arc::clone(&commit_metrics),
                snapshot_producer: Arc::clone(&snapshot_producer),
            });
            node.set_commit_callback(commit_callback);
            // Gate F5 Stage 4: the node serves cached bundles from the same
            // producer the commit hook feeds. Attached here so serving and
            // producing can never disagree about which bundle is current.
            node.set_snapshot_producer(Arc::clone(&snapshot_producer));

            // Wire gossip: allows peers to insert received txs into our mempool
            node.set_gossip_mempool(
                Arc::clone(&mempool),
                Arc::clone(&nonce_provider) as Arc<dyn NonceProvider + Send + Sync>,
            );

            let node = Arc::new(node);

            // Start listener
            let bind_addr = format!("127.0.0.1:{port}")
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
            // H-03: Log all resolved IPs so operators can audit for DNS poisoning.
            for seed in &seeds {
                use std::net::ToSocketAddrs;
                match seed.to_socket_addrs() {
                    Ok(addrs) => {
                        let resolved: Vec<_> = addrs.collect();
                        // Log ALL resolved addresses for operator auditing
                        let ip_list: Vec<String> = resolved
                            .iter()
                            .map(std::string::ToString::to_string)
                            .collect();
                        tracing::info!(
                            seed = %seed,
                            resolved_ips = ?ip_list,
                            "DNS seed resolved — verify IPs match expected infrastructure"
                        );

                        let mut connected = false;
                        for addr in &resolved {
                            match node.connect_to_peer(*addr) {
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
                        tracing::warn!(seed = %seed, %e, "Failed to resolve seed node DNS — check DNS configuration");
                    }
                }
            }

            // M-04: Log genesis identity hash so operators can verify chain identity.
            {
                let db_guard = node.db.lock_or_recover();
                if let Ok(Some(root_bytes)) = db_guard.get(novai_state::KEY_SMT_ROOT) {
                    if let Ok(root) = novai_state::decode_smt_root_v1(&root_bytes) {
                        tracing::info!(
                            genesis_hash = %hex::encode(root),
                            "Chain identity — compare with other validators to verify same genesis"
                        );
                    }
                }
            }

            tracing::info!("Node started, waiting for peers...");
            std::thread::sleep(Duration::from_millis(500));

            // Graceful shutdown flag — created early so AI service can share it
            let shutdown = Arc::new(AtomicBool::new(false));

            // Gate F5 Stage 2: the off-lock snapshot production thread. It owns
            // no handle to the database and never takes the commit lock; it
            // only ever opens, by path, the checkpoint the commit hook created.
            // That is what keeps the audit, the key scan and the SMT rebuild
            // off the consensus path. Idle cost is one atomic read per 250 ms,
            // because nothing is ever pending unless a snapshot was asked for.
            if !snapshot_work_dir.is_empty() {
                let producer = Arc::clone(&snapshot_producer);
                let producer_shutdown = Arc::clone(&shutdown);
                std::thread::Builder::new()
                    .name("snapshot-producer".into())
                    .spawn(move || {
                        while !producer_shutdown.load(Ordering::Relaxed) {
                            if producer.has_pending() {
                                let _ = producer.run_pending_production();
                            } else {
                                std::thread::sleep(Duration::from_millis(250));
                            }
                        }
                    })
                    .expect("spawn snapshot producer thread");
            }

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
                let commit_metrics = Arc::clone(&commit_metrics);
                // Gate F5 Stage 1: the snapshot-sync detection phase for the
                // novai_sync_mode gauge. Read through the retry lock the same
                // way the other collectors read theirs, so the consensus path
                // stays untouched.
                let sync_retry = Arc::clone(&node.sync_retry);
                // Gate F5 Stage 2: snapshot production cost and cached height.
                let producer_metrics = Arc::clone(&snapshot_producer);
                // WEDGE-20260718: commit age clock for the
                // novai_seconds_since_last_commit gauge, owned by the
                // collector so the consensus path stays untouched.
                let commit_clock = Mutex::new(metrics::CommitClock::new());
                move || {
                    // Acquire state lock once for all state fields
                    let (committed_height, current_round, view_changes_total, highest_qc_height) = {
                        let s = state.lock_or_recover();
                        (
                            s.committed_height,
                            s.round,
                            s.view_changes_total,
                            s.highest_qc.as_ref().map_or(0, |q| q.height),
                        )
                    };
                    let seconds_since_last_commit =
                        commit_clock.lock_or_recover().observe(committed_height);
                    metrics::MetricsSnapshot {
                        committed_height,
                        highest_qc_height,
                        seconds_since_last_commit,
                        sync_mode: sync_retry.lock_or_recover().snapshot_sync.gauge(),
                        snapshot_produce_seconds: producer_metrics.last_checkpoint_seconds(),
                        snapshot_background_seconds: producer_metrics.last_background_seconds(),
                        snapshot_height: producer_metrics.cached_height(),
                        current_round,
                        peer_count: peer_manager.peer_count() as u64,
                        mempool_size: mempool.lock_or_recover().len() as u64,
                        // Gate SOAK C1/C2: read the cached census and the
                        // admission counters lock-free.
                        mempool_ready: metrics::pool_metrics::READY.load(Ordering::Relaxed),
                        mempool_waiting: metrics::pool_metrics::WAITING.load(Ordering::Relaxed),
                        mempool_gapped: metrics::pool_metrics::GAPPED.load(Ordering::Relaxed),
                        mempool_senders: metrics::pool_metrics::SENDERS.load(Ordering::Relaxed),
                        mempool_rejects_nonce_too_low: metrics::pool_metrics::REJ_NONCE_TOO_LOW
                            .load(Ordering::Relaxed),
                        mempool_rejects_nonce_too_high: metrics::pool_metrics::REJ_NONCE_TOO_HIGH
                            .load(Ordering::Relaxed),
                        mempool_rejects_sender_limit: metrics::pool_metrics::REJ_SENDER_LIMIT
                            .load(Ordering::Relaxed),
                        mempool_rejects_fee_too_low: metrics::pool_metrics::REJ_FEE_TOO_LOW
                            .load(Ordering::Relaxed),
                        mempool_rejects_full: metrics::pool_metrics::REJ_FULL
                            .load(Ordering::Relaxed),
                        view_changes_total,
                        block_tx_count: commit_metrics.block_tx_count.load(Ordering::Relaxed),
                        total_txs_committed: commit_metrics
                            .total_txs_committed
                            .load(Ordering::Relaxed),
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

            let metrics_addr = format!("0.0.0.0:{metrics_port}");
            if let Err(e) = metrics::start_metrics_server(&metrics_addr, metrics_collect) {
                tracing::error!(%e, "Failed to start metrics server");
            }

            // Start RPC server with state access (transaction submission + queries)
            let rpc_host = rpc_bind.unwrap_or_else(|| "127.0.0.1".to_string());
            if rpc_host == "0.0.0.0" {
                tracing::warn!(
                    "RPC server binding to 0.0.0.0 — ALL interfaces publicly accessible. \
                     Use --rpc-bind 127.0.0.1 for local-only access."
                );
            }
            let rpc_addr = format!("{rpc_host}:{rpc_port}");

            // C-04: Load faucet key from file if provided, else use deterministic
            // dev key (acceptable for local dev), else disable faucet.
            let faucet_key: Option<ed25519_dalek::SigningKey> =
                if let Some(ref path) = faucet_key_path {
                    let key = load_key_file(path);
                    tracing::info!("Faucet key loaded from {}", path);
                    Some(key)
                } else if dev_keys {
                    tracing::info!("Faucet using DETERMINISTIC dev key (dev-mode only)");
                    None // rpc.rs will derive it internally
                } else {
                    tracing::info!("Faucet disabled (no --faucet-key and not in dev-mode)");
                    None
                };

            // Parse --faucet-trusted-proxy CIDR blocks. Invalid CIDRs are a
            // hard startup error: the operator opted in explicitly, so silent
            // misconfiguration would be worse than a refuse-to-start.
            let faucet_trusted_proxies: Vec<rpc::CidrBlock> = faucet_trusted_proxies_raw
                .iter()
                .map(|s| match rpc::CidrBlock::parse(s) {
                    Ok(cidr) => {
                        tracing::info!("Faucet trusted-proxy CIDR: {}", s);
                        cidr
                    }
                    Err(e) => fatal(format!("invalid --faucet-trusted-proxy '{s}': {e}")),
                })
                .collect();
            if faucet_trusted_proxies.is_empty() {
                tracing::info!(
                    "Faucet X-Forwarded-For parsing DISABLED (no --faucet-trusted-proxy configured)"
                );
            }

            // Derive the persistent faucet rate-limit path from the same
            // data-dir convention used by the RocksDB store, so a multi-node
            // devnet does not have nodes overwriting each other's state.
            let faucet_rate_limit_path = {
                let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
                let base = data_dir
                    .as_deref()
                    .map(String::from)
                    .unwrap_or_else(|| format!("{home}/.novai/data"));
                let subdir = if dev_keys {
                    format!(
                        "validator-{}",
                        validator_idx.expect("validator_idx set in dev-keys")
                    )
                } else {
                    format!("validator-{}", &hex::encode(our_addr)[..16])
                };
                std::path::PathBuf::from(format!("{base}/{subdir}/faucet_rate_limit.json"))
            };

            if let Err(e) = rpc::start_rpc_server_with_state(
                &rpc_addr,
                Arc::clone(&mempool),
                Arc::clone(&nonce_provider) as Arc<dyn NonceProvider + Send + Sync>,
                Arc::clone(&node.db),
                dev_keys,
                Arc::clone(&blockchain_index),
                faucet_key,
                faucet_trusted_proxies,
                faucet_rate_limit_path,
                Some(Arc::clone(&node.peer_manager)),
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
            let mut last_census = std::time::Instant::now();
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
                    let timed_out = {
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
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    };
                    // F1: a timeout is a failed cycle exactly like a served
                    // empty response; record the strike (after releasing the
                    // pending lock) so the retry gate backs off.
                    if timed_out {
                        node.on_sync_request_timeout();
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
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
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

                // Gate SOAK C1: refresh the cached pool census every 5
                // seconds. READ ONLY: it evicts nothing, and no eviction
                // decision consults it. Cached here rather than computed at
                // scrape time so the scrape never holds the pool mutex.
                if last_census.elapsed() >= Duration::from_secs(5) {
                    last_census = std::time::Instant::now();
                    let census = {
                        let mp = mempool.lock_or_recover();
                        mp.census(&*nonce_provider)
                    };
                    metrics::pool_metrics::publish_census(&census);
                }

                // Gate SOAK A3: the 30 second / 120 second age purge that used
                // to run here is gone. Eviction is event driven now: a
                // transaction becomes provably dead at the commit that
                // advances its sender's expected nonce past it, and the
                // deferred-removal drain below evicts it at exactly that
                // moment on every node. No clock participates in any eviction
                // decision, so a deep backlog of transactions that are still
                // waiting their turn survives indefinitely, including through
                // a commit stall.

                // Propose every proposal_interval_ms (must be less than BASE_TIMEOUT_MS for consensus to work)
                if last_proposal_attempt.elapsed() >= Duration::from_millis(proposal_interval_ms) {
                    last_proposal_attempt = std::time::Instant::now();

                    // Drain committed-tx removals queued by on_commit. Deferred
                    // here to dodge the AB-BA cycle: on_commit fires while a
                    // peer thread holds db, and the propose loop holds mempool
                    // and acquires db inside try_propose_block; an inline lock
                    // on the mempool inside on_commit would deadlock. At ~5 tps
                    // across 4 nodes, the queue gains a handful of entries
                    // between ticks and drains the same tick, so the peak length
                    // is in the single digits under steady load.
                    {
                        let mut pending = pending_mempool_removals.lock_or_recover();
                        if !pending.is_empty() {
                            let mut mp = mempool.lock_or_recover();
                            let mut touched: BTreeSet<Address> = BTreeSet::new();
                            for (txid, from) in pending.drain(..) {
                                mp.remove(&txid);
                                touched.insert(from);
                            }
                            // Gate SOAK A2: the commit that produced these
                            // removals also advanced each sender's expected
                            // nonce, which is the only event that can turn a
                            // pooled transaction into a dead-past one. Sweep
                            // exactly those senders, now, on EVERY node.
                            //
                            // Before this, the only paths that reclaimed a
                            // dead-past slot were the leader's drain_ready
                            // (which never runs on a follower) and the age
                            // purge. A transaction whose txid never committed
                            // (a same-nonce loser, say) is unreachable by the
                            // removal queue above, so on a follower it held
                            // its sender's slot indefinitely.
                            //
                            // Lock order is mempool then nonce map, which is
                            // the order already taken by the abandoned-tx
                            // recovery immediately below and by drain_ready
                            // inside try_propose_block.
                            let mut evicted = 0usize;
                            for from in touched {
                                evicted += mp
                                    .evict_dead_past(&from, nonce_provider.expected_nonce(&from));
                            }
                            if evicted > 0 {
                                tracing::debug!(evicted, "Evicted dead-past txs after commit");
                            }
                        }
                    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use novai_consensus_types::Block;
    use novai_crypto::sign_tx_v1;
    use novai_node::consensus_node::CommitCallback;
    use novai_state::MemKv;

    /// Exercises the full deferred-removal path that fixes the
    /// duplicate-inclusion bug: on_commit appends committed txids to the
    /// pending-removals queue, then the propose-loop drain pulls each one
    /// out of the mempool.
    ///
    /// Three signed transfer txs are reinserted to a fresh mempool. A block
    /// containing only T1 and T2 is fed to on_commit. The queue must hold
    /// exactly [txid(T1), txid(T2)] afterward; the existing nonce-advance
    /// step must move expected past T2; the drain step then strips T1 and
    /// T2 from the mempool while leaving T3 untouched.
    #[test]
    fn on_commit_queues_committed_txs_and_drain_removes_them() {
        let nonce_provider = Arc::new(InMemoryNonceProvider::new());
        let blockchain_index = Arc::new(Mutex::new(rpc::BlockchainIndex::new()));
        let pending_mempool_removals: Arc<Mutex<Vec<(TxId, Address)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let callback = ExecutionCommitCallback {
            nonce_provider: Arc::clone(&nonce_provider),
            blockchain_index: Arc::clone(&blockchain_index),
            pending_mempool_removals: Arc::clone(&pending_mempool_removals),
            commit_metrics: Arc::new(CommitMetrics::default()),
            // Gate F5 Stage 2: a producer with no demand set, so its
            // commit-path hook returns Skipped and these tests exercise the
            // commit path exactly as before.
            snapshot_producer: Arc::new(
                novai_node::snapshot::producer::SnapshotProducer::new(
                    std::path::PathBuf::from("unused-no-demand"),
                ),
            ),
        };

        let (sk, pk) = generate_keypair();
        let from = address_from_pubkey(&pk);
        let pubkey = pk.to_bytes();

        let mut t1 = build_tx(from, pubkey, 0, 1, String::new());
        let mut t2 = build_tx(from, pubkey, 1, 1, String::new());
        let mut t3 = build_tx(from, pubkey, 2, 1, String::new());
        sign_tx_v1(&sk, &mut t1).expect("sign t1");
        sign_tx_v1(&sk, &mut t2).expect("sign t2");
        sign_tx_v1(&sk, &mut t3).expect("sign t3");

        let id1 = txid_v1(&t1).expect("txid t1");
        let id2 = txid_v1(&t2).expect("txid t2");
        let id3 = txid_v1(&t3).expect("txid t3");

        let mempool = Arc::new(Mutex::new(TxMempool::new(1, 1000)));
        {
            let mut mp = mempool
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            mp.reinsert_unchecked(t1.clone()).expect("reinsert t1");
            mp.reinsert_unchecked(t2.clone()).expect("reinsert t2");
            mp.reinsert_unchecked(t3.clone()).expect("reinsert t3");
            assert!(mp.contains(&id1));
            assert!(mp.contains(&id2));
            assert!(mp.contains(&id3));
        }

        let block = Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            // Post-state header (gate ACCEL Stage B): every tx fails against
            // the empty state and is skipped root-neutrally, so the block's
            // post-state is the empty root. A fake root would now (correctly)
            // refuse before applying.
            state_root: novai_execution::empty_smt_root(),
            txs: vec![t1.clone(), t2.clone()],
        };

        // Txs may fail per-tx (no account state, no balance), but for a
        // committed block the queue-append and nonce-advance side effects
        // still run (a consensus-committed tx permanently occupies its nonce
        // slot), which is what we're verifying.
        let mut storage = Storage::Memory(MemKv::new());
        callback
            .on_commit(&mut storage, &block, None)
            .expect("on_commit");

        // First half of the deferred path: queue must hold the two committed
        // txids in commit order, and the existing nonce-advance must still work.
        {
            let pending = pending_mempool_removals
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(
                pending.len(),
                2,
                "queue should hold exactly the two committed txids"
            );
            assert_eq!(pending[0].0, id1, "queue order must match block tx order");
            assert_eq!(pending[1].0, id2, "queue order must match block tx order");
            // Gate SOAK A2: each entry carries its sender so the drain side
            // can run the dead-past sweep for exactly the senders whose
            // expected nonce just moved. Same queue, same lock, wider element.
            assert_eq!(pending[0].1, from, "queue entry must carry the sender");
            assert_eq!(pending[1].1, from, "queue entry must carry the sender");
        }
        assert_eq!(
            nonce_provider.expected_nonce(&from),
            2,
            "expected_nonce should advance past the last committed nonce",
        );

        // Second half of the deferred path: simulate the propose-loop drain.
        {
            let mut pending = pending_mempool_removals
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut mp = mempool
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut touched: BTreeSet<Address> = BTreeSet::new();
            for (txid, sender) in pending.drain(..) {
                mp.remove(&txid);
                touched.insert(sender);
            }
            for sender in touched {
                mp.evict_dead_past(&sender, nonce_provider.expected_nonce(&sender));
            }
        }

        // Final invariants.
        {
            let mp = mempool
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(!mp.contains(&id1), "T1 should be removed from mempool");
            assert!(!mp.contains(&id2), "T2 should be removed from mempool");
            assert!(mp.contains(&id3), "T3 must remain (not in committed block)");
        }
        {
            let pending = pending_mempool_removals
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(pending.is_empty(), "queue should be drained");
        }
    }

    /// Bisection-proof for the novai_block_tx_count and
    /// novai_total_txs_committed metric wiring. Before the on_commit writes
    /// land, CommitMetrics stays at zero and the assertions below trip; with
    /// the writes wired, block_tx_count tracks the LAST committed block's tx
    /// count and total_txs_committed accumulates monotonically across commits.
    #[test]
    fn on_commit_updates_commit_metrics_block_tx_count_and_total() {
        let nonce_provider = Arc::new(InMemoryNonceProvider::new());
        let blockchain_index = Arc::new(Mutex::new(rpc::BlockchainIndex::new()));
        let pending_mempool_removals: Arc<Mutex<Vec<(TxId, Address)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let commit_metrics = Arc::new(CommitMetrics::default());
        let callback = ExecutionCommitCallback {
            nonce_provider: Arc::clone(&nonce_provider),
            blockchain_index: Arc::clone(&blockchain_index),
            pending_mempool_removals: Arc::clone(&pending_mempool_removals),
            commit_metrics: Arc::clone(&commit_metrics),
            // Gate F5 Stage 2: a producer with no demand set, so its
            // commit-path hook returns Skipped and these tests exercise the
            // commit path exactly as before.
            snapshot_producer: Arc::new(
                novai_node::snapshot::producer::SnapshotProducer::new(
                    std::path::PathBuf::from("unused-no-demand"),
                ),
            ),
        };

        let (sk, pk) = generate_keypair();
        let from = address_from_pubkey(&pk);
        let pubkey = pk.to_bytes();

        let mut t1 = build_tx(from, pubkey, 0, 1, String::new());
        let mut t2 = build_tx(from, pubkey, 1, 1, String::new());
        sign_tx_v1(&sk, &mut t1).expect("sign t1");
        sign_tx_v1(&sk, &mut t2).expect("sign t2");

        let block_a = Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            // Post-state header: all txs skip against the empty state, so the
            // post-state is the empty root (gate ACCEL Stage B).
            state_root: novai_execution::empty_smt_root(),
            txs: vec![t1, t2],
        };

        let mut storage = Storage::Memory(MemKv::new());
        callback
            .on_commit(&mut storage, &block_a, None)
            .expect("on_commit block_a");

        assert_eq!(
            commit_metrics.block_tx_count.load(Ordering::Relaxed),
            2,
            "block_tx_count should reflect the committed block's tx count",
        );
        assert_eq!(
            commit_metrics.total_txs_committed.load(Ordering::Relaxed),
            2,
            "total_txs_committed should accumulate per commit",
        );

        let mut t3 = build_tx(from, pubkey, 2, 1, String::new());
        let mut t4 = build_tx(from, pubkey, 3, 1, String::new());
        let mut t5 = build_tx(from, pubkey, 4, 1, String::new());
        sign_tx_v1(&sk, &mut t3).expect("sign t3");
        sign_tx_v1(&sk, &mut t4).expect("sign t4");
        sign_tx_v1(&sk, &mut t5).expect("sign t5");

        let block_b = Block {
            height: 2,
            round: 0,
            parent_hash: [0u8; 32],
            // Post-state header: all txs skip against the empty state, so the
            // post-state is the empty root (gate ACCEL Stage B).
            state_root: novai_execution::empty_smt_root(),
            txs: vec![t3, t4, t5],
        };
        callback
            .on_commit(&mut storage, &block_b, None)
            .expect("on_commit block_b");

        assert_eq!(
            commit_metrics.block_tx_count.load(Ordering::Relaxed),
            3,
            "block_tx_count should reflect the LAST committed block's tx count, not cumulative",
        );
        assert_eq!(
            commit_metrics.total_txs_committed.load(Ordering::Relaxed),
            5,
            "total_txs_committed should accumulate across multiple commits",
        );
    }

    fn read_root_of(storage: &Storage) -> [u8; 32] {
        match storage.get(novai_state::KEY_SMT_ROOT).expect("get root") {
            Some(b) => novai_state::decode_smt_root_v1(&b).expect("decode root"),
            None => novai_execution::empty_smt_root(),
        }
    }

    fn read_cursor_of(storage: &Storage) -> Option<u64> {
        storage
            .get(KEY_EXECUTED_HEIGHT)
            .expect("get cursor")
            .map(|b| {
                let mut a = [0u8; 8];
                a.copy_from_slice(&b);
                u64::from_be_bytes(a)
            })
    }

    const C_SENDER: Address = [0x11; 32];
    const C_RECIPIENT: Address = [0x22; 32];

    /// Shared Stage B crash-window fixture: a funded store where the empty
    /// H=1 is fully executed (cursor 1) and the tx-bearing H=2 has its COMMIT
    /// batch durable (the real `persist_commit_atomic`: both block records
    /// plus the committed height, one atomic batch) while its EXECUTION batch
    /// deliberately never ran, the simulated crash between the two writes.
    /// Returns the storage, H=2, its pre-computed execution, and the pre-block
    /// root R0.
    fn stageb_crash_window_fixture() -> (
        Storage,
        Block,
        novai_execution::BlockExecution,
        [u8; 32],
    ) {
        let mut storage = Storage::Memory(MemKv::new());

        // Fund through the canonical SMT path so KEY_SMT_ROOT matches the rows.
        for (k, v) in [
            (
                account_key(&C_SENDER),
                encode_account_v1(&AccountStateV1 {
                    balance: 1_000_000,
                    nonce: 0,
                })
                .to_vec(),
            ),
            (
                account_key(&C_RECIPIENT),
                encode_account_v1(&AccountStateV1 {
                    balance: 10_000,
                    nonce: 0,
                })
                .to_vec(),
            ),
            (
                novai_state::KEY_FEE_POOL.to_vec(),
                novai_state::encode_fee_pool_v1(&novai_state::FeePoolV1 { balance: 0 }).to_vec(),
            ),
        ] {
            let ops = vec![novai_state::WriteOp::Put(k, v)];
            let mut all = ops.clone();
            novai_execution::append_smt_ops_for_state_ops(&storage, &ops, &mut all)
                .expect("append smt ops");
            storage.apply_batch(&all).expect("seed batch");
        }
        let r0 = read_root_of(&storage);

        // H=1: empty, fully executed through the applier (cursor 1).
        let block1 = Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: r0,
            txs: vec![],
        };
        novai_node::exec_apply::apply_block_execution(&mut storage, 1, Vec::new())
            .expect("execute H=1");

        // H=2: one transfer, header stamped from the computed post-state.
        // Execution never verifies signatures, so a dummy pubkey is correct.
        let tx = TxV1 {
            version: TxVersion::V1,
            from: C_SENDER,
            pubkey: C_SENDER,
            nonce: 0,
            fee: 1_000,
            payload: novai_execution::encode_transfer_payload_v1(
                &novai_execution::TransferPayloadV1 {
                    to: C_RECIPIENT,
                    amount: 5_000,
                },
            )
            .to_vec(),
            sig: [0u8; 64],
        };
        let exec2 = novai_execution::execute_block_to_root(&storage, std::slice::from_ref(&tx), 2)
            .expect("execute H=2");
        assert!(
            exec2
                .outcomes
                .iter()
                .all(|o| *o == novai_execution::TxOutcome::Applied),
            "the transfer must apply (non-vacuous fixture)"
        );
        assert_ne!(exec2.post_root, r0, "the transfer must move the root");
        let block2 = Block {
            height: 2,
            round: 0,
            parent_hash: novai_consensus_types::block_hash(&block1),
            state_root: exec2.post_root,
            txs: vec![tx],
        };

        // The COMMIT batch, via the real persist_commit_atomic. The execution
        // batch for H=2 deliberately does NOT run (the crash).
        let cs = novai_consensus::ConsensusState::new([0u8; 32]);
        let qc2 = novai_consensus_types::QC {
            height: 2,
            round: 0,
            block_hash: novai_consensus_types::block_hash(&block2),
            votes: vec![],
        };
        cs.persist_commit_atomic(&mut storage, &[block1, block2.clone()], &qc2, 2, None)
            .expect("commit batch");

        (storage, block2, exec2, r0)
    }

    /// Stage B arm c1: the inter-batch crash window is owned by replay. A
    /// node that crashed after the commit batch but before the execution
    /// batch boots with committed ahead of executed; replay re-executes the
    /// missing block through the applier and lands rows, nodes, root, and
    /// cursor as one batch.
    #[test]
    fn stageb_replay_closes_the_inter_batch_crash_window() {
        let (mut storage, block2, _exec2, r0) = stageb_crash_window_fixture();

        // The crash state: committed 2, executed 1, H=2's state absent.
        assert_eq!(read_cursor_of(&storage), Some(1), "crash state: cursor behind");
        assert_eq!(read_root_of(&storage), r0, "crash state: root pre-block");

        replay_range(&mut storage, 1, 2).expect("replay must complete the window");

        let root = read_root_of(&storage);
        assert_eq!(
            root, block2.state_root,
            "the replayed root equals the post-state header"
        );
        assert_ne!(root, r0, "the transfer landed (non-vacuous)");
        assert_eq!(
            read_cursor_of(&storage),
            Some(2),
            "the cursor advanced to the replayed height inside the batch"
        );
        let recipient_row = storage
            .get(&account_key(&C_RECIPIENT))
            .unwrap()
            .expect("recipient row");
        assert_eq!(
            recipient_row,
            encode_account_v1(&AccountStateV1 {
                balance: 15_000,
                nonce: 0,
            })
            .to_vec(),
            "the recipient was credited by the replayed transfer"
        );
    }

    /// Stage B arm c2: the forbidden rows-without-root split is detected and
    /// halts BEFORE applying anything further, never completing silently.
    /// This is the accel-C silent-divergence scenario (rows land, root record
    /// stale, replay skips the already-applied txs) made loud: the skipped
    /// re-execution reproduces the stale root, the pre-apply refusal fires,
    /// and state is left exactly as the poison left it.
    #[test]
    fn stageb_replay_halts_on_rows_without_root_split() {
        let (mut storage, _block2, exec2, r0) = stageb_crash_window_fixture();

        // Hand-poison the forbidden split: H=2's rows and SMT nodes land, the
        // KEY_SMT_ROOT record does NOT, the cursor stays behind. The real
        // applier cannot produce this state (one batch); a two-batch applier
        // bug plus a crash between the batches would.
        let all_ops = exec2.write_ops();
        let poison: Vec<novai_state::WriteOp> = all_ops
            .iter()
            .filter(|op| {
                !matches!(op, novai_state::WriteOp::Put(k, _)
                    if k.as_slice() == novai_state::KEY_SMT_ROOT)
            })
            .cloned()
            .collect();
        assert_eq!(
            poison.len(),
            all_ops.len() - 1,
            "exactly the root record op is withheld"
        );
        storage.apply_batch(&poison).expect("apply poison");

        // Poison sanity: rows advanced while the root record is stale.
        let sender_row = storage
            .get(&account_key(&C_SENDER))
            .unwrap()
            .expect("sender row");
        assert_eq!(
            sender_row,
            encode_account_v1(&AccountStateV1 {
                balance: 1_000_000 - 5_000 - 1_000,
                nonce: 1,
            })
            .to_vec(),
            "sender rows advanced (the poison landed)"
        );
        assert_eq!(read_root_of(&storage), r0, "root record left stale (the split)");
        assert_eq!(read_cursor_of(&storage), Some(1), "cursor behind (the split)");

        // Replay must refuse loudly before applying anything further.
        let err = replay_range(&mut storage, 1, 2)
            .expect_err("rows-without-root must halt replay, never complete silently");
        assert!(
            err.contains("REPLAY FAILED: post-execution state root mismatch at height 2"),
            "the halt must be the root-mismatch refusal; got: {err}"
        );

        // Fail-closed: nothing further landed on the refused path.
        assert_eq!(
            read_cursor_of(&storage),
            Some(1),
            "cursor untouched by the refused replay"
        );
        assert_eq!(
            read_root_of(&storage),
            r0,
            "root record untouched by the refused replay"
        );
    }
}

#[cfg(test)]
mod seed_from_state_tests {
    use super::*;
    use novai_crypto::sign_tx_v1;
    use novai_state::MemKv;

    fn put_account(storage: &mut Storage, addr: &Address, balance: u128, nonce: u64) {
        storage
            .put(
                &account_key(addr),
                &encode_account_v1(&AccountStateV1 { balance, nonce }),
            )
            .expect("put account");
    }

    /// Write an AI entity row plus the address-to-entity reverse index,
    /// the exact rows execution resolves senders through. The code hash is
    /// derived from the pubkey so every entity gets a distinct id.
    fn put_entity(storage: &mut Storage, pubkey: [u8; 32], nonce: u64) -> Address {
        let mut entity = novai_ai_entities::AiEntity::new_with_pubkey(
            pubkey,
            [0x01u8; 32],
            novai_ai_entities::AutonomyMode::Gated,
            novai_ai_entities::Capabilities::gated(),
            pubkey,
            1,
        );
        entity.nonce = nonce;
        let addr = novai_crypto::address_from_pubkey_bytes(&pubkey);
        let ops = vec![
            novai_execution::write_ai_entity_op(&entity),
            WriteOp::Put(
                novai_state::ai_entity_by_address_key(&addr),
                entity.id.to_vec(),
            ),
        ];
        storage.apply_batch(&ops).expect("write entity");
        addr
    }

    fn signed_tx(sk: &ed25519_dalek::SigningKey, nonce: u64) -> TxV1 {
        let pk = sk.verifying_key();
        let mut tx = build_tx(
            address_from_pubkey(&pk),
            pk.to_bytes(),
            nonce,
            1_000,
            String::new(),
        );
        sign_tx_v1(sk, &mut tx).expect("sign");
        tx
    }

    /// The live strand, killed: a non-dev account with on-chain history must
    /// be proposable at its state nonce after a restart. Under dev-range
    /// seeding this fails mechanically: the tx is admitted (nonce >=
    /// expected 0) but drain_ready needs equality, so it is never selected,
    /// ages out at the purge sweep, and each resubmission repeats the cycle.
    #[test]
    fn restart_seeds_non_dev_account_and_tx_at_state_nonce_is_proposable() {
        let (sk, pk) = generate_keypair();
        let addr = address_from_pubkey(&pk);
        let mut storage = Storage::Memory(MemKv::new());
        put_account(&mut storage, &addr, 1_000_000, 45);

        let provider = InMemoryNonceProvider::new();
        provider.seed_from_state(&storage).expect("seed");

        let tx = signed_tx(&sk, 45);
        let id = txid_v1(&tx).expect("txid");
        let mut mp = TxMempool::new(1, 1000);
        mp.insert(tx, &provider)
            .expect("admission accepts nonce >= expected");
        let drained = mp.drain_ready(10, &provider);
        assert!(
            drained.iter().any(|t| txid_v1(t).expect("txid") == id),
            "tx at the on-chain nonce must be proposable after restart, not admitted-and-stranded",
        );
        assert_eq!(provider.expected_nonce(&addr), 45);
    }

    /// Entity signers restart the same way: the expected view must equal the
    /// persisted entity nonce, which is the minimum execution accepts.
    #[test]
    fn restart_seeds_entity_signer_and_tx_at_entity_nonce_is_proposable() {
        let (sk, pk) = generate_keypair();
        let mut storage = Storage::Memory(MemKv::new());
        let addr = put_entity(&mut storage, pk.to_bytes(), 1_982);
        assert_eq!(addr, address_from_pubkey(&pk), "index addr matches signer");

        let provider = InMemoryNonceProvider::new();
        provider.seed_from_state(&storage).expect("seed");

        let tx = signed_tx(&sk, 1_982);
        let id = txid_v1(&tx).expect("txid");
        let mut mp = TxMempool::new(1, 1000);
        mp.insert(tx, &provider).expect("admitted");
        let drained = mp.drain_ready(10, &provider);
        assert!(
            drained.iter().any(|t| txid_v1(t).expect("txid") == id),
            "entity-signed tx at entity.nonce must be proposable after restart",
        );
        assert_eq!(provider.expected_nonce(&addr), 1_982);
    }

    /// A type-8 registration keys the entity index on the creator address,
    /// so one address can hold both an account row and an entity index
    /// entry. Execution resolves senders entity-first and denies
    /// account-only tx types from entity addresses, so entity.nonce is the
    /// only nonce execution can accept there; the seed must agree.
    #[test]
    fn entity_nonce_overrides_account_nonce_at_the_same_address() {
        let (_sk, pk) = generate_keypair();
        let mut storage = Storage::Memory(MemKv::new());
        let addr = put_entity(&mut storage, pk.to_bytes(), 7);
        put_account(&mut storage, &addr, 500, 3);

        let provider = InMemoryNonceProvider::new();
        provider.seed_from_state(&storage).expect("seed");
        assert_eq!(
            provider.expected_nonce(&addr),
            7,
            "entity nonce must win over the account row at the same address",
        );
    }

    /// Fresh genesis parity: an empty state seeds cleanly and a first tx at
    /// nonce 0 is admitted and proposable through the map-miss fallback,
    /// identical to the dev-range behavior on an empty DB.
    #[test]
    fn empty_state_seeds_cleanly_and_nonce_zero_flows() {
        let (sk, pk) = generate_keypair();
        let addr = address_from_pubkey(&pk);
        let storage = Storage::Memory(MemKv::new());

        let provider = InMemoryNonceProvider::new();
        provider.seed_from_state(&storage).expect("seed on empty state");
        assert_eq!(provider.expected_nonce(&addr), 0);

        let tx = signed_tx(&sk, 0);
        let id = txid_v1(&tx).expect("txid");
        let mut mp = TxMempool::new(1, 1000);
        mp.insert(tx, &provider).expect("admitted");
        let drained = mp.drain_ready(10, &provider);
        assert!(drained.iter().any(|t| txid_v1(t).expect("txid") == id));
    }

    /// The seed source is the persisted nonce, which is exactly what
    /// execution will accept next: a committed-but-failed tx does not
    /// advance it (every failure return precedes the atomic batch write),
    /// and a successful tx advances it by one.
    #[test]
    fn seed_equals_what_execution_accepts_after_failed_and_successful_commits() {
        let (sk, pk) = generate_keypair();
        let addr = address_from_pubkey(&pk);
        let recipient = [0x77u8; 32];
        let mut storage = Storage::Memory(MemKv::new());
        put_account(&mut storage, &addr, 500, 0);
        put_account(
            &mut storage,
            &recipient,
            novai_execution::MIN_ACCOUNT_BALANCE,
            0,
        );

        let payload = novai_execution::encode_transfer_payload_v1(
            &novai_execution::TransferPayloadV1 {
                to: recipient,
                amount: 1_000,
            },
        );
        let mut failing = build_tx(addr, pk.to_bytes(), 0, 1_000, String::new());
        failing.payload = payload.to_vec();
        sign_tx_v1(&sk, &mut failing).expect("sign");
        assert!(
            novai_execution::dispatch_tx(&mut storage, &failing, 1).is_err(),
            "balance 500 cannot cover amount 1000 plus fee 1000",
        );

        let provider = InMemoryNonceProvider::new();
        provider.seed_from_state(&storage).expect("seed");
        assert_eq!(
            provider.expected_nonce(&addr),
            0,
            "failed execution must not advance the seeded nonce",
        );

        put_account(&mut storage, &addr, 1_000_000, 0);
        let mut ok_tx = build_tx(addr, pk.to_bytes(), 0, 1_000, String::new());
        ok_tx.payload = payload.to_vec();
        sign_tx_v1(&sk, &mut ok_tx).expect("sign");
        novai_execution::dispatch_tx(&mut storage, &ok_tx, 2).expect("transfer executes");

        let reseeded = InMemoryNonceProvider::new();
        reseeded.seed_from_state(&storage).expect("seed");
        assert_eq!(
            reseeded.expected_nonce(&addr),
            1,
            "successful execution advances the seeded nonce to state nonce 1",
        );
    }

    /// The dev band needs no special casing: a dev-derived account with
    /// history is seeded from its state row like any other account.
    #[test]
    fn dev_derived_account_is_seeded_from_its_state_row() {
        let index: usize = 3;
        let seed_byte = (index % 256) as u8;
        let mut seed = [seed_byte; 32];
        let index_bytes = index.to_le_bytes();
        for (j, &b) in index_bytes.iter().enumerate() {
            seed[j] ^= b;
        }
        let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
        let addr = address_from_pubkey(&sk.verifying_key());

        let mut storage = Storage::Memory(MemKv::new());
        put_account(&mut storage, &addr, 10_000, 42);

        let provider = InMemoryNonceProvider::new();
        provider.seed_from_state(&storage).expect("seed");
        assert_eq!(provider.expected_nonce(&addr), 42);
    }

    /// The production backend end to end: a RocksDB-backed seed over a
    /// synthetic 50k-account, 100-entity state, with correctness probes.
    /// Prints the scan wall time for the boot-latency record.
    #[test]
    fn rocksdb_seed_covers_synthetic_state_at_scale() {
        let dir = std::env::temp_dir().join(format!(
            "novai-seed-scale-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let kv = RocksKv::open(&dir).expect("open rocksdb");
        let mut storage = Storage::Rocks(kv);

        for i in 0u64..50_000 {
            let mut addr = [0u8; 32];
            addr[..8].copy_from_slice(&i.to_be_bytes());
            addr[31] = 0xA5;
            put_account(&mut storage, &addr, 1_000, i);
        }
        let mut entity_addrs = Vec::with_capacity(100);
        for e in 0u64..100 {
            let mut pubkey = [0u8; 32];
            pubkey[..8].copy_from_slice(&e.to_be_bytes());
            pubkey[31] = 0x5A;
            entity_addrs.push(put_entity(&mut storage, pubkey, 1_000 + e));
        }

        let started = std::time::Instant::now();
        let provider = InMemoryNonceProvider::new();
        let (accounts, entity_signers) = provider.seed_from_state(&storage).expect("seed");
        let elapsed = started.elapsed();
        eprintln!(
            "seed_from_state: {accounts} accounts + {entity_signers} entity signers in {elapsed:?}"
        );

        for probe in [0u64, 1, 24_999, 49_999] {
            let mut addr = [0u8; 32];
            addr[..8].copy_from_slice(&probe.to_be_bytes());
            addr[31] = 0xA5;
            assert_eq!(provider.expected_nonce(&addr), probe);
        }
        for (e, addr) in [(0usize, entity_addrs[0]), (99, entity_addrs[99])] {
            assert_eq!(provider.expected_nonce(&addr), 1_000 + e as u64);
        }
        assert_eq!(accounts, 50_000);
        assert_eq!(entity_signers, 100);

        drop(storage);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fail closed: malformed rows under the scanned prefixes abort the
    /// boot seed instead of silently skipping a sender. A skipped sender
    /// would re-create the admitted-but-never-proposable strand.
    #[test]
    fn malformed_rows_fail_the_seed_loudly() {
        let mut storage = Storage::Memory(MemKv::new());
        storage.put(b"accounts/short", &[0u8]).expect("put");
        assert!(
            InMemoryNonceProvider::new().seed_from_state(&storage).is_err(),
            "truncated account key must fail the seed",
        );

        let mut storage = Storage::Memory(MemKv::new());
        storage
            .put(&account_key(&[0x11u8; 32]), &[1, 2, 3])
            .expect("put");
        assert!(
            InMemoryNonceProvider::new().seed_from_state(&storage).is_err(),
            "undecodable account value must fail the seed",
        );

        let mut storage = Storage::Memory(MemKv::new());
        storage
            .put(&novai_state::ai_entity_by_address_key(&[0x22u8; 32]), &[9, 9])
            .expect("put");
        assert!(
            InMemoryNonceProvider::new().seed_from_state(&storage).is_err(),
            "non-32-byte entity id in the index must fail the seed",
        );

        let mut storage = Storage::Memory(MemKv::new());
        storage
            .put(
                &novai_state::ai_entity_by_address_key(&[0x33u8; 32]),
                &[0x44u8; 32],
            )
            .expect("put");
        assert!(
            InMemoryNonceProvider::new().seed_from_state(&storage).is_err(),
            "dangling entity index must fail the seed",
        );
    }
}
