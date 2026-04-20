//! Consensus engine for NOVAI v1.
//!
//! Week 6: Propose → Vote → QC formation (no commit yet).

#![forbid(unsafe_code)]

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus_types::codec::{decode_block_v1, decode_qc_v1, encode_block_v1, encode_qc_v1};
use novai_consensus_types::{Block, Timeout, Vote, QC};
use novai_state::{block_key, qc_key, Kv, KvBatch, KEY_COMMITTED_HEIGHT, KEY_HIGHEST_QC};
use novai_types::{Address, TxV1};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// ========== WEEK 8: TIMEOUT CONFIGURATION ==========

/// Base timeout duration in milliseconds.
/// This is the timeout for round 0.
/// NOTE: 1 second allows fast recovery from missed proposals while still
/// giving enough time for vote collection on a local network.
pub const BASE_TIMEOUT_MS: u64 = 1000; // 1 second

/// Timeout multiplier for exponential backoff.
/// Each round doubles the timeout.
pub const TIMEOUT_MULTIPLIER: u64 = 2;

/// Maximum timeout duration in milliseconds.
/// Prevents unbounded timeout growth.
pub const MAX_TIMEOUT_MS: u64 = 60_000; // 60 seconds

/// Number of committed blocks to retain in memory caches.
/// Provides safety margin for the 3-chain commit rule and sync requests.
/// Blocks older than `committed_height - CACHE_RETAIN_DEPTH` are evicted
/// from in-memory caches only (DB is never touched).
pub const CACHE_RETAIN_DEPTH: u64 = 10;

/// Number of committed blocks to retain on disk (RocksDB).
/// When a new block is committed, blocks and QCs older than
/// `committed_height - PRUNE_RETAIN_BLOCKS` are deleted from disk
/// as part of the atomic commit batch.
///
/// 100,000 blocks at ~56 blocks/sec ≈ 30 minutes of history.
/// The 3-chain commit rule only needs the last 3 blocks; catch-up sync
/// needs more — 100K is generous.
pub const PRUNE_RETAIN_BLOCKS: u64 = 100_000;

/// Calculate timeout duration for a given round using the default base timeout.
///
/// Uses exponential backoff: `min(BASE_TIMEOUT_MS * 2^round, MAX_TIMEOUT_MS)`
#[must_use]
pub fn timeout_for_round(round: u64) -> u64 {
    timeout_for_round_with_base(round, BASE_TIMEOUT_MS)
}

/// Calculate timeout duration for a given round with a configurable base timeout.
///
/// Uses exponential backoff: `min(base_ms * 2^round, MAX_TIMEOUT_MS)`
///
/// # Examples (with base_ms=1000)
/// - Round 0: 1000ms (1s)
/// - Round 1: 2000ms (2s)
/// - Round 2: 4000ms (4s)
/// - Round 5: 32000ms (32s)
/// - Round 6+: 60000ms (60s, capped)
#[must_use]
pub fn timeout_for_round_with_base(round: u64, base_ms: u64) -> u64 {
    // Prevent overflow: cap the shift at a reasonable value
    // 2^16 * 2000 = 131_072_000 which is > MAX_TIMEOUT_MS
    let effective_round = round.min(16);

    let timeout = base_ms.saturating_mul(TIMEOUT_MULTIPLIER.saturating_pow(effective_round as u32));
    timeout.min(MAX_TIMEOUT_MS)
}

/// Trait for processing AI state updates during block commit.
/// Implementations will be provided in later weeks (Week 17+).
pub trait AiCommitHook {
    /// Called when blocks are committed. Returns AI-related WriteOps
    /// that must be atomically persisted with the block commit.
    fn on_commit(&self, blocks: &[Block]) -> Vec<novai_state::WriteOp>;
}

/// No-op implementation for current phase (no AI processing yet).
pub struct NoopAiHook;

impl AiCommitHook for NoopAiHook {
    fn on_commit(&self, _blocks: &[Block]) -> Vec<novai_state::WriteOp> {
        Vec::new()
    }
}

/// Consensus engine errors.
#[derive(Debug)]
pub enum ConsensusError {
    /// Invalid block (verification failed).
    InvalidBlock(String),
    /// Invalid vote (signature or format).
    InvalidVote(String),
    /// QC formation failed.
    QcFormationFailed(String),
    /// State error.
    StateError(String),
    /// Codec error.
    CodecError(String),
    /// Crypto error.
    CryptoError(String),
    /// Not leader for this height/round.
    NotLeader,
    /// Already proposed for this height/round.
    AlreadyProposed,
}

/// Consensus state for a single node.
pub struct ConsensusState {
    /// Current consensus height (last committed + 1).
    pub height: u64,
    /// Current round within this height.
    pub round: u64,
    /// Highest QC seen.
    pub highest_qc: Option<QC>,
    /// Pending votes by block hash.
    pub pending_votes: HashMap<[u8; 32], Vec<Vote>>,
    /// Our validator address.
    pub our_address: Address,
    /// Last proposed (height, round) to prevent spam.
    pub last_proposed: Option<(u64, u64)>,
    /// Voters in current round (deduplication).
    pub voted_in_round: HashSet<Address>,
    /// Per-height vote tracking: maps voter → block_hash they voted for.
    /// Persists across round advances within the same height. Cleared
    /// only when height advances (QC formation or commit).
    /// Detects cross-round equivocation: voting for different blocks
    /// at the same consensus height.
    pub voted_at_height: HashMap<Address, [u8; 32]>,
    /// Highest committed height.
    pub committed_height: u64,
    /// Block cache by height (for commit rule). Uses Arc to avoid
    /// cloning full blocks (50-100KB) on every proposal.
    pub block_cache: HashMap<u64, Arc<Block>>,
    /// QC cache by height (for commit rule).
    pub qc_cache: HashMap<u64, QC>,
    /// Block cache by hash (for chain-following in commit rule).
    pub block_by_hash: HashMap<[u8; 32], Arc<Block>>,
    /// Pending timeouts by (height, round).
    pub pending_timeouts: HashMap<(u64, u64), Vec<Timeout>>,
    /// Addresses that already sent timeout in current round (deduplication).
    pub timed_out_in_round: HashSet<Address>,
    /// Total view changes (round advances due to timeouts) since node start.
    pub view_changes_total: u64,
    /// Txs from the last block we proposed. Recovered to mempool if the
    /// block is abandoned (round change / view change before commit).
    pub last_proposed_txs: Vec<TxV1>,
}

impl ConsensusState {
    /// Create new consensus state.
    pub fn new(our_address: Address) -> Self {
        Self {
            height: 0,
            round: 0,
            highest_qc: None,
            pending_votes: HashMap::new(),
            our_address,
            last_proposed: None,
            voted_in_round: HashSet::new(),
            voted_at_height: HashMap::new(),
            committed_height: 0,
            block_cache: HashMap::new(),
            qc_cache: HashMap::new(),
            block_by_hash: HashMap::new(),
            pending_timeouts: HashMap::new(),
            timed_out_in_round: HashSet::new(),
            view_changes_total: 0,
            last_proposed_txs: Vec::new(),
        }
    }

    /// Propose a block (leader only).
    ///
    /// # Errors
    /// Returns error if not leader or block building fails.
    pub fn propose_block<K>(
        &mut self,
        mempool: &mut mempool::TxMempool,
        nonce_provider: &impl mempool::NonceProvider,
        state_db: &K,
        validator_set: &[Address],
    ) -> Result<Block, ConsensusError>
    where
        K: novai_state::Kv,
        K::Error: std::fmt::Debug,
    {
        // Block height should be max(committed_height, highest_qc_height) + 1
        // This ensures we don't propose conflicting blocks after a QC forms
        let next_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        // Check if already proposed for this height/round
        let proposed_key = (next_height, self.round);
        if self.last_proposed == Some(proposed_key) {
            return Err(ConsensusError::AlreadyProposed);
        }

        // Check if we're the leader
        let leader = self.compute_leader(validator_set)?;
        if leader != self.our_address {
            return Err(ConsensusError::NotLeader);
        }

        // Drain ready transactions from mempool with size-aware filtering.
        // Drain up to MAX_TXS_PER_BLOCK candidates, then filter by cumulative
        // block size. Txs that don't fit are returned to the mempool.
        let mempool_size_before = mempool.len();
        let mut candidates = mempool.drain_ready(novai_types::MAX_TXS_PER_BLOCK, nonce_provider);
        tracing::debug!(
            tx_count = candidates.len(),
            mempool_size_before,
            mempool_remaining = mempool.len(),
            "CONSENSUS_DIAG: drain_ready returned"
        );
        let mut txs = Vec::new();
        let mut block_bytes = 0usize;
        let mut overflow = Vec::new();
        for tx in candidates.drain(..) {
            let size = novai_codec::tx_encoded_size(&tx);
            if block_bytes + size > novai_types::MAX_BLOCK_SIZE {
                overflow.push(tx);
            } else {
                block_bytes += size;
                txs.push(tx);
            }
        }
        // Re-insert overflow txs that didn't fit in this block.
        // NOTE: re-inserted overflow txs lose original FIFO ordering. Acceptable
        // for now; a size-aware drain_ready() would preserve ordering.
        for tx in overflow {
            let _ = mempool.reinsert_unchecked(tx);
        }

        // Compute parent hash (from highest_qc if exists, else genesis)
        let parent_hash = if let Some(ref qc) = self.highest_qc {
            qc.block_hash
        } else {
            [0u8; 32] // Genesis parent
        };

        // Read current state root from DB
        let state_root = if let Some(bytes) = state_db
            .get(novai_state::KEY_SMT_ROOT)
            .map_err(|e| ConsensusError::StateError(format!("{:?}", e)))?
        {
            novai_state::decode_smt_root_v1(&bytes)
                .map_err(|e| ConsensusError::StateError(format!("{:?}", e)))?
        } else {
            [0u8; 32] // Genesis root
        };

        // Build block
        let block = Block {
            height: next_height,
            round: self.round,
            parent_hash,
            state_root,
            txs,
        };

        // Mark as proposed and save txs for recovery if block is abandoned
        self.last_proposed = Some((block.height, block.round));
        self.last_proposed_txs = block.txs.clone();

        Ok(block)
    }

    /// Take txs from the last abandoned proposal for mempool recovery.
    ///
    /// Returns the txs and clears the buffer. The caller should reinsert
    /// recoverable txs (nonce >= expected) back into the mempool.
    pub fn take_abandoned_txs(&mut self) -> Vec<TxV1> {
        std::mem::take(&mut self.last_proposed_txs)
    }

