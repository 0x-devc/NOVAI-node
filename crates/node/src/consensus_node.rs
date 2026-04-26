//! Networked consensus node implementation.

use crate::MutexExt;
use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::{ConsensusError, ConsensusState};
use novai_consensus_types::{Block, SignedProposal, Timeout, Vote, QC};
use novai_crypto::{address_from_pubkey, sign_bytes};
use novai_p2p::noise::{
    handshake_initiator, handshake_responder, is_known_validator, noise_keypair_from_seed,
};
use novai_p2p::{
    connect_to_peer, read_wire_message, start_listener, ConnectionLimiter, NetworkMessage,
    PeerBanList, PeerManager,
};
use novai_state::{Kv, KvBatch, MemKv, RocksKv, WriteOp};
use novai_types::Address;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

/// Maximum number of blocks to request in a single sync chunk.
/// Prevents timeout on large catch-up ranges (e.g., 50k+ blocks).
pub const SYNC_CHUNK_SIZE: u64 = 500;

/// Sized wrapper around a shared `NonceProvider` trait object for gossip tx insertion.
struct GossipNonceProvider(Arc<dyn mempool::NonceProvider + Send + Sync>);

impl mempool::NonceProvider for GossipNonceProvider {
    fn expected_nonce(&self, from: &Address) -> u64 {
        self.0.expected_nonce(from)
    }
}

/// Storage backend for the consensus node.
///
/// Unifies `MemKv` (in-memory, volatile) and `RocksKv` (persistent, disk-backed)
/// behind a single type so `ConsensusNode` is backend-agnostic.
pub enum Storage {
    Memory(MemKv),
    Rocks(RocksKv),
}

impl Kv for Storage {
    type Error = String;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        match self {
            Storage::Memory(kv) => kv.get(key).map_err(|()| "in-memory storage error".into()),
            Storage::Rocks(kv) => kv.get(key).map_err(|e| e.to_string()),
        }
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), String> {
        match self {
            Storage::Memory(kv) => kv
                .put(key, value)
                .map_err(|()| "in-memory storage error".into()),
            Storage::Rocks(kv) => kv.put(key, value).map_err(|e| e.to_string()),
        }
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), String> {
        match self {
            Storage::Memory(kv) => kv
                .delete(key)
                .map_err(|()| "in-memory storage error".into()),
            Storage::Rocks(kv) => kv.delete(key).map_err(|e| e.to_string()),
        }
    }

    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        match self {
            Storage::Memory(kv) => kv
                .scan_prefix(prefix)
                .map_err(|()| "in-memory storage error".into()),
            Storage::Rocks(kv) => kv.scan_prefix(prefix).map_err(|e| e.to_string()),
        }
    }
}

impl KvBatch for Storage {
    fn apply_batch(&mut self, ops: &[WriteOp]) -> Result<(), String> {
        match self {
            Storage::Memory(kv) => kv
                .apply_batch(ops)
                .map_err(|()| "in-memory storage error".into()),
            Storage::Rocks(kv) => kv.apply_batch(ops).map_err(|e| e.to_string()),
        }
    }
}

impl Storage {
    /// Force a compaction over `[start, end)` on the default column family.
    ///
    /// No-op on the in-memory backend. On RocksDB, used to materialize block
    /// and QC delete tombstones written by `persist_commit_atomic` so the
    /// underlying SST bytes are actually reclaimed. Without periodic
    /// compaction, tombstones accumulate and disk usage grows beyond the
    /// `PRUNE_RETAIN_BLOCKS` retention window.
    pub fn compact_range_default(&self, start: Option<&[u8]>, end: Option<&[u8]>) {
        match self {
            Storage::Memory(_) => {}
            Storage::Rocks(kv) => kv.compact_range_default(start, end),
        }
    }
}

/// Callback invoked after blocks are committed and consensus state is updated.
///
/// Implementations execute transactions against the state DB and update the
/// nonce provider. The DB lock is already held by the caller.
pub trait CommitCallback: Send + Sync {
    fn on_commit(&self, db: &mut Storage, blocks: &[Block]);
}

/// Cache for tracking which QCs have been broadcasted (to avoid duplicates).
type QcBroadcastCache = Arc<Mutex<HashSet<(u64, u64, [u8; 32])>>>;

/// Consensus node with networking.
/// Tracks a pending block sync request.
#[derive(Debug, Clone)]
pub struct PendingSyncRequest {
    pub peer: Address,
    pub start_height: u64,
    pub end_height: u64,
    pub request_time: Instant,
}

// L-05: Lock contention metrics (e.g., time spent waiting on state/db mutexes)
// are planned for future observability improvements. Currently, the H-11 fix
// (signature verification outside lock) is the primary contention mitigation.
// When adding metrics, instrument lock_or_recover() with Instant::now() delta.
pub struct ConsensusNode {
    pub our_address: Address,
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
    pub state: Arc<Mutex<ConsensusState>>,
    pub db: Arc<Mutex<Storage>>,
    pub peer_manager: Arc<PeerManager>,
    pub validator_set: Vec<Address>,
    pub validator_pubkeys: HashMap<Address, VerifyingKey>,
    /// Cached (address, pubkey) pairs to avoid repeated allocations in hot path
    pub validator_pubkeys_vec: Vec<(Address, VerifyingKey)>,
    pub qc_broadcasted: QcBroadcastCache,
    pub round_start_time: Arc<Mutex<Instant>>,
    /// When we last broadcast a timeout for the current round (None = haven't timed out yet).
    pub last_timeout_time: Arc<Mutex<Option<Instant>>>,
    pub pending_sync_request: Arc<Mutex<Option<PendingSyncRequest>>>,
    /// Configurable base timeout in milliseconds (default: BASE_TIMEOUT_MS = 1000).
    /// Server environments may need higher values (e.g., 3000) to avoid spurious timeouts.
    pub base_timeout_ms: u64,
    /// X25519 static key for Noise encryption (None = plaintext mode).
    encryption_key: Option<[u8; 32]>,
    /// Known validators' X25519 static keys for peer authentication.
    known_noise_keys: Vec<[u8; 32]>,
    /// Connection limiter for incoming TCP connections (C-03, C-04).
    pub connection_limiter: Arc<ConnectionLimiter>,
    /// Ban list for misbehaving peers (C-02).
    pub ban_list: Arc<PeerBanList>,
    /// Callback for post-commit execution (dispatch_tx + nonce updates).
    pub commit_callback: Option<Arc<dyn CommitCallback>>,
    /// Shared mempool for inserting gossipped transactions from peers.
    pub gossip_mempool: Option<Arc<Mutex<mempool::TxMempool>>>,
    /// Nonce provider for validating gossipped transactions.
    gossip_nonce: Option<Arc<dyn mempool::NonceProvider + Send + Sync>>,
}

impl ConsensusNode {
    /// Create a node with in-memory storage (volatile — for tests and backward compat).
    pub fn new(
        signing_key: SigningKey,
        validator_set: Vec<Address>,
        validator_pubkeys: HashMap<Address, VerifyingKey>,
        base_timeout_ms: u64,
    ) -> Self {
        Self::new_with_storage(
            signing_key,
            validator_set,
            validator_pubkeys,
            base_timeout_ms,
            Storage::Memory(MemKv::new()),
            None,
        )
    }

