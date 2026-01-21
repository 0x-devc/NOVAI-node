//! Canonical encodings for consensus messages.
//!
//! All encodings are deterministic and use big-endian byte order.
//! Format versioning: each message type has a version byte prefix.

use crate::{Block, BlockRequest, BlockResponse, Proposal, SignedProposal, Timeout, Vote, QC};

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

/// Codec version for `BlockRequest`.
pub const BLOCK_REQUEST_V1: u8 = 0x01;

/// Codec version for `BlockResponse`.
pub const BLOCK_RESPONSE_V1: u8 = 0x01;

/// Maximum transactions per block (`DoS` prevention).
pub const MAX_TXS_PER_BLOCK: usize = 10_000;

/// Maximum votes per QC (`DoS` prevention).
pub const MAX_VOTES_PER_QC: usize = 11_000;

/// Minimum bytes per transaction (conservative estimate for validation).
const MIN_TX_BYTES: usize = 100;

/// Minimum bytes per vote (conservative estimate for validation).
// Minimum vote size: version(1) + height(8) + round(8) + block_hash(32) + voter(32) + signature(64) + has_signal(1) = 146
const MIN_VOTE_BYTES: usize = 146;

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
///
/// Note: AI signal commitment is NOT included in unsigned bytes (not signed).
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

/// Encode a Vote to canonical signed bytes (includes signature and optional AI signal).
///
/// Format:
/// - Without signal: `[unsigned_bytes][signature:64]`
/// - With signal: `[unsigned_bytes][signature:64][has_signal:1][commitment:32]`
#[must_use]
pub fn encode_vote_v1_signed(vote: &Vote) -> Vec<u8> {
    let mut buf = encode_vote_v1_unsigned(vote);
    buf.extend_from_slice(&vote.signature);

    // Add optional AI signal commitment
    if let Some(commitment) = vote.ai_signal_commitment {
        buf.push(1); // has_signal = true
        buf.extend_from_slice(&commitment);
    } else {
        buf.push(0); // has_signal = false
    }

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

/// Decode a Timeout from signed wire format.
///
/// Format:
/// ```text
/// [version:1][height:8][round:8][voter:32][has_qc:1][qc_bytes?][signature:64]
/// ```
///
/// # Errors
/// Returns error if buffer is too short or data is malformed.
pub fn decode_timeout_v1_signed(buf: &[u8]) -> Result<Timeout, CodecError> {
    // Minimum size: version + height + round + voter + has_qc + signature
    const MIN_SIZE: usize = 1 + 8 + 8 + 32 + 1 + 64; // 114 bytes

    if buf.len() < MIN_SIZE {
        return Err(CodecError::BufferTooShort);
    }

    let mut input = buf;

    let version = read_u8(&mut input)?;
    if version != TIMEOUT_UNSIGNED_V1 {
        return Err(CodecError::UnsupportedVersion);
    }

    let height = read_u64_be(&mut input)?;
    let round = read_u64_be(&mut input)?;
    let voter = read_32(&mut input)?;

    // Decode optional QC
    let has_qc = read_u8(&mut input)?;
    let highest_qc = if has_qc == 0x01 {
        Some(decode_qc_v1_internal(&mut input)?)
    } else {
        None
    };

    let signature = read_64(&mut input)?;

    Ok(Timeout {
        height,
        round,
        voter,
        highest_qc,
        signature,
    })
}

// =============================================================================
// Decode functions
// ============================================================================

/// Decode a Vote from signed wire format.
///
/// # Errors
/// Returns error if buffer is too short or data is malformed.
/// Decode a Vote from signed wire format.
///
/// Format:
/// - V1 without signal (145 bytes): `[version:1][height:8][round:8][block_hash:32][voter:32][signature:64]`
/// - V1 with signal (178 bytes): `[version:1][height:8][round:8][block_hash:32][voter:32][signature:64][has_signal:1][commitment:32]`
///
/// # Errors
/// Returns error if buffer is too short or data is malformed.
pub fn decode_vote_v1_signed(buf: &[u8]) -> Result<Vote, CodecError> {
    // Minimum size: version + height + round + block_hash + voter + signature
    const MIN_SIZE: usize = 1 + 8 + 8 + 32 + 32 + 64; // 145 bytes

    if buf.len() < MIN_SIZE {
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
    let vote_voter = read_32(&mut input)?;
    let signature = read_64(&mut input)?;

    // Check for optional AI signal commitment (backward compatible)
    let ai_signal_commitment = if input.is_empty() {
        None
    } else {
        let has_signal = read_u8(&mut input)?;
        if has_signal == 1 {
            Some(read_32(&mut input)?)
        } else {
            None
        }
    };

    Ok(Vote {
        height,
        round,
        block_hash,
        voter: vote_voter,
        signature,
        ai_signal_commitment,
    })
}

/// Decode a QC from wire format.
///
/// # Errors
/// Returns error if buffer is too short, too many votes, or data is malformed.
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
    #[allow(clippy::cast_possible_truncation)]
    if vote_count > (MAX_VOTES_PER_QC as u32) {
        return Err(CodecError::TooManyVotes);
    }

    let mut votes = Vec::with_capacity(vote_count as usize);
    for _ in 0..vote_count {
        let vote_height = read_u64_be(&mut input)?;
        let vote_round = read_u64_be(&mut input)?;
        let vote_block_hash = read_32(&mut input)?;
        let vote_voter = read_32(&mut input)?;
        let signature = read_64(&mut input)?;

        votes.push(Vote {
            height: vote_height,
            round: vote_round,
            block_hash: vote_block_hash,
            voter: vote_voter,
            signature,
            ai_signal_commitment: None,
        });
    }

    Ok(QC {
        height,
        round,
        block_hash,
        votes,
    })
}

/// Decode a `SignedProposal` from wire format.
///
/// # Errors
/// Returns error if buffer is too short or data is malformed.
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

    Ok(Proposal { block, justify_qc })
}