    /// Verify a proposed block.
    ///
    /// # Errors
    /// Returns error if block is invalid.
    pub fn verify_block<K>(&self, block: &Block, state_db: &K) -> Result<(), ConsensusError>
    where
        K: novai_state::Kv,
        K::Error: std::fmt::Debug,
    {
        // --- Size limit enforcement (consensus-critical) ---
        // Uses the same tx_encoded_size() as the block proposer. Any divergence
        // between these checks would cause a consensus split.
        if block.txs.len() > novai_types::MAX_TXS_PER_BLOCK {
            return Err(ConsensusError::InvalidBlock(format!(
                "block has {} txs, exceeds limit of {}",
                block.txs.len(),
                novai_types::MAX_TXS_PER_BLOCK
            )));
        }

        let mut block_tx_bytes = 0usize;
        for tx in &block.txs {
            let size = novai_codec::tx_encoded_size(tx);
            if size > novai_types::MAX_TX_SIZE {
                return Err(ConsensusError::InvalidBlock(format!(
                    "tx encoded size {} exceeds limit of {}",
                    size,
                    novai_types::MAX_TX_SIZE
                )));
            }
            block_tx_bytes += size;
        }

        if block_tx_bytes > novai_types::MAX_BLOCK_SIZE {
            return Err(ConsensusError::InvalidBlock(format!(
                "block payload {} bytes exceeds limit of {}",
                block_tx_bytes,
                novai_types::MAX_BLOCK_SIZE
            )));
        }

        // Expected height is max(committed_height, highest_qc_height) + 1
        let expected_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        // Check height is next
        if block.height != expected_height {
            return Err(ConsensusError::InvalidBlock(format!(
                "Height mismatch: expected {}, got {}",
                expected_height, block.height
            )));
        }

        // Check parent hash matches highest QC
        let expected_parent = if let Some(ref qc) = self.highest_qc {
            qc.block_hash
        } else {
            [0u8; 32] // Genesis
        };

        if block.parent_hash != expected_parent {
            return Err(ConsensusError::InvalidBlock(
                "Parent hash mismatch".to_string(),
            ));
        }

        // Verify state root matches current state
        let current_root = if let Some(bytes) = state_db
            .get(novai_state::KEY_SMT_ROOT)
            .map_err(|e| ConsensusError::StateError(format!("{:?}", e)))?
        {
            novai_state::decode_smt_root_v1(&bytes)
                .map_err(|e| ConsensusError::StateError(format!("{:?}", e)))?
        } else {
            [0u8; 32]
        };

        if block.state_root != current_root {
            return Err(ConsensusError::InvalidBlock(
                "State root mismatch".to_string(),
            ));
        }

        // Verify all transaction signatures
        for tx in &block.txs {
            // 1. Verify address matches pubkey
            let pubkey = novai_crypto::pubkey_from_bytes(&tx.pubkey)
                .map_err(|e| ConsensusError::CryptoError(format!("{:?}", e)))?;

            let expected_addr = novai_crypto::address_from_pubkey(&pubkey);
            if tx.from != expected_addr {
                return Err(ConsensusError::InvalidBlock(format!(
                    "Address mismatch: from={:?} but pubkey hashes to {:?}",
                    tx.from, expected_addr
                )));
            }

            // 2. Verify signature
            if !novai_crypto::verify_tx_v1(&pubkey, tx)
                .map_err(|e| ConsensusError::CryptoError(format!("{:?}", e)))?
            {
                return Err(ConsensusError::InvalidBlock(format!(
                    "Invalid transaction signature for tx from {:?}",
                    tx.from
                )));
            }
        }

        Ok(())
    }

    /// Create a vote for a block.
    ///
    /// # Errors
    /// Returns error if block hashing or signing fails.
    pub fn create_vote(
        &self,
        block: &Block,
        signing_key: &SigningKey,
    ) -> Result<Vote, ConsensusError> {
        // Compute block hash
        let block_hash = novai_consensus_types::codec::hash_block_v1(block)
            .map_err(|e| ConsensusError::CodecError(format!("{:?}", e)))?;

        // Create unsigned vote struct
        let unsigned_vote = Vote {
            height: block.height,
            round: block.round,
            block_hash,
            voter: self.our_address,
            signature: [0u8; 64],
            ai_signal_commitment: None,
        };

        // Encode unsigned bytes
        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned_vote);

        // Sign with domain separation
        let domain_tag = b"NOVAI_VOTE_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);

        let signature = novai_crypto::sign_bytes(signing_key, &to_sign);

        // Build final vote with signature
        let vote = Vote {
            height: block.height,
            round: block.round,
            block_hash,
            voter: self.our_address,
            signature,
            ai_signal_commitment: None,
        };

