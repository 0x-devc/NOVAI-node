//! Consensus engine for NOVAI v1.
//!
//! Week 6: Propose → Vote → QC formation (no commit yet).

#![forbid(unsafe_code)]

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus_types::{Block, Vote, QC};
use novai_types::Address;
use std::collections::HashMap;

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
    /// Current height.
    pub height: u64,
    /// Current round.
    pub round: u64,
    /// Highest QC we've seen.
    pub highest_qc: Option<QC>,
    /// Pending votes for current round (keyed by block_hash).
    pub pending_votes: HashMap<[u8; 32], Vec<Vote>>,
    /// Our validator address.
    pub our_address: Address,
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
            height: self.height,
            round: self.round,
            parent_hash,
            state_root,
            txs,
        };

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
        // Check height matches
        if block.height != self.height {
            return Err(ConsensusError::InvalidBlock(format!(
                "Height mismatch: expected {}, got {}",
                self.height, block.height
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

        // Week 6: Basic structure checks only
        // Transaction signature verification happens in execution layer
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
            height: self.height,
            round: self.round,
            block_hash,
            voter: self.our_address,
            signature: [0u8; 64], // Placeholder
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
            height: self.height,
            round: self.round,
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
        // Verify vote matches current height/round
        if vote.height != self.height || vote.round != self.round {
            return Err(ConsensusError::InvalidVote(
                "Vote height/round mismatch".to_string(),
            ));
        }

        // Find voter's public key in validator set
        let pubkey = validator_pubkeys
            .iter()
            .find(|(addr, _)| *addr == vote.voter)
            .map(|(_, pk)| pk)
            .ok_or_else(|| ConsensusError::InvalidVote("Voter not in validator set".to_string()))?;

        // Check for duplicate vote from same voter (before expensive signature check)
        if let Some(votes) = self.pending_votes.get(&vote.block_hash) {
            if votes.iter().any(|v| v.voter == vote.voter) {
                return Err(ConsensusError::InvalidVote(
                    "Duplicate vote from same voter (equivocation)".to_string(),
                ));
            }
        }

        // Create unsigned vote for verification
        let unsigned_vote = Vote {
            height: vote.height,
            round: vote.round,
            block_hash: vote.block_hash,
            voter: vote.voter,
            signature: [0u8; 64], // Placeholder
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
            return Ok(None); // Not enough votes yet
        }

        // Form QC with exactly quorum votes
        let qc_votes: Vec<Vote> = votes.iter().take(quorum).cloned().collect();

        let qc = QC {
            height: self.height,
            round: self.round,
            block_hash: *block_hash,
            votes: qc_votes,
        };

        Ok(Some(qc))
    }

    /// Compute leader for current height/round.
    fn compute_leader(&self, validator_set: &[Address]) -> Result<Address, ConsensusError> {
        if validator_set.is_empty() {
            return Err(ConsensusError::InvalidBlock(
                "Empty validator set".to_string(),
            ));
        }
        let idx = (self.height.wrapping_add(self.round) as usize) % validator_set.len();
        Ok(validator_set[idx])
    }
}
