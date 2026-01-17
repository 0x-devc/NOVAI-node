//! Canonical encoding for AI signal types.
//!
//! Field order is CONSENSUS-RELEVANT. Changing it is a hard fork.

use crate::CodecError;
use novai_ai_entities::{AiSignalType, AiSignalV1, SignalCommitment};

/// Version byte for AiSignalV1 encoding.
pub const AI_SIGNAL_V1: u8 = 0x01;

/// Version byte for SignalCommitment encoding.
pub const SIGNAL_COMMITMENT_V1: u8 = 0x01;

/// Fixed encoded size of SignalCommitment v1.
///
/// Layout:
/// - 1   version
/// - 32  commitment_hash
/// - 1   signal_type
/// - 8   height (LE)
/// - 32  issuer
pub const SIGNAL_COMMITMENT_V1_SIZE: usize = 74;

/// Maximum allowed ZK proof size (64KB).
pub const MAX_ZK_PROOF_SIZE: usize = 65_536;

/// Encode AiSignalV1 to canonical bytes. Variable length due to optional proof.
pub fn encode_ai_signal_v1(signal: &AiSignalV1) -> Result<Vec<u8>, CodecError> {
    if let Some(p) = &signal.zk_proof {
        if p.len() > MAX_ZK_PROOF_SIZE {
            return Err(CodecError::ProofTooLarge);
        }
    }

    let mut out = Vec::new();
    out.push(AI_SIGNAL_V1);

    out.push(signal.signal_type.to_byte());
    out.extend_from_slice(&signal.height.to_le_bytes());
    out.extend_from_slice(&signal.issuer);
    out.push(signal.confidence);
    out.extend_from_slice(&signal.payload_hash);

    match &signal.zk_proof {
        None => {
            out.push(0u8);
        }
        Some(p) => {
            out.push(1u8);
            let len_u32: u32 = p.len().try_into().map_err(|_| CodecError::LengthOverflow)?;
            out.extend_from_slice(&len_u32.to_le_bytes());
            out.extend_from_slice(p);
        }
    }

    out.extend_from_slice(&signal.signature);
    Ok(out)
}

/// Decode AiSignalV1 from canonical bytes.
pub fn decode_ai_signal_v1(bytes: &[u8]) -> Result<AiSignalV1, CodecError> {
    let mut cursor = 0usize;

    // version
    if bytes.is_empty() {
        return Err(CodecError::UnexpectedEof);
    }
    let version = bytes[cursor];
    cursor += 1;
    if version != AI_SIGNAL_V1 {
        return Err(CodecError::InvalidVersion);
    }

    // signal_type
    if bytes.len() < cursor + 1 {
        return Err(CodecError::UnexpectedEof);
    }
    let st = bytes[cursor];
    cursor += 1;
    let signal_type = AiSignalType::from_byte(st).ok_or(CodecError::InvalidSignalType)?;

    // height
    if bytes.len() < cursor + 8 {
        return Err(CodecError::UnexpectedEof);
    }
    let height = u64::from_le_bytes(
        bytes[cursor..cursor + 8]
            .try_into()
            .expect("slice is 8 bytes"),
    );
    cursor += 8;

    // issuer
    if bytes.len() < cursor + 32 {
        return Err(CodecError::UnexpectedEof);
    }
    let mut issuer = [0u8; 32];
    issuer.copy_from_slice(&bytes[cursor..cursor + 32]);
    cursor += 32;

    // confidence
    if bytes.len() < cursor + 1 {
        return Err(CodecError::UnexpectedEof);
    }
    let confidence = bytes[cursor];
    cursor += 1;

    // payload_hash
    if bytes.len() < cursor + 32 {
        return Err(CodecError::UnexpectedEof);
    }
    let mut payload_hash = [0u8; 32];
    payload_hash.copy_from_slice(&bytes[cursor..cursor + 32]);
    cursor += 32;

    // proof flag
    if bytes.len() < cursor + 1 {
        return Err(CodecError::UnexpectedEof);
    }
    let proof_flag = bytes[cursor];
    cursor += 1;

    let zk_proof = match proof_flag {
        0 => None,
        1 => {
            if bytes.len() < cursor + 4 {
                return Err(CodecError::UnexpectedEof);
            }
            let len = u32::from_le_bytes(
                bytes[cursor..cursor + 4]
                    .try_into()
                    .expect("slice is 4 bytes"),
            ) as usize;
            cursor += 4;

            if len > MAX_ZK_PROOF_SIZE {
                return Err(CodecError::ProofTooLarge);
            }
            if bytes.len() < cursor + len {
                return Err(CodecError::UnexpectedEof);
            }
            let proof = bytes[cursor..cursor + len].to_vec();
            cursor += len;
            Some(proof)
        }
        _ => return Err(CodecError::InvalidFlag),
    };

    // signature
    if bytes.len() < cursor + 64 {
        return Err(CodecError::UnexpectedEof);
    }
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&bytes[cursor..cursor + 64]);
    cursor += 64;

    if bytes.len() != cursor {
        return Err(CodecError::TrailingBytes);
    }

    Ok(AiSignalV1 {
        signal_type,
        height,
        issuer,
        confidence,
        payload_hash,
        zk_proof,
        signature,
    })
}