    /// Create a node with the given storage backend.
    ///
    /// If the storage contains committed state from a previous run, the node
    /// recovers automatically via `ConsensusState::recover()`.
    ///
    /// `ed25519_seed` enables Noise encryption when `Some`. The seed is used
    /// to derive an X25519 static key for the Noise XX handshake.
    pub fn new_with_storage(
        signing_key: SigningKey,
        validator_set: Vec<Address>,
        validator_pubkeys: HashMap<Address, VerifyingKey>,
        base_timeout_ms: u64,
        storage: Storage,
        ed25519_seed: Option<[u8; 32]>,
    ) -> Self {
        let verifying_key = signing_key.verifying_key();
        let our_address = address_from_pubkey(&verifying_key);

        // Pre-cache pubkeys as Vec to avoid repeated allocations in hot path
        let validator_pubkeys_vec: Vec<(Address, VerifyingKey)> = validator_pubkeys
            .iter()
            .map(|(addr, pk)| (*addr, *pk))
            .collect();

        // Attempt recovery from persistent state, pre-populating the block
        // cache so the commit chain walk works immediately after restart.
        // Without this, the cache is empty and the first QCs trigger
        // "Missing block at height X (chain broken)" until sync fills the gap.
        let state = match ConsensusState::recover_with_cache(
            our_address,
            &storage,
            novai_consensus::CACHE_RETAIN_DEPTH,
        ) {
            Ok(recovered) => {
                tracing::info!(
                    committed_height = recovered.committed_height,
                    highest_qc = recovered.highest_qc.as_ref().map(|q| q.height).unwrap_or(0),
                    block_cache = recovered.block_cache.len(),
                    "Recovered state with cache"
                );
                recovered
            }
            Err(e) => {
                tracing::info!(?e, "No prior state to recover, starting fresh");
                ConsensusState::new(our_address)
            }
        };

        // Derive encryption key and known validator noise keys
        let encryption_key = ed25519_seed.map(|s| noise_keypair_from_seed(&s));
        let known_noise_keys: Vec<[u8; 32]> = if ed25519_seed.is_some() {
            // We don't have raw seeds for other validators, but we DO have their
            // X25519 public keys derived during handshake. For peer authentication,
            // we build this list lazily. For dev-keys mode, main.rs will pass the
            // precomputed list via set_known_noise_keys().
            Vec::new()
        } else {
            Vec::new()
        };

        Self {
            our_address,
            signing_key,
            verifying_key,
            state: Arc::new(Mutex::new(state)),
            db: Arc::new(Mutex::new(storage)),
            peer_manager: Arc::new(PeerManager::new()),
            validator_set,
            validator_pubkeys,
            validator_pubkeys_vec,
            qc_broadcasted: Arc::new(Mutex::new(HashSet::new())),
            round_start_time: Arc::new(Mutex::new(Instant::now())),
            last_timeout_time: Arc::new(Mutex::new(None)),
            pending_sync_request: Arc::new(Mutex::new(None)),
            base_timeout_ms,
            encryption_key,
            known_noise_keys,
            connection_limiter: Arc::new(ConnectionLimiter::new(
                novai_p2p::MAX_PEERS,
                novai_p2p::MAX_CONNECTIONS_PER_IP,
            )),
            ban_list: Arc::new(PeerBanList::new()),
            commit_callback: None,
            gossip_mempool: None,
            gossip_nonce: None,
        }
    }

    /// Set the shared mempool and nonce provider for transaction gossip.
    ///
    /// When set, incoming `Transaction` messages from peers are decoded and
    /// inserted into the mempool so all validators have txs available for proposal.
    pub fn set_gossip_mempool(
        &mut self,
        mempool: Arc<Mutex<mempool::TxMempool>>,
        nonce_provider: Arc<dyn mempool::NonceProvider + Send + Sync>,
    ) {
        self.gossip_mempool = Some(mempool);
        self.gossip_nonce = Some(nonce_provider);
    }

    /// Set the commit callback for post-persist transaction execution.
    ///
    /// Must be called before the node starts handling peer connections.
    pub fn set_commit_callback(&mut self, cb: Arc<dyn CommitCallback>) {
        self.commit_callback = Some(cb);
    }

    /// Execute committed blocks via the commit callback.
    ///
    /// Called after `persist_commit_atomic` + `apply_commits` with the DB
    /// lock still held. Execution writes to different key namespaces than
    /// consensus persistence (no overlap), so this is safe.
    fn execute_committed_blocks(&self, db: &mut Storage, blocks: &[Block]) {
        let total_txs: usize = blocks.iter().map(|b| b.txs.len()).sum();
        for block in blocks {
            let hash = novai_consensus_types::codec::hash_block_v1(block).ok();
            tracing::debug!(
                height = block.height,
                round = block.round,
                tx_count = block.txs.len(),
                block_hash = ?hash.as_ref().map(|h| &h[..4]),
                "COMMIT_DIAG: committed block"
            );
        }
        tracing::debug!(
            block_count = blocks.len(),
            total_txs,
            "COMMIT_DIAG: execute_committed_blocks"
        );
        if total_txs > 0 {
            let block_count = blocks.len();
            tracing::info!(block_count, total_txs, "Committed blocks with transactions");
        }
        if let Some(ref cb) = self.commit_callback {
            cb.on_commit(db, blocks);
        }
    }

    /// Set the known X25519 noise keys for peer identity verification.
    ///
    /// In dev-keys mode, all validator seeds are known so we can precompute
    /// X25519 keys. In production mode, this is populated from genesis data.
    pub fn set_known_noise_keys(&mut self, keys: Vec<[u8; 32]>) {
        self.known_noise_keys = keys;
    }

