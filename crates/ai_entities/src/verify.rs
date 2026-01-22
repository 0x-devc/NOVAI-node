//! Verification flow for payloads and memory (Week 15).
//!
//! PURPOSE: Provides verification functions to validate off-chain payloads
//! against on-chain commitments, and manages AI memory values that can be
//! stored inline or as artifact references.
//!
//! INVARIANTS:
//! - Hash verification uses constant-time comparison
//! - Payload must decode successfully after hash verification
//! - MemoryValue encoding is canonical: tag byte + content
//!
//! FAILURE MODES:
//! - verify_payload_hash returns HashMismatch if hashes don't match
//! - verify_signal_payload returns PayloadDecodeError if decode fails after hash match
//! - MemoryValue::decode returns None for invalid tag or malformed data

use crate::{artifact_hash, ArtifactError, ArtifactStore, SignalPayload};

/// Maximum size for inline memory values (256 bytes).
pub const MAX_INLINE_MEMORY_SIZE: usize = 256;

/// Tag byte for inline memory values.
const MEMORY_TAG_INLINE: u8 = 0x00;

/// Tag byte for artifact reference memory values.
const MEMORY_TAG_ARTIFACT: u8 = 0x01;

/// Result of payload verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyResult {
    /// Payload is valid and matches the commitment.
    Valid,
    /// Hash of payload does not match the commitment.
    HashMismatch {
        expected: [u8; 32],
        actual: [u8; 32],
    },
    /// Payload bytes could not be decoded.
    PayloadDecodeError,
    /// Signature verification failed (for signed payloads).
    SignatureInvalid,
}

impl VerifyResult {
    /// Returns true if verification succeeded.
    pub fn is_valid(&self) -> bool {
        matches!(self, VerifyResult::Valid)
    }
}

/// Verify that raw payload bytes match an expected commitment hash.
///
/// This performs only hash verification without decoding the payload.
pub fn verify_payload_hash(payload_bytes: &[u8], expected_hash: &[u8; 32]) -> VerifyResult {
    let actual_hash = artifact_hash(payload_bytes);

    // Constant-time comparison to prevent timing attacks
    if constant_time_eq(&actual_hash, expected_hash) {
        VerifyResult::Valid
    } else {
        VerifyResult::HashMismatch {
            expected: *expected_hash,
            actual: actual_hash,
        }
    }
}

/// Verify payload bytes and decode into a SignalPayload.
///
/// This first verifies the hash, then attempts to decode the payload.
/// Returns the decoded payload on success.
pub fn verify_signal_payload(
    payload_bytes: &[u8],
    expected_hash: &[u8; 32],
) -> Result<SignalPayload, VerifyResult> {
    // First verify the hash
    let hash_result = verify_payload_hash(payload_bytes, expected_hash);
    if !hash_result.is_valid() {
        return Err(hash_result);
    }

    // Then decode the payload
    SignalPayload::decode(payload_bytes).ok_or(VerifyResult::PayloadDecodeError)
}

/// Fetch a payload from an artifact store and verify it.
///
/// This is the complete client-side verification flow:
/// 1. Fetch payload bytes from the store using the commitment hash
/// 2. Verify the fetched bytes match the commitment
/// 3. Decode and return the payload
pub fn fetch_and_verify_payload<S: ArtifactStore>(
    store: &S,
    commitment_hash: &[u8; 32],
) -> Result<SignalPayload, FetchVerifyError> {
    // Fetch the payload
    let payload_bytes = store.fetch(commitment_hash)?;

    // Verify and decode
    verify_signal_payload(&payload_bytes, commitment_hash).map_err(FetchVerifyError::Verification)
}

/// Errors that can occur during fetch-and-verify.
#[derive(Debug)]
pub enum FetchVerifyError {
    /// Error fetching the artifact.
    Artifact(ArtifactError),
    /// Verification failed.
    Verification(VerifyResult),
}

impl From<ArtifactError> for FetchVerifyError {
    fn from(e: ArtifactError) -> Self {
        FetchVerifyError::Artifact(e)
    }
}

