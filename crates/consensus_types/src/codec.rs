//! Canonical encodings for consensus messages.
//!
//! All encodings are deterministic and use big-endian byte order.
//! Format versioning: each message type has a version byte prefix.

use crate::{Block, Proposal, SignedProposal, Timeout, Vote, QC};

/// Codec errors for consensus messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// Transaction encoding failed.
    TxEncodingFailed,
    /// Transaction decoding failed.
    TxDecodingFailed,
    /// QC has duplicate voters.
    DuplicateVoter,
    /// QC votes are not sorted by voter.
    VotesNotSorted,
    /// Block has too many transactions.
    TooManyTransactions,
    /// QC has too many votes.
    TooManyVotes,
    /// Input buffer too short for decoding.
    BufferTooShort,
    /// Unsupported version byte.
    UnsupportedVersion,
}

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

/// Maximum transactions per block (`DoS` prevention).
pub const MAX_TXS_PER_BLOCK: usize = 10_000;

/// Maximum votes per QC (`DoS` prevention).
pub const MAX_VOTES_PER_QC: usize = 11_000;

/// Minimum bytes per transaction (conservative estimate for validation).
const MIN_TX_BYTES: usize = 100;

/// Minimum bytes per vote (conservative estimate for validation).
const MIN_VOTE_BYTES: usize = 100;

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
/// # Errors
/// Returns `CodecError::TooManyTransactions` if `block.txs.len() > MAX_TXS_PER_BLOCK`.
/// Returns `CodecError::TxEncodingFailed` if any transaction fails to encode.
pub fn encode_block_v1(block: &Block) -> Result<Vec<u8>, CodecError> {
    // Check size limit
    if block.txs.len() > MAX_TXS_PER_BLOCK {
        return Err(CodecError::TooManyTransactions);
    }

    let mut buf = Vec::new();
    buf.push(BLOCK_V1);
    buf.extend_from_slice(&block.height.to_be_bytes());
    buf.extend_from_slice(&block.round.to_be_bytes());
    buf.extend_from_slice(&block.parent_hash);
    buf.extend_from_slice(&block.state_root);

    // Encode transaction count
    #[allow(clippy::cast_possible_truncation)]
    let tx_count = block.txs.len() as u32; // Safe: checked above
    buf.extend_from_slice(&tx_count.to_be_bytes());

    // Encode each transaction using TxV1 signed encoding
    for tx in &block.txs {
        let tx_bytes =
            novai_codec::encode_tx_v1_signed(tx).map_err(|_| CodecError::TxEncodingFailed)?;
        buf.extend_from_slice(&tx_bytes);
    }

    Ok(buf)
}