    /// Start listening for incoming connections.
    ///
    /// When encryption is enabled, performs a Noise XX responder handshake on
    /// each accepted connection and verifies the remote peer's identity.
    pub fn start_listener(self: &Arc<Self>, bind_addr: SocketAddr) -> Result<(), String> {
        let node = Arc::clone(self);
        start_listener(bind_addr, move |mut stream| {
            let node_clone = Arc::clone(&node);

            // C-03/C-04: Check connection limits BEFORE spawning thread.
            // This prevents thread exhaustion from SYN floods and eclipse attacks.
            let ip = match stream.peer_addr() {
                Ok(addr) => addr.ip(),
                Err(_) => return,
            };

            // C-02: Reject banned peers before acquiring connection slot.
            if node_clone.ban_list.is_banned(&ip) {
                tracing::debug!(%ip, "Connection rejected: peer is banned");
                return;
            }

            let guard = match ConnectionLimiter::try_acquire(&node_clone.connection_limiter, ip) {
                Some(g) => g,
                None => {
                    tracing::warn!(%ip, "Connection rejected: limit exceeded");
                    return;
                }
            };

            thread::spawn(move || {
                // Guard released when thread exits, freeing the connection slot.
                let _conn_guard = guard;

                // Bound how long broadcast() can block writing to this peer.
                // Shared with the Noise handshake's save/restore_timeout.
                let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));

                if let Some(key) = node_clone.encryption_key {
                    // Encrypted mode: Noise XX responder handshake
                    match handshake_responder(&mut stream, &key) {
                        Ok(result) => {
                            if !node_clone.verify_peer_identity(&result.remote_static_key) {
                                node_clone.ban_list.ban(ip, "unknown peer identity");
                                return;
                            }
                            if !node_clone.peer_manager.add_peer(Box::new(result.writer)) {
                                tracing::warn!("Peer rejected: connection limit reached");
                                return;
                            }
                            node_clone.handle_peer_connection(result.reader, ip);
                        }
                        Err(e) => {
                            tracing::warn!(?e, "Noise handshake failed (responder)");
                            node_clone.ban_list.ban(ip, "handshake failure");
                        }
                    }
                } else {
                    // Plaintext mode
                    let write_stream = match stream.try_clone() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("Failed to clone accepted stream: {e}");
                            return;
                        }
                    };
                    if !node_clone.peer_manager.add_peer(Box::new(write_stream)) {
                        tracing::warn!("Peer rejected: connection limit reached");
                        return;
                    }
                    node_clone.handle_peer_connection(stream, ip);
                }
            });
        })
        .map_err(|e| format!("Failed to start listener: {e:?}"))
    }

    /// Connect to a peer and start reader thread.
    ///
    /// When encryption is enabled, performs a Noise XX initiator handshake
    /// and verifies the remote peer's identity.
    pub fn connect_to_peer(self: &Arc<Self>, addr: SocketAddr) -> Result<(), String> {
        let mut stream =
            connect_to_peer(addr).map_err(|e| format!("Failed to connect to peer: {e:?}"))?;

        if let Some(key) = self.encryption_key {
            // Encrypted mode: Noise XX initiator handshake
            let result = handshake_initiator(&mut stream, &key)
                .map_err(|e| format!("Noise handshake failed (initiator): {e:?}"))?;

            if !self.verify_peer_identity(&result.remote_static_key) {
                return Err("Rejected: remote peer not in validator set".into());
            }

            if !self.peer_manager.add_peer(Box::new(result.writer)) {
                return Err("Peer rejected: connection limit reached".into());
            }

            let node = Arc::clone(self);
            let peer_ip = addr.ip();
            thread::spawn(move || {
                node.handle_peer_connection(result.reader, peer_ip);
            });
        } else {
            // Plaintext mode
            let write_stream = stream
                .try_clone()
                .map_err(|e| format!("Failed to clone stream: {e:?}"))?;
            if !self.peer_manager.add_peer(Box::new(write_stream)) {
                return Err("Peer rejected: connection limit reached".into());
            }

            let node = Arc::clone(self);
            let peer_ip = addr.ip();
            thread::spawn(move || {
                node.handle_peer_connection(stream, peer_ip);
            });
        }

        Ok(())
    }

    /// Verify a remote peer's Noise static key against known validator keys.
    ///
    /// Returns `true` if the peer is authorized, `false` otherwise.
    fn verify_peer_identity(&self, remote_static: &[u8; 32]) -> bool {
        if self.known_noise_keys.is_empty() {
            // H-02: Warn loudly when accepting peers without verification.
            // In production, known_noise_keys should be distributed via genesis.
            tracing::warn!(
                peer_key = %hex::encode(&remote_static[..16]),
                "Peer identity verification DISABLED (known_noise_keys empty). \
                 Any peer can connect — eclipse attack risk. \
                 Configure validator noise pubkeys for production."
            );
            return true;
        }

        if is_known_validator(remote_static, &self.known_noise_keys) {
            true
        } else {
            tracing::warn!(
                noise_key = %hex::encode(remote_static),
                "Rejected unknown peer"
            );
            false
        }
    }

    /// Broadcast a message to all peers.
    pub fn broadcast(&self, msg: NetworkMessage) -> Result<(), String> {
        self.peer_manager
            .broadcast(&msg)
            .map_err(|e| format!("Broadcast failed: {e:?}"))
    }

    /// Prune the QC broadcast dedup cache, removing entries below the retention window.
    ///
    /// Without pruning, this `HashSet` grows by ~100 bytes per block forever,
    /// causing unbounded memory growth (~50MB per 500k blocks per node).
    fn prune_qc_broadcast_cache(&self, committed_height: u64) {
        if committed_height <= novai_consensus::CACHE_RETAIN_DEPTH {
            return;
        }
        let prune_below = committed_height - novai_consensus::CACHE_RETAIN_DEPTH;
        let mut cache = self.qc_broadcasted.lock_or_recover();
        let before = cache.len();
        cache.retain(|&(height, _, _)| height >= prune_below);
        let pruned = before - cache.len();
        if pruned > 0 {
            // Reclaim backing array capacity after pruning. Without
            // shrink_to_fit, the HashSet keeps high-watermark capacity
            // across millions of insert/retain cycles.
            cache.shrink_to_fit();
            tracing::debug!(pruned, remaining = cache.len(), "Pruned QC broadcast cache");
        }
    }

    /// Check if timeout should be triggered and create it.
    ///
    /// Returns Some(Timeout) if timeout duration elapsed and not already timed out.
    pub fn check_timeout(&self) -> Option<Timeout> {
        // FAST PATH: Read round_start_time WITHOUT the state lock to see if
        // the minimum possible timeout (base_timeout_ms for round 0) has elapsed.
        // This avoids acquiring the expensive state lock on ~99% of loop iterations
        // (every 5ms when base_timeout_ms is typically 1000+ms).
        //
        // This is safe because the worst case of a stale read is that we acquire
        // the state lock one extra time — we re-read round_start_time under the
        // state lock below to prevent the TOCTOU race.
        {
            let start_time = *self.round_start_time.lock_or_recover();
            if start_time.elapsed() < std::time::Duration::from_millis(self.base_timeout_ms) {
                return None; // Definitely not timed out yet (even round 0 hasn't elapsed)
            }
        }

        // SLOW PATH: Acquire state lock to get the actual round and recheck.
        // Lock order: state → round_start_time (matches handle_vote, handle_qc,
        // handle_proposal, try_propose_block — all reset round_start_time while
        // holding state lock).
        //
        // The previous ordering (round_start_time → state) caused a TOCTOU race:
        //   1. check_timeout reads old round_start_time (T0)
        //   2. handle_qc acquires state, advances view, resets round_start_time to NOW
        //   3. check_timeout acquires state, sees round=0 but has stale T0
        //   4. T0.elapsed() > timeout → spurious timeout fires at round 0
        // This race caused the chain stall after hours of running.
        let state = self.state.lock_or_recover();
        let start_time = *self.round_start_time.lock_or_recover();

        let timeout_ms =
            novai_consensus::timeout_for_round_with_base(state.round, self.base_timeout_ms);
        let timeout_duration = std::time::Duration::from_millis(timeout_ms);

        if start_time.elapsed() < timeout_duration {
            return None; // Not yet timed out
        }

        // Allow re-broadcast after the full timeout duration to handle lost messages.
        // This replaces the old boolean flag that permanently blocked re-timeout.
        let last_timeout = *self.last_timeout_time.lock_or_recover();
        if let Some(last) = last_timeout {
            let rebroadcast_interval = std::time::Duration::from_millis(timeout_ms);
            if last.elapsed() < rebroadcast_interval {
                return None; // Too soon to re-broadcast
            }
        }

        // Create timeout
        match state.create_timeout(&self.signing_key) {
            Ok(timeout) => {
                *self.last_timeout_time.lock_or_recover() = Some(Instant::now());
                tracing::debug!(
                    round = state.round,
                    elapsed = ?start_time.elapsed(),
                    highest_qc = ?state.highest_qc.as_ref().map(|q| q.height),
                    "TIMEOUT_DIAG: timeout triggered"
                );
                Some(timeout)
            }
            Err(e) => {
                tracing::error!(?e, "Failed to create timeout");
                None
            }
        }
    }

    /// Handle incoming timeout message.
    ///
    /// Returns Ok(true) if round was advanced, Ok(false) otherwise.
    pub fn handle_timeout(&self, timeout: Timeout) -> Result<bool, String> {
        tracing::debug!(voter = ?&timeout.voter[..4], "Received timeout");

        let mut state = self.state.lock_or_recover();

        // Record round before add_timeout to detect round sync fast-forward
        let round_before = state.round;

        // Add timeout to state (use cached pubkeys_vec to avoid allocation).
        // Duplicate timeouts (same voter, same round) are expected during
        // re-broadcast and treated as no-ops, not errors — same pattern as
        // handle_vote's duplicate handling.
        match state.add_timeout(timeout, &self.validator_pubkeys_vec) {
            Ok(()) => {}
            Err(novai_consensus::ConsensusError::InvalidVote(ref msg))
                if msg.contains("Duplicate timeout")
                    || msg.contains("height mismatch")
                    || msg.contains("at capacity") =>
            {
                return Ok(false);
            }
            Err(e) => return Err(format!("Add timeout failed: {e:?}")),
        }

        // If add_timeout performed round sync (fast-forwarded our round),
        // reset timeout timers so we get a fresh timeout window at the new round
        if state.round > round_before {
            *self.round_start_time.lock_or_recover() = Instant::now();
            *self.last_timeout_time.lock_or_recover() = None;
        }

        // Try to advance round
        let advanced = state.try_advance_round(&self.validator_set);

        if advanced {
            // Reset round timer
            *self.round_start_time.lock_or_recover() = Instant::now();
            *self.last_timeout_time.lock_or_recover() = None;

            tracing::info!(
                round = state.round,
                height = state.height + 1,
                "ROUND ADVANCED"
            );
        }

        Ok(advanced)
    }

    /// Request blocks from a peer for catch-up.
    ///
    /// Returns `Ok(())` if request was sent, or error if already pending or no peers available.
    pub fn request_blocks_from_peer(
        &self,
        start_height: u64,
        end_height: u64,
    ) -> Result<(), String> {
        // Check if there's already a pending request
        let mut pending = self.pending_sync_request.lock_or_recover();
        if pending.is_some() {
            return Err("Sync request already pending".to_string());
        }

        // Select a peer (simple: just take the first validator that's not us)
        let peer = self
            .validator_set
            .iter()
            .find(|&&addr| addr != self.our_address)
            .copied()
            .ok_or_else(|| "No peers available for sync".to_string())?;

        // Create and send request
        let request = novai_consensus_types::BlockRequest {
            requester: self.our_address,
            start_height,
            end_height,
        };

        tracing::debug!(
            start_height,
            end_height,
            peer = ?&peer[..4],
            "Requesting blocks"
        );

        // Store pending request
        *pending = Some(PendingSyncRequest {
            peer,
            start_height,
            end_height,
            request_time: Instant::now(),
        });

        drop(pending);

        // Broadcast request
        self.broadcast(NetworkMessage::BlockRequest(request))?;

        Ok(())
    }

    /// Handle incoming block request from a peer.
    ///
    /// Serves blocks from DB first, falling back to in-memory cache for blocks
    /// that have been proposed/voted on but not yet committed.
    pub fn handle_block_request(
        &self,
        request: novai_consensus_types::BlockRequest,
    ) -> Result<(), String> {
        tracing::debug!(
            start_height = request.start_height,
            end_height = request.end_height,
            requester = ?&request.requester[..4],
            "Received block request"
        );

        // Clamp range to SYNC_CHUNK_SIZE to prevent malicious large requests
        let clamped_end = request
            .end_height
            .min(request.start_height.saturating_add(SYNC_CHUNK_SIZE - 1));

        let state = self.state.lock_or_recover();
        let db = self.db.lock_or_recover();

        // Load individual blocks from DB, falling back to in-memory cache
        let mut blocks = Vec::new();
        for height in request.start_height..=clamped_end {
            match ConsensusState::load_block(&*db, height) {
                Ok(Some(block)) => blocks.push(block),
                _ => {
                    // Fallback: check in-memory block cache
                    if let Some(block) = state.block_cache.get(&height) {
                        blocks.push(Block::clone(block));
                    } else {
                        break; // Stop at first missing block
                    }
                }
            }
        }

        drop(db);
        drop(state);

        tracing::debug!(
            count = blocks.len(),
            start_height = request.start_height,
            end_height = request.end_height,
            requester = ?&request.requester[..4],
            "Sending blocks"
        );

        // Send response with whatever blocks we have
        let response = novai_consensus_types::BlockResponse {
            responder: self.our_address,
            request_start: request.start_height,
            request_end: request.end_height,
            blocks,
        };

        self.broadcast(NetworkMessage::BlockResponse(response))?;

        Ok(())
    }

    /// Handle incoming block response from a peer.
    ///
    /// Accepts responses from ANY peer (we broadcast requests to all).
    /// Caches received blocks in memory AND stores to DB, then retries
    /// the commit rule with highest_qc.
    pub fn handle_block_response(
        &self,
        response: novai_consensus_types::BlockResponse,
    ) -> Result<(), String> {
        tracing::debug!(
            count = response.blocks.len(),
            responder = ?&response.responder[..4],
            "Received blocks"
        );

        // Accept block responses regardless of pending_sync_request state.
        // Previously, responses arriving after the 5-second pending timeout
        // were silently discarded, causing rejoining validators to never sync.
        // Now we always process non-empty responses (idempotent: already-committed
        // blocks are filtered out below).
        if response.blocks.is_empty() {
            tracing::debug!(
                responder = ?&response.responder[..4],
                "Peer sent empty block response"
            );
            return Ok(());
        }

        // Clear pending request if set, so new requests can be made
        {
            let mut pending = self.pending_sync_request.lock_or_recover();
            *pending = None;
        }

        // Lock order: state → db
        let mut state = self.state.lock_or_recover();
        let mut db = self.db.lock_or_recover();
        let committed_height = state.committed_height;

        // Filter out blocks we've already committed (stale sync response).
        // This happens when committed_height advances between request and response.
        let blocks: Vec<_> = response
            .blocks
            .iter()
            .filter(|b| b.height > committed_height)
            .cloned()
            .collect();

        if blocks.is_empty() {
            if let (Some(first), Some(last)) = (response.blocks.first(), response.blocks.last()) {
                tracing::debug!(
                    committed_height,
                    response_start = first.height,
                    response_end = last.height,
                    "Stale sync response — all blocks already committed"
                );
            } else {
                tracing::debug!(committed_height, "Empty sync response");
            }
            drop(state);
            drop(db);
            self.try_request_missing_blocks();
            return Ok(());
        }

        // First fresh block must connect to committed chain (height contiguity)
        if blocks[0].height != committed_height + 1 {
            return Err(format!(
                "Block chain gap: committed_height={}, first block height={}",
                committed_height, blocks[0].height
            ));
        }

        // Verify internal chain consistency (each block connects to the previous
        // one via parent_hash). We use the first block's own parent_hash as the
        // anchor — NOT our local block at committed_height — because the local
        // block may have been overwritten by a stale proposal from a different
        // round (handle_proposal stores all received blocks to DB by height).
        // The sync blocks come from a peer's committed chain and are already
        // verified by the BFT network.
        let anchor_parent = blocks[0].parent_hash;
        if let Err(e) = ConsensusState::verify_block_chain(&blocks, anchor_parent) {
            return Err(format!("Block chain verification failed: {e:?}"));
        }

        // C-01: Reject sync response if peer's chain doesn't connect to our
        // committed block. NEVER overwrite committed blocks from peer responses.
        if committed_height > 0 {
            if let Ok(Some(local_block)) = ConsensusState::load_block(&*db, committed_height) {
                let local_hash = novai_consensus_types::block_hash(&local_block);
                if local_hash != anchor_parent {
                    return Err(format!(
                        "Sync rejected: peer's chain doesn't connect to our committed \
                         block at height {} (local={:?}, peer_parent={:?})",
                        committed_height,
                        &local_hash[..8],
                        &anchor_parent[..8],
                    ));
                }
            }
        }

        // C-01 FIX: Verify synced blocks' state_root against our committed state.
        // The first synced block's state_root must match the current SMT root in
        // our DB. This prevents a malicious peer from injecting blocks with valid
        // parent hashes but fabricated state roots from a different chain.
        {
            let current_root = if let Ok(Some(bytes)) = db.get(novai_state::KEY_SMT_ROOT) {
                novai_state::decode_smt_root_v1(&bytes)
                    .map_err(|e| format!("Failed to decode SMT root: {e:?}"))?
            } else {
                [0u8; 32] // Genesis state (no commits yet)
            };

            if blocks[0].state_root != current_root {
                return Err(format!(
                    "Sync rejected: state root mismatch at height {} \
                     (local={}, peer={}). Peer may be on a different chain.",
                    blocks[0].height,
                    hex::encode(&current_root[..8]),
                    hex::encode(&blocks[0].state_root[..8]),
                ));
            }

            tracing::debug!(
                count = blocks.len(),
                start = blocks[0].height,
                end = blocks.last().unwrap().height,
                state_root = %hex::encode(&current_root[..8]),
                "Synced blocks passed state root verification"
            );
        }

        // Store blocks to DB AND cache in memory for commit rule
        for block in &blocks {
            let key = novai_state::block_key(block.height);
            let value = novai_consensus_types::codec::encode_block_v1(block)
                .map_err(|e| format!("Failed to encode block: {e:?}"))?;
            db.put(&key, &value)
                .map_err(|e| format!("Failed to store block: {e:?}"))?;

            // Cache in memory so commit rule can find them via block_by_hash
            state
                .cache_block(block.clone())
                .map_err(|e| format!("Cache block failed: {e:?}"))?;
        }

        tracing::info!(
            count = blocks.len(),
            start = blocks.first().unwrap().height,
            end = blocks.last().unwrap().height,
            "Cached synced blocks"
        );

        let last_received_height = blocks.last().unwrap().height;

        // Try commit rule with current highest_qc (may succeed if we now
        // have enough blocks for the 3-chain rule).
        if let Some(hqc) = state.highest_qc.clone() {
            match state.cache_qc_and_check_commit(hqc.clone(), &*db) {
                Ok(to_commit) if !to_commit.is_empty() => {
                    let new_committed_height = to_commit.last().unwrap().height;
                    state
                        .persist_commit_atomic(
                            &mut *db,
                            &to_commit,
                            &hqc,
                            new_committed_height,
                            None,
                        )
                        .map_err(|e| format!("Sync commit persist failed: {e:?}"))?;
                    state
                        .apply_commits(&to_commit)
                        .map_err(|e| format!("CONSENSUS SAFETY VIOLATION during sync: {e:?}"))?;
                    self.execute_committed_blocks(&mut db, &to_commit);
                    tracing::info!(
                        committed_height = new_committed_height,
                        count = to_commit.len(),
                        "Synced and committed"
                    );
                }
                Ok(_) | Err(_) => {
                    // Commit chain incomplete — not enough blocks to reach
                    // highest_qc yet. This is expected during chunked sync.
                }
            }
        }

        // Advance committed_height for verified sync blocks even if the
        // full commit chain to highest_qc isn't available yet.
        // These blocks are chain-verified and stored to DB — they were
        // already committed by network consensus.
        if state.committed_height < last_received_height {
            // Execute synced blocks not already committed via the QC path above
            let already = state.committed_height;
            let remaining: Vec<_> = blocks
                .iter()
                .filter(|b| b.height > already)
                .cloned()
                .collect();
            if !remaining.is_empty() {
                self.execute_committed_blocks(&mut db, &remaining);
            }
            state.committed_height = last_received_height;
            db.put(
                novai_state::KEY_COMMITTED_HEIGHT,
                &last_received_height.to_be_bytes(),
            )
            .map_err(|e| format!("Failed to persist committed_height: {e:?}"))?;
            tracing::info!(
                committed_height = last_received_height,
                "Sync: advanced committed_height (chunk complete)"
            );
        }

        let final_committed = state.committed_height;

        // Drop locks before requesting next chunk
        drop(state);
        drop(db);

        // Prune QC broadcast cache to bound memory growth
        self.prune_qc_broadcast_cache(final_committed);

        // If still behind, request next chunk (chunked sync)
        self.try_request_missing_blocks();

        Ok(())
    }

    /// Check if we're the leader for current view.
    /// Uses view_height = max(committed_height, highest_qc.height) for consistency with propose_block.
    pub fn are_we_leader(&self) -> bool {
        let state = self.state.lock_or_recover();
        let view_height = match &state.highest_qc {
            Some(qc) => std::cmp::max(state.height, qc.height),
            None => state.height,
        };
        match ConsensusState::compute_leader_for_view(view_height, state.round, &self.validator_set)
        {
            Ok(leader) => leader == self.our_address,
            Err(_) => false,
        }
    }

    /// Recover txs from the last abandoned proposal.
    ///
    /// When a round changes (timeout or QC catch-up) before our proposed block
    /// is committed, the drained txs are lost. This method returns them so the
    /// caller can reinsert valid ones into the mempool.
    pub fn recover_abandoned_txs(&self) -> Vec<novai_types::TxV1> {
        let mut state = self.state.lock_or_recover();
        state.take_abandoned_txs()
    }

    /// Propose a block (leader only).
    pub fn propose_block(
        &self,
        mempool: &mut mempool::TxMempool,
        nonce_provider: &impl mempool::NonceProvider,
    ) -> Result<(), String> {
        let mut state = self.state.lock_or_recover();
        let db = self.db.lock_or_recover();

        let block = state
            .propose_block(mempool, nonce_provider, &*db, &self.validator_set)
            .map_err(|e| format!("Propose block failed: {e:?}"))?;

        // CRITICAL: Cache our own proposed block so we can form QC when votes arrive
        state
            .cache_block(block.clone())
            .map_err(|e| format!("Cache block failed: {e:?}"))?;

        // justify_qc should certify the parent block (height - 1)
        // For height 1: use GenesisQC (height=0)
        // For height > 1: use highest_qc (which should be for height - 1)
        let justify_qc = if block.height == 1 {
            QC {
                height: 0,
                round: 0,
                block_hash: [0u8; 32],
                votes: vec![],
            }
        } else {
            state.highest_qc.clone().ok_or_else(|| {
                format!("Cannot propose height {} without highest_qc", block.height)
            })?
        };

        let proposal = novai_consensus_types::Proposal {
            block: block.clone(),
            justify_qc,
        };

        let unsigned_bytes = novai_consensus_types::codec::encode_proposal_v1_unsigned(&proposal)
            .map_err(|e| format!("Encode proposal failed: {e:?}"))?;

        let signature = sign_bytes(&self.signing_key, &unsigned_bytes);

        let signed_proposal = SignedProposal {
            proposer: self.our_address,
            proposal,
            signature,
        };

        drop(state);
        drop(db);

        tracing::debug!(
            height = block.height,
            round = block.round,
            "Proposing block"
        );

        self.broadcast(NetworkMessage::SignedProposal(signed_proposal))
    }

    /// Atomically check leadership and propose a block.
    ///
    /// This method avoids the TOCTOU race between checking leadership and proposing
    /// by performing both operations within a single lock acquisition.
    ///
    /// # Returns
    /// - `Ok(true)` if we successfully proposed a block
    /// - `Ok(false)` if we're not the leader or already proposed (expected, not an error)
    /// - `Err(...)` for actual errors (signing, broadcasting, etc.)
    pub fn try_propose_block(
        &self,
        mempool: &mut mempool::TxMempool,
        nonce_provider: &impl mempool::NonceProvider,
    ) -> Result<bool, String> {
        let mut state = self.state.lock_or_recover();
        let db = self.db.lock_or_recover();

        // Try to propose - NotLeader and AlreadyProposed are expected outcomes, not errors
        let block = match state.propose_block(mempool, nonce_provider, &*db, &self.validator_set) {
            Ok(block) => block,
            Err(ConsensusError::NotLeader) => return Ok(false),
            Err(ConsensusError::AlreadyProposed) => return Ok(false),
            Err(e) => return Err(format!("Propose block failed: {e:?}")),
        };

        // Cache our own proposed block so we can form QC when votes arrive
        state
            .cache_block(block.clone())
            .map_err(|e| format!("Cache block failed: {e:?}"))?;

        // Leader self-vote: add our own vote so we only need (quorum - 1) external votes.
        // With 4 validators and quorum=3, this means we need 2 of 3 peers instead of all 3.
        let self_vote = state
            .create_vote(&block, &self.signing_key)
            .map_err(|e| format!("Leader self-vote creation failed: {e:?}"))?;
        state
            .add_vote(self_vote, &self.validator_pubkeys_vec)
            .map_err(|e| format!("Leader self-vote add failed: {e:?}"))?;

        // Build justify_qc for the proposal
        let justify_qc = if block.height == 1 {
            QC {
                height: 0,
                round: 0,
                block_hash: [0u8; 32],
                votes: vec![],
            }
        } else {
            state.highest_qc.clone().ok_or_else(|| {
                format!("Cannot propose height {} without highest_qc", block.height)
            })?
        };

        let proposal = novai_consensus_types::Proposal {
            block: block.clone(),
            justify_qc,
        };

        let unsigned_bytes = novai_consensus_types::codec::encode_proposal_v1_unsigned(&proposal)
            .map_err(|e| format!("Encode proposal failed: {e:?}"))?;

        let signature = sign_bytes(&self.signing_key, &unsigned_bytes);

        let signed_proposal = SignedProposal {
            proposer: self.our_address,
            proposal,
            signature,
        };

        // Reset timeout timer BEFORE dropping state lock — we just proposed,
        // give ourselves a full fresh timeout window to collect votes.
        // Must happen before dropping state to prevent check_timeout race.
        *self.round_start_time.lock_or_recover() = Instant::now();
        *self.last_timeout_time.lock_or_recover() = None;

        // Release locks before broadcasting
        drop(state);
        drop(db);

        let block_hash = novai_consensus_types::codec::hash_block_v1(&block)
            .map_err(|e| format!("Hash block failed: {e:?}"))?;
        tracing::debug!(
            height = block.height,
            round = block.round,
            tx_count = block.txs.len(),
            block_hash = ?&block_hash[..4],
            "PROPOSE_DIAG: proposed block"
        );

        self.broadcast(NetworkMessage::SignedProposal(signed_proposal))?;
        Ok(true)
    }

    /// Handle incoming proposal.
    pub fn handle_proposal(&self, signed_proposal: SignedProposal) -> Result<(), String> {
        tracing::debug!(proposer = ?&signed_proposal.proposer[..4], "Received proposal");

        let block = &signed_proposal.proposal.block;

        // 1. Check proposer is expected leader for this height/round
        // For a block at height H, the leader is determined at view_height H-1
        let view_height = block.height.saturating_sub(1);
        let expected_leader =
            ConsensusState::compute_leader_for_view(view_height, block.round, &self.validator_set)
                .map_err(|e| format!("Failed to compute leader: {e:?}"))?;

        if signed_proposal.proposer != expected_leader {
            return Err(format!(
                "Invalid proposer: expected {:?}, got {:?}",
                &expected_leader[..4],
                &signed_proposal.proposer[..4]
            ));
        }

        // 2. Verify proposal signature
        let proposer_pubkey = self
            .validator_pubkeys
            .get(&signed_proposal.proposer)
            .ok_or_else(|| {
                format!(
                    "Proposer {:?} not in validator set",
                    &signed_proposal.proposer[..4]
                )
            })?;

        let unsigned_bytes =
            novai_consensus_types::codec::encode_proposal_v1_unsigned(&signed_proposal.proposal)
                .map_err(|e| format!("Encode proposal failed: {e:?}"))?;

        if !novai_crypto::verify_bytes(proposer_pubkey, &unsigned_bytes, &signed_proposal.signature)
        {
            return Err(format!(
                "Invalid proposal signature from {:?}",
                &signed_proposal.proposer[..4]
            ));
        }

        // 3. Validate justify_qc
        let justify_qc = &signed_proposal.proposal.justify_qc;
        if block.height == 1 {
            // Height 1 MUST use GenesisQC
            if justify_qc.height != 0 || justify_qc.round != 0 || !justify_qc.votes.is_empty() {
                return Err(format!(
                    "Height 1 proposal must use GenesisQC (height=0, round=0, votes=[]), got height={} round={} votes={}",
                    justify_qc.height, justify_qc.round, justify_qc.votes.len()
                ));
            }
        } else {
            // Height > 1 MUST have valid QC for height - 1
            if justify_qc.height != block.height - 1 {
                return Err(format!(
                    "Height {} proposal must have justify_qc for height {}, got height={}",
                    block.height,
                    block.height - 1,
                    justify_qc.height
                ));
            }

            // MUST have quorum votes
            let n = self.validator_set.len();
            let f = (n - 1) / 3;
            let quorum = 2 * f + 1;

            if justify_qc.votes.len() < quorum {
                return Err(format!(
                    "Height {} proposal has insufficient justify_qc votes: {} < quorum {}",
                    block.height,
                    justify_qc.votes.len(),
                    quorum
                ));
            }

            // Reject oversized QCs before expensive signature verification.
            // A legitimate QC has at most validator_count votes; anything beyond
            // quorum + 5 is either malicious or malformed.
            let max_qc_votes = quorum + 5;
            if justify_qc.votes.len() > max_qc_votes {
                return Err(format!(
                    "Height {} proposal has too many justify_qc votes: {} > max {}",
                    block.height,
                    justify_qc.votes.len(),
                    max_qc_votes
                ));
            }

            // Verify each vote signature in justify_qc (prevents malicious leader
            // from fabricating a QC with fake votes that passes the count check).
            for vote in &justify_qc.votes {
                let pubkey = self.validator_pubkeys.get(&vote.voter).ok_or_else(|| {
                    format!(
                        "justify_qc vote from unknown validator {:?}",
                        &vote.voter[..4]
                    )
                })?;
                let unsigned_vote = Vote {
                    signature: [0u8; 64],
                    ai_signal_commitment: vote.ai_signal_commitment,
                    ..*vote
                };
                let unsigned_bytes =
                    novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned_vote);
                let domain_tag = b"NOVAI_VOTE_V1";
                let mut to_verify = Vec::new();
                to_verify.extend_from_slice(domain_tag);
                to_verify.extend_from_slice(&unsigned_bytes);
                if !novai_crypto::verify_bytes(pubkey, &to_verify, &vote.signature) {
                    return Err(format!(
                        "justify_qc contains invalid vote signature from {:?}",
                        &vote.voter[..4]
                    ));
                }
            }
        }

        // 4. Apply justify_qc if it advances our state (QC catch-up).
        //    This fixes the race where proposal for N+1 arrives before the
        //    standalone QC(N) broadcast. The justify_qc was fully validated
        //    above (correct height, quorum votes, and all vote signatures verified).
        //    Idempotent: cache_qc_and_check_commit is a no-op when the QC
        //    does not dominate the current highest_qc.
        // 5. Verify block validity, create vote, and cache block in single lock acquisition
        let mut needs_sync = false;
        let mut committed_height_for_prune: Option<u64> = None;
        let vote = {
            // Lock order: state → db (must match try_propose_block, handle_vote,
            // handle_qc to prevent deadlock between main loop and receive threads).
            let mut state = self.state.lock_or_recover();
            let mut db = self.db.lock_or_recover();

            // Check if justify_qc would advance our view
            let dominated = match &state.highest_qc {
                None => justify_qc.height > 0,
                Some(existing) => {
                    justify_qc.height > existing.height
                        || (justify_qc.height == existing.height
                            && justify_qc.round > existing.round)
                }
            };

            if dominated {
                tracing::debug!(
                    qc_height = justify_qc.height,
                    qc_round = justify_qc.round,
                    our_highest_qc = ?state.highest_qc.as_ref().map(|q| (q.height, q.round)),
                    "QC catch-up from proposal"
                );

                match state.cache_qc_and_check_commit(justify_qc.clone(), &*db) {
                    Ok(to_commit) if !to_commit.is_empty() => {
                        let new_committed_height = to_commit.last().unwrap().height;
                        state
                            .persist_commit_atomic(
                                &mut *db,
                                &to_commit,
                                justify_qc,
                                new_committed_height,
                                None,
                            )
                            .map_err(|e| format!("QC catch-up atomic persist failed: {e:?}"))?;
                        state.apply_commits(&to_commit).map_err(|e| {
                            format!("CONSENSUS SAFETY VIOLATION during QC catch-up: {e:?}")
                        })?;
                        self.execute_committed_blocks(&mut db, &to_commit);
                        committed_height_for_prune = Some(new_committed_height);
                        tracing::debug!(
                            committed_height = new_committed_height,
                            "QC catch-up committed blocks"
                        );
                    }
                    Ok(_) => {
                        state
                            .persist_highest_qc(&mut *db)
                            .map_err(|e| format!("QC catch-up persist highest QC failed: {e:?}"))?;
                    }
                    Err(e) => {
                        // Commit chain incomplete — blocks missing from cache.
                        // highest_qc was already updated by cache_qc_and_check_commit.
                        // Continue to verify and vote; sync will be triggered after
                        // locks are dropped.
                        tracing::warn!(?e, "QC catch-up commit chain incomplete");
                        needs_sync = true;
                        state
                            .persist_highest_qc(&mut *db)
                            .map_err(|e| format!("QC catch-up persist highest QC failed: {e:?}"))?;
                    }
                }
            }

            // Bug 1 fix: Detect late-arriving blocks BEFORE verify_block rejects
            // them. If block.height is behind our expected height but ahead of
            // committed_height, cache + persist and return without voting. This
            // prevents the in-memory cache gap that breaks the commit chain walk.
            let expected_height = match &state.highest_qc {
                Some(hqc) => std::cmp::max(state.height, hqc.height) + 1,
                None => state.height + 1,
            };

            // Skip proposals for already-committed heights. This happens when
            // committed_height advances via QC catch-up or sync before a delayed
            // proposal arrives on a duplicate peer connection.
            if block.height <= state.committed_height {
                tracing::debug!(
                    block_height = block.height,
                    committed_height = state.committed_height,
                    "Proposal for already-committed height — skipping"
                );
                drop(db);
                drop(state);
                return Ok(());
            }

            if block.height < expected_height && block.height > state.committed_height {
                tracing::warn!(
                    block_height = block.height,
                    expected_height,
                    committed_height = state.committed_height,
                    "Late-arriving block — caching without voting"
                );
                state
                    .cache_block(block.clone())
                    .map_err(|e| format!("Cache block failed: {e:?}"))?;

                // Persist to DB so chain walk DB fallback can find this block
                // after in-memory cache eviction. Only write if no block exists
                // at this height yet — avoids overwriting synced/committed blocks
                // with stale proposals from a different round.
                let key = novai_state::block_key(block.height);
                if db.get(&key).ok().flatten().is_none() {
                    let value = novai_consensus_types::codec::encode_block_v1(block)
                        .map_err(|e| format!("Failed to encode block: {e:?}"))?;
                    db.put(&key, &value)
                        .map_err(|e| format!("Failed to store block: {e:?}"))?;
                }

                drop(db);
                drop(state);
                return Ok(());
            }

            if let Err(e) = state.verify_block(block, &*db) {
                tracing::debug!(
                    height = block.height,
                    round = block.round,
                    tx_count = block.txs.len(),
                    proposer = ?&signed_proposal.proposer[..4],
                    error = %format!("{:?}", e),
                    "VERIFY_DIAG: block verification FAILED"
                );
                return Err(format!("Block verification failed: {e:?}"));
            }

            let recv_block_hash = novai_consensus_types::codec::hash_block_v1(block)
                .map_err(|e| format!("Hash block failed: {e:?}"))?;
            tracing::debug!(
                height = block.height,
                round = block.round,
                tx_count = block.txs.len(),
                block_hash = ?&recv_block_hash[..4],
                "VERIFY_DIAG: block verified OK, voting"
            );

            // 6. Cache block for commit rule (combined to avoid re-acquiring lock)
            state
                .check_no_fork(block)
                .map_err(|e| format!("Fork detection failed: {e:?}"))?;
            state
                .cache_block(block.clone())
                .map_err(|e| format!("Cache block failed: {e:?}"))?;

            // Persist block to DB so the commit chain walk DB fallback can
            // recover it after in-memory cache eviction. Only write if no
            // block exists at this height yet — avoids overwriting synced or
            // committed blocks with proposals from a different round.
            // persist_commit_atomic and handle_block_response always overwrite
            // unconditionally (they store canonical committed blocks).
            let key = novai_state::block_key(block.height);
            if db.get(&key).ok().flatten().is_none() {
                let value = novai_consensus_types::codec::encode_block_v1(block)
                    .map_err(|e| format!("Failed to encode block: {e:?}"))?;
                db.put(&key, &value)
                    .map_err(|e| format!("Failed to store block: {e:?}"))?;
            }

            // 5. Create vote (skip if we already voted in this round — dedup
            // against duplicate proposals arriving via redundant connections)
            if state.voted_in_round.contains(&self.our_address) {
                tracing::debug!(
                    height = block.height,
                    "Already voted in this round, skipping duplicate proposal"
                );
                drop(db);
                drop(state);
                return Ok(());
            }

            let vote = state
                .create_vote(block, &self.signing_key)
                .map_err(|e| format!("Vote creation failed: {e:?}"))?;

            // Mark ourselves as voted so duplicate proposals are rejected
            state.voted_in_round.insert(self.our_address);

            // Reset round timer BEFORE dropping state lock to prevent race with
            // check_timeout (same pattern as handle_vote and handle_qc).
            *self.round_start_time.lock_or_recover() = Instant::now();
            *self.last_timeout_time.lock_or_recover() = None;

            vote
        };

        // Trigger block sync if commit chain was incomplete (locks are now dropped)
        if needs_sync {
            self.try_request_missing_blocks();
        }

        // Prune QC broadcast cache after commit (locks already dropped)
        if let Some(ch) = committed_height_for_prune {
            self.prune_qc_broadcast_cache(ch);
        }

        tracing::info!(height = block.height, "Voting for block");

        self.broadcast(NetworkMessage::Vote(vote))
    }

    /// Handle incoming vote.
    pub fn handle_vote(&self, vote: Vote) -> Result<(), String> {
        // H-11: Verify vote signature BEFORE acquiring state lock.
        // Crypto verification (~100µs) no longer blocks other consensus operations.
        let pubkey = self
            .validator_pubkeys
            .get(&vote.voter)
            .ok_or_else(|| format!("Vote from unknown validator {:?}", &vote.voter[..4]))?;
        {
            let unsigned_vote = Vote {
                height: vote.height,
                round: vote.round,
                block_hash: vote.block_hash,
                voter: vote.voter,
                signature: [0u8; 64],
                ai_signal_commitment: vote.ai_signal_commitment,
            };
            let unsigned_bytes =
                novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned_vote);
            let domain_tag = b"NOVAI_VOTE_V1";
            let mut to_verify = Vec::new();
            to_verify.extend_from_slice(domain_tag);
            to_verify.extend_from_slice(&unsigned_bytes);
            if !novai_crypto::verify_bytes(pubkey, &to_verify, &vote.signature) {
                return Err("Invalid vote signature".to_string());
            }
        }

        let mut state = self.state.lock_or_recover();

        // Use add_vote_verified since we already checked the signature above.
        // Duplicate/equivocation votes are expected during normal operation
        // (e.g., redundant network paths) — treat them as no-ops, not errors.
        match state.add_vote_verified(vote.clone(), &self.validator_pubkeys_vec) {
            Ok(()) => {}
            Err(novai_consensus::ConsensusError::InvalidVote(ref msg))
                if msg.contains("Duplicate vote") || msg.contains("height mismatch") =>
            {
                return Ok(());
            }
            Err(e) => return Err(format!("Add vote failed: {e:?}")),
        }

        // Log AI signal if present (advisory only)
        if let Some(commitment) = vote.ai_signal_commitment {
            tracing::debug!(?commitment, "Node received vote with AI signal");
        }

        // Check if we're leader for the block's height
        // Leader for height N is determined at state height N-1
        let leader_for_vote = {
            let proposal_state_height = vote.height.saturating_sub(1);
            let leader_idx =
                ((proposal_state_height + vote.round) as usize) % self.validator_set.len();
            self.validator_set[leader_idx] == self.our_address
        };

        // Only leader forms QC - non-leaders just collect votes
        if !leader_for_vote {
            return Ok(());
        }

        if let Some(qc) = state
            .try_form_qc(&vote.block_hash, &self.validator_set)
            .map_err(|e| format!("QC formation failed: {e:?}"))?
        {
            let key = (qc.height, qc.round, qc.block_hash);

            {
                let mut sent = self.qc_broadcasted.lock_or_recover();
                if sent.contains(&key) {
                    return Ok(());
                }
                sent.insert(key);
            }

            // Look up the certified block's tx_count for diagnostics
            let qc_block_txs = state.block_by_hash.get(&qc.block_hash).map(|b| b.txs.len());
            tracing::debug!(
                qc_height = qc.height,
                qc_round = qc.round,
                votes = qc.votes.len(),
                block_hash = ?&qc.block_hash[..4],
                certified_block_txs = ?qc_block_txs,
                "QC_DIAG: QC formed"
            );

            // Process the QC locally before broadcasting.
            // Commit chain errors are non-fatal — highest_qc is updated
            // regardless, and the QC MUST always be broadcast so other
            // nodes can advance.
            // Lock order: state (already held) → db
            let mut vote_committed_height: Option<u64> = None;
            let mut committed_blocks: Vec<novai_consensus_types::Block> = Vec::new();
            let mut db = self.db.lock_or_recover();
            match state.cache_qc_and_check_commit(qc.clone(), &*db) {
                Ok(to_commit) if !to_commit.is_empty() => {
                    let new_committed_height = to_commit.last().unwrap().height;
                    state
                        .persist_commit_atomic(
                            &mut *db,
                            &to_commit,
                            &qc,
                            new_committed_height,
                            None,
                        )
                        .map_err(|e| format!("Atomic persist failed: {e:?}"))?;
                    state.apply_commits(&to_commit).map_err(|e| {
                        format!("CONSENSUS SAFETY VIOLATION during vote commit: {e:?}")
                    })?;
                    vote_committed_height = Some(new_committed_height);
                    committed_blocks = to_commit;
                    tracing::debug!(
                        committed_height = new_committed_height,
                        "Committed blocks (formed QC locally)"
                    );
                }
                Ok(_) => {
                    state
                        .persist_highest_qc(&mut *db)
                        .map_err(|e| format!("Failed to persist highest QC: {e:?}"))?;
                    tracing::debug!(
                        qc_height = qc.height,
                        "Persisted highest_qc (formed locally)"
                    );
                }
                Err(e) => {
                    // Commit chain incomplete — blocks missing from cache.
                    // highest_qc was already updated. Persist it and ALWAYS
                    // broadcast the QC so other nodes can advance.
                    tracing::warn!(?e, "Commit chain incomplete (will sync)");
                    state
                        .persist_highest_qc(&mut *db)
                        .map_err(|e| format!("Failed to persist highest QC: {e:?}"))?;
                }
            }

            // Reset timeout timer BEFORE dropping state lock to prevent a race
            // with check_timeout. If we drop state first, check_timeout can read
            // the stale round_start_time and the advanced state, firing a spurious
            // timeout immediately after QC formation.
            *self.round_start_time.lock_or_recover() = Instant::now();
            *self.last_timeout_time.lock_or_recover() = None;

            // Release state lock EARLY — execution writes to different DB
            // namespaces than consensus (no overlap), so we only need the
            // db lock. This frees the state lock for other threads during
            // tx execution.
            drop(state);

            if !committed_blocks.is_empty() {
                self.execute_committed_blocks(&mut db, &committed_blocks);
            }
            drop(db);

            // Trigger block sync for missing blocks (locks now dropped)
            self.try_request_missing_blocks();

            // Prune QC broadcast cache after commit
            if let Some(ch) = vote_committed_height {
                self.prune_qc_broadcast_cache(ch);
            }

            self.broadcast(NetworkMessage::Qc(qc))?;
        }

        Ok(())
    }

    /// Handle incoming QC.
    pub fn handle_qc(&self, qc: QC) -> Result<(), String> {
        tracing::debug!(height = qc.height, round = qc.round, "Received QC");

        // CRITICAL FIX: Hold state lock across cache_qc_and_check_commit AND apply_commits
        // to prevent race condition where timeouts arriving between the two operations
        // get wiped out by apply_commits clearing pending_timeouts.
        let mut state = self.state.lock_or_recover();

        // Record current round and highest_qc height before processing QC
        let old_round = state.round;
        let old_highest = state.highest_qc.as_ref().map(|q| q.height);

        // Check commit rule and get blocks to commit.
        // Commit chain errors are non-fatal — highest_qc is updated regardless,
        // and commits will happen when missing blocks arrive via sync.
        // Lock order: state (already held) → db
        let mut db = self.db.lock_or_recover();
        let mut committed = false;
        let mut qc_committed_height: Option<u64> = None;
        let mut committed_blocks: Vec<novai_consensus_types::Block> = Vec::new();
        match state.cache_qc_and_check_commit(qc.clone(), &*db) {
            Ok(to_commit) if !to_commit.is_empty() => {
                let new_committed_height = to_commit.last().unwrap().height;
                state
                    .persist_commit_atomic(&mut *db, &to_commit, &qc, new_committed_height, None)
                    .map_err(|e| format!("Atomic persist failed: {e:?}"))?;
                state
                    .apply_commits(&to_commit)
                    .map_err(|e| format!("CONSENSUS SAFETY VIOLATION during QC commit: {e:?}"))?;
                committed = true;
                qc_committed_height = Some(new_committed_height);
                committed_blocks = to_commit;
                tracing::debug!(
                    committed_height = state.committed_height(),
                    highest_qc = state.highest_qc.as_ref().map(|q| q.height).unwrap_or(0),
                    "Persisted state (atomic)"
                );
            }
            Ok(_) => {
                if state.highest_qc.as_ref().map(|q| q.height) == Some(qc.height) {
                    state
                        .persist_highest_qc(&mut *db)
                        .map_err(|e| format!("Failed to persist highest QC: {e:?}"))?;
                    tracing::debug!(
                        qc_height = qc.height,
                        "Persisted highest_qc (no commit triggered)"
                    );
                }
            }
            Err(e) => {
                // Commit chain incomplete — blocks missing from cache.
                // highest_qc was already updated. Persist it and continue.
                tracing::warn!(?e, "Commit chain incomplete (will sync)");
                state
                    .persist_highest_qc(&mut *db)
                    .map_err(|e| format!("Failed to persist highest QC: {e:?}"))?;
            }
        }

        // Check if round was reset (view height advanced) or highest_qc advanced
        let new_highest = state.highest_qc.as_ref().map(|q| q.height);
        let qc_advanced = new_highest > old_highest;
        let round_was_reset = state.round == 0 && old_round != 0;

        // Reset timeout timer BEFORE dropping state lock to prevent a race
        // with check_timeout. If we drop state first, check_timeout can read
        // the stale round_start_time and the advanced state, firing a spurious
        // timeout immediately after receiving a QC.
        if round_was_reset || committed || qc_advanced {
            *self.round_start_time.lock_or_recover() = Instant::now();
            *self.last_timeout_time.lock_or_recover() = None;
        }

        // Release state lock EARLY — execution writes to different DB
        // namespaces than consensus (no overlap), so we only need the
        // db lock. This frees the state lock for other threads during
        // tx execution.
        drop(state);

        if !committed_blocks.is_empty() {
            self.execute_committed_blocks(&mut db, &committed_blocks);
        }
        drop(db);

        // Prune QC broadcast cache after commit
        if let Some(ch) = qc_committed_height {
            self.prune_qc_broadcast_cache(ch);
        }

        // Trigger block sync if commit chain was incomplete
        if !committed {
            self.try_request_missing_blocks();
        }

        Ok(())
    }

    /// Handle a peer connection (blocking, spawned per peer).
    ///
    /// Uses `catch_unwind` around message handling to prevent a panic from
    /// poisoning shared mutexes and cascading to all other threads.
    pub fn handle_peer_connection(
        self: Arc<Self>,
        mut reader: impl std::io::Read,
        peer_ip: IpAddr,
    ) {
        tracing::debug!(%peer_ip, "Starting receive loop for peer");

        // C-03: Per-peer message rate limiting.
        // Simple sliding window: count messages per second, disconnect if exceeded.
        let mut msg_count: u64 = 0;
        let mut window_start = Instant::now();

        loop {
            match read_wire_message(&mut reader) {
                Ok(msg) => {
                    // Rate limit: reset window every second, disconnect if exceeded.
                    let elapsed = window_start.elapsed();
                    if elapsed >= std::time::Duration::from_secs(1) {
                        msg_count = 1;
                        window_start = Instant::now();
                    } else {
                        msg_count += 1;
                        if msg_count > novai_p2p::MAX_MESSAGES_PER_SECOND {
                            tracing::warn!(
                                msg_count,
                                limit = novai_p2p::MAX_MESSAGES_PER_SECOND,
                                %peer_ip,
                                "Peer exceeded message rate limit, banning"
                            );
                            self.ban_list.ban(peer_ip, "rate limit exceeded");
                            break;
                        }
                    }

                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        self.handle_network_message(msg)
                    }));
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            tracing::error!(%e, "Message handling failed");
                        }
                        Err(panic_payload) => {
                            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                                (*s).to_string()
                            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "unknown panic".to_string()
                            };
                            tracing::error!(
                                %panic_msg,
                                %peer_ip,
                                "PANIC in message handler — banning peer"
                            );
                            self.ban_list.ban(peer_ip, "caused panic in handler");
                            break;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(?e, "Read failed from peer, disconnecting");
                    break;
                }
            }
        }
    }

    /// Trigger a block sync request if committed_height is behind highest_qc.
    ///
    /// Called after "commit chain incomplete" errors to actually initiate sync
    /// instead of just logging. Uses existing dedup via `pending_sync_request`.
    /// MUST be called without holding the state lock.
    pub fn try_request_missing_blocks(&self) {
        let (committed, hqc_height) = {
            let state = self.state.lock_or_recover();
            let committed = state.committed_height;
            let hqc_height = state.highest_qc.as_ref().map(|q| q.height).unwrap_or(0);
            (committed, hqc_height)
        };

        // Need at least 3-chain gap to have committable blocks
        if hqc_height <= committed + 2 {
            return;
        }

        // Cap to SYNC_CHUNK_SIZE blocks per request to avoid timeout on large ranges
        let end = std::cmp::min(committed + SYNC_CHUNK_SIZE, hqc_height);

        // request_blocks_from_peer already checks pending_sync_request for dedup
        match self.request_blocks_from_peer(committed + 1, end) {
            Ok(()) => {}
            Err(e) => {
                // Expected: "Sync request already pending" — not an error
                if !e.contains("already pending") {
                    tracing::warn!(%e, "Block sync request failed");
                }
            }
        }
    }

    /// Dispatch network message to appropriate handler.
    fn handle_network_message(&self, msg: NetworkMessage) -> Result<(), String> {
        match msg {
            NetworkMessage::SignedProposal(sp) => self.handle_proposal(sp),
            NetworkMessage::Vote(v) => self.handle_vote(v),
            NetworkMessage::Qc(qc) => self.handle_qc(qc),
            NetworkMessage::Timeout(t) => self.handle_timeout(t).map(|_| ()),
            NetworkMessage::BlockRequest(req) => self.handle_block_request(req),
            NetworkMessage::BlockResponse(resp) => self.handle_block_response(resp),
            NetworkMessage::Transaction(bytes) => self.handle_gossip_tx(bytes),
        }
    }

    /// Handle a gossipped transaction from a peer.
    ///
    /// Decodes, validates via nonce check, and inserts into the local mempool.
    /// Duplicates and nonce-stale txs are silently ignored (expected).
    fn handle_gossip_tx(&self, bytes: Vec<u8>) -> Result<(), String> {
        let (mempool, nonce) = match (&self.gossip_mempool, &self.gossip_nonce) {
            (Some(mp), Some(np)) => (mp, np),
            _ => return Ok(()), // gossip not configured
        };

        let tx = novai_codec::decode_tx_v1_signed(&bytes)
            .map_err(|e| format!("Invalid gossipped tx: {e:?}"))?;

        let nonce_wrapper = GossipNonceProvider(Arc::clone(nonce));
        let mut mp = mempool.lock_or_recover();
        match mp.insert(tx, &nonce_wrapper) {
            Ok(txid) => {
                tracing::debug!(txid = %hex::encode(txid), "Gossip tx accepted");
            }
            Err(_) => {
                // Duplicate or nonce-invalid — expected for already-known txs
            }
        }
        Ok(())
    }
}