/// Memory value that can be stored inline or as an artifact reference.
///
/// Small values (≤256 bytes) are stored inline for efficiency.
/// Large values are stored as artifacts and referenced by hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryValue {
    /// Value stored inline (≤256 bytes).
    Inline(Vec<u8>),
    /// Reference to an artifact by hash.
    Artifact([u8; 32]),
}

impl MemoryValue {
    /// Create a MemoryValue from data, automatically choosing inline or artifact.
    ///
    /// If data is ≤256 bytes, returns Inline.
    /// Otherwise, computes the artifact hash and returns Artifact.
    pub fn from_data(data: Vec<u8>) -> Self {
        if data.len() <= MAX_INLINE_MEMORY_SIZE {
            MemoryValue::Inline(data)
        } else {
            let hash = artifact_hash(&data);
            MemoryValue::Artifact(hash)
        }
    }

    /// Encode the memory value to bytes.
    ///
    /// Format:
    /// - Inline: 0x00 + 4 bytes LE length + data
    /// - Artifact: 0x01 + 32 bytes hash
    pub fn encode(&self) -> Vec<u8> {
        match self {
            MemoryValue::Inline(data) => {
                let mut buf = Vec::with_capacity(1 + 4 + data.len());
                buf.push(MEMORY_TAG_INLINE);
                buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
                buf.extend_from_slice(data);
                buf
            }
            MemoryValue::Artifact(hash) => {
                let mut buf = Vec::with_capacity(1 + 32);
                buf.push(MEMORY_TAG_ARTIFACT);
                buf.extend_from_slice(hash);
                buf
            }
        }
    }

    /// Decode a memory value from bytes.
    ///
    /// Returns None if:
    /// - Data is empty
    /// - Tag byte is invalid
    /// - Inline data exceeds MAX_INLINE_MEMORY_SIZE
    /// - Data is truncated
    /// - There are trailing bytes
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.is_empty() {
            return None;
        }

        let tag = data[0];
        match tag {
            MEMORY_TAG_INLINE => {
                if data.len() < 5 {
                    return None;
                }
                let len = u32::from_le_bytes(data[1..5].try_into().ok()?) as usize;
                if len > MAX_INLINE_MEMORY_SIZE {
                    return None;
                }
                if data.len() != 5 + len {
                    return None;
                }
                Some(MemoryValue::Inline(data[5..].to_vec()))
            }
            MEMORY_TAG_ARTIFACT => {
                if data.len() != 33 {
                    return None;
                }
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&data[1..33]);
                Some(MemoryValue::Artifact(hash))
            }
            _ => None,
        }
    }

    /// Resolve the memory value to actual bytes.
    ///
    /// For Inline values, returns the data directly.
    /// For Artifact values, fetches from the provided store.
    pub fn resolve<S: ArtifactStore>(&self, store: &S) -> Result<Vec<u8>, ArtifactError> {
        match self {
            MemoryValue::Inline(data) => Ok(data.clone()),
            MemoryValue::Artifact(hash) => store.fetch(hash),
        }
    }

    /// Returns true if this is an inline value.
    pub fn is_inline(&self) -> bool {
        matches!(self, MemoryValue::Inline(_))
    }

    /// Returns true if this is an artifact reference.
    pub fn is_artifact(&self) -> bool {
        matches!(self, MemoryValue::Artifact(_))
    }

    /// Get the hash for this value.
    ///
    /// For Artifact values, returns the stored hash.
    /// For Inline values, computes the hash on demand.
    pub fn hash(&self) -> [u8; 32] {
        match self {
            MemoryValue::Inline(data) => artifact_hash(data),
            MemoryValue::Artifact(hash) => *hash,
        }
    }
}