/// Compute the hash of a Block.
///
/// # Errors
/// Returns error if block encoding fails.
pub fn hash_block_v1(block: &Block) -> Result<[u8; 32], CodecError> {
    let bytes = encode_block_v1(block)?;
    Ok(blake3::hash(&bytes).into())
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
///
/// # Canonical Ordering
/// Votes MUST be sorted by `voter` (lexicographic order) before encoding.
/// This function will sort the votes automatically to ensure determinism.
///
/// # Errors
/// Returns `CodecError::TooManyVotes` if `qc.votes.len() > MAX_VOTES_PER_QC`.
/// Returns `CodecError::DuplicateVoter` if any voter appears more than once.
pub fn encode_qc_v1(qc: &QC) -> Result<Vec<u8>, CodecError> {
    // Check size limit
    if qc.votes.len() > MAX_VOTES_PER_QC {
        return Err(CodecError::TooManyVotes);
    }

    // Sort votes by voter (canonical ordering)
    let mut sorted_votes = qc.votes.clone();
    sorted_votes.sort_by(|a, b| a.voter.cmp(&b.voter));

    // Check for duplicates (adjacent after sorting)
    for i in 1..sorted_votes.len() {
        if sorted_votes[i].voter == sorted_votes[i - 1].voter {
            return Err(CodecError::DuplicateVoter);
        }
    }

    let mut buf = Vec::new();
    buf.push(QC_V1);
    buf.extend_from_slice(&qc.height.to_be_bytes());
    buf.extend_from_slice(&qc.round.to_be_bytes());
    buf.extend_from_slice(&qc.block_hash);

    // Encode vote count
    #[allow(clippy::cast_possible_truncation)]
    let vote_count = sorted_votes.len() as u32; // Safe: checked above
    buf.extend_from_slice(&vote_count.to_be_bytes());

    // Encode each vote (signed format) in sorted order
    for vote in &sorted_votes {
        let vote_bytes = encode_vote_v1_signed(vote);
        buf.extend_from_slice(&vote_bytes);
    }

    Ok(buf)
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
///
/// # Errors
/// Returns error if block or QC encoding fails.
pub fn encode_proposal_v1(proposal: &Proposal) -> Result<Vec<u8>, CodecError> {
    let mut buf = Vec::new();
    buf.push(PROPOSAL_V1);

    // Encode block
    let block_bytes = encode_block_v1(&proposal.block)?;
    buf.extend_from_slice(&block_bytes);

    // Encode justify QC
    let qc_bytes = encode_qc_v1(&proposal.justify_qc)?;
    buf.extend_from_slice(&qc_bytes);

    Ok(buf)
}

/// Encode a Proposal to canonical unsigned bytes (for signing).
///
/// # Errors
/// Returns error if block or QC encoding fails.
pub fn encode_proposal_v1_unsigned(proposal: &Proposal) -> Result<Vec<u8>, CodecError> {
    let mut buf = Vec::new();
    let block_bytes = encode_block_v1(&proposal.block)?;
    buf.extend_from_slice(&block_bytes);
    let qc_bytes = encode_qc_v1(&proposal.justify_qc)?;
    buf.extend_from_slice(&qc_bytes);
    Ok(buf)
}

/// Encode a `SignedProposal` to canonical bytes.
///
/// # Errors
/// Returns error if proposal encoding fails.
pub fn encode_signed_proposal_v1(sp: &SignedProposal) -> Result<Vec<u8>, CodecError> {
    let mut buf = Vec::new();
    buf.push(PROPOSAL_V1);
    buf.extend_from_slice(&sp.proposer);
    let proposal_bytes = encode_proposal_v1_unsigned(&sp.proposal)?;
    buf.extend_from_slice(&proposal_bytes);
    buf.extend_from_slice(&sp.signature);
    Ok(buf)
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
///
/// # Errors
/// Returns error if `highest_qc` encoding fails.
pub fn encode_timeout_v1_unsigned(timeout: &Timeout) -> Result<Vec<u8>, CodecError> {
    let mut buf = Vec::new();
    buf.push(TIMEOUT_UNSIGNED_V1);
    buf.extend_from_slice(&timeout.height.to_be_bytes());
    buf.extend_from_slice(&timeout.round.to_be_bytes());
    buf.extend_from_slice(&timeout.voter);

    // Encode optional QC
    match &timeout.highest_qc {
        Some(qc) => {
            buf.push(0x01); // has_qc = true
            let qc_bytes = encode_qc_v1(qc)?;
            buf.extend_from_slice(&qc_bytes);
        }
        None => {
            buf.push(0x00); // has_qc = false
        }
    }

    Ok(buf)
}

/// Encode a Timeout to canonical signed bytes (includes signature).
///
/// Format:
/// ```text
/// [unsigned_bytes][signature:64]
/// ```
///
/// # Errors
/// Returns error if unsigned encoding fails.
pub fn encode_timeout_v1_signed(timeout: &Timeout) -> Result<Vec<u8>, CodecError> {
    let mut buf = encode_timeout_v1_unsigned(timeout)?;
    buf.extend_from_slice(&timeout.signature);
    Ok(buf)
}

// =============================================================================
// Decode functions
// ============================================================================

/// Decode a Vote from signed wire format.
pub fn decode_vote_v1_signed(buf: &[u8]) -> Result<Vote, CodecError> {
    if buf.len() < 1 + 8 + 8 + 32 + 32 + 64 {
        return Err(CodecError::BufferTooShort);
    }

    let mut input = buf;
    
    let version = read_u8(&mut input)?;
    if version != 1 {
        return Err(CodecError::UnsupportedVersion);
    }

    let height = read_u64_be(&mut input)?;
    let round = read_u64_be(&mut input)?;
    let block_hash = read_32(&mut input)?;
    let voter = read_32(&mut input)?;
    let signature = read_64(&mut input)?;

    Ok(Vote {
        height,
        round,
        block_hash,
        voter,
        signature,
    })
}

/// Decode a QC from wire format.
pub fn decode_qc_v1(buf: &[u8]) -> Result<QC, CodecError> {
    if buf.len() < 1 + 8 + 8 + 32 + 4 {
        return Err(CodecError::BufferTooShort);
    }

    let mut input = buf;
    
    let version = read_u8(&mut input)?;
    if version != 1 {
        return Err(CodecError::UnsupportedVersion);
    }

    let height = read_u64_be(&mut input)?;
    let round = read_u64_be(&mut input)?;
    let block_hash = read_32(&mut input)?;
    
    let vote_count = read_u32_be(&mut input)?;
    if vote_count > MAX_VOTES_PER_QC as u32 {
        return Err(CodecError::TooManyVotes);
    }

    let mut votes = Vec::with_capacity(vote_count as usize);
    for _ in 0..vote_count {
        let vote_height = read_u64_be(&mut input)?;
        let vote_round = read_u64_be(&mut input)?;
        let vote_block_hash = read_32(&mut input)?;
        let voter = read_32(&mut input)?;
        let signature = read_64(&mut input)?;

        votes.push(Vote {
            height: vote_height,
            round: vote_round,
            block_hash: vote_block_hash,
            voter,
            signature,
        });
    }

    Ok(QC {
        height,
        round,
        block_hash,
        votes,
    })
}

/// Decode a SignedProposal from wire format.
pub fn decode_signed_proposal_v1(buf: &[u8]) -> Result<SignedProposal, CodecError> {
    if buf.len() < 1 + 32 + 64 {
        return Err(CodecError::BufferTooShort);
    }

    let mut input = buf;
    
    let version = read_u8(&mut input)?;
    if version != 1 {
        return Err(CodecError::UnsupportedVersion);
    }

    let proposer = read_32(&mut input)?;
    
    // Decode proposal (block + justify_qc)
    let proposal = decode_proposal_v1(&mut input)?;
    
    let signature = read_64(&mut input)?;

    Ok(SignedProposal {
        proposer,
        proposal,
        signature,
    })
}

fn decode_proposal_v1(input: &mut &[u8]) -> Result<Proposal, CodecError> {
    // Decode block
    let block = decode_block_v1(input)?;
    
    // Decode justify_qc
    let justify_qc = decode_qc_v1_internal(input)?;

    Ok(Proposal {
        block,
        justify_qc,
    })
}

fn decode_block_v1(input: &mut &[u8]) -> Result<Block, CodecError> {
    let version = read_u8(input)?;
    if version != 1 {
        return Err(CodecError::UnsupportedVersion);
    }

    let height = read_u64_be(input)?;
    let round = read_u64_be(input)?;
    let parent_hash = read_32(input)?;
    let state_root = read_32(input)?;
    
    let tx_count = read_u32_be(input)?;
    if tx_count as usize > MAX_TXS_PER_BLOCK {
        return Err(CodecError::TooManyTransactions);
    }
    
    // Check buffer has enough bytes for claimed tx count (DoS prevention)
    let min_required_bytes = (tx_count as usize).saturating_mul(MIN_TX_BYTES);
    if input.len() < min_required_bytes {
        return Err(CodecError::BufferTooShort);
    }
    
    let mut txs: Vec<novai_types::TxV1> = Vec::with_capacity(tx_count as usize);
    
    for _ in 0..tx_count {
        let tx = novai_codec::decode_tx_v1_signed(input)
            .map_err(|_| CodecError::TxDecodingFailed)?;
        txs.push(tx);
    }

    Ok(Block {
        height,
        round,
        parent_hash,
        state_root,
        txs,
    })
}

fn decode_qc_v1_internal(input: &mut &[u8]) -> Result<QC, CodecError> {
    let version = read_u8(input)?;
    if version != 1 {
        return Err(CodecError::UnsupportedVersion);
    }

    let height = read_u64_be(input)?;
    let round = read_u64_be(input)?;
    let block_hash = read_32(input)?;
    
    let vote_count = read_u32_be(input)?;
    if vote_count > MAX_VOTES_PER_QC as u32 {
        return Err(CodecError::TooManyVotes);
    }

    // Check buffer has enough bytes for claimed vote count (DoS prevention)
    let min_required_bytes = (vote_count as usize).saturating_mul(MIN_VOTE_BYTES);
    if input.len() < min_required_bytes {
        return Err(CodecError::BufferTooShort);
    }

    let mut votes = Vec::with_capacity(vote_count as usize);
    for _ in 0..vote_count {
        let vote_height = read_u64_be(input)?;
        let vote_round = read_u64_be(input)?;
        let vote_block_hash = read_32(input)?;
        let voter = read_32(input)?;
        let signature = read_64(input)?;

        votes.push(Vote {
            height: vote_height,
            round: vote_round,
            block_hash: vote_block_hash,
            voter,
            signature,
        });
    }

    Ok(QC {
        height,
        round,
        block_hash,
        votes,
    })
}

// Helper read functions
fn read_u8(input: &mut &[u8]) -> Result<u8, CodecError> {
    if input.is_empty() {
        return Err(CodecError::BufferTooShort);
    }
    let val = input[0];
    *input = &input[1..];
    Ok(val)
}

fn read_u32_be(input: &mut &[u8]) -> Result<u32, CodecError> {
    if input.len() < 4 {
        return Err(CodecError::BufferTooShort);
    }
    let bytes: [u8; 4] = input[..4].try_into().unwrap();
    *input = &input[4..];
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64_be(input: &mut &[u8]) -> Result<u64, CodecError> {
    if input.len() < 8 {
        return Err(CodecError::BufferTooShort);
    }
    let bytes: [u8; 8] = input[..8].try_into().unwrap();
    *input = &input[8..];
    Ok(u64::from_be_bytes(bytes))
}

fn read_32(input: &mut &[u8]) -> Result<[u8; 32], CodecError> {
    if input.len() < 32 {
        return Err(CodecError::BufferTooShort);
    }
    let bytes: [u8; 32] = input[..32].try_into().unwrap();
    *input = &input[32..];
    Ok(bytes)
}

fn read_64(input: &mut &[u8]) -> Result<[u8; 64], CodecError> {
    if input.len() < 64 {
        return Err(CodecError::BufferTooShort);
    }
    let bytes: [u8; 64] = input[..64].try_into().unwrap();
    *input = &input[64..];
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use novai_types::{TxV1, TxVersion};

    fn dummy_tx() -> TxV1 {
        TxV1 {
            version: TxVersion::V1,
            from: [0x00; 32],
            pubkey: [0x00; 32],
            nonce: 0,
            fee: 0,
            payload: vec![],
            sig: [0x00; 64],
        }
    }

    #[test]
    fn block_encoding_deterministic() {
        let block = Block {
            height: 42,
            round: 7,
            parent_hash: [0xaa; 32],
            state_root: [0xbb; 32],
            txs: vec![],
        };

        let bytes1 = encode_block_v1(&block).unwrap();
        let bytes2 = encode_block_v1(&block).unwrap();
        assert_eq!(bytes1, bytes2);
    }

    #[test]
    fn block_too_many_txs_rejected() {
        let block = Block {
            height: 1,
            round: 0,
            parent_hash: [0x00; 32],
            state_root: [0x00; 32],
            txs: vec![dummy_tx(); MAX_TXS_PER_BLOCK + 1],
        };

        let result = encode_block_v1(&block);
        assert_eq!(result, Err(CodecError::TooManyTransactions));
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

        let bytes1 = encode_qc_v1(&qc).unwrap();
        let bytes2 = encode_qc_v1(&qc).unwrap();
        assert_eq!(bytes1, bytes2);
    }

    #[test]
    fn qc_votes_sorted_automatically() {
        // Create votes in reverse order
        let vote_a = Vote {
            height: 1,
            round: 0,
            block_hash: [0x00; 32],
            voter: [0xaa; 32],
            signature: [0x00; 64],
        };
        let vote_b = Vote {
            height: 1,
            round: 0,
            block_hash: [0x00; 32],
            voter: [0xbb; 32],
            signature: [0x00; 64],
        };
        let vote_c = Vote {
            height: 1,
            round: 0,
            block_hash: [0x00; 32],
            voter: [0xcc; 32],
            signature: [0x00; 64],
        };

        // Create QC with unsorted votes
        let qc_unsorted = QC {
            height: 1,
            round: 0,
            block_hash: [0x00; 32],
            votes: vec![vote_c.clone(), vote_a.clone(), vote_b.clone()],
        };

        // Create QC with sorted votes
        let qc_sorted = QC {
            height: 1,
            round: 0,
            block_hash: [0x00; 32],
            votes: vec![vote_a, vote_b, vote_c],
        };

        // Both should encode to same bytes
        let bytes_unsorted = encode_qc_v1(&qc_unsorted).unwrap();
        let bytes_sorted = encode_qc_v1(&qc_sorted).unwrap();
        assert_eq!(bytes_unsorted, bytes_sorted);
    }

    #[test]
    fn qc_duplicate_voter_rejected() {
        let vote = Vote {
            height: 1,
            round: 0,
            block_hash: [0x00; 32],
            voter: [0xaa; 32],
            signature: [0x00; 64],
        };

        let qc = QC {
            height: 1,
            round: 0,
            block_hash: [0x00; 32],
            votes: vec![vote.clone(), vote],
        };

        let result = encode_qc_v1(&qc);
        assert_eq!(result, Err(CodecError::DuplicateVoter));
    }

    #[test]
    fn qc_too_many_votes_rejected() {
        #[allow(clippy::cast_possible_truncation)]
        let votes: Vec<Vote> = (0..=MAX_VOTES_PER_QC)
            .map(|i| Vote {
                height: 1,
                round: 0,
                block_hash: [0x00; 32],
                voter: {
                    let mut addr = [0x00; 32];
                    addr[0] = (i % 256) as u8;
                    addr[1] = (i / 256) as u8;
                    addr
                },
                signature: [0x00; 64],
            })
            .collect();

        let qc = QC {
            height: 1,
            round: 0,
            block_hash: [0x00; 32],
            votes,
        };

        let result = encode_qc_v1(&qc);
        assert_eq!(result, Err(CodecError::TooManyVotes));
    }

    #[test]
    fn block_decode_rejects_huge_tx_count() {
        let mut buf = vec![0x01]; // version
        buf.extend_from_slice(&1u64.to_be_bytes()); // height
        buf.extend_from_slice(&0u64.to_be_bytes()); // round
        buf.extend_from_slice(&[0u8; 32]); // parent_hash
        buf.extend_from_slice(&[0u8; 32]); // state_root
        buf.extend_from_slice(&(MAX_TXS_PER_BLOCK as u32 + 1).to_be_bytes()); // tx_count: exceeds limit
        
        let mut input = buf.as_slice();
        let result = decode_block_v1(&mut input);
        assert_eq!(result, Err(CodecError::TooManyTransactions));
    }

    #[test]
    fn qc_decode_rejects_huge_vote_count() {
        let mut buf = vec![0x01]; // version
        buf.extend_from_slice(&1u64.to_be_bytes()); // height
        buf.extend_from_slice(&0u64.to_be_bytes()); // round
        buf.extend_from_slice(&[0u8; 32]); // block_hash
        buf.extend_from_slice(&(MAX_VOTES_PER_QC as u32 + 1).to_be_bytes()); // vote_count: exceeds limit
        
        let mut input = buf.as_slice();
        let result = decode_qc_v1(&mut input);
        assert_eq!(result, Err(CodecError::TooManyVotes));
    }
}