/// Encode SignalCommitment v1. Fixed 74 bytes.
pub fn encode_signal_commitment_v1(c: &SignalCommitment) -> Vec<u8> {
    let mut out = Vec::with_capacity(SIGNAL_COMMITMENT_V1_SIZE);
    out.push(SIGNAL_COMMITMENT_V1);
    out.extend_from_slice(&c.commitment_hash);
    out.push(c.signal_type.to_byte());
    out.extend_from_slice(&c.height.to_le_bytes());
    out.extend_from_slice(&c.issuer);
    debug_assert_eq!(out.len(), SIGNAL_COMMITMENT_V1_SIZE);
    out
}

/// Decode SignalCommitment v1 from fixed 74 bytes.
pub fn decode_signal_commitment_v1(bytes: &[u8]) -> Result<SignalCommitment, CodecError> {
    if bytes.len() < SIGNAL_COMMITMENT_V1_SIZE {
        return Err(CodecError::UnexpectedEof);
    }
    if bytes.len() > SIGNAL_COMMITMENT_V1_SIZE {
        return Err(CodecError::TrailingBytes);
    }

    let mut cursor = 0usize;
    let version = bytes[cursor];
    cursor += 1;
    if version != SIGNAL_COMMITMENT_V1 {
        return Err(CodecError::InvalidVersion);
    }

    let mut commitment_hash = [0u8; 32];
    commitment_hash.copy_from_slice(&bytes[cursor..cursor + 32]);
    cursor += 32;

    let st = bytes[cursor];
    cursor += 1;
    let signal_type = AiSignalType::from_byte(st).ok_or(CodecError::InvalidSignalType)?;

    let height = u64::from_le_bytes(
        bytes[cursor..cursor + 8]
            .try_into()
            .expect("slice is 8 bytes"),
    );
    cursor += 8;

    let mut issuer = [0u8; 32];
    issuer.copy_from_slice(&bytes[cursor..cursor + 32]);

    Ok(SignalCommitment {
        commitment_hash,
        signal_type,
        height,
        issuer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_signal_no_proof() -> AiSignalV1 {
        AiSignalV1 {
            signal_type: AiSignalType::Anomaly,
            height: 777,
            issuer: [0x11u8; 32],
            confidence: 250,
            payload_hash: [0x22u8; 32],
            zk_proof: None,
            signature: [0x33u8; 64],
        }
    }

    #[test]
    fn roundtrip_no_proof() {
        let s = sample_signal_no_proof();
        let enc = encode_ai_signal_v1(&s).unwrap();
        let dec = decode_ai_signal_v1(&enc).unwrap();
        assert_eq!(s, dec);
    }

    #[test]
    fn roundtrip_with_proof() {
        let mut s = sample_signal_no_proof();
        s.zk_proof = Some(vec![0x99u8; 256]);
        let enc = encode_ai_signal_v1(&s).unwrap();
        let dec = decode_ai_signal_v1(&enc).unwrap();
        assert_eq!(s, dec);
    }

    #[test]
    fn oversized_proof_rejected() {
        let mut s = sample_signal_no_proof();
        s.zk_proof = Some(vec![0u8; MAX_ZK_PROOF_SIZE + 1]);
        let err = encode_ai_signal_v1(&s).unwrap_err();
        assert_eq!(err, CodecError::ProofTooLarge);
    }

    #[test]
    fn commitment_codec_fixed_size() {
        let s = sample_signal_no_proof();
        let c = s.to_commitment();
        let enc = encode_signal_commitment_v1(&c);
        assert_eq!(enc.len(), SIGNAL_COMMITMENT_V1_SIZE);
        let dec = decode_signal_commitment_v1(&enc).unwrap();
        assert_eq!(c, dec);
    }
}
