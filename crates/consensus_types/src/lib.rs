//! Consensus message types for NOVAI v1.
//!
//! This crate defines the core consensus data structures:
//! - Block
//! - Proposal
//! - Vote
//! - QC (Quorum Certificate)
//! - Timeout
//!
//! All types have canonical encodings defined in the codec module.
//! Leader selection is deterministic and defined in the leader module.

#![forbid(unsafe_code)]

use novai_types::Address;

pub mod codec;
pub mod leader;

/// Consensus block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub height: u64,
    pub round: u64,
    pub parent_hash: [u8; 32],
    pub state_root: [u8; 32],
    pub txs: Vec<novai_types::TxV1>,
}

/// Proposal message (block + justification).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub block: Block,
    pub justify_qc: QC,
}
/// Signed proposal for network transmission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedProposal {
    pub proposer: Address,
    pub proposal: Proposal,
    pub signature: [u8; 64],
}
/// Vote message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vote {
    pub height: u64,
    pub round: u64,
    pub block_hash: [u8; 32],
    pub voter: Address,
    pub signature: [u8; 64],
    /// Optional AI signal commitment (hash only, advisory). Does not affect vote validity.
    pub ai_signal_commitment: Option<[u8; 32]>,
}

/// Quorum Certificate (aggregated votes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QC {
    pub height: u64,
    pub round: u64,
    pub block_hash: [u8; 32],
    pub votes: Vec<Vote>,
}

/// Timeout message for view-change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeout {
    pub height: u64,
    pub round: u64,
    pub voter: Address,
    pub highest_qc: Option<QC>,
    pub signature: [u8; 64],
}

/// Block request for peer-to-peer sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRequest {
    pub requester: Address,
    pub start_height: u64,
    pub end_height: u64,
}

/// Block response for peer-to-peer sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockResponse {
    pub responder: Address,
    pub request_start: u64,
    pub request_end: u64,
    pub blocks: Vec<Block>,
}

/// Compute the hash of a block using canonical encoding.
///
/// # Panics
/// Panics if block encoding fails (should never happen for valid blocks).
#[must_use]
pub fn block_hash(block: &Block) -> [u8; 32] {
    let encoded = codec::encode_block_v1(block).expect("block encoding should never fail");
    *blake3::hash(&encoded).as_bytes()
}