/// Decode a block from canonical bytes.
///
/// # Errors
/// Returns error if decoding fails or data is malformed.
pub fn decode_block_v1(input: &mut &[u8]) -> Result<Block, CodecError> {
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
        let tx =
            novai_codec::decode_tx_v1_signed(input).map_err(|_| CodecError::TxDecodingFailed)?;
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
    #[allow(clippy::cast_possible_truncation)]
    if vote_count > (MAX_VOTES_PER_QC as u32) {
        return Err(CodecError::TooManyVotes);
    }

    // Check buffer has enough bytes for claimed vote count (DoS prevention)
    let min_required_bytes = (vote_count as usize).saturating_mul(MIN_VOTE_BYTES);
    if input.len() < min_required_bytes {
        return Err(CodecError::BufferTooShort);
    }

    let mut votes = Vec::with_capacity(vote_count as usize);
    for _ in 0..vote_count {
        // Read vote version byte (must match VOTE_UNSIGNED_V1)
        let vote_version = read_u8(input)?;
        if vote_version != VOTE_UNSIGNED_V1 {
            return Err(CodecError::UnsupportedVersion);
        }

        let vote_height = read_u64_be(input)?;
        let vote_round = read_u64_be(input)?;
        let vote_block_hash = read_32(input)?;
        let vote_voter = read_32(input)?;
        let signature = read_64(input)?;

        // Read has_signal indicator and optional commitment
        let has_signal = read_u8(input)?;
        let ai_signal_commitment = if has_signal == 1 {
            Some(read_32(input)?)
        } else {
            None
        };

        votes.push(Vote {
            height: vote_height,
            round: vote_round,
            block_hash: vote_block_hash,
            voter: vote_voter,
            signature,
            ai_signal_commitment,
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

// ============================================================================
// BlockRequest Encoding
// ============================================================================

/// Encode a `BlockRequest` to canonical bytes.
///
/// Format:
/// ```text
/// [version:1][requester:32][start_height:8][end_height:8]
/// ```
///
/// # Errors
/// Currently infallible, but returns `Result` for consistency with other codec functions.
pub fn encode_block_request_v1(req: &BlockRequest) -> Result<Vec<u8>, CodecError> {
    let mut buf = Vec::with_capacity(49);
    buf.push(BLOCK_REQUEST_V1);
    buf.extend_from_slice(&req.requester);
    buf.extend_from_slice(&req.start_height.to_be_bytes());
    buf.extend_from_slice(&req.end_height.to_be_bytes());
    Ok(buf)
}

/// Decode a `BlockRequest` from canonical bytes.
///
/// # Errors
/// Returns error if buffer is too short or version is unsupported.
pub fn decode_block_request_v1(buf: &[u8]) -> Result<BlockRequest, CodecError> {
    const EXPECTED_SIZE: usize = 49;

    if buf.len() != EXPECTED_SIZE {
        return Err(CodecError::BufferTooShort);
    }

    let mut input = buf;

    let version = read_u8(&mut input)?;
    if version != BLOCK_REQUEST_V1 {
        return Err(CodecError::UnsupportedVersion);
    }

    let requester = read_32(&mut input)?;
    let start_height = read_u64_be(&mut input)?;
    let end_height = read_u64_be(&mut input)?;

    Ok(BlockRequest {
        requester,
        start_height,
        end_height,
    })
}

// ============================================================================
// BlockResponse Encoding
// ============================================================================

/// Maximum blocks per response (`DoS` prevention).
pub const MAX_BLOCKS_PER_RESPONSE: usize = 1000;

/// Encode a `BlockResponse` to canonical bytes.
///
/// Format:
/// ```text
/// [version:1][responder:32][request_start:8][request_end:8][block_count:4][blocks_bytes]
/// ```
///
/// # Errors
/// Returns error if too many blocks or block encoding fails.
pub fn encode_block_response_v1(resp: &BlockResponse) -> Result<Vec<u8>, CodecError> {
    if resp.blocks.len() > MAX_BLOCKS_PER_RESPONSE {
        return Err(CodecError::TooManyTransactions); // Reuse error for now
    }

    let mut buf = Vec::new();
    buf.push(BLOCK_RESPONSE_V1);
    buf.extend_from_slice(&resp.responder);
    buf.extend_from_slice(&resp.request_start.to_be_bytes());
    buf.extend_from_slice(&resp.request_end.to_be_bytes());

    #[allow(clippy::cast_possible_truncation)]
    let block_count = resp.blocks.len() as u32;
    buf.extend_from_slice(&block_count.to_be_bytes());

    for block in &resp.blocks {
        let block_bytes = encode_block_v1(block)?;
        buf.extend_from_slice(&block_bytes);
    }

    Ok(buf)
}

/// Decode a `BlockResponse` from canonical bytes.
///
/// # Errors
/// Returns error if buffer is too short, version is unsupported, or block decoding fails.
///
/// # Panics
/// Panics if `MAX_BLOCKS_PER_RESPONSE` constant doesn't fit in u32 (should never happen).
pub fn decode_block_response_v1(buf: &[u8]) -> Result<BlockResponse, CodecError> {
    const MIN_SIZE: usize = 1 + 32 + 8 + 8 + 4; // 53 bytes

    if buf.len() < MIN_SIZE {
        return Err(CodecError::BufferTooShort);
    }

    let mut input = buf;

    let version = read_u8(&mut input)?;
    if version != BLOCK_RESPONSE_V1 {
        return Err(CodecError::UnsupportedVersion);
    }

    let responder = read_32(&mut input)?;
    let request_start = read_u64_be(&mut input)?;
    let request_end = read_u64_be(&mut input)?;
    let block_count = read_u32_be(&mut input)?;

    let max_blocks =
        u32::try_from(MAX_BLOCKS_PER_RESPONSE).expect("MAX_BLOCKS_PER_RESPONSE should fit in u32");
    if block_count > max_blocks {
        return Err(CodecError::TooManyTransactions); // Reuse error
    }

    let mut blocks = Vec::with_capacity(block_count as usize);
    for _ in 0..block_count {
        let block = decode_block_v1(&mut input)?;
        blocks.push(block);
    }

    Ok(BlockResponse {
        responder,
        request_start,
        request_end,
        blocks,
    })
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
            ai_signal_commitment: None,
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
            ai_signal_commitment: None,
        };
        let vote_b = Vote {
            height: 1,
            round: 0,
            block_hash: [0x00; 32],
            voter: [0xbb; 32],
            signature: [0x00; 64],
            ai_signal_commitment: None,
        };
        let vote_c = Vote {
            height: 1,
            round: 0,
            block_hash: [0x00; 32],
            voter: [0xcc; 32],
            signature: [0x00; 64],
            ai_signal_commitment: None,
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
            ai_signal_commitment: None,
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
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        addr[0] = (i % 256) as u8;
                        addr[1] = (i / 256) as u8;
                    }
                    addr
                },
                signature: [0x00; 64],
                ai_signal_commitment: None,
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
        #[allow(clippy::cast_possible_truncation)]
        let tx_count = (MAX_TXS_PER_BLOCK as u32) + 1;
        buf.extend_from_slice(&tx_count.to_be_bytes()); // tx_count: exceeds limit

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
        #[allow(clippy::cast_possible_truncation)]
        let vote_count = (MAX_VOTES_PER_QC as u32) + 1;
        buf.extend_from_slice(&vote_count.to_be_bytes()); // vote_count: exceeds limit

        let input = buf.as_slice();
        let result = decode_qc_v1(input);
        assert_eq!(result, Err(CodecError::TooManyVotes));
    }

    #[test]
    fn timeout_decode_roundtrip_no_qc() {
        let timeout = Timeout {
            height: 42,
            round: 7,
            voter: [0xaa; 32],
            highest_qc: None,
            signature: [0xbb; 64],
        };

        let bytes = encode_timeout_v1_signed(&timeout).unwrap();
        let decoded = decode_timeout_v1_signed(&bytes).unwrap();

        assert_eq!(decoded.height, timeout.height);
        assert_eq!(decoded.round, timeout.round);
        assert_eq!(decoded.voter, timeout.voter);
        assert_eq!(decoded.highest_qc, timeout.highest_qc);
        assert_eq!(decoded.signature, timeout.signature);
    }

    #[test]
    fn timeout_decode_roundtrip_with_qc() {
        let qc = QC {
            height: 10,
            round: 3,
            block_hash: [0xcc; 32],
            votes: vec![],
        };

        let timeout = Timeout {
            height: 11,
            round: 4,
            voter: [0xdd; 32],
            highest_qc: Some(qc),
            signature: [0xee; 64],
        };

        let bytes = encode_timeout_v1_signed(&timeout).unwrap();
        let decoded = decode_timeout_v1_signed(&bytes).unwrap();

        assert_eq!(decoded.height, timeout.height);
        assert_eq!(decoded.round, timeout.round);
        assert_eq!(decoded.voter, timeout.voter);
        assert!(decoded.highest_qc.is_some());
        assert_eq!(decoded.signature, timeout.signature);
    }

    #[test]
    fn test_block_request_response_roundtrip() {
        // Test BlockRequest roundtrip
        let request = BlockRequest {
            requester: [0xaa; 32],
            start_height: 10,
            end_height: 20,
        };

        let encoded = encode_block_request_v1(&request).unwrap();
        let decoded = decode_block_request_v1(&encoded).unwrap();

        assert_eq!(decoded.requester, request.requester);
        assert_eq!(decoded.start_height, request.start_height);
        assert_eq!(decoded.end_height, request.end_height);
        assert_eq!(encoded.len(), 49); // version + requester + start + end

        // Test BlockResponse roundtrip (empty blocks)
        let response_empty = BlockResponse {
            responder: [0xbb; 32],
            request_start: 10,
            request_end: 20,
            blocks: vec![],
        };

        let encoded = encode_block_response_v1(&response_empty).unwrap();
        let decoded = decode_block_response_v1(&encoded).unwrap();

        assert_eq!(decoded.responder, response_empty.responder);
        assert_eq!(decoded.request_start, response_empty.request_start);
        assert_eq!(decoded.request_end, response_empty.request_end);
        assert_eq!(decoded.blocks.len(), 0);
        assert_eq!(encoded.len(), 53); // version + responder + start + end + count

        // Test BlockResponse roundtrip (with blocks)
        let block1 = Block {
            height: 10,
            round: 5,
            parent_hash: [0xaa; 32],
            state_root: [0xbb; 32],
            txs: vec![],
        };

        let block2 = Block {
            height: 11,
            round: 6,
            parent_hash: [0xcc; 32],
            state_root: [0xdd; 32],
            txs: vec![],
        };

        let response_with_blocks = BlockResponse {
            responder: [0xbb; 32],
            request_start: 10,
            request_end: 11,
            blocks: vec![block1.clone(), block2.clone()],
        };

        let encoded = encode_block_response_v1(&response_with_blocks).unwrap();
        let decoded = decode_block_response_v1(&encoded).unwrap();

        assert_eq!(decoded.responder, response_with_blocks.responder);
        assert_eq!(decoded.request_start, response_with_blocks.request_start);
        assert_eq!(decoded.request_end, response_with_blocks.request_end);
        assert_eq!(decoded.blocks.len(), 2);
        assert_eq!(decoded.blocks[0].height, block1.height);
        assert_eq!(decoded.blocks[1].height, block2.height);
    }
}