/// Constant-time equality comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalFileStore;

    fn sample_payload() -> SignalPayload {
        SignalPayload::new(
            "test-model".to_string(),
            "1.0.0".to_string(),
            "test input".to_string(),
            vec![0x42, 0x43],
            "test explanation".to_string(),
        )
    }

    #[test]
    fn verify_payload_hash_valid() {
        let payload = sample_payload();
        let encoded = payload.encode();
        let hash = artifact_hash(&encoded);

        let result = verify_payload_hash(&encoded, &hash);
        assert!(result.is_valid());
    }

    #[test]
    fn verify_payload_hash_mismatch() {
        let payload = sample_payload();
        let encoded = payload.encode();
        let wrong_hash = [0xFFu8; 32];

        let result = verify_payload_hash(&encoded, &wrong_hash);
        assert!(!result.is_valid());

        if let VerifyResult::HashMismatch { expected, actual } = result {
            assert_eq!(expected, wrong_hash);
            assert_eq!(actual, artifact_hash(&encoded));
        } else {
            panic!("Expected HashMismatch");
        }
    }

    #[test]
    fn verify_signal_payload_valid() {
        let payload = sample_payload();
        let encoded = payload.encode();
        let hash = artifact_hash(&encoded);

        let result = verify_signal_payload(&encoded, &hash);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), payload);
    }

    #[test]
    fn verify_signal_payload_hash_mismatch() {
        let payload = sample_payload();
        let encoded = payload.encode();
        let wrong_hash = [0xFFu8; 32];

        let result = verify_signal_payload(&encoded, &wrong_hash);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            VerifyResult::HashMismatch { .. }
        ));
    }

    #[test]
    fn verify_signal_payload_decode_error() {
        // Valid hash but invalid payload content
        let garbage = vec![0x01, 0xFF, 0xFF, 0xFF, 0xFF]; // Version 1 but garbage
        let hash = artifact_hash(&garbage);

        let result = verify_signal_payload(&garbage, &hash);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), VerifyResult::PayloadDecodeError);
    }

    #[test]
    fn fetch_and_verify_payload_works() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

        let payload = sample_payload();
        let encoded = payload.encode();
        let hash = store.store(&encoded).unwrap();

        let result = fetch_and_verify_payload(&store, &hash);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), payload);
    }

    #[test]
    fn fetch_and_verify_payload_not_found() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = LocalFileStore::new(temp_dir.path()).unwrap();

        let missing_hash = [0x42u8; 32];
        let result = fetch_and_verify_payload(&store, &missing_hash);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FetchVerifyError::Artifact(ArtifactError::NotFound(_))
        ));
    }

    #[test]
    fn memory_value_inline_encode_decode() {
        let data = vec![0x01, 0x02, 0x03];
        let value = MemoryValue::Inline(data.clone());

        let encoded = value.encode();
        let decoded = MemoryValue::decode(&encoded).unwrap();

        assert_eq!(decoded, value);
        assert!(decoded.is_inline());
    }

    #[test]
    fn memory_value_artifact_encode_decode() {
        let hash = [0x42u8; 32];
        let value = MemoryValue::Artifact(hash);

        let encoded = value.encode();
        let decoded = MemoryValue::decode(&encoded).unwrap();

        assert_eq!(decoded, value);
        assert!(decoded.is_artifact());
    }

    #[test]
    fn memory_value_decode_empty_fails() {
        assert!(MemoryValue::decode(&[]).is_none());
    }

    #[test]
    fn memory_value_decode_invalid_tag_fails() {
        assert!(MemoryValue::decode(&[0xFF]).is_none());
    }

    #[test]
    fn memory_value_decode_inline_truncated_fails() {
        // Tag + partial length
        assert!(MemoryValue::decode(&[0x00, 0x01]).is_none());
    }

    #[test]
    fn memory_value_decode_inline_too_large_fails() {
        let mut data = vec![0x00];
        // Length = MAX_INLINE_MEMORY_SIZE + 1
        data.extend_from_slice(&((MAX_INLINE_MEMORY_SIZE + 1) as u32).to_le_bytes());
        assert!(MemoryValue::decode(&data).is_none());
    }

    #[test]
    fn memory_value_decode_inline_trailing_bytes_fails() {
        let mut data = vec![0x00];
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&[0x01, 0x02]);
        data.push(0xFF); // Extra byte
        assert!(MemoryValue::decode(&data).is_none());
    }

    #[test]
    fn memory_value_decode_artifact_wrong_length_fails() {
        // Tag + only 16 bytes (should be 32)
        let data = vec![0x01; 17];
        assert!(MemoryValue::decode(&data).is_none());
    }

    #[test]
    fn memory_value_from_data_small() {
        let small_data = vec![0x42; 100];
        let value = MemoryValue::from_data(small_data.clone());

        assert!(value.is_inline());
        if let MemoryValue::Inline(data) = value {
            assert_eq!(data, small_data);
        }
    }

    #[test]
    fn memory_value_from_data_large() {
        let large_data = vec![0x42; MAX_INLINE_MEMORY_SIZE + 1];
        let expected_hash = artifact_hash(&large_data);
        let value = MemoryValue::from_data(large_data);

        assert!(value.is_artifact());
        if let MemoryValue::Artifact(hash) = value {
            assert_eq!(hash, expected_hash);
        }
    }

    #[test]
    fn memory_value_from_data_boundary() {
        // Exactly MAX_INLINE_MEMORY_SIZE should be inline
        let boundary_data = vec![0x42; MAX_INLINE_MEMORY_SIZE];
        let value = MemoryValue::from_data(boundary_data);
        assert!(value.is_inline());

        // MAX_INLINE_MEMORY_SIZE + 1 should be artifact
        let over_boundary = vec![0x42; MAX_INLINE_MEMORY_SIZE + 1];
        let value = MemoryValue::from_data(over_boundary);
        assert!(value.is_artifact());
    }

    #[test]
    fn memory_value_resolve_inline() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = LocalFileStore::new(temp_dir.path()).unwrap();

        let data = vec![0x01, 0x02, 0x03];
        let value = MemoryValue::Inline(data.clone());

        let resolved = value.resolve(&store).unwrap();
        assert_eq!(resolved, data);
    }

    #[test]
    fn memory_value_resolve_artifact() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

        let data = vec![0x01, 0x02, 0x03];
        let hash = store.store(&data).unwrap();
        let value = MemoryValue::Artifact(hash);

        let resolved = value.resolve(&store).unwrap();
        assert_eq!(resolved, data);
    }

    #[test]
    fn memory_value_hash_inline() {
        let data = vec![0x42, 0x43];
        let value = MemoryValue::Inline(data.clone());

        assert_eq!(value.hash(), artifact_hash(&data));
    }

    #[test]
    fn memory_value_hash_artifact() {
        let hash = [0x42u8; 32];
        let value = MemoryValue::Artifact(hash);

        assert_eq!(value.hash(), hash);
    }

    // Golden vector tests
    #[test]
    fn golden_vector_memory_inline() {
        let value = MemoryValue::Inline(vec![0xDE, 0xAD]);
        let encoded = value.encode();

        // Expected: 0x00 + LE u32 length (2) + data
        let expected = vec![0x00, 0x02, 0x00, 0x00, 0x00, 0xDE, 0xAD];
        assert_eq!(encoded, expected);
    }

    #[test]
    fn golden_vector_memory_artifact() {
        let hash = [0x42u8; 32];
        let value = MemoryValue::Artifact(hash);
        let encoded = value.encode();

        // Expected: 0x01 + 32 bytes hash
        let mut expected = vec![0x01];
        expected.extend_from_slice(&hash);
        assert_eq!(encoded, expected);
        assert_eq!(encoded.len(), 33);
    }

    #[test]
    fn constant_time_eq_works() {
        let a = [0x42u8; 32];
        let b = [0x42u8; 32];
        let c = [0x43u8; 32];

        assert!(constant_time_eq(&a, &b));
        assert!(!constant_time_eq(&a, &c));
    }

    #[test]
    fn verify_result_is_valid() {
        assert!(VerifyResult::Valid.is_valid());
        assert!(!VerifyResult::HashMismatch {
            expected: [0; 32],
            actual: [0; 32]
        }
        .is_valid());
        assert!(!VerifyResult::PayloadDecodeError.is_valid());
        assert!(!VerifyResult::SignatureInvalid.is_valid());
    }
}