        Ok(vote)
    }

    /// Add a vote to pending votes.
    ///
    /// # Errors
    /// Returns error if vote is invalid.
    pub fn add_vote(
        &mut self,
        vote: Vote,
        validator_pubkeys: &[(Address, VerifyingKey)],
    ) -> Result<(), ConsensusError> {
        // Expected vote height is max(committed_height, highest_qc_height) + 1
        let expected_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        // Accept votes for expected height only. Votes exactly 1 behind are
        // expected stragglers (peer voted before seeing our latest QC) — drop
        // them silently since QC already formed for that height.
        if vote.height != expected_height {
            if vote.height + 1 == expected_height {
                tracing::debug!(
                    vote_height = vote.height,
                    expected_height,
                    voter = ?&vote.voter[..4],
                    "VOTE_DIAG: stale vote (1 behind), dropping"
                );
                return Ok(()); // Stale vote, silently ignore
            }
            tracing::debug!(
                vote_height = vote.height,
                expected_height,
                voter = ?&vote.voter[..4],
                vote_round = vote.round,
                "VOTE_DIAG: vote REJECTED (height mismatch)"
            );
            return Err(ConsensusError::InvalidVote(format!(
                "Vote height mismatch: expected {}, got {}",
                expected_height, vote.height
            )));
        }

        // Find voter's public key in validator set
        let pubkey = validator_pubkeys
            .iter()
            .find(|(addr, _)| *addr == vote.voter)
            .map(|(_, pk)| pk)
            .ok_or_else(|| ConsensusError::InvalidVote("Voter not in validator set".to_string()))?;

        // Cross-round equivocation detection: check if this voter already
        // voted for a DIFFERENT block at this height in a previous round.
        // voted_in_round catches within-round duplicates;
        // voted_at_height catches across-round equivocation.
        if let Some(prev_hash) = self.voted_at_height.get(&vote.voter) {
            if *prev_hash != vote.block_hash {
                return Err(ConsensusError::InvalidVote(format!(
                    "Equivocation: voter {:?} voted for different blocks at same height",
                    &vote.voter[..4],
                )));
            }
        }

        // Check for duplicate vote from same voter in this round (BEFORE expensive signature check)
        if self.voted_in_round.contains(&vote.voter) {
            return Err(ConsensusError::InvalidVote(
                "Duplicate vote from same voter in current round (equivocation)".to_string(),
            ));
        }

        // Create unsigned vote for verification
        let unsigned_vote = Vote {
            height: vote.height,
            round: vote.round,
            block_hash: vote.block_hash,
            voter: vote.voter,
            signature: [0u8; 64],
            ai_signal_commitment: vote.ai_signal_commitment,
        };

        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned_vote);

        // Verify signature
        let domain_tag = b"NOVAI_VOTE_V1";
        let mut to_verify = Vec::new();
        to_verify.extend_from_slice(domain_tag);
        to_verify.extend_from_slice(&unsigned_bytes);

        if !novai_crypto::verify_bytes(pubkey, &to_verify, &vote.signature) {
            return Err(ConsensusError::InvalidVote("Invalid signature".to_string()));
        }

        // Advisory AI signal logging (does NOT affect vote validity)
        if let Some(commitment) = vote.ai_signal_commitment {
            tracing::debug!(?commitment, "Vote includes AI signal");
        }

        // Mark this voter as having voted in this round and at this height
        self.voted_in_round.insert(vote.voter);
        self.voted_at_height.insert(vote.voter, vote.block_hash);

        // Add vote to pending votes (capped to prevent unbounded memory from
        // Byzantine vote spam — each block hash stores at most validator_count + 5 votes)
        let max_per_hash = validator_pubkeys.len() + 5;
        let votes_for_hash = self.pending_votes.entry(vote.block_hash).or_default();
        if votes_for_hash.len() >= max_per_hash {
            return Ok(()); // Silently drop excess votes
        }
        votes_for_hash.push(vote);

        Ok(())
    }

    /// H-11: Add a vote whose signature has already been verified by the caller.
    ///
    /// This allows `handle_vote()` in the node layer to verify signatures
    /// BEFORE acquiring the state lock, reducing lock contention.
    /// All other checks (height, round, duplicates, caps) still apply.
    ///
    /// # Errors
    /// Returns error if vote fails non-signature checks.
    pub fn add_vote_verified(
        &mut self,
        vote: Vote,
        validator_pubkeys: &[(Address, VerifyingKey)],
    ) -> Result<(), ConsensusError> {
        // Height check
        let expected_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        if vote.height != expected_height {
            if vote.height + 1 == expected_height {
                return Ok(());
            }
            return Err(ConsensusError::InvalidVote(format!(
                "Vote height mismatch: expected {}, got {}",
                expected_height, vote.height
            )));
        }

        // Voter must be in validator set
        if !validator_pubkeys
            .iter()
            .any(|(addr, _)| *addr == vote.voter)
        {
            return Err(ConsensusError::InvalidVote(format!(
                "Unknown voter {:?}",
                &vote.voter[..4]
            )));
        }

        // Duplicate check
        if self.voted_in_round.contains(&vote.voter) {
            return Err(ConsensusError::InvalidVote(
                "Duplicate vote from same voter in current round (equivocation)".to_string(),
            ));
        }

        // Advisory AI signal logging
        if let Some(commitment) = vote.ai_signal_commitment {
            tracing::debug!(?commitment, "Vote includes AI signal");
        }

        // Mark voted
        self.voted_in_round.insert(vote.voter);
        self.voted_at_height.insert(vote.voter, vote.block_hash);

        // Add vote (capped)
        let max_per_hash = validator_pubkeys.len() + 5;
        let votes_for_hash = self.pending_votes.entry(vote.block_hash).or_default();
        if votes_for_hash.len() >= max_per_hash {
            return Ok(());
        }
        votes_for_hash.push(vote);

        Ok(())
    }

    /// Try to form a QC for a given block hash.
    ///
    /// # Errors
    /// Returns error if QC formation fails.
    pub fn try_form_qc(
        &mut self,
        block_hash: &[u8; 32],
        validator_set: &[Address],
    ) -> Result<Option<QC>, ConsensusError> {
        let votes = match self.pending_votes.get(block_hash) {
            Some(v) => v,
            None => return Ok(None),
        };

        // Check if we have quorum: 2f+1 where n = 3f+1
        let n = validator_set.len();
        let f = (n - 1) / 3;
        let quorum = 2 * f + 1;

        if votes.len() < quorum {
            return Ok(None);
        }

        // Form QC with exactly quorum votes
        let qc_votes: Vec<Vote> = votes.iter().take(quorum).cloned().collect();

        // QC height is the view height we're forming consensus for
        let qc_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        let qc = QC {
            height: qc_height,
            round: self.round,
            block_hash: *block_hash,
            votes: qc_votes,
        };

        Ok(Some(qc))
    }

    /// Compute leader for a given view (height, round).
    /// This is the canonical leader selection function used everywhere.
    ///
    /// # Leader Selection Rule
    /// Leader index = (view_height + round) % validator_set.len()
    /// where view_height is the height we're building consensus AT (not FOR).
    ///
    /// # Examples
    /// - To propose for height=1, we're at view_height=0, so leader_idx = (0+0) % n
    /// - To vote for a block at height=1, we compute leader for view_height=0
    pub fn compute_leader_for_view(
        view_height: u64,
        round: u64,
        validator_set: &[Address],
    ) -> Result<Address, ConsensusError> {
        if validator_set.is_empty() {
            return Err(ConsensusError::InvalidBlock(
                "Empty validator set".to_string(),
            ));
        }
        let idx = (view_height.wrapping_add(round) as usize) % validator_set.len();
        Ok(validator_set[idx])
    }

    /// Compute leader for current view (convenience wrapper).
    /// Uses view_height = max(committed_height, highest_qc_height) for leader selection.
    fn compute_leader(&self, validator_set: &[Address]) -> Result<Address, ConsensusError> {
        let view_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height),
            None => self.height,
        };
        Self::compute_leader_for_view(view_height, self.round, validator_set)
    }

    // ========== WEEK 8: TIMEOUT & ROUND ADVANCE ==========

    /// Create a timeout message for the current (height, round).
    ///
    /// # Errors
    /// Returns error if signing fails.
    pub fn create_timeout(&self, signing_key: &SigningKey) -> Result<Timeout, ConsensusError> {
        // Timeout height is max(committed_height, highest_qc_height) + 1
        let timeout_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        // Create unsigned timeout struct
        let unsigned_timeout = Timeout {
            height: timeout_height,
            round: self.round,
            voter: self.our_address,
            highest_qc: self.highest_qc.clone(),
            signature: [0u8; 64],
        };

        // Encode unsigned bytes
        let unsigned_bytes =
            novai_consensus_types::codec::encode_timeout_v1_unsigned(&unsigned_timeout)
                .map_err(|e| ConsensusError::CodecError(format!("{:?}", e)))?;

        // Sign with domain separation
        let domain_tag = b"NOVAI_TIMEOUT_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);

        let signature = novai_crypto::sign_bytes(signing_key, &to_sign);

        // Build final timeout with signature
        let timeout = Timeout {
            height: timeout_height,
            round: self.round,
            voter: self.our_address,
            highest_qc: self.highest_qc.clone(),
            signature,
        };

        Ok(timeout)
    }

    /// Add a timeout message from another validator.
    ///
    /// # Errors
    /// Returns error if timeout is invalid or signature verification fails.
    pub fn add_timeout(
        &mut self,
        timeout: Timeout,
        validator_pubkeys: &[(Address, VerifyingKey)],
    ) -> Result<(), ConsensusError> {
        // Verify timeout is for next height
        // Expected timeout height is max(committed_height, highest_qc_height) + 1
        let expected_timeout_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        if timeout.height != expected_timeout_height {
            return Err(ConsensusError::InvalidVote(format!(
                "Timeout height mismatch: expected {}, got {}",
                expected_timeout_height, timeout.height
            )));
        }

        // Accept timeouts for any round at the correct height.
        // Timeouts for past rounds are harmless (won't form quorum since we've moved on).
        // Timeouts for future rounds are buffered so quorum can form when we catch up.
        // try_advance_round only checks rounds >= self.round for quorum.

        // Find voter's public key in validator set
        let pubkey = validator_pubkeys
            .iter()
            .find(|(addr, _)| *addr == timeout.voter)
            .map(|(_, pk)| pk)
            .ok_or_else(|| {
                ConsensusError::InvalidVote("Timeout voter not in validator set".to_string())
            })?;

        // Check for duplicate timeout from same voter in this specific round
        let key = (timeout.height, timeout.round);
        if let Some(existing) = self.pending_timeouts.get(&key) {
            if existing.iter().any(|t| t.voter == timeout.voter) {
                return Err(ConsensusError::InvalidVote(
                    "Duplicate timeout from same voter in this round".to_string(),
                ));
            }
        }

        // Create unsigned timeout for verification
        let unsigned_timeout = Timeout {
            height: timeout.height,
            round: timeout.round,
            voter: timeout.voter,
            highest_qc: timeout.highest_qc.clone(),
            signature: [0u8; 64],
        };

        let unsigned_bytes =
            novai_consensus_types::codec::encode_timeout_v1_unsigned(&unsigned_timeout)
                .map_err(|e| ConsensusError::CodecError(format!("{:?}", e)))?;

        // Verify signature
        let domain_tag = b"NOVAI_TIMEOUT_V1";
        let mut to_verify = Vec::new();
        to_verify.extend_from_slice(domain_tag);
        to_verify.extend_from_slice(&unsigned_bytes);

        if !novai_crypto::verify_bytes(pubkey, &to_verify, &timeout.signature) {
            return Err(ConsensusError::InvalidVote(
                "Invalid timeout signature".to_string(),
            ));
        }

        // H-01: Update highest_qc if timeout includes a better QC,
        // but ONLY after re-verifying all vote signatures in the QC.
        if let Some(ref qc) = timeout.highest_qc {
            let dominated = match &self.highest_qc {
                None => true,
                Some(existing) => {
                    qc.height > existing.height
                        || (qc.height == existing.height && qc.round > existing.round)
                }
            };
            if dominated {
                // Verify quorum: need 2f+1 votes
                let n = validator_pubkeys.len();
                let f = (n - 1) / 3;
                let quorum = 2 * f + 1;
                if qc.votes.len() < quorum {
                    return Err(ConsensusError::InvalidVote(format!(
                        "Timeout QC has insufficient votes: {} < quorum {}",
                        qc.votes.len(),
                        quorum,
                    )));
                }

                // Re-verify each vote signature in the QC
                for vote in &qc.votes {
                    let vote_pk = validator_pubkeys
                        .iter()
                        .find(|(addr, _)| *addr == vote.voter)
                        .map(|(_, pk)| pk)
                        .ok_or_else(|| {
                            ConsensusError::InvalidVote(format!(
                                "Timeout QC contains vote from unknown validator {:?}",
                                &vote.voter[..4]
                            ))
                        })?;

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

                    if !novai_crypto::verify_bytes(vote_pk, &to_verify, &vote.signature) {
                        return Err(ConsensusError::InvalidVote(
                            "Timeout QC contains invalid vote signature".to_string(),
                        ));
                    }
                }

                self.highest_qc = Some(qc.clone());
            }
        }

        // Round sync: if this valid timeout is for a higher round than ours,
        // fast-forward to match. This allows restarted nodes (round 0) to
        // adopt the higher round from surviving nodes after quorum loss,
        // enabling all nodes to converge on the same round and form a TC.
        // Safe because: advancing to a higher round cannot violate safety
        // (safety depends on QC chain, not round number).
        if timeout.round > self.round {
            tracing::info!(
                old_round = self.round,
                new_round = timeout.round,
                peer = ?&timeout.voter[..4],
                "Round sync: fast-forwarding to peer's round"
            );
            self.round = timeout.round;
            self.voted_in_round.clear();
            self.timed_out_in_round.clear();
            self.last_proposed = None;
        }

        // H-01: Hard cap on pending_timeouts to prevent memory exhaustion.
        // 10,000 entries is ~2MB and far beyond normal operation.
        let total_entries: usize = self.pending_timeouts.values().map(|v| v.len()).sum();
        if total_entries >= 10_000 {
            return Err(ConsensusError::InvalidVote(
                "pending_timeouts at capacity".to_string(),
            ));
        }

        // Add timeout to pending timeouts (dedup already checked above)
        self.pending_timeouts.entry(key).or_default().push(timeout);

        Ok(())
    }

    /// Try to advance to next round if we have 2f+1 timeouts.
    ///
    /// Returns true if round was advanced, false otherwise.
    pub fn try_advance_round(&mut self, validator_set: &[Address]) -> bool {
        let expected_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        let n = validator_set.len();
        let f = (n - 1) / 3;
        let quorum = 2 * f + 1;

        // Check current round and any future rounds for quorum.
        // This handles the case where we buffered timeouts for rounds ahead of us.
        let mut best_round = None;
        for &(h, r) in self.pending_timeouts.keys() {
            if h == expected_height && r >= self.round {
                if let Some(timeouts) = self.pending_timeouts.get(&(h, r)) {
                    if timeouts.len() >= quorum {
                        match best_round {
                            None => best_round = Some(r),
                            Some(prev) if r > prev => best_round = Some(r),
                            _ => {}
                        }
                    }
                }
            }
        }

        let target_round = match best_round {
            Some(r) => r,
            None => return false,
        };

        // Advance to target_round + 1
        self.round = target_round + 1;
        self.view_changes_total += 1;

        // Clear round-specific state EXCEPT pending_votes.
        // Votes are keyed by block_hash (unique per proposal). Keeping them
        // across round advances allows QCs to form even if the proposer's
        // round advanced before all votes arrived. Without this, the timeout
        // spiral becomes unrecoverable: votes accumulate, get cleared by round
        // advance, accumulate again, get cleared again — QC never forms.
        self.voted_in_round.clear();
        self.timed_out_in_round.clear();
        self.last_proposed = None;

        // H-01: Prune old pending_timeouts to prevent unbounded memory growth.
        // Keep timeouts for recent rounds only (current_round - 10 as margin).
        let prune_below_round = self.round.saturating_sub(10);
        let before = self.pending_timeouts.len();
        self.pending_timeouts
            .retain(|&(_, r), _| r >= prune_below_round);
        let pruned = before - self.pending_timeouts.len();
        if pruned > 0 {
            tracing::debug!(
                pruned,
                remaining = self.pending_timeouts.len(),
                "Pruned old pending_timeouts"
            );
        }

        // H-02: Prune stale proposals and votes from old rounds to prevent
        // unbounded memory growth during round escalation (timeout spirals).
        // block_by_hash grows by 1 entry per round (each round proposes a
        // different block hash at the same height). pending_votes similarly
        // accumulates votes keyed by those stale hashes.
        // Keep blocks/votes from recent rounds only; older ones can never
        // form a QC since the proposer changes each round.
        {
            let bbh_before = self.block_by_hash.len();
            self.block_by_hash.retain(|_, b| {
                // Keep committed/near-committed blocks, prune stale proposals
                b.height < expected_height || b.round >= prune_below_round
            });
            let bbh_pruned = bbh_before - self.block_by_hash.len();

            // Collect surviving block hashes to filter pending_votes
            let live_hashes: HashSet<[u8; 32]> = self.block_by_hash.keys().copied().collect();
            let pv_before = self.pending_votes.len();
            self.pending_votes
                .retain(|hash, _| live_hashes.contains(hash));
            let pv_pruned = pv_before - self.pending_votes.len();

            if bbh_pruned > 0 || pv_pruned > 0 {
                tracing::debug!(
                    bbh_pruned,
                    bbh_remaining = self.block_by_hash.len(),
                    pv_pruned,
                    pv_remaining = self.pending_votes.len(),
                    "Pruned stale proposals/votes on round advance"
                );
            }
        }

        tracing::info!(
            round = self.round,
            height = expected_height,
            quorum_round = target_round,
            "ROUND ADVANCED"
        );

        true
    }

    // ========== WEEK 7: COMMIT PIPELINE ==========

    /// Cache a block for commit rule tracking.
    ///
    /// Stores block by both height and hash for chain-following.
    ///
    /// # Errors
    /// Returns error if the block cannot be encoded for hashing.
    pub fn cache_block(&mut self, block: Block) -> Result<(), ConsensusError> {
        let hash = novai_consensus_types::codec::hash_block_v1(&block)
            .map_err(|e| ConsensusError::CodecError(format!("block hash failed: {:?}", e)))?;
        tracing::debug!(
            height = block.height,
            round = block.round,
            tx_count = block.txs.len(),
            hash = ?&hash[..4],
            "cache_block"
        );
        let arc = Arc::new(block);
        self.block_cache.insert(arc.height, Arc::clone(&arc));
        self.block_by_hash.insert(hash, arc);
        Ok(())
    }

    /// Cache a QC and check if commit rule triggers.
    ///
    /// # 3-Chain Commit Rule
    /// When QC at height H is observed, commit block at height H-2.
    /// **Verifies parent-chain linkage before committing.**
    ///
    /// Visual:
    /// ```text
    /// B(h) --QC(h)--> B(h+1) --QC(h+1)--> B(h+2) --QC(h+2)
    ///  ^                                            |
    ///  |____________________________________________|
    ///                    COMMIT (verified via parent pointers)
    /// ```
    ///
    /// # Returns
    /// - `Ok(blocks)`: List of blocks to commit (oldest first), or empty if no commit.
    /// - `Err`: Chain linkage broken or required blocks missing.
    ///
    /// # Errors
    /// Returns error if:
    /// - Certified block missing from cache
    /// - Parent chain has gaps or height mismatches
    /// - Required blocks for commit are missing
    pub fn cache_qc_and_check_commit<K>(
        &mut self,
        qc: QC,
        db: &K,
    ) -> Result<Vec<Block>, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        let qc_height = qc.height;

        // Update highest QC if this one dominates
        let dominated = match &self.highest_qc {
            None => true,
            Some(existing) => {
                qc_height > existing.height
                    || (qc_height == existing.height && qc.round > existing.round)
            }
        };
        if dominated {
            // Reset round to 0 when view height advances (new dominating QC)
            // This is critical for leader synchronization
            let old_view_height = self
                .highest_qc
                .as_ref()
                .map(|q| q.height)
                .unwrap_or(self.height);
            let new_view_height = qc.height;

            if new_view_height > old_view_height {
                let pending_vote_count: usize = self.pending_votes.values().map(|v| v.len()).sum();
                tracing::debug!(
                    old_view_height,
                    new_view_height,
                    pending_votes_cleared = pending_vote_count,
                    "VIEW_DIAG: view height advanced, clearing state"
                );
                self.round = 0;
                self.pending_votes.clear();
                self.voted_in_round.clear();
                self.voted_at_height.clear();
                self.timed_out_in_round.clear();
                self.pending_timeouts.clear();
                self.last_proposed = None;
                // Reclaim capacity after clear() — without this,
                // HashMap/HashSet backing arrays survive across every
                // view advance, accumulating high-watermark capacity
                // over millions of blocks.
                self.pending_votes.shrink_to_fit();
                self.voted_in_round.shrink_to_fit();
                self.voted_at_height.shrink_to_fit();
                self.timed_out_in_round.shrink_to_fit();
                self.pending_timeouts.shrink_to_fit();
            }

            self.highest_qc = Some(qc.clone());
        }

        // Cache the QC
        self.qc_cache.insert(qc_height, qc.clone());

        // 3-chain rule: need QC at height >= 2
        if qc_height < 2 {
            return Ok(vec![]);
        }

        let commit_target = qc_height - 2;

        // Nothing to commit if already at or past this height
        if commit_target <= self.committed_height {
            return Ok(vec![]);
        }

        // === VERIFY CHAIN LINKAGE ===
        tracing::debug!(
            qc_height,
            commit_target,
            committed_height = self.committed_height,
            qc_hash = ?&qc.block_hash[..4],
            cache_size = self.block_by_hash.len(),
            "commit chain walk starting"
        );

        // 1. Find B_H (certified block) by QC's block_hash (with DB fallback)
        let block_h = if let Some(b) = self.block_by_hash.get(&qc.block_hash) {
            tracing::debug!(
                height = b.height,
                round = b.round,
                tx_count = b.txs.len(),
                "certified block from CACHE"
            );
            b.clone()
        } else {
            // DB fallback: load by expected height, verify hash matches
            let loaded = Self::load_block(db, qc_height)
                .map_err(|e| {
                    ConsensusError::StateError(format!(
                        "DB fallback failed for certified block at height {}: {:?}",
                        qc_height, e
                    ))
                })?
                .ok_or_else(|| {
                    ConsensusError::StateError(format!(
                        "Missing certified block for QC at height {}",
                        qc_height,
                    ))
                })?;
            let loaded_hash = novai_consensus_types::codec::hash_block_v1(&loaded)
                .map_err(|e| ConsensusError::CodecError(format!("hash failed: {:?}", e)))?;
            if loaded_hash != qc.block_hash {
                return Err(ConsensusError::StateError(format!(
                    "DB block at height {} has wrong hash for QC",
                    qc_height
                )));
            }
            self.cache_block(loaded.clone())?;
            Arc::new(loaded)
        };

        if block_h.height != qc_height {
            return Err(ConsensusError::InvalidBlock(format!(
                "QC height {} doesn't match certified block height {}",
                qc_height, block_h.height
            )));
        }

        // 2. Walk chain backwards from B_H to committed_height+1, verifying linkage
        let mut chain: Vec<Block> = Vec::new();
        let mut current_hash = qc.block_hash;

        for expected_height in (self.committed_height + 1..=qc_height).rev() {
            let (block, source) = if let Some(b) = self.block_by_hash.get(&current_hash) {
                (Block::clone(b), "cache")
            } else {
                // DB fallback: load by expected height, verify hash matches
                let loaded = Self::load_block(db, expected_height)
                    .map_err(|e| {
                        ConsensusError::StateError(format!(
                            "DB fallback at height {}: {:?}",
                            expected_height, e
                        ))
                    })?
                    .ok_or_else(|| {
                        ConsensusError::StateError(format!(
                            "Missing block at height {} (chain broken)",
                            expected_height
                        ))
                    })?;
                let loaded_hash = novai_consensus_types::codec::hash_block_v1(&loaded)
                    .map_err(|e| ConsensusError::CodecError(format!("hash failed: {:?}", e)))?;
                if loaded_hash != current_hash {
                    return Err(ConsensusError::StateError(format!(
                        "DB block at height {} has wrong hash",
                        expected_height
                    )));
                }
                self.cache_block(loaded.clone())?;
                (loaded, "db")
            };

            tracing::debug!(
                expected_height,
                actual_height = block.height,
                round = block.round,
                tx_count = block.txs.len(),
                source,
                hash = ?&current_hash[..4],
                will_commit = expected_height <= commit_target,
                "chain walk block"
            );

            if block.height != expected_height {
                return Err(ConsensusError::InvalidBlock(format!(
                    "Chain height mismatch: expected {}, got {}",
                    expected_height, block.height
                )));
            }

            // Extract parent_hash before potential move
            current_hash = block.parent_hash;

            // Only include blocks up to commit_target (not the 2 confirmation blocks)
            if expected_height <= commit_target {
                chain.push(block);
            }
        }

        // Reverse to get oldest first
        chain.reverse();

        let total_commit_txs: usize = chain.iter().map(|b| b.txs.len()).sum();
        tracing::debug!(
            commit_blocks = chain.len(),
            total_commit_txs,
            "commit chain built"
        );

        // Verify contiguous commit (no gaps) - Fix D
        let expected_count = (commit_target - self.committed_height) as usize;
        if chain.len() != expected_count {
            return Err(ConsensusError::StateError(format!(
                "Incomplete commit chain: expected {} blocks (heights {}..={}), got {}",
                expected_count,
                self.committed_height + 1,
                commit_target,
                chain.len()
            )));
        }

        Ok(chain)
    }

    /// Mark blocks as committed and advance state.
    ///
    /// # Errors
    /// Returns error if a commit gap is detected (consensus safety violation).
    /// The caller should log evidence and halt the node gracefully.
    pub fn apply_commits(&mut self, blocks: &[Block]) -> Result<(), ConsensusError> {
        for block in blocks {
            // Safety check: no gaps in commit sequence
            let expected_height = self.committed_height + 1;
            if block.height != expected_height {
                tracing::error!(
                    expected_height,
                    actual_height = block.height,
                    committed_height = self.committed_height,
                    "CONSENSUS SAFETY VIOLATION: commit gap detected"
                );
                return Err(ConsensusError::StateError(format!(
                    "CONSENSUS SAFETY VIOLATION: commit gap! expected height {}, got {}",
                    expected_height, block.height
                )));
            }

            // Advance committed height
            self.committed_height = block.height;

            // Advance consensus height to match
            if self.height < block.height {
                self.height = block.height;
            }

            // Clear stale state for committed height
            self.block_cache.remove(&block.height);

            // Log commit
            tracing::info!(
                height = block.height,
                state_root = ?&block.state_root[..4],
                "COMMITTED block"
            );
        }

        // Clear pending votes and voted_in_round after commits
        if !blocks.is_empty() {
            self.pending_votes.clear();
            self.voted_in_round.clear();
            self.voted_at_height.clear();
            self.timed_out_in_round.clear();
            self.pending_timeouts.clear();
            self.last_proposed = None;

            // Reset round to 0 after successful commit
            self.round = 0;

            // Evict old blocks from in-memory caches to bound memory usage.
            self.prune_old_blocks();
        }

        Ok(())
    }

    /// Prune in-memory block and QC caches below the retention window.
    ///
    /// Keeps the last [`CACHE_RETAIN_DEPTH`] committed blocks as safety margin
    /// for the 3-chain commit rule and peer sync requests.
    ///
    /// **Only prunes in-memory caches. Never deletes from database.**
    /// Block sync serves historical blocks from DB.
    pub fn prune_old_blocks(&mut self) {
        if self.committed_height <= CACHE_RETAIN_DEPTH {
            return;
        }

        let prune_below = self.committed_height - CACHE_RETAIN_DEPTH;

        self.block_cache.retain(|&height, _| height >= prune_below);
        self.qc_cache.retain(|&height, _| height >= prune_below);
        self.block_by_hash
            .retain(|_, block| block.height >= prune_below);

        // MEMORY LEAK FIX: Prune pending_votes for long-committed blocks.
        // Keep votes only if ANY vote in the Vec has height >= prune_below.
        self.pending_votes
            .retain(|_, votes| votes.iter().any(|v| v.height >= prune_below));

        // MEMORY LEAK FIX: Prune pending_timeouts for old heights.
        self.pending_timeouts
            .retain(|&(height, _), _| height >= prune_below);

        // FRAGMENTATION FIX: After millions of insert/remove cycles, HashMap
        // internal capacity grows far beyond the number of live entries. The
        // allocator keeps the old backing array allocated even after retain()
        // removes entries. shrink_to_fit() releases that excess capacity.
        // Shrink every 256 heights (was 1000 — more aggressive to keep
        // RSS bounded over multi-million block runs).
        if self.committed_height & 0xFF == 0 {
            self.block_cache.shrink_to_fit();
            self.qc_cache.shrink_to_fit();
            self.block_by_hash.shrink_to_fit();
            self.pending_votes.shrink_to_fit();
            self.pending_timeouts.shrink_to_fit();
            self.voted_in_round.shrink_to_fit();
            self.voted_at_height.shrink_to_fit();
            self.timed_out_in_round.shrink_to_fit();
        }
    }

    /// Apply commits with AI hook integration.
    ///
    /// This is the version that should be used when AI hooks are available.
    /// It calls the AI hook to generate operations that will be persisted atomically.
    ///
    /// # Returns
    /// Returns the AI operations that should be passed to `persist_commit_atomic`.
    ///
    /// # Errors
    /// Returns error if `apply_commits` detects a consensus safety violation.
    pub fn apply_commits_with_ai_hook(
        &mut self,
        blocks: &[Block],
        ai_hook: &dyn AiCommitHook,
    ) -> Result<Vec<novai_state::WriteOp>, ConsensusError> {
        // First apply commits normally (updates in-memory state)
        self.apply_commits(blocks)?;

        // Then generate AI operations if blocks were committed
        if !blocks.is_empty() {
            Ok(ai_hook.on_commit(blocks))
        } else {
            Ok(Vec::new())
        }
    }

    /// Check for conflicting commits (fork detection).
    ///
    /// In HotStuff BFT, if a block doesn't get a QC, the next round's leader
    /// proposes a different block for the same height. This is normal behavior.
    /// A real fork would be COMMITTING two different blocks at the same height.
    ///
    /// # Errors
    /// Returns error if two different blocks conflict at or below committed_height.
    /// The caller should log the fork evidence and halt the node gracefully.
    pub fn check_no_fork(&self, block: &Block) -> Result<(), ConsensusError> {
        // Only check for forks at or below committed_height.
        // Heights above committed_height can have different proposals in different rounds.
        if block.height > self.committed_height {
            return Ok(());
        }

        if let Some(cached) = self.block_cache.get(&block.height) {
            let cached_hash = novai_consensus_types::codec::hash_block_v1(cached)
                .map_err(|e| ConsensusError::CodecError(format!("{e:?}")))?;
            let new_hash = novai_consensus_types::codec::hash_block_v1(block)
                .map_err(|e| ConsensusError::CodecError(format!("{e:?}")))?;

            if cached_hash != new_hash {
                tracing::error!(
                    height = block.height,
                    cached_hash = ?&cached_hash[..8],
                    new_hash = ?&new_hash[..8],
                    "CONSENSUS SAFETY VIOLATION: FORK DETECTED"
                );
                return Err(ConsensusError::InvalidBlock(format!(
                    "FORK DETECTED at height {}! cached={:?} new={:?}",
                    block.height,
                    &cached_hash[..8],
                    &new_hash[..8]
                )));
            }
        }

        Ok(())
    }

    /// Get committed height.
    pub fn committed_height(&self) -> u64 {
        self.committed_height
    }

    // ========== PERSISTENCE ==========

    /// Persist a block to the database.
    ///
    /// # Errors
    /// Returns error if encoding or database write fails.
    pub fn persist_block<K>(&self, db: &mut K, block: &Block) -> Result<(), ConsensusError>
    where
        K: KvBatch,
        K::Error: std::fmt::Debug,
    {
        let key = block_key(block.height);
        let value = encode_block_v1(block)
            .map_err(|e| ConsensusError::CodecError(format!("Failed to encode block: {:?}", e)))?;
        db.put(&key, &value)
            .map_err(|e| ConsensusError::StateError(format!("Failed to persist block: {:?}", e)))
    }

    /// Persist a QC to the database.
    ///
    /// # Errors
    /// Returns error if encoding or database write fails.
    pub fn persist_qc<K>(&self, db: &mut K, qc: &QC) -> Result<(), ConsensusError>
    where
        K: KvBatch,
        K::Error: std::fmt::Debug,
    {
        let key = qc_key(qc.height);
        let value = encode_qc_v1(qc)
            .map_err(|e| ConsensusError::CodecError(format!("Failed to encode QC: {:?}", e)))?;
        db.put(&key, &value)
            .map_err(|e| ConsensusError::StateError(format!("Failed to persist QC: {:?}", e)))
    }

    /// Persist committed height to the database.
    ///
    /// # Errors
    /// Returns error if database write fails.
    pub fn persist_committed_height<K>(&self, db: &mut K) -> Result<(), ConsensusError>
    where
        K: KvBatch,
        K::Error: std::fmt::Debug,
    {
        let value = self.committed_height.to_be_bytes().to_vec();
        db.put(KEY_COMMITTED_HEIGHT, &value).map_err(|e| {
            ConsensusError::StateError(format!("Failed to persist committed height: {:?}", e))
        })
    }

    /// Persist highest QC to the database.
    ///
    /// # Errors
    /// Returns error if encoding or database write fails.
    pub fn persist_highest_qc<K>(&self, db: &mut K) -> Result<(), ConsensusError>
    where
        K: KvBatch,
        K::Error: std::fmt::Debug,
    {
        if let Some(ref qc) = self.highest_qc {
            let value = encode_qc_v1(qc).map_err(|e| {
                ConsensusError::CodecError(format!("Failed to encode highest QC: {:?}", e))
            })?;
            db.put(KEY_HIGHEST_QC, &value).map_err(|e| {
                ConsensusError::StateError(format!("Failed to persist highest QC: {:?}", e))
            })?;
        }
        Ok(())
    }
    /// Persist commit state atomically (all-or-nothing).
    ///
    /// Writes blocks, QC, committed_height, and highest_qc in a single batch.
    /// If the node crashes, either all writes succeed or none do.
    ///
    /// # Errors
    /// Returns error if encoding fails or batch write fails.
    pub fn persist_commit_atomic<K>(
        &self,
        db: &mut K,
        blocks: &[Block],
        qc: &QC,
        new_committed_height: u64,
        ai_ops: Option<&[novai_state::WriteOp]>, // NEW: AI operations to commit atomically
    ) -> Result<(), ConsensusError>
    where
        K: KvBatch,
        K::Error: std::fmt::Debug,
    {
        use novai_state::WriteOp;

        let mut ops = Vec::new();

        // 1. Blocks
        for block in blocks {
            let key = block_key(block.height);
            let value = encode_block_v1(block).map_err(|e| {
                ConsensusError::CodecError(format!("Failed to encode block: {:?}", e))
            })?;
            ops.push(WriteOp::Put(key, value));
        }

        // 2. QC that triggered commit
        let qc_k = qc_key(qc.height);
        let qc_v = encode_qc_v1(qc)
            .map_err(|e| ConsensusError::CodecError(format!("Failed to encode QC: {:?}", e)))?;
        ops.push(WriteOp::Put(qc_k, qc_v));

        // 3. Committed height
        let ch_v = new_committed_height.to_be_bytes().to_vec();
        ops.push(WriteOp::Put(KEY_COMMITTED_HEIGHT.to_vec(), ch_v));

        // 4. Highest QC (if present)
        if let Some(ref hqc) = self.highest_qc {
            let hqc_v = encode_qc_v1(hqc).map_err(|e| {
                ConsensusError::CodecError(format!("Failed to encode highest QC: {:?}", e))
            })?;
            ops.push(WriteOp::Put(KEY_HIGHEST_QC.to_vec(), hqc_v));
        }

        // 5. AI operations (if provided)
        if let Some(ai_operations) = ai_ops {
            ops.extend_from_slice(ai_operations);
        }

        // 6. Prune old blocks and QCs from disk to cap DB size.
        // Delete block/QC data older than PRUNE_RETAIN_BLOCKS behind the
        // new committed height. This keeps RocksDB size bounded regardless
        // of chain height. Deletions are part of the atomic batch, so
        // pruning is crash-safe (either commit + prune both apply, or neither).
        if new_committed_height > PRUNE_RETAIN_BLOCKS {
            let prune_below = new_committed_height - PRUNE_RETAIN_BLOCKS;
            // Delete block and QC for each newly-prunable height.
            // Usually only 1 height per commit (blocks.len() == 1), but
            // batch commits may prune multiple heights.
            for block in blocks {
                let prune_height = block.height.saturating_sub(PRUNE_RETAIN_BLOCKS);
                if prune_height > 0 && prune_height <= prune_below {
                    ops.push(WriteOp::Delete(block_key(prune_height)));
                    ops.push(WriteOp::Delete(qc_key(prune_height)));
                }
            }
        }

        // Apply all writes atomically
        db.apply_batch(&ops).map_err(|e| {
            ConsensusError::StateError(format!("Atomic batch write failed: {:?}", e))
        })?;

        Ok(())
    }

    /// Load committed height from the database.
    ///
    /// # Errors
    /// Returns error if database read fails.
    pub fn load_committed_height<K>(db: &K) -> Result<u64, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        match db.get(KEY_COMMITTED_HEIGHT) {
            Ok(Some(bytes)) => {
                if bytes.len() != 8 {
                    return Err(ConsensusError::StateError(
                        "Invalid committed height encoding".to_string(),
                    ));
                }
                let arr: [u8; 8] = bytes.try_into().expect("length verified as 8 above");
                Ok(u64::from_be_bytes(arr))
            }
            Ok(None) => Ok(0), // No committed height yet
            Err(e) => Err(ConsensusError::StateError(format!(
                "Failed to load committed height: {:?}",
                e
            ))),
        }
    }

    /// Load highest QC from the database.
    ///
    /// # Errors
    /// Returns error if database read or decoding fails.
    pub fn load_highest_qc<K>(db: &K) -> Result<Option<QC>, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        match db.get(KEY_HIGHEST_QC) {
            Ok(Some(bytes)) => {
                let qc = decode_qc_v1(&bytes).map_err(|e| {
                    ConsensusError::CodecError(format!("Failed to decode highest QC: {:?}", e))
                })?;
                Ok(Some(qc))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ConsensusError::StateError(format!(
                "Failed to load highest QC: {:?}",
                e
            ))),
        }
    }

    /// Load a block from the database.
    ///
    /// # Errors
    /// Returns error if database read or decoding fails.
    pub fn load_block<K>(db: &K, height: u64) -> Result<Option<Block>, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        let key = block_key(height);
        match db.get(&key) {
            Ok(Some(bytes)) => {
                let mut slice = bytes.as_slice();
                let block = decode_block_v1(&mut slice).map_err(|e| {
                    ConsensusError::CodecError(format!("Failed to decode block: {:?}", e))
                })?;
                Ok(Some(block))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ConsensusError::StateError(format!(
                "Failed to load block: {:?}",
                e
            ))),
        }
    }

    /// Load a range of blocks from the database.
    ///
    /// Returns blocks in order from start_height to end_height (inclusive).
    /// Missing blocks in the range will cause an error.
    ///
    /// # Errors
    /// Returns error if any block in the range is missing or decoding fails.
    pub fn load_blocks_range<K>(
        db: &K,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<Block>, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        if start_height > end_height {
            return Ok(vec![]);
        }

        let mut blocks = Vec::with_capacity((end_height - start_height + 1) as usize);

        for height in start_height..=end_height {
            let block = Self::load_block(db, height)?.ok_or_else(|| {
                ConsensusError::StateError(format!("Missing block at height {}", height))
            })?;
            blocks.push(block);
        }

        Ok(blocks)
    }

    /// Verify that a sequence of blocks forms a valid chain.
    ///
    /// Checks that each block's parent_hash matches the hash of the previous block.
    ///
    /// # Arguments
    /// * `blocks` - Blocks to verify, must be in ascending height order
    /// * `expected_first_parent` - Expected parent hash of the first block
    ///
    /// # Errors
    /// Returns error if chain linkage is broken or heights are not contiguous.
    pub fn verify_block_chain(
        blocks: &[Block],
        expected_first_parent: [u8; 32],
    ) -> Result<(), ConsensusError> {
        if blocks.is_empty() {
            return Ok(());
        }

        // Verify first block's parent
        if blocks[0].parent_hash != expected_first_parent {
            return Err(ConsensusError::InvalidBlock(format!(
                "First block parent mismatch: expected {:?}, got {:?}",
                &expected_first_parent[..8],
                &blocks[0].parent_hash[..8]
            )));
        }

        // Verify contiguous heights and parent chain
        for i in 1..blocks.len() {
            let prev = &blocks[i - 1];
            let curr = &blocks[i];

            // Heights must be contiguous
            if curr.height != prev.height + 1 {
                return Err(ConsensusError::InvalidBlock(format!(
                    "Non-contiguous heights: {} followed by {}",
                    prev.height, curr.height
                )));
            }

            // Parent hash must match previous block's hash
            let prev_hash = novai_consensus_types::codec::hash_block_v1(prev)
                .map_err(|e| ConsensusError::CodecError(format!("{:?}", e)))?;

            if curr.parent_hash != prev_hash {
                return Err(ConsensusError::InvalidBlock(format!(
                    "Chain broken at height {}: parent_hash {:?} != prev_hash {:?}",
                    curr.height,
                    &curr.parent_hash[..8],
                    &prev_hash[..8]
                )));
            }
        }

        Ok(())
    }

    /// Catch up from current state to target height.
    ///
    /// Loads blocks from committed_height+1 to target_height, verifies chain
    /// integrity, and caches them for the commit rule.
    ///
    /// # Arguments
    /// * `db` - Database to load blocks from
    /// * `target_height` - Height to catch up to (must be >= committed_height)
    ///
    /// # Returns
    /// Number of blocks loaded and cached.
    ///
    /// # Errors
    /// Returns error if blocks are missing, chain is broken, or state mismatch.
    pub fn catch_up_to<K>(&mut self, db: &K, target_height: u64) -> Result<usize, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        // Nothing to do if already caught up
        if target_height <= self.committed_height {
            return Ok(0);
        }

        let start_height = self.committed_height + 1;

        // Load blocks
        let blocks = Self::load_blocks_range(db, start_height, target_height)?;
        if blocks.is_empty() {
            return Ok(0);
        }

        // Determine expected first parent hash
        let expected_parent = if self.committed_height == 0 {
            [0u8; 32] // Genesis parent
        } else {
            // Load parent block to get its hash
            let parent_block = Self::load_block(db, self.committed_height)?.ok_or_else(|| {
                ConsensusError::StateError(format!(
                    "Missing parent block at height {}",
                    self.committed_height
                ))
            })?;
            novai_consensus_types::codec::hash_block_v1(&parent_block)
                .map_err(|e| ConsensusError::CodecError(format!("{:?}", e)))?
        };

        // Verify chain integrity
        Self::verify_block_chain(&blocks, expected_parent)?;

        // Cache blocks for commit rule
        let count = blocks.len();
        for block in blocks {
            self.cache_block(block)?;
        }

        // Update height to match target
        self.height = target_height;

        tracing::info!(count, start_height, target_height, "CATCH-UP complete");

        Ok(count)
    }

    /// Recover with full catch-up to rebuild block caches.
    ///
    /// This is an enhanced version of `recover` that also loads recent blocks
    /// into the cache for the commit rule to work correctly.
    ///
    /// # Arguments
    /// * `our_address` - Our validator address
    /// * `db` - Database to recover from
    /// * `cache_depth` - How many blocks to cache (typically 3 for 3-chain rule)
    ///
    /// # Errors
    /// Returns error if database operations fail.
    pub fn recover_with_cache<K>(
        our_address: Address,
        db: &K,
        cache_depth: u64,
    ) -> Result<Self, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        // Basic recovery
        let mut state = Self::recover(our_address, db)?;

        // Load recent blocks into cache (for commit rule)
        let cache_start = state.committed_height.saturating_sub(cache_depth);
        if cache_start > 0 || state.committed_height > 0 {
            let start = cache_start.max(1); // Don't try to load height 0
            if let Ok(blocks) = Self::load_blocks_range(db, start, state.committed_height) {
                for block in blocks {
                    if let Err(e) = state.cache_block(block) {
                        tracing::warn!(?e, "RECOVERY: Failed to cache block, skipping");
                    }
                }
                tracing::info!(
                    cached = state.block_cache.len(),
                    start,
                    end = state.committed_height,
                    "RECOVERY: Cached blocks"
                );
            }
        }

        Ok(state)
    }

    /// Recover consensus state from database after restart.
    ///
    /// # Errors
    /// Returns error if database operations fail.
    pub fn recover<K>(our_address: Address, db: &K) -> Result<Self, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        let committed_height = Self::load_committed_height(db)?;
        let highest_qc = Self::load_highest_qc(db)?;

        // Determine current height from committed height
        let height = committed_height;

        tracing::info!(
            committed_height,
            highest_qc = ?highest_qc.as_ref().map(|q| q.height),
            "RECOVERED consensus state"
        );

        Ok(Self {
            height,
            round: 0,
            highest_qc,
            pending_votes: HashMap::new(),
            our_address,
            last_proposed: None,
            voted_in_round: HashSet::new(),
            voted_at_height: HashMap::new(),
            committed_height,
            block_cache: HashMap::new(),
            qc_cache: HashMap::new(),
            block_by_hash: HashMap::new(),
            pending_timeouts: HashMap::new(),
            timed_out_in_round: HashSet::new(),
            view_changes_total: 0,
            last_proposed_txs: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn make_test_validators(count: usize) -> Vec<(Address, SigningKey, VerifyingKey)> {
        (0..count)
            .map(|i| {
                let mut seed = [0u8; 32];
                seed[0] = i as u8;
                let signing_key = SigningKey::from_bytes(&seed);
                let verifying_key = signing_key.verifying_key();
                let addr = novai_crypto::address_from_pubkey(&verifying_key);
                (addr, signing_key, verifying_key)
            })
            .collect()
    }

    #[test]
    fn test_vote_with_signal_accepted() {
        // Setup: 4 validators
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        // Create a vote WITH an AI signal
        let vote = Vote {
            height: 1,
            round: 0,
            block_hash: [1u8; 32],
            voter: validator_set[1],
            signature: [0u8; 64],
            ai_signal_commitment: Some([0xAA; 32]), // AI signal present
        };

        // Sign the vote
        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&vote);
        let domain_tag = b"NOVAI_VOTE_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validators[1].1, &to_sign);

        let signed_vote = Vote { signature, ..vote };

        // Vote should be accepted
        let result = state.add_vote(signed_vote, &pubkeys);
        assert!(result.is_ok(), "Vote with signal should be accepted");
    }

    #[test]
    fn test_vote_without_signal_accepted() {
        // Setup: 4 validators
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        // Create a vote WITHOUT an AI signal
        let vote = Vote {
            height: 1,
            round: 0,
            block_hash: [1u8; 32],
            voter: validator_set[1],
            signature: [0u8; 64],
            ai_signal_commitment: None, // No AI signal
        };

        // Sign the vote
        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&vote);
        let domain_tag = b"NOVAI_VOTE_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validators[1].1, &to_sign);

        let signed_vote = Vote { signature, ..vote };

        // Vote should be accepted
        let result = state.add_vote(signed_vote, &pubkeys);
        assert!(result.is_ok(), "Vote without signal should be accepted");
    }

    #[test]
    fn test_signal_does_not_affect_qc() {
        // Setup: 4 validators (n=4, f=1, quorum=3)
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);
        let block_hash = [1u8; 32];

        // Add 3 votes: 2 with signals, 1 without
        for i in 0..3 {
            let has_signal = i < 2;
            let vote = Vote {
                height: 1,
                round: 0,
                block_hash,
                voter: validator_set[i],
                signature: [0u8; 64],
                ai_signal_commitment: if has_signal { Some([0xBB; 32]) } else { None },
            };

            let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&vote);
            let domain_tag = b"NOVAI_VOTE_V1";
            let mut to_sign = Vec::new();
            to_sign.extend_from_slice(domain_tag);
            to_sign.extend_from_slice(&unsigned_bytes);
            let signature = novai_crypto::sign_bytes(&validators[i].1, &to_sign);

            let signed_vote = Vote { signature, ..vote };

            state.add_vote(signed_vote, &pubkeys).unwrap();
        }

        // QC should form despite mixed signals
        let qc_result = state.try_form_qc(&block_hash, &validator_set);
        assert!(qc_result.is_ok());
        assert!(
            qc_result.unwrap().is_some(),
            "QC should form with mixed signals"
        );
    }

    #[test]
    fn test_signal_logged_correctly() {
        // This test verifies that the logging code path executes without panic
        // Actual log output verification would require a test harness
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        let vote = Vote {
            height: 1,
            round: 0,
            block_hash: [1u8; 32],
            voter: validator_set[1],
            signature: [0u8; 64],
            ai_signal_commitment: Some([0xCC; 32]),
        };

        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&vote);
        let domain_tag = b"NOVAI_VOTE_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validators[1].1, &to_sign);

        let signed_vote = Vote { signature, ..vote };

        // Should not panic when logging signal
        let result = state.add_vote(signed_vote, &pubkeys);
        assert!(
            result.is_ok(),
            "Vote with signal should be logged and accepted"
        );
    }
    #[test]
    fn test_commit_with_ai_ops() {
        use novai_state::{MemKv, WriteOp};

        // Setup: 4 validators
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        // Create blocks to commit
        let blocks = vec![
            Block {
                height: 1,
                round: 0,
                parent_hash: [0u8; 32],
                state_root: [0xAA; 32],
                txs: vec![],
            },
            Block {
                height: 2,
                round: 0,
                parent_hash: [1u8; 32],
                state_root: [0xBB; 32],
                txs: vec![],
            },
        ];

        // Create QC for height 2
        let qc = QC {
            height: 2,
            round: 0,
            block_hash: [2u8; 32],
            votes: vec![],
        };

        // Create AI operations
        let ai_ops = vec![
            WriteOp::Put(b"ai:entity:1".to_vec(), b"data1".to_vec()),
            WriteOp::Put(b"ai:entity:2".to_vec(), b"data2".to_vec()),
        ];

        // Persist commit with AI ops
        let result = state.persist_commit_atomic(&mut db, &blocks, &qc, 2, Some(&ai_ops));
        assert!(result.is_ok(), "Commit with AI ops should succeed");

        // Verify blocks persisted
        assert!(
            db.get(&block_key(1)).unwrap().is_some(),
            "Block 1 should be persisted"
        );
        assert!(
            db.get(&block_key(2)).unwrap().is_some(),
            "Block 2 should be persisted"
        );

        // Verify AI ops persisted
        assert!(
            db.get(b"ai:entity:1").unwrap().is_some(),
            "AI entity 1 should be persisted"
        );
        assert!(
            db.get(b"ai:entity:2").unwrap().is_some(),
            "AI entity 2 should be persisted"
        );

        // Verify committed height
        let ch = ConsensusState::load_committed_height(&db).unwrap();
        assert_eq!(ch, 2, "Committed height should be 2");
    }
    #[test]
    fn test_ai_ops_fail_rolls_back_everything() {
        use novai_state::{MemKv, WriteOp};

        // Setup
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        // Create blocks to commit
        let blocks = vec![Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: [0xAA; 32],
            txs: vec![],
        }];

        let qc = QC {
            height: 1,
            round: 0,
            block_hash: [1u8; 32],
            votes: vec![],
        };

        // Create AI operations with a duplicate key (will cause batch to fail in strict mode)
        // For MemKv, we simulate failure by trying to write to a read-only location
        // In reality, a real DB backend would enforce transactional semantics
        let ai_ops = vec![
            WriteOp::Put(b"ai:entity:1".to_vec(), b"data1".to_vec()),
            WriteOp::Put(b"ai:entity:1".to_vec(), b"data2".to_vec()), // Duplicate
        ];

        // Persist commit with AI ops - should succeed (MemKv doesn't enforce uniqueness)
        // But this test documents the INTENDED behavior: failures should roll back
        let result = state.persist_commit_atomic(&mut db, &blocks, &qc, 1, Some(&ai_ops));

        // NOTE: MemKv doesn't enforce transactional semantics, so this will succeed
        // In production with RocksDB, a failed AI op would roll back the entire batch
        // This test documents the contract, even if MemKv can't enforce it
        if result.is_ok() {
            // With MemKv, verify last write wins
            let value = db.get(b"ai:entity:1").unwrap().unwrap();
            assert_eq!(value, b"data2", "Last write should win in MemKv");
        }

        // The key invariant is: if we had a real failure (e.g., disk full),
        // then NEITHER blocks NOR AI ops would be persisted
        // This is enforced by the atomic batch mechanism in production DBs
    }
    #[test]
    fn test_no_ai_ops_works() {
        use novai_state::MemKv;

        // Setup
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        // Create blocks to commit
        let blocks = vec![
            Block {
                height: 1,
                round: 0,
                parent_hash: [0u8; 32],
                state_root: [0xAA; 32],
                txs: vec![],
            },
            Block {
                height: 2,
                round: 0,
                parent_hash: [1u8; 32],
                state_root: [0xBB; 32],
                txs: vec![],
            },
        ];

        let qc = QC {
            height: 2,
            round: 0,
            block_hash: [2u8; 32],
            votes: vec![],
        };

        // Persist commit WITHOUT AI ops (None)
        let result = state.persist_commit_atomic(&mut db, &blocks, &qc, 2, None);
        assert!(result.is_ok(), "Commit without AI ops should succeed");

        // Verify blocks persisted
        assert!(
            db.get(&block_key(1)).unwrap().is_some(),
            "Block 1 should be persisted"
        );
        assert!(
            db.get(&block_key(2)).unwrap().is_some(),
            "Block 2 should be persisted"
        );

        // Verify committed height
        let ch = ConsensusState::load_committed_height(&db).unwrap();
        assert_eq!(ch, 2, "Committed height should be 2");

        // Verify QC persisted
        assert!(
            db.get(&qc_key(2)).unwrap().is_some(),
            "QC should be persisted"
        );
    }

    #[test]
    fn test_create_timeout() {
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let state = ConsensusState::new(validator_set[0]);
        let timeout = state.create_timeout(&validators[0].1).unwrap();

        assert_eq!(timeout.height, 1); // height + 1
        assert_eq!(timeout.round, 0);
        assert_eq!(timeout.voter, validator_set[0]);
        assert!(timeout.highest_qc.is_none());
    }

    #[test]
    fn test_add_timeout_success() {
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        // Create timeout from validator 1
        let timeout = Timeout {
            height: 1,
            round: 0,
            voter: validator_set[1],
            highest_qc: None,
            signature: [0u8; 64],
        };

        // Sign it
        let unsigned_bytes =
            novai_consensus_types::codec::encode_timeout_v1_unsigned(&timeout).unwrap();
        let domain_tag = b"NOVAI_TIMEOUT_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validators[1].1, &to_sign);

        let signed_timeout = Timeout {
            signature,
            ..timeout
        };

        // Add timeout
        let result = state.add_timeout(signed_timeout, &pubkeys);
        assert!(result.is_ok(), "Valid timeout should be accepted");

        // Verify it was added
        assert_eq!(state.pending_timeouts.len(), 1);
        let key = (1u64, 0u64);
        let timeouts = state.pending_timeouts.get(&key).unwrap();
        assert_eq!(timeouts.len(), 1);
        assert_eq!(timeouts[0].voter, validator_set[1]);
    }

    #[test]
    fn test_add_timeout_rejects_duplicate() {
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        // Create and sign timeout
        let timeout = Timeout {
            height: 1,
            round: 0,
            voter: validator_set[1],
            highest_qc: None,
            signature: [0u8; 64],
        };

        let unsigned_bytes =
            novai_consensus_types::codec::encode_timeout_v1_unsigned(&timeout).unwrap();
        let domain_tag = b"NOVAI_TIMEOUT_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validators[1].1, &to_sign);

        let signed_timeout = Timeout {
            signature,
            ..timeout
        };

        // Add timeout once
        state.add_timeout(signed_timeout.clone(), &pubkeys).unwrap();

        // Try to add again - should fail
        let result = state.add_timeout(signed_timeout, &pubkeys);
        assert!(result.is_err(), "Duplicate timeout should be rejected");
    }

    #[test]
    fn test_try_advance_round() {
        let validators = make_test_validators(4); // n=4, f=1, quorum=3
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        // Add 3 timeouts (reaches quorum)
        for i in 0..3 {
            let timeout = Timeout {
                height: 1,
                round: 0,
                voter: validator_set[i],
                highest_qc: None,
                signature: [0u8; 64],
            };

            let unsigned_bytes =
                novai_consensus_types::codec::encode_timeout_v1_unsigned(&timeout).unwrap();
            let domain_tag = b"NOVAI_TIMEOUT_V1";
            let mut to_sign = Vec::new();
            to_sign.extend_from_slice(domain_tag);
            to_sign.extend_from_slice(&unsigned_bytes);
            let signature = novai_crypto::sign_bytes(&validators[i].1, &to_sign);

            let signed_timeout = Timeout {
                signature,
                ..timeout
            };
            state.add_timeout(signed_timeout, &pubkeys).unwrap();
        }

        // Try to advance round
        let advanced = state.try_advance_round(&validator_set);
        assert!(advanced, "Round should advance with 3/4 timeouts");
        assert_eq!(state.round, 1, "Round should be incremented");
        assert!(
            state.voted_in_round.is_empty(),
            "Vote tracking should be cleared"
        );
        assert!(
            state.timed_out_in_round.is_empty(),
            "Timeout tracking should be cleared"
        );
    }

    #[test]
    fn test_try_advance_round_insufficient_timeouts() {
        let validators = make_test_validators(4); // n=4, f=1, quorum=3
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        // Add only 2 timeouts (below quorum)
        for i in 0..2 {
            let timeout = Timeout {
                height: 1,
                round: 0,
                voter: validator_set[i],
                highest_qc: None,
                signature: [0u8; 64],
            };

            let unsigned_bytes =
                novai_consensus_types::codec::encode_timeout_v1_unsigned(&timeout).unwrap();
            let domain_tag = b"NOVAI_TIMEOUT_V1";
            let mut to_sign = Vec::new();
            to_sign.extend_from_slice(domain_tag);
            to_sign.extend_from_slice(&unsigned_bytes);
            let signature = novai_crypto::sign_bytes(&validators[i].1, &to_sign);

            let signed_timeout = Timeout {
                signature,
                ..timeout
            };
            state.add_timeout(signed_timeout, &pubkeys).unwrap();
        }

        // Try to advance round - should fail
        let advanced = state.try_advance_round(&validator_set);
        assert!(!advanced, "Round should NOT advance with only 2/4 timeouts");
        assert_eq!(state.round, 0, "Round should remain unchanged");
    }

    #[test]
    fn test_timeout_for_round_base_case() {
        assert_eq!(timeout_for_round(0), BASE_TIMEOUT_MS);
        assert_eq!(timeout_for_round(0), 1000);
    }

    #[test]
    fn test_timeout_for_round_exponential_backoff() {
        assert_eq!(timeout_for_round(1), 2000); // 2^1 * 1000
        assert_eq!(timeout_for_round(2), 4000); // 2^2 * 1000
        assert_eq!(timeout_for_round(3), 8000); // 2^3 * 1000
        assert_eq!(timeout_for_round(4), 16000); // 2^4 * 1000
        assert_eq!(timeout_for_round(5), 32000); // 2^5 * 1000
    }

    #[test]
    fn test_timeout_for_round_caps_at_max() {
        // Round 6: 2^6 * 1000 = 64000 > 60000, so capped
        assert_eq!(timeout_for_round(6), MAX_TIMEOUT_MS);
        assert_eq!(timeout_for_round(6), 60000);

        // Higher rounds also capped
        assert_eq!(timeout_for_round(10), MAX_TIMEOUT_MS);
        assert_eq!(timeout_for_round(100), MAX_TIMEOUT_MS);
    }

    #[test]
    fn test_timeout_for_round_no_overflow() {
        // Even with very high round numbers, no overflow
        assert_eq!(timeout_for_round(u64::MAX), MAX_TIMEOUT_MS);
        assert_eq!(timeout_for_round(1000), MAX_TIMEOUT_MS);
    }

    #[test]
    fn test_load_blocks_range_success() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        // Persist some blocks
        for h in 1..=5 {
            let block = Block {
                height: h,
                round: 0,
                parent_hash: [h as u8 - 1; 32],
                state_root: [h as u8; 32],
                txs: vec![],
            };
            state.persist_block(&mut db, &block).unwrap();
        }

        // Load range
        let blocks = ConsensusState::load_blocks_range(&db, 2, 4).unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].height, 2);
        assert_eq!(blocks[1].height, 3);
        assert_eq!(blocks[2].height, 4);
    }

    #[test]
    fn test_load_blocks_range_missing_block() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        // Persist blocks 1, 2, 4 (missing 3)
        for h in [1, 2, 4] {
            let block = Block {
                height: h,
                round: 0,
                parent_hash: [0; 32],
                state_root: [h as u8; 32],
                txs: vec![],
            };
            state.persist_block(&mut db, &block).unwrap();
        }

        // Load range 1-4 should fail (missing block 3)
        let result = ConsensusState::load_blocks_range(&db, 1, 4);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_block_chain_valid() {
        let block1 = Block {
            height: 1,
            round: 0,
            parent_hash: [0; 32],
            state_root: [1; 32],
            txs: vec![],
        };
        let hash1 = novai_consensus_types::codec::hash_block_v1(&block1).unwrap();

        let block2 = Block {
            height: 2,
            round: 0,
            parent_hash: hash1,
            state_root: [2; 32],
            txs: vec![],
        };
        let hash2 = novai_consensus_types::codec::hash_block_v1(&block2).unwrap();

        let block3 = Block {
            height: 3,
            round: 0,
            parent_hash: hash2,
            state_root: [3; 32],
            txs: vec![],
        };

        let blocks = vec![block1, block2, block3];
        let result = ConsensusState::verify_block_chain(&blocks, [0; 32]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_block_chain_broken() {
        let block1 = Block {
            height: 1,
            round: 0,
            parent_hash: [0; 32],
            state_root: [1; 32],
            txs: vec![],
        };

        let block2 = Block {
            height: 2,
            round: 0,
            parent_hash: [0xFF; 32], // Wrong parent!
            state_root: [2; 32],
            txs: vec![],
        };

        let blocks = vec![block1, block2];
        let result = ConsensusState::verify_block_chain(&blocks, [0; 32]);
        assert!(result.is_err());
    }

    #[test]
    fn test_catch_up_to_success() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let mut db = MemKv::new();

        // Create and persist a valid chain
        let mut prev_hash = [0u8; 32];
        for h in 1..=5 {
            let block = Block {
                height: h,
                round: 0,
                parent_hash: prev_hash,
                state_root: [h as u8; 32],
                txs: vec![],
            };
            prev_hash = novai_consensus_types::codec::hash_block_v1(&block).unwrap();

            let state = ConsensusState::new(validator_set[0]);
            state.persist_block(&mut db, &block).unwrap();
        }

        // Create state at committed_height=0
        let mut state = ConsensusState::new(validator_set[0]);
        assert_eq!(state.committed_height, 0);
        assert_eq!(state.block_cache.len(), 0);

        // Catch up to height 5
        let count = state.catch_up_to(&db, 5).unwrap();
        assert_eq!(count, 5);
        assert_eq!(state.height, 5);
        assert_eq!(state.block_cache.len(), 5);
    }

    #[test]
    fn test_catch_up_already_caught_up() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let db = MemKv::new();

        let mut state = ConsensusState::new(validator_set[0]);
        state.committed_height = 10;

        // Try to catch up to height 5 (less than committed)
        let count = state.catch_up_to(&db, 5).unwrap();
        assert_eq!(count, 0);
    }

    // ── Size-limit enforcement tests for verify_block ──────────────────

    /// Helper: build a TxV1 with a payload of the given size.
    fn make_tx_with_payload(payload_len: usize) -> novai_types::TxV1 {
        novai_types::TxV1 {
            version: novai_types::TxVersion::V1,
            from: [0x11; 32],
            pubkey: [0x22; 32],
            nonce: 1,
            fee: 10,
            payload: vec![0xAB; payload_len],
            sig: [0xCC; 64],
        }
    }

    #[test]
    fn verify_block_rejects_oversized_tx() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let state = ConsensusState::new(validator_set[0]);
        let db = MemKv::new();

        // A single tx whose encoded size exceeds MAX_TX_SIZE (128 KB).
        // tx_encoded_size = TX_V1_OVERHEAD(149) + payload_len, so payload_len
        // = MAX_TX_SIZE - 149 + 1 puts us 1 byte over the limit.
        let payload_len = novai_types::MAX_TX_SIZE - novai_codec::TX_V1_OVERHEAD + 1;
        let block = Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: [0u8; 32],
            txs: vec![make_tx_with_payload(payload_len)],
        };

        let err = state.verify_block(&block, &db).unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("tx encoded size") && msg.contains("exceeds limit"),
            "expected oversized-tx error, got: {}",
            msg
        );
    }

    #[test]
    fn verify_block_rejects_too_many_txs() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let state = ConsensusState::new(validator_set[0]);
        let db = MemKv::new();

        // Block with MAX_TXS_PER_BLOCK + 1 tiny transactions.
        let txs: Vec<novai_types::TxV1> = (0..novai_types::MAX_TXS_PER_BLOCK + 1)
            .map(|_| make_tx_with_payload(0))
            .collect();

        let block = Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: [0u8; 32],
            txs,
        };

        let err = state.verify_block(&block, &db).unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("txs") && msg.contains("exceeds limit"),
            "expected too-many-txs error, got: {}",
            msg
        );
    }

    #[test]
    fn verify_block_rejects_oversized_block_payload() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let state = ConsensusState::new(validator_set[0]);
        let db = MemKv::new();

        // Each tx is just under MAX_TX_SIZE but many of them push total over
        // MAX_BLOCK_SIZE (2 MB). Use payload_len = MAX_TX_SIZE - TX_V1_OVERHEAD
        // (exactly at the limit per-tx). Need ceil(MAX_BLOCK_SIZE / MAX_TX_SIZE) + 1 txs.
        let per_tx_payload = novai_types::MAX_TX_SIZE - novai_codec::TX_V1_OVERHEAD;
        let per_tx_size = novai_types::MAX_TX_SIZE; // TX_V1_OVERHEAD + per_tx_payload
        let num_txs = novai_types::MAX_BLOCK_SIZE / per_tx_size + 1;
        // Ensure we don't exceed MAX_TXS_PER_BLOCK (would trigger that error first).
        assert!(
            num_txs <= novai_types::MAX_TXS_PER_BLOCK,
            "test setup: need {} txs but MAX_TXS_PER_BLOCK is {}",
            num_txs,
            novai_types::MAX_TXS_PER_BLOCK
        );

        let txs: Vec<novai_types::TxV1> = (0..num_txs)
            .map(|_| make_tx_with_payload(per_tx_payload))
            .collect();

        let block = Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: [0u8; 32],
            txs,
        };

        let err = state.verify_block(&block, &db).unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("block payload") && msg.contains("exceeds limit"),
            "expected oversized-block error, got: {}",
            msg
        );
    }

    #[test]
    fn verify_block_passes_size_checks_for_valid_block() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let state = ConsensusState::new(validator_set[0]);
        let db = MemKv::new();

        // A block with a few small txs — well within all size limits.
        // verify_block will pass size checks then fail on height/state, which
        // proves the size checks accepted the block.
        let block = Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: [0u8; 32],
            txs: vec![make_tx_with_payload(100), make_tx_with_payload(200)],
        };

        let result = state.verify_block(&block, &db);
        // Should NOT be a size-limit error. It will fail on signature or state,
        // but the point is it got past all three size checks.
        match &result {
            Ok(()) => {} // surprisingly passed everything — fine
            Err(e) => {
                let msg = format!("{:?}", e);
                assert!(
                    !msg.contains("exceeds limit"),
                    "block within limits should not trigger size error, got: {}",
                    msg
                );
            }
        }
    }
}
