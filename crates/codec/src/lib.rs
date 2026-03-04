pub mod ai_entity_codec;
pub use ai_entity_codec::*;

pub mod ai_signal_codec;
pub use ai_signal_codec::*;

pub mod gate_codec;
pub use gate_codec::*;

use novai_types::{
    Address, BlockHeaderV1, BlockHeaderVersion, Hash32, SignatureBytes, TxId, TxV1, TxVersion,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    UnexpectedEof,
    TrailingBytes,
    InvalidVersion,
    InvalidSignalType,
    InvalidFlag,
    ProofTooLarge,
    LengthOverflow,
}

fn take<'a>(input: &mut &'a [u8], n: usize) -> Result<&'a [u8], CodecError> {
    if input.len() < n {
        return Err(CodecError::UnexpectedEof);
    }
    let (a, b) = input.split_at(n);
    *input = b;
    Ok(a)
}

fn read_u8(input: &mut &[u8]) -> Result<u8, CodecError> {
    Ok(take(input, 1)?[0])
}

fn read_u32_le(input: &mut &[u8]) -> Result<u32, CodecError> {
    let b = take(input, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u64_le(input: &mut &[u8]) -> Result<u64, CodecError> {
    let b = take(input, 8)?;
    Ok(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

fn read_32(input: &mut &[u8]) -> Result<[u8; 32], CodecError> {
    let b = take(input, 32)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(b);
    Ok(out)
}

fn read_64(input: &mut &[u8]) -> Result<[u8; 64], CodecError> {
    let b = take(input, 64)?;
    let mut out = [0u8; 64];
    out.copy_from_slice(b);
    Ok(out)
}

fn write_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}

fn write_u32_le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn write_u64_le(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn write_32(out: &mut Vec<u8>, v: &[u8; 32]) {
    out.extend_from_slice(v);
}

fn write_64(out: &mut Vec<u8>, v: &[u8; 64]) {
    out.extend_from_slice(v);
}

#[allow(dead_code)]
fn write_bytes(out: &mut Vec<u8>, b: &[u8]) -> Result<(), CodecError> {
    let len_u32: u32 = b.len().try_into().map_err(|_| CodecError::LengthOverflow)?;
    write_u32_le(out, len_u32);
    out.extend_from_slice(b);
    Ok(())
}

/// Canonical encoding of TxV1 without signature.
/// Field order is CONSENSUS-RELEVANT. Changing it is a hard fork.
pub fn encode_tx_v1_unsigned(tx: &TxV1) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::new();
    write_u8(&mut out, tx.version as u8);
    write_32(&mut out, &tx.from);
    write_32(&mut out, &tx.pubkey);
    write_u64_le(&mut out, tx.nonce);
    write_u64_le(&mut out, tx.fee);
    write_u32_le(&mut out, tx.payload.len() as u32);
    out.extend_from_slice(&tx.payload);
    Ok(out)
}

/// Canonical encoding of TxV1 including signature.
pub fn encode_tx_v1_signed(tx: &TxV1) -> Result<Vec<u8>, CodecError> {
    let mut out = encode_tx_v1_unsigned(tx)?;
    write_64(&mut out, &tx.sig);
    Ok(out)
}

pub fn decode_tx_v1_unsigned(bytes: &[u8]) -> Result<TxV1, CodecError> {
    let mut input = bytes;
    let v = read_u8(&mut input)?;
    let version = TxVersion::from_u8(v).ok_or(CodecError::InvalidVersion)?;
    let from: Address = read_32(&mut input)?;
    let pubkey: [u8; 32] = read_32(&mut input)?;
    let nonce = read_u64_le(&mut input)?;
    let fee = read_u64_le(&mut input)?;
    let payload_len = read_u32_le(&mut input)? as usize;
    let payload = take(&mut input, payload_len)?.to_vec();

    // unsigned decode sets sig to zeros
    let sig: SignatureBytes = [0u8; 64];

    if !input.is_empty() {
        return Err(CodecError::TrailingBytes);
    }

    Ok(TxV1 {
        version,
        from,
        pubkey,
        nonce,
        fee,
        payload,
        sig,
    })
}

pub fn decode_tx_v1_signed(bytes: &[u8]) -> Result<TxV1, CodecError> {
    let mut input = bytes;
    let tx = decode_tx_v1_signed_streaming(&mut input)?;

    if !input.is_empty() {
        return Err(CodecError::TrailingBytes);
    }

    Ok(tx)
}

/// Streaming variant: reads one signed TxV1 from `input` and advances the
/// cursor past the consumed bytes. Does NOT check for trailing bytes, so
/// callers can decode multiple concatenated txs from a single buffer
/// (e.g., inside `decode_block_v1`).
pub fn decode_tx_v1_signed_streaming(input: &mut &[u8]) -> Result<TxV1, CodecError> {
    let v = read_u8(input)?;
    let version = TxVersion::from_u8(v).ok_or(CodecError::InvalidVersion)?;
    let from: Address = read_32(input)?;
    let pubkey: [u8; 32] = read_32(input)?;
    let nonce = read_u64_le(input)?;
    let fee = read_u64_le(input)?;

    let payload_len = read_u32_le(input)? as usize;
    let payload = take(input, payload_len)?.to_vec();

    let sig: SignatureBytes = read_64(input)?;

    Ok(TxV1 {
        version,
        from,
        pubkey,
        nonce,
        fee,
        payload,
        sig,
    })
}

/// Canonical encoding of BlockHeaderV1.
pub fn encode_block_header_v1(h: &BlockHeaderV1) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::new();
    write_u8(&mut out, h.version as u8);
    write_u64_le(&mut out, h.height);
    write_32(&mut out, &h.prev_hash);
    write_32(&mut out, &h.state_root);
    write_32(&mut out, &h.tx_root);
    write_32(&mut out, &h.proposer);
    write_32(&mut out, &h.qc_hash);
    Ok(out)
}

pub fn decode_block_header_v1(bytes: &[u8]) -> Result<BlockHeaderV1, CodecError> {
    let mut input = bytes;
    let v = read_u8(&mut input)?;
    let version = BlockHeaderVersion::from_u8(v).ok_or(CodecError::InvalidVersion)?;
    let height = read_u64_le(&mut input)?;
    let prev_hash: Hash32 = read_32(&mut input)?;
    let state_root: Hash32 = read_32(&mut input)?;
    let tx_root: Hash32 = read_32(&mut input)?;
    let proposer: Address = read_32(&mut input)?;
    let qc_hash: Hash32 = read_32(&mut input)?;

    if !input.is_empty() {
        return Err(CodecError::TrailingBytes);
    }

    Ok(BlockHeaderV1 {
        version,
        height,
        prev_hash,
        state_root,
        tx_root,
        proposer,
        qc_hash,
    })
}

/// Helper: compute TxId as blake3(encode_tx_v1_unsigned(tx))
pub fn txid_v1(tx: &TxV1) -> Result<TxId, CodecError> {
    let unsigned = encode_tx_v1_unsigned(tx)?;
    let hash = blake3::hash(&unsigned);
    let mut out = [0u8; 32];
    out.copy_from_slice(hash.as_bytes());
    Ok(out)
}

/// Fixed overhead of a V1 signed transaction encoding (everything except payload).
/// 1(version) + 32(from) + 32(pubkey) + 8(nonce) + 8(fee) + 4(payload_len) + 64(sig) = 149
pub const TX_V1_OVERHEAD: usize = 149;

/// Compute the canonical encoded size of a signed `TxV1` without allocating.
///
/// Uses the same field layout as `encode_tx_v1_signed` so the result is
/// identical to `encode_tx_v1_signed(tx).unwrap().len()`. This function
/// MUST be used at all enforcement layers (RPC, mempool, block proposal,
/// consensus validation) to prevent consensus divergence.
pub fn tx_encoded_size(tx: &TxV1) -> usize {
    TX_V1_OVERHEAD + tx.payload.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use novai_types::TxVersion;

    /// Verify tx_encoded_size() matches actual encode_tx_v1_signed().len()
    /// for several payload sizes. This is consensus-critical: any divergence
    /// between the formula and the actual encoding would cause validators to
    /// disagree on size limit enforcement.
    #[test]
    fn tx_encoded_size_matches_actual_encoding() {
        let payloads: &[&[u8]] = &[
            b"",                // empty payload
            b"hello",           // small payload
            &[0xAB; 1024],      // 1 KB
            &[0xCD; 64 * 1024], // 64 KB
        ];

        for payload in payloads {
            let tx = TxV1 {
                version: TxVersion::V1,
                from: [0x11; 32],
                pubkey: [0x22; 32],
                nonce: 42,
                fee: 100,
                payload: payload.to_vec(),
                sig: [0xCC; 64],
            };

            let actual_bytes = encode_tx_v1_signed(&tx).expect("encode must succeed");
            let computed_size = tx_encoded_size(&tx);

            assert_eq!(
                computed_size,
                actual_bytes.len(),
                "tx_encoded_size mismatch for payload len={}",
                payload.len()
            );
        }
    }

    #[test]
    fn tx_v1_overhead_is_correct() {
        // An empty-payload tx should encode to exactly TX_V1_OVERHEAD bytes
        let tx = TxV1 {
            version: TxVersion::V1,
            from: [0; 32],
            pubkey: [0; 32],
            nonce: 0,
            fee: 0,
            payload: vec![],
            sig: [0; 64],
        };

        let encoded = encode_tx_v1_signed(&tx).expect("encode must succeed");
        assert_eq!(encoded.len(), TX_V1_OVERHEAD);
    }
}
