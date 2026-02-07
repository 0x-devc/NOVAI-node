//! Networked consensus node implementation.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::{ConsensusError, ConsensusState};
use novai_consensus_types::{SignedProposal, Timeout, Vote, QC};
use novai_crypto::{address_from_pubkey, sign_bytes};
use novai_p2p::noise::{
    handshake_initiator, handshake_responder, is_known_validator, noise_keypair_from_seed,
};
use novai_p2p::{connect_to_peer, read_wire_message, start_listener, NetworkMessage, PeerManager};
use novai_state::{Kv, KvBatch, MemKv, RocksKv, WriteOp};
use novai_types::Address;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

/// Maximum number of blocks to request in a single sync chunk.
/// Prevents timeout on large catch-up ranges (e.g., 50k+ blocks).
pub const SYNC_CHUNK_SIZE: u64 = 500;

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

        // Attempt recovery from persistent state
        let state = match ConsensusState::recover(our_address, &storage) {
            Ok(recovered) => {
                tracing::info!(
                    committed_height = recovered.committed_height,
                    highest_qc = recovered.highest_qc.as_ref().map(|q| q.height).unwrap_or(0),
                    "Recovered state"
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
            thread::spawn(move || {
                if let Some(key) = node_clone.encryption_key {
                    // Encrypted mode: Noise XX responder handshake
                    match handshake_responder(&mut stream, &key) {
                        Ok(result) => {
                            if !node_clone.verify_peer_identity(&result.remote_static_key) {
                                return;
                            }
                            if !node_clone.peer_manager.add_peer(Box::new(result.writer)) {
                                tracing::warn!("Peer rejected: connection limit reached");
                                return;
                            }
                            node_clone.handle_peer_connection(result.reader);
                        }
                        Err(e) => {
                            tracing::warn!(?e, "Noise handshake failed (responder)");
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
                    node_clone.handle_peer_connection(stream);
                }
            });
        })
        .map_err(|e| format!("Failed to start listener: {:?}", e))
    }

    /// Connect to a peer and start reader thread.
    ///
    /// When encryption is enabled, performs a Noise XX initiator handshake
    /// and verifies the remote peer's identity.
    pub fn connect_to_peer(self: &Arc<Self>, addr: SocketAddr) -> Result<(), String> {
        let mut stream =
            connect_to_peer(addr).map_err(|e| format!("Failed to connect to peer: {:?}", e))?;

        if let Some(key) = self.encryption_key {
            // Encrypted mode: Noise XX initiator handshake
            let result = handshake_initiator(&mut stream, &key)
                .map_err(|e| format!("Noise handshake failed (initiator): {:?}", e))?;

            if !self.verify_peer_identity(&result.remote_static_key) {
                return Err("Rejected: remote peer not in validator set".into());
            }

            if !self.peer_manager.add_peer(Box::new(result.writer)) {
                return Err("Peer rejected: connection limit reached".into());
            }

            let node = Arc::clone(self);
            thread::spawn(move || {
                node.handle_peer_connection(result.reader);
            });
        } else {
            // Plaintext mode
            let write_stream = stream
                .try_clone()
                .map_err(|e| format!("Failed to clone stream: {:?}", e))?;
            if !self.peer_manager.add_peer(Box::new(write_stream)) {
                return Err("Peer rejected: connection limit reached".into());
            }

            let node = Arc::clone(self);
            thread::spawn(move || {
                node.handle_peer_connection(stream);
            });
        }

        Ok(())
    }

    /// Verify a remote peer's Noise static key against known validator keys.
    ///
    /// Returns `true` if the peer is authorized, `false` otherwise.
    fn verify_peer_identity(&self, remote_static: &[u8; 32]) -> bool {
        if self.known_noise_keys.is_empty() {
            // No known keys configured — skip verification (production bootstrapping)
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
            .map_err(|e| format!("Broadcast failed: {:?}", e))
    }

    /// Check if timeout should be triggered and create it.
    ///
    /// Returns Some(Timeout) if timeout duration elapsed and not already timed out.
    pub fn check_timeout(&self) -> Option<Timeout> {
        let start_time = *self.round_start_time.lock().unwrap();
        let state = self.state.lock().unwrap();

        let timeout_ms =
            novai_consensus::timeout_for_round_with_base(state.round, self.base_timeout_ms);
        let timeout_duration = std::time::Duration::from_millis(timeout_ms);

        if start_time.elapsed() < timeout_duration {
            return None; // Not yet timed out
        }

        // Allow re-broadcast after the full timeout duration to handle lost messages.
        // This replaces the old boolean flag that permanently blocked re-timeout.
        let last_timeout = *self.last_timeout_time.lock().unwrap();
        if let Some(last) = last_timeout {
            let rebroadcast_interval = std::time::Duration::from_millis(timeout_ms);
            if last.elapsed() < rebroadcast_interval {
                return None; // Too soon to re-broadcast
            }
        }

        // Create timeout
        match state.create_timeout(&self.signing_key) {
            Ok(timeout) => {
                *self.last_timeout_time.lock().unwrap() = Some(Instant::now());
                tracing::info!(
                    round = state.round,
                    elapsed = ?start_time.elapsed(),
                    "TIMEOUT triggered"
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

        let mut state = self.state.lock().unwrap();

        // Add timeout to state (use cached pubkeys_vec to avoid allocation)
        state
            .add_timeout(timeout, &self.validator_pubkeys_vec)
            .map_err(|e| format!("Add timeout failed: {:?}", e))?;

        // Try to advance round
        let advanced = state.try_advance_round(&self.validator_set);

        if advanced {
            // Reset round timer
            *self.round_start_time.lock().unwrap() = Instant::now();
            *self.last_timeout_time.lock().unwrap() = None;

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
        let mut pending = self.pending_sync_request.lock().unwrap();
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

        let state = self.state.lock().unwrap();
        let db = self.db.lock().unwrap();

        // Load individual blocks from DB, falling back to in-memory cache
        let mut blocks = Vec::new();
        for height in request.start_height..=clamped_end {
            match ConsensusState::load_block(&*db, height) {
                Ok(Some(block)) => blocks.push(block),
                _ => {
                    // Fallback: check in-memory block cache
                    if let Some(block) = state.block_cache.get(&height) {
                        blocks.push(block.clone());
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

        // Clear pending request if we have one (accept from any peer)
        {
            let mut pending = self.pending_sync_request.lock().unwrap();
            if pending.is_some() {
                *pending = None;
            } else {
                // No pending request — unsolicited response, ignore
                return Ok(());
            }
        }

        if response.blocks.is_empty() {
            tracing::warn!(
                responder = ?&response.responder[..4],
                "Peer sent empty block response"
            );
            return Ok(());
        }

        // Lock order: state → db
        let mut state = self.state.lock().unwrap();
        let mut db = self.db.lock().unwrap();
        let committed_height = state.committed_height;

        // Check that first block connects to our committed chain
        if response.blocks[0].height != committed_height + 1 {
            return Err(format!(
                "Block chain gap: committed_height={}, first block height={}",
                committed_height, response.blocks[0].height
            ));
        }

        // Get expected parent hash
        let expected_parent_hash = if committed_height > 0 {
            let committed_block = ConsensusState::load_block(&*db, committed_height)
                .map_err(|e| format!("Failed to load committed block: {:?}", e))?
                .ok_or_else(|| "Missing committed block".to_string())?;
            novai_consensus_types::block_hash(&committed_block)
        } else {
            [0u8; 32] // Genesis parent
        };

        // Verify the blocks form a valid chain
        if let Err(e) = ConsensusState::verify_block_chain(&response.blocks, expected_parent_hash) {
            return Err(format!("Block chain verification failed: {:?}", e));
        }

        // Store blocks to DB AND cache in memory for commit rule
        for block in &response.blocks {
            let key = novai_state::block_key(block.height);
            let value = novai_consensus_types::codec::encode_block_v1(block)
                .map_err(|e| format!("Failed to encode block: {:?}", e))?;
            db.put(&key, &value)
                .map_err(|e| format!("Failed to store block: {:?}", e))?;

            // Cache in memory so commit rule can find them via block_by_hash
            state.cache_block(block.clone());
        }

        tracing::info!(
            count = response.blocks.len(),
            start = response.blocks.first().unwrap().height,
            end = response.blocks.last().unwrap().height,
            "Cached synced blocks"
        );

        let last_received_height = response.blocks.last().unwrap().height;

        // Try commit rule with current highest_qc (may succeed if we now
        // have enough blocks for the 3-chain rule).
        if let Some(hqc) = state.highest_qc.clone() {
            match state.cache_qc_and_check_commit(hqc.clone()) {
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
                        .map_err(|e| format!("Sync commit persist failed: {:?}", e))?;
                    state.apply_commits(&to_commit);
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
            state.committed_height = last_received_height;
            db.put(
                novai_state::KEY_COMMITTED_HEIGHT,
                &last_received_height.to_be_bytes(),
            )
            .map_err(|e| format!("Failed to persist committed_height: {:?}", e))?;
            tracing::info!(
                committed_height = last_received_height,
                "Sync: advanced committed_height (chunk complete)"
            );
        }

        // Drop locks before requesting next chunk
        drop(state);
        drop(db);

        // If still behind, request next chunk (chunked sync)
        self.try_request_missing_blocks();

        Ok(())
    }

    /// Check if we're the leader for current view.
    /// Uses view_height = max(committed_height, highest_qc.height) for consistency with propose_block.
    pub fn are_we_leader(&self) -> bool {
        let state = self.state.lock().unwrap();
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

    /// Propose a block (leader only).
    pub fn propose_block(
        &self,
        mempool: &mut mempool::TxMempool,
        nonce_provider: &impl mempool::NonceProvider,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let db = self.db.lock().unwrap();

        let block = state
            .propose_block(mempool, nonce_provider, &*db, &self.validator_set)
            .map_err(|e| format!("Propose block failed: {:?}", e))?;

        // CRITICAL: Cache our own proposed block so we can form QC when votes arrive
        state.cache_block(block.clone());

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
            .map_err(|e| format!("Encode proposal failed: {:?}", e))?;

        let signature = sign_bytes(&self.signing_key, &unsigned_bytes);

        let signed_proposal = SignedProposal {
            proposer: self.our_address,
            proposal,
            signature,
        };

        drop(state);
        drop(db);

        tracing::debug!(height = block.height, round = block.round, "Proposing block");

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
        let mut state = self.state.lock().unwrap();
        let db = self.db.lock().unwrap();

        // Try to propose - NotLeader and AlreadyProposed are expected outcomes, not errors
        let block = match state.propose_block(mempool, nonce_provider, &*db, &self.validator_set) {
            Ok(block) => block,
            Err(ConsensusError::NotLeader) => return Ok(false),
            Err(ConsensusError::AlreadyProposed) => return Ok(false),
            Err(e) => return Err(format!("Propose block failed: {:?}", e)),
        };

        // Cache our own proposed block so we can form QC when votes arrive
        state.cache_block(block.clone());

        // Leader self-vote: add our own vote so we only need (quorum - 1) external votes.
        // With 4 validators and quorum=3, this means we need 2 of 3 peers instead of all 3.
        let self_vote = state
            .create_vote(&block, &self.signing_key)
            .map_err(|e| format!("Leader self-vote creation failed: {:?}", e))?;
        state
            .add_vote(self_vote, &self.validator_pubkeys_vec)
            .map_err(|e| format!("Leader self-vote add failed: {:?}", e))?;

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
            .map_err(|e| format!("Encode proposal failed: {:?}", e))?;

        let signature = sign_bytes(&self.signing_key, &unsigned_bytes);

        let signed_proposal = SignedProposal {
            proposer: self.our_address,
            proposal,
            signature,
        };

        // Release locks before broadcasting
        drop(state);
        drop(db);

        // Reset timeout timer — we just proposed, give ourselves a full fresh
        // timeout window to collect votes. Without this, the stale round_start_time
        // from the last received proposal can cause an immediate spurious timeout
        // that clears pending_votes (including our self-vote) before QC forms.
        *self.round_start_time.lock().unwrap() = Instant::now();
        *self.last_timeout_time.lock().unwrap() = None;

        tracing::debug!(height = block.height, round = block.round, "Proposing block");

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
                .map_err(|e| format!("Failed to compute leader: {:?}", e))?;

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
                .map_err(|e| format!("Encode proposal failed: {:?}", e))?;

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
        }

        // 4. Apply justify_qc if it advances our state (QC catch-up).
        //    This fixes the race where proposal for N+1 arrives before the
        //    standalone QC(N) broadcast. The justify_qc was already validated
        //    above (correct height, quorum votes, proposer signature covers it).
        //    Idempotent: cache_qc_and_check_commit is a no-op when the QC
        //    does not dominate the current highest_qc.
        // 5. Verify block validity, create vote, and cache block in single lock acquisition
        let mut needs_sync = false;
        let vote = {
            // Lock order: state → db (must match try_propose_block, handle_vote,
            // handle_qc to prevent deadlock between main loop and receive threads).
            let mut state = self.state.lock().unwrap();
            let mut db = self.db.lock().unwrap();

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
                tracing::info!(
                    qc_height = justify_qc.height,
                    qc_round = justify_qc.round,
                    our_highest_qc = ?state.highest_qc.as_ref().map(|q| (q.height, q.round)),
                    "QC catch-up from proposal"
                );

                match state.cache_qc_and_check_commit(justify_qc.clone()) {
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
                            .map_err(|e| format!("QC catch-up atomic persist failed: {:?}", e))?;
                        state.apply_commits(&to_commit);
                        tracing::info!(
                            committed_height = new_committed_height,
                            "QC catch-up committed blocks"
                        );
                    }
                    Ok(_) => {
                        state.persist_highest_qc(&mut *db).map_err(|e| {
                            format!("QC catch-up persist highest QC failed: {:?}", e)
                        })?;
                    }
                    Err(e) => {
                        // Commit chain incomplete — blocks missing from cache.
                        // highest_qc was already updated by cache_qc_and_check_commit.
                        // Continue to verify and vote; sync will be triggered after
                        // locks are dropped.
                        tracing::warn!(?e, "QC catch-up commit chain incomplete");
                        needs_sync = true;
                        state.persist_highest_qc(&mut *db).map_err(|e| {
                            format!("QC catch-up persist highest QC failed: {:?}", e)
                        })?;
                    }
                }
            }

            state
                .verify_block(block, &*db)
                .map_err(|e| format!("Block verification failed: {:?}", e))?;

            // 5. Create vote
            let vote = state
                .create_vote(block, &self.signing_key)
                .map_err(|e| format!("Vote creation failed: {:?}", e))?;

            // 6. Cache block for commit rule (combined to avoid re-acquiring lock)
            state.check_no_fork(block);
            state.cache_block(block.clone());

            vote
        };

        // Trigger block sync if commit chain was incomplete (locks are now dropped)
        if needs_sync {
            self.try_request_missing_blocks();
        }

        // Reset round timer - we received a valid proposal
        *self.round_start_time.lock().unwrap() = Instant::now();
        *self.last_timeout_time.lock().unwrap() = None;

        tracing::info!(height = block.height, "Voting for block");

        self.broadcast(NetworkMessage::Vote(vote))
    }

    /// Handle incoming vote.
    pub fn handle_vote(&self, vote: Vote) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();

        // Use cached pubkeys_vec to avoid allocation on every vote
        state
            .add_vote(vote.clone(), &self.validator_pubkeys_vec)
            .map_err(|e| format!("Add vote failed: {:?}", e))?;

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
            .map_err(|e| format!("QC formation failed: {:?}", e))?
        {
            let key = (qc.height, qc.round, qc.block_hash);

            {
                let mut sent = self.qc_broadcasted.lock().unwrap();
                if sent.contains(&key) {
                    return Ok(());
                }
                sent.insert(key);
            }

            tracing::info!(votes = qc.votes.len(), "QC formed");

            // Process the QC locally before broadcasting.
            // Commit chain errors are non-fatal — highest_qc is updated
            // regardless, and the QC MUST always be broadcast so other
            // nodes can advance.
            match state.cache_qc_and_check_commit(qc.clone()) {
                Ok(to_commit) if !to_commit.is_empty() => {
                    let mut db = self.db.lock().unwrap();
                    let new_committed_height = to_commit.last().unwrap().height;
                    state
                        .persist_commit_atomic(
                            &mut *db,
                            &to_commit,
                            &qc,
                            new_committed_height,
                            None,
                        )
                        .map_err(|e| format!("Atomic persist failed: {:?}", e))?;
                    state.apply_commits(&to_commit);
                    tracing::info!(
                        committed_height = new_committed_height,
                        "Committed blocks (formed QC locally)"
                    );
                }
                Ok(_) => {
                    let mut db = self.db.lock().unwrap();
                    state
                        .persist_highest_qc(&mut *db)
                        .map_err(|e| format!("Failed to persist highest QC: {:?}", e))?;
                    tracing::debug!(qc_height = qc.height, "Persisted highest_qc (formed locally)");
                }
                Err(e) => {
                    // Commit chain incomplete — blocks missing from cache.
                    // highest_qc was already updated. Persist it and ALWAYS
                    // broadcast the QC so other nodes can advance.
                    tracing::warn!(?e, "Commit chain incomplete (will sync)");
                    let mut db = self.db.lock().unwrap();
                    state
                        .persist_highest_qc(&mut *db)
                        .map_err(|e| format!("Failed to persist highest QC: {:?}", e))?;
                }
            }

            drop(state);

            // Trigger block sync for missing blocks (locks are now dropped)
            self.try_request_missing_blocks();

            // Reset timeout timer — view height just advanced via our own QC.
            // Without this, the stale round_start_time from the previous round
            // causes immediate spurious timeouts that clear pending_votes.
            *self.round_start_time.lock().unwrap() = Instant::now();
            *self.last_timeout_time.lock().unwrap() = None;

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
        let mut state = self.state.lock().unwrap();

        // Record current round and highest_qc height before processing QC
        let old_round = state.round;
        let old_highest = state.highest_qc.as_ref().map(|q| q.height);

        // Check commit rule and get blocks to commit.
        // Commit chain errors are non-fatal — highest_qc is updated regardless,
        // and commits will happen when missing blocks arrive via sync.
        let mut committed = false;
        match state.cache_qc_and_check_commit(qc.clone()) {
            Ok(to_commit) if !to_commit.is_empty() => {
                let mut db = self.db.lock().unwrap();
                let new_committed_height = to_commit.last().unwrap().height;
                state
                    .persist_commit_atomic(&mut *db, &to_commit, &qc, new_committed_height, None)
                    .map_err(|e| format!("Atomic persist failed: {:?}", e))?;
                state.apply_commits(&to_commit);
                committed = true;
                tracing::debug!(
                    committed_height = state.committed_height(),
                    highest_qc = state.highest_qc.as_ref().map(|q| q.height).unwrap_or(0),
                    "Persisted state (atomic)"
                );
            }
            Ok(_) => {
                if state.highest_qc.as_ref().map(|q| q.height) == Some(qc.height) {
                    let mut db = self.db.lock().unwrap();
                    state
                        .persist_highest_qc(&mut *db)
                        .map_err(|e| format!("Failed to persist highest QC: {:?}", e))?;
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
                let mut db = self.db.lock().unwrap();
                state
                    .persist_highest_qc(&mut *db)
                    .map_err(|e| format!("Failed to persist highest QC: {:?}", e))?;
            }
        }

        // Check if round was reset (view height advanced) or highest_qc advanced
        let new_highest = state.highest_qc.as_ref().map(|q| q.height);
        let qc_advanced = new_highest > old_highest;
        let round_was_reset = state.round == 0 && old_round != 0;

        // Release state lock before updating node-level flags
        drop(state);

        // When progress is made (round reset, commit, or QC advanced), reset the
        // timeout timer so the new round gets a fresh timeout window.
        if round_was_reset || committed || qc_advanced {
            *self.round_start_time.lock().unwrap() = Instant::now();
            *self.last_timeout_time.lock().unwrap() = None;
        }

        // Trigger block sync if commit chain was incomplete
        if !committed {
            self.try_request_missing_blocks();
        }

        Ok(())
    }

    /// Handle a peer connection (blocking, spawned per peer).
    pub fn handle_peer_connection(self: Arc<Self>, mut reader: impl std::io::Read) {
        tracing::debug!("Starting receive loop for peer");

        loop {
            match read_wire_message(&mut reader) {
                Ok(msg) => {
                    if let Err(e) = self.handle_network_message(msg) {
                        tracing::error!(%e, "Message handling failed");
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
            let state = self.state.lock().unwrap();
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
        }
    }
}
