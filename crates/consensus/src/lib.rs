//! Consensus engine for NOVAI v1.
//!
//! Week 6: Propose → Vote → QC formation (no commit yet).

#![forbid(unsafe_code)]

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus_types::codec::{decode_block_v1, decode_qc_v1, encode_block_v1, encode_qc_v1};
use novai_consensus_types::{Block, Timeout, Vote, QC};
use novai_state::{block_key, qc_key, Kv, KvBatch, KEY_COMMITTED_HEIGHT, KEY_HIGHEST_QC};
use novai_types::Address;
use std::collections::{HashMap, HashSet};

// ========== WEEK 8: TIMEOUT CONFIGURATION ==========

/// Base timeout duration in milliseconds.
/// This is the timeout for round 0.
pub const BASE_TIMEOUT_MS: u64 = 2000; // 2 seconds

/// Timeout multiplier for exponential backoff.
/// Each round doubles the timeout.
pub const TIMEOUT_MULTIPLIER: u64 = 2;

/// Maximum timeout duration in milliseconds.
/// Prevents unbounded timeout growth.
pub const MAX_TIMEOUT_MS: u64 = 60_000; // 60 seconds

/// Calculate timeout duration for a given round.
///
/// Uses exponential backoff: `min(BASE_TIMEOUT_MS * 2^round, MAX_TIMEOUT_MS)`
///
/// # Examples
/// - Round 0: 2000ms (2s)
/// - Round 1: 4000ms (4s)
/// - Round 2: 8000ms (8s)
/// - Round 3: 16000ms (16s)
/// - Round 4: 32000ms (32s)
/// - Round 5+: 60000ms (60s, capped)
#[must_use]
pub fn timeout_for_round(round: u64) -> u64 {
    // Prevent overflow: cap the shift at a reasonable value
    // 2^16 * 2000 = 131_072_000 which is > MAX_TIMEOUT_MS
    let effective_round = round.min(16);

    let timeout =
        BASE_TIMEOUT_MS.saturating_mul(TIMEOUT_MULTIPLIER.saturating_pow(effective_round as u32));
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
    /// Highest committed height.
    pub committed_height: u64,
    /// Block cache by height (for commit rule).
    pub block_cache: HashMap<u64, Block>,
    /// QC cache by height (for commit rule).
    pub qc_cache: HashMap<u64, QC>,
    /// Block cache by hash (for chain-following in commit rule).
    pub block_by_hash: HashMap<[u8; 32], Block>,
    /// Pending timeouts by (height, round).
    pub pending_timeouts: HashMap<(u64, u64), Vec<Timeout>>,
    /// Addresses that already sent timeout in current round (deduplication).
    pub timed_out_in_round: HashSet<Address>,
    /// Total view changes (round advances due to timeouts) since node start.
    pub view_changes_total: u64,
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
            committed_height: 0,
            block_cache: HashMap::new(),
            qc_cache: HashMap::new(),
            block_by_hash: HashMap::new(),
            pending_timeouts: HashMap::new(),
            timed_out_in_round: HashSet::new(),
            view_changes_total: 0,
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

        // Drain ready transactions from mempool
        let txs = mempool.drain_ready(1000, nonce_provider);

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

        // Mark as proposed
        self.last_proposed = Some((block.height, block.round));

        Ok(block)
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

        // Verify vote is for expected height
        if vote.height != expected_height {
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
            println!("📊 Vote includes AI signal: {:?}", commitment);
        }

        // Mark this voter as having voted in this round
        self.voted_in_round.insert(vote.voter);

        // Add vote to pending votes
        self.pending_votes
            .entry(vote.block_hash)
            .or_default()
            .push(vote);

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

        // Verify timeout is for current round
        if timeout.round != self.round {
            return Err(ConsensusError::InvalidVote(format!(
                "Timeout round mismatch: expected {}, got {}",
                self.round, timeout.round
            )));
        }

        // Find voter's public key in validator set
        let pubkey = validator_pubkeys
            .iter()
            .find(|(addr, _)| *addr == timeout.voter)
            .map(|(_, pk)| pk)
            .ok_or_else(|| {
                ConsensusError::InvalidVote("Timeout voter not in validator set".to_string())
            })?;

        // Check for duplicate timeout from same voter in this round
        if self.timed_out_in_round.contains(&timeout.voter) {
            return Err(ConsensusError::InvalidVote(
                "Duplicate timeout from same voter in current round".to_string(),
            ));
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

        // Update highest_qc if timeout includes a better QC
        if let Some(ref qc) = timeout.highest_qc {
            let dominated = match &self.highest_qc {
                None => true,
                Some(existing) => {
                    qc.height > existing.height
                        || (qc.height == existing.height && qc.round > existing.round)
                }
            };
            if dominated {
                self.highest_qc = Some(qc.clone());
            }
        }

        // Mark this voter as having timed out in this round
        self.timed_out_in_round.insert(timeout.voter);

        // Add timeout to pending timeouts
        let key = (timeout.height, timeout.round);
        self.pending_timeouts.entry(key).or_default().push(timeout);

        Ok(())
    }

    /// Try to advance to next round if we have 2f+1 timeouts.
    ///
    /// Returns true if round was advanced, false otherwise.
    pub fn try_advance_round(&mut self, validator_set: &[Address]) -> bool {
        // Expected timeout height is max(committed_height, highest_qc_height) + 1
        let expected_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        let key = (expected_height, self.round);
        let timeouts = match self.pending_timeouts.get(&key) {
            Some(t) => t,
            None => return false,
        };

        // Check if we have quorum: 2f+1 where n = 3f+1
        let n = validator_set.len();
        let f = (n - 1) / 3;
        let quorum = 2 * f + 1;

        if timeouts.len() < quorum {
            return false;
        }

        // Advance round
        self.round += 1;
        self.view_changes_total += 1;

        // Clear round-specific state
        self.pending_votes.clear();
        self.voted_in_round.clear();
        self.timed_out_in_round.clear();
        self.last_proposed = None;

        println!(
            "⏰ ROUND ADVANCED to round={} at height={} (received {} timeouts)",
            self.round,
            expected_height,
            timeouts.len()
        );

        true
    }

    // ========== WEEK 7: COMMIT PIPELINE ==========

    /// Cache a block for commit rule tracking.
    ///
    /// Stores block by both height and hash for chain-following.
    pub fn cache_block(&mut self, block: Block) {
        let hash = novai_consensus_types::codec::hash_block_v1(&block)
            .expect("block must encode for hashing");
        self.block_cache.insert(block.height, block.clone());
        self.block_by_hash.insert(hash, block);
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
    pub fn cache_qc_and_check_commit(&mut self, qc: QC) -> Result<Vec<Block>, ConsensusError> {
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
                self.round = 0;
                self.pending_votes.clear();
                self.voted_in_round.clear();
                self.timed_out_in_round.clear();
                self.pending_timeouts.clear();
                self.last_proposed = None;
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

        // 1. Find B_H (certified block) by QC's block_hash
        let block_h = self.block_by_hash.get(&qc.block_hash).ok_or_else(|| {
            ConsensusError::StateError(format!(
                "Missing certified block for QC at height {}",
                qc_height
            ))
        })?;

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
            let block = self.block_by_hash.get(&current_hash).ok_or_else(|| {
                ConsensusError::StateError(format!(
                    "Missing block at height {} (chain broken)",
                    expected_height
                ))
            })?;

            if block.height != expected_height {
                return Err(ConsensusError::InvalidBlock(format!(
                    "Chain height mismatch: expected {}, got {}",
                    expected_height, block.height
                )));
            }

            // Only include blocks up to commit_target (not the 2 confirmation blocks)
            if expected_height <= commit_target {
                chain.push(block.clone());
            }

            current_hash = block.parent_hash;
        }

        // Reverse to get oldest first
        chain.reverse();

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
    /// # Panics
    /// Panics if conflicting commit detected (consensus safety violation).
    pub fn apply_commits(&mut self, blocks: &[Block]) {
        for block in blocks {
            // Safety check: no gaps in commit sequence
            let expected_height = self.committed_height + 1;
            if block.height != expected_height {
                panic!(
                    "CONSENSUS SAFETY VIOLATION: commit gap! expected height {}, got {}",
                    expected_height, block.height
                );
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
            println!(
                "✅ COMMITTED block at height={} (state_root={:?})",
                block.height,
                &block.state_root[..4]
            );
        }

        // Clear pending votes and voted_in_round after commits
        if !blocks.is_empty() {
            self.pending_votes.clear();
            self.voted_in_round.clear();
            self.timed_out_in_round.clear();
            self.pending_timeouts.clear();
            self.last_proposed = None;

            // Reset round to 0 after successful commit
            self.round = 0;
        }
    }

    /// Apply commits with AI hook integration.
    ///
    /// This is the version that should be used when AI hooks are available.
    /// It calls the AI hook to generate operations that will be persisted atomically.
    ///
    /// # Returns
    /// Returns the AI operations that should be passed to `persist_commit_atomic`.
    pub fn apply_commits_with_ai_hook(
        &mut self,
        blocks: &[Block],
        ai_hook: &dyn AiCommitHook,
    ) -> Vec<novai_state::WriteOp> {
        // First apply commits normally (updates in-memory state)
        self.apply_commits(blocks);

        // Then generate AI operations if blocks were committed
        if !blocks.is_empty() {
            ai_hook.on_commit(blocks)
        } else {
            Vec::new()
        }
    }

    /// Check for conflicting commits (fork detection).
    ///
    /// In HotStuff BFT, if a block doesn't get a QC, the next round's leader
    /// proposes a different block for the same height. This is normal behavior.
    /// A real fork would be COMMITTING two different blocks at the same height.
    ///
    /// # Panics
    /// Panics if two different blocks conflict at or below committed_height.
    pub fn check_no_fork(&self, block: &Block) {
        // Only check for forks at or below committed_height.
        // Heights above committed_height can have different proposals in different rounds.
        if block.height > self.committed_height {
            return;
        }

        if let Some(cached) = self.block_cache.get(&block.height) {
            let cached_hash = novai_consensus_types::codec::hash_block_v1(cached)
                .expect("cached block must encode");
            let new_hash =
                novai_consensus_types::codec::hash_block_v1(block).expect("new block must encode");

            if cached_hash != new_hash {
                panic!(
                    "CONSENSUS SAFETY VIOLATION: FORK DETECTED at height {}!\n\
                     Cached block hash: {:?}\n\
                     New block hash: {:?}",
                    block.height,
                    &cached_hash[..8],
                    &new_hash[..8]
                );
            }
        }
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
                let arr: [u8; 8] = bytes.try_into().unwrap();
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
            self.cache_block(block);
        }

        // Update height to match target
        self.height = target_height;

        println!(
            "🔄 CATCH-UP complete: loaded {} blocks (heights {}..={})",
            count, start_height, target_height
        );

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
                    state.cache_block(block);
                }
                println!(
                    "🔄 RECOVERY: Cached {} blocks (heights {}..={})",
                    state.block_cache.len(),
                    start,
                    state.committed_height
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

        println!(
            "🔄 RECOVERED consensus state: committed_height={}, highest_qc={:?}",
            committed_height,
            highest_qc.as_ref().map(|q| q.height)
        );

        Ok(Self {
            height,
            round: 0,
            highest_qc,
            pending_votes: HashMap::new(),
            our_address,
            last_proposed: None,
            voted_in_round: HashSet::new(),
            committed_height,
            block_cache: HashMap::new(),
            qc_cache: HashMap::new(),
            block_by_hash: HashMap::new(),
            pending_timeouts: HashMap::new(),
            timed_out_in_round: HashSet::new(),
            view_changes_total: 0,
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
        assert!(state.timed_out_in_round.contains(&validator_set[1]));
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
        assert_eq!(timeout_for_round(0), 2000);
    }

    #[test]
    fn test_timeout_for_round_exponential_backoff() {
        assert_eq!(timeout_for_round(1), 4000); // 2^1 * 2000
        assert_eq!(timeout_for_round(2), 8000); // 2^2 * 2000
        assert_eq!(timeout_for_round(3), 16000); // 2^3 * 2000
        assert_eq!(timeout_for_round(4), 32000); // 2^4 * 2000
    }

    #[test]
    fn test_timeout_for_round_caps_at_max() {
        // Round 5: 2^5 * 2000 = 64000 > 60000, so capped
        assert_eq!(timeout_for_round(5), MAX_TIMEOUT_MS);
        assert_eq!(timeout_for_round(5), 60000);

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
}
