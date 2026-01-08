//! Canonical encodings for consensus messages.
//!
//! All encodings are deterministic and use big-endian byte order.
//! Format versioning: each message type has a version byte prefix.

use crate::{Block, Proposal, Timeout, Vote, QC};

/// Codec version for Block.
pub const BLOCK_V1: u8 = 0x01;

/// Codec version for Vote (unsigned).
pub const VOTE_UNSIGNED_V1: u8 = 0x01;

/// Codec version for Vote (signed).
pub const VOTE_SIGNED_V1: u8 = 0x01;

/// Codec version for QC.
pub const QC_V1: u8 = 0x01;

/// Codec version for Proposal.
pub const PROPOSAL_V1: u8 = 0x01;

/// Codec version for Timeout (unsigned).
pub const TIMEOUT_UNSIGNED_V1: u8 = 0x01;

/// Codec version for Timeout (signed).
pub const TIMEOUT_SIGNED_V1: u8 = 0x01;

// ============================================================================
// Block Encoding
// ============================================================================

/// Encode a Block to canonical bytes.
///
/// Format:
/// ```text
/// [version:1][height:8][round:8][parent_hash:32][state_root:32][tx_count:4][txs_bytes]
/// ```
///
/// # Panics
/// Panics if transaction encoding fails (should never happen for valid `TxV1`).
#[must_use]
pub fn encode_block_v1(block: &Block) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(BLOCK_V1);
    buf.extend_from_slice(&block.height.to_be_bytes());
    buf.extend_from_slice(&block.round.to_be_bytes());
    buf.extend_from_slice(&block.parent_hash);
    buf.extend_from_slice(&block.state_root);

    // Encode transaction count
    #[allow(clippy::cast_possible_truncation)]
    let tx_count = block.txs.len() as u32; // Safe: consensus blocks won't have 4B+ txs
    buf.extend_from_slice(&tx_count.to_be_bytes());

    // Encode each transaction using TxV1 signed encoding
    for tx in &block.txs {
        let tx_bytes = novai_codec::encode_tx_v1_signed(tx).expect("tx encoding should not fail");
        buf.extend_from_slice(&tx_bytes);
    }

    buf
}

/// Compute the hash of a Block.
#[must_use]
pub fn hash_block_v1(block: &Block) -> [u8; 32] {
    let bytes = encode_block_v1(block);
    blake3::hash(&bytes).into()
}

// ============================================================================
// Vote Encoding
// ============================================================================

/// Encode a Vote to canonical unsigned bytes (for signing).
///
/// Format:
/// ```text
/// [version:1][height:8][round:8][block_hash:32][voter:32]
/// ```
#[must_use]
pub fn encode_vote_v1_unsigned(vote: &Vote) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(VOTE_UNSIGNED_V1);
    buf.extend_from_slice(&vote.height.to_be_bytes());
    buf.extend_from_slice(&vote.round.to_be_bytes());
    buf.extend_from_slice(&vote.block_hash);
    buf.extend_from_slice(&vote.voter);
    buf
}

/// Encode a Vote to canonical signed bytes (includes signature).
///
/// Format:
/// ```text
/// [unsigned_bytes][signature:64]
/// ```
#[must_use]
pub fn encode_vote_v1_signed(vote: &Vote) -> Vec<u8> {
    let mut buf = encode_vote_v1_unsigned(vote);
    buf.extend_from_slice(&vote.signature);
    buf
}

// ============================================================================
// QC Encoding
// ============================================================================

/// Encode a QC to canonical bytes.
///
/// Format:
/// ```text
/// [version:1][height:8][round:8][block_hash:32][vote_count:4][votes_bytes]
/// ```
#[must_use]
pub fn encode_qc_v1(qc: &QC) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(QC_V1);
    buf.extend_from_slice(&qc.height.to_be_bytes());
    buf.extend_from_slice(&qc.round.to_be_bytes());
    buf.extend_from_slice(&qc.block_hash);

    // Encode vote count
    #[allow(clippy::cast_possible_truncation)]
    let vote_count = qc.votes.len() as u32; // Safe: QCs won't have 4B+ votes
    buf.extend_from_slice(&vote_count.to_be_bytes());

    // Encode each vote (signed format)
    for vote in &qc.votes {
        let vote_bytes = encode_vote_v1_signed(vote);
        buf.extend_from_slice(&vote_bytes);
    }

    buf
}

// ============================================================================
// Proposal Encoding
// ============================================================================

/// Encode a Proposal to canonical bytes.
///
/// Format:
/// ```text
/// [version:1][block_bytes][qc_bytes]
/// ```
#[must_use]
pub fn encode_proposal_v1(proposal: &Proposal) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(PROPOSAL_V1);

    // Encode block
    let block_bytes = encode_block_v1(&proposal.block);
    buf.extend_from_slice(&block_bytes);

    // Encode justify QC
    let qc_bytes = encode_qc_v1(&proposal.justify_qc);
    buf.extend_from_slice(&qc_bytes);

    buf
}

// ============================================================================
// Timeout Encoding
// ============================================================================

/// Encode a Timeout to canonical unsigned bytes (for signing).
///
/// Format:
/// ```text
/// [version:1][height:8][round:8][voter:32][has_qc:1][qc_bytes?]
/// ```
#[must_use]
pub fn encode_timeout_v1_unsigned(timeout: &Timeout) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(TIMEOUT_UNSIGNED_V1);
    buf.extend_from_slice(&timeout.height.to_be_bytes());
    buf.extend_from_slice(&timeout.round.to_be_bytes());
    buf.extend_from_slice(&timeout.voter);

    // Encode optional QC
    match &timeout.highest_qc {
        Some(qc) => {
            buf.push(0x01); // has_qc = true
            let qc_bytes = encode_qc_v1(qc);
            buf.extend_from_slice(&qc_bytes);
        }
        None => {
            buf.push(0x00); // has_qc = false
        }
    }

    buf
}

/// Encode a Timeout to canonical signed bytes (includes signature).
///
/// Format:
/// ```text
/// [unsigned_bytes][signature:64]
/// ```
#[must_use]
pub fn encode_timeout_v1_signed(timeout: &Timeout) -> Vec<u8> {
    let mut buf = encode_timeout_v1_unsigned(timeout);
    buf.extend_from_slice(&timeout.signature);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_encoding_deterministic() {
        let block = Block {
            height: 42,
            round: 7,
            parent_hash: [0xaa; 32],
            state_root: [0xbb; 32],
            txs: vec![],
        };

        let bytes1 = encode_block_v1(&block);
        let bytes2 = encode_block_v1(&block);
        assert_eq!(bytes1, bytes2);
    }

    #[test]
    fn vote_encoding_deterministic() {
        let vote = Vote {
            height: 10,
            round: 3,
            block_hash: [0xcc; 32],
            voter: [0xdd; 32],
            signature: [0xee; 64],
        };

        let unsigned1 = encode_vote_v1_unsigned(&vote);
        let unsigned2 = encode_vote_v1_unsigned(&vote);
        assert_eq!(unsigned1, unsigned2);

        let signed1 = encode_vote_v1_signed(&vote);
        let signed2 = encode_vote_v1_signed(&vote);
        assert_eq!(signed1, signed2);
    }

    #[test]
    fn qc_encoding_deterministic() {
        let qc = QC {
            height: 5,
            round: 2,
            block_hash: [0x11; 32],
            votes: vec![],
        };

        let bytes1 = encode_qc_v1(&qc);
        let bytes2 = encode_qc_v1(&qc);
        assert_eq!(bytes1, bytes2);
    }
}
