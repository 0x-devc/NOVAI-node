//! Consensus engine for NOVAI v1.
//!
//! Week 6: Propose → Vote → QC formation (no commit yet).

#![forbid(unsafe_code)]

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus_types::codec::{decode_block_v1, decode_qc_v1, encode_block_v1, encode_qc_v1};
use novai_consensus_types::{Block, Vote, QC};
use novai_state::{block_key, qc_key, Kv, KvBatch, KEY_COMMITTED_HEIGHT, KEY_HIGHEST_QC};
use novai_types::Address;
use std::collections::{HashMap, HashSet};

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
        // Check if already proposed for this height/round
        let proposed_key = (self.height + 1, self.round);
        if self.last_proposed == Some(proposed_key) {
            return Err(ConsensusError::NotLeader);
        }

        // Check if we're the leader
        let leader = self.compute_leader(validator_set)?;
        if leader != self.our_address {
            return Err(ConsensusError::NotLeader);
        }

        // Drain ready transactions from mempool
        let txs = mempool.drain_ready(1000, nonce_provider);

        // Compute parent hash
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
            height: self.height + 1,
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
        // Check height is next
        if block.height != self.height + 1 {
            return Err(ConsensusError::InvalidBlock(format!(
                "Height mismatch: expected {}, got {}",
                self.height + 1,
                block.height
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
        // Verify vote is for next height
        if vote.height != self.height + 1 {
            return Err(ConsensusError::InvalidVote(format!(
                "Vote height mismatch: expected {}, got {}",
                self.height + 1,
                vote.height
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

        let qc = QC {
            height: self.height + 1,
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

    /// Compute leader for current height/round (convenience wrapper).
    fn compute_leader(&self, validator_set: &[Address]) -> Result<Address, ConsensusError> {
        Self::compute_leader_for_view(self.height, self.round, validator_set)
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
            self.last_proposed = None;
        }
    }

    /// Check for conflicting commits (fork detection).
    ///
    /// # Panics
    /// Panics if two different blocks claim the same height.
    pub fn check_no_fork(&self, block: &Block) {
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
        })
    }
}
