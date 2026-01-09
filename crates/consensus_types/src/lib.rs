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
