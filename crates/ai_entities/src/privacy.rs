//! NNPX Privacy Types (Week 22 - D22.3)
//!
//! PURPOSE: Define cryptographic commitment structures for private payloads.
//! These structures enable privacy-preserving transactions while maintaining
//! consensus verifiability.
//!
//! INVARIANTS:
//! - All hashing uses domain-separated blake3
//! - Encoding is canonical (big-endian, versioned)
//! - Commitments hide payload content (binding + hiding)
//! - Nullifiers prevent double-spend
//!
//! FAILURE MODES:
//! - Invalid version byte → decode error
//! - Truncated input → decode error
//! - Duplicate nullifier → execution error (not handled here)

use blake3::Hasher;

// ============================================================================
// DOMAIN SEPARATION CONSTANTS
// ============================================================================

/// Domain separator for commitment hash computation.
pub const NNPX_COMMITMENT_DOMAIN: &[u8] = b"NOVAI_NNPX_COMMITMENT_V1";

/// Domain separator for nullifier computation.
pub const NNPX_NULLIFIER_DOMAIN: &[u8] = b"NOVAI_NNPX_NULLIFIER_V1";

/// Domain separator for encryption key derivation.
pub const NNPX_KEY_DERIVE_DOMAIN: &[u8] = b"NOVAI_NNPX_KEY_DERIVE_V1";

/// Domain separator for ZK proof binding.
pub const NNPX_ZK_PROOF_DOMAIN: &[u8] = b"NOVAI_NNPX_ZK_PROOF_V1";

// ============================================================================
// ENCODING CONSTANTS
// ============================================================================

/// Private payload commitment encoding version.
pub const PRIVATE_PAYLOAD_COMMITMENT_V1: u8 = 1;

/// Encoded size of PrivatePayloadCommitment: version(1) + commitment_hash(32) +
/// nullifier(32) + encryption_pubkey(32) + zk_proof(32) = 129 bytes.
pub const PRIVATE_PAYLOAD_COMMITMENT_LEN: usize = 129;

// ============================================================================
// PRIVATE PAYLOAD COMMITMENT (D22.3)
// ============================================================================

/// A commitment to an encrypted private payload.
///
/// This structure is stored on-chain and reveals nothing about the underlying
/// payload content. It enables privacy-preserving transactions while maintaining
/// consensus verifiability.
///
/// # Fields
///
/// - `commitment_hash`: Binding commitment to the encrypted payload content.
///   Computed as `blake3(NNPX_COMMITMENT_DOMAIN || encrypted_payload)`.
///   Hides the payload while binding to its content.
///
/// - `nullifier`: Unique identifier that prevents double-spend.
///   Computed as `blake3(NNPX_NULLIFIER_DOMAIN || spending_secret || counter)`.
///   Once spent, the nullifier is recorded to prevent reuse.
///
/// - `encryption_pubkey`: X25519 public key for payload encryption.
///   The recipient can decrypt using their private key.
///
/// - `zk_proof`: Zero-knowledge proof stub (placeholder).
///   In future weeks, this will contain a validity proof showing the
///   payload satisfies required constraints without revealing content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrivatePayloadCommitment {
    /// blake3(DOMAIN || encrypted_payload) - hides and binds to content.
    pub commitment_hash: [u8; 32],

    /// blake3(DOMAIN || secret || counter) - prevents double-spend.
    pub nullifier: [u8; 32],

    /// X25519 public key for payload encryption.
    pub encryption_pubkey: [u8; 32],

    /// ZK proof stub (placeholder for future validity proofs).
    pub zk_proof: [u8; 32],
}

impl PrivatePayloadCommitment {
    /// Compute a commitment hash from encrypted payload bytes.
    ///
    /// Uses domain separation to prevent cross-context attacks.
    #[must_use]
    pub fn compute_commitment_hash(encrypted_payload: &[u8]) -> [u8; 32] {
        let mut hasher = Hasher::new();
        hasher.update(NNPX_COMMITMENT_DOMAIN);
        hasher.update(encrypted_payload);
        *hasher.finalize().as_bytes()
    }

    /// Compute a nullifier from a spending secret and counter.
    ///
    /// The counter ensures each spending event produces a unique nullifier
    /// even when using the same secret.
    #[must_use]
    pub fn compute_nullifier(spending_secret: &[u8; 32], counter: u64) -> [u8; 32] {
        let mut hasher = Hasher::new();
        hasher.update(NNPX_NULLIFIER_DOMAIN);
        hasher.update(spending_secret);
        hasher.update(&counter.to_be_bytes());
        *hasher.finalize().as_bytes()
    }

    /// Create a new commitment with a stub ZK proof.
    ///
    /// # Arguments
    ///
    /// - `encrypted_payload`: The encrypted payload bytes
    /// - `spending_secret`: Secret for nullifier derivation
    /// - `counter`: Counter for nullifier uniqueness
    /// - `encryption_pubkey`: X25519 public key for encryption
    #[must_use]
    pub fn new(
        encrypted_payload: &[u8],
        spending_secret: &[u8; 32],
        counter: u64,
        encryption_pubkey: [u8; 32],
    ) -> Self {
        let commitment_hash = Self::compute_commitment_hash(encrypted_payload);
        let nullifier = Self::compute_nullifier(spending_secret, counter);

        // Stub ZK proof: hash of commitment + nullifier (placeholder)
        let mut hasher = Hasher::new();
        hasher.update(NNPX_ZK_PROOF_DOMAIN);
        hasher.update(&commitment_hash);
        hasher.update(&nullifier);
        let zk_proof = *hasher.finalize().as_bytes();

        Self {
            commitment_hash,
            nullifier,
            encryption_pubkey,
            zk_proof,
        }
    }

    /// Create a commitment with explicit fields (for testing or deserialization).
    #[must_use]
    pub const fn from_parts(
        commitment_hash: [u8; 32],
        nullifier: [u8; 32],
        encryption_pubkey: [u8; 32],
        zk_proof: [u8; 32],
    ) -> Self {
        Self {
            commitment_hash,
            nullifier,
            encryption_pubkey,
            zk_proof,
        }
    }
}

// ============================================================================
// ENCODING / DECODING
// ============================================================================

/// Error type for private payload commitment decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivatePayloadDecodeError {
    /// Input too short for required fields.
    UnexpectedEof { expected: usize, got: usize },
    /// Invalid codec version.
    BadVersion { expected: u8, got: u8 },
}

/// Encode a `PrivatePayloadCommitment` to canonical bytes.
///
/// Format: `[version:1][commitment_hash:32][nullifier:32][encryption_pubkey:32][zk_proof:32]`
///
/// Total: 129 bytes
#[must_use]
pub fn encode_private_payload_commitment_v1(
    commitment: &PrivatePayloadCommitment,
) -> [u8; PRIVATE_PAYLOAD_COMMITMENT_LEN] {
    let mut out = [0u8; PRIVATE_PAYLOAD_COMMITMENT_LEN];
    let mut pos = 0;

    // Version
    out[pos] = PRIVATE_PAYLOAD_COMMITMENT_V1;
    pos += 1;

    // Commitment hash
    out[pos..pos + 32].copy_from_slice(&commitment.commitment_hash);
    pos += 32;

    // Nullifier
    out[pos..pos + 32].copy_from_slice(&commitment.nullifier);
    pos += 32;

    // Encryption pubkey
    out[pos..pos + 32].copy_from_slice(&commitment.encryption_pubkey);
    pos += 32;

    // ZK proof
    out[pos..pos + 32].copy_from_slice(&commitment.zk_proof);

    out
}

/// Decode a `PrivatePayloadCommitment` from canonical bytes.
///
/// # Errors
///
/// Returns error if bytes are malformed or version is invalid.
pub fn decode_private_payload_commitment_v1(
    bytes: &[u8],
) -> Result<PrivatePayloadCommitment, PrivatePayloadDecodeError> {
    if bytes.len() < PRIVATE_PAYLOAD_COMMITMENT_LEN {
        return Err(PrivatePayloadDecodeError::UnexpectedEof {
            expected: PRIVATE_PAYLOAD_COMMITMENT_LEN,
            got: bytes.len(),
        });
    }

    let mut pos = 0;

    // Version
    let version = bytes[pos];
    if version != PRIVATE_PAYLOAD_COMMITMENT_V1 {
        return Err(PrivatePayloadDecodeError::BadVersion {
            expected: PRIVATE_PAYLOAD_COMMITMENT_V1,
            got: version,
        });
    }
    pos += 1;

    // Commitment hash
    let mut commitment_hash = [0u8; 32];
    commitment_hash.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    // Nullifier
    let mut nullifier = [0u8; 32];
    nullifier.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    // Encryption pubkey
    let mut encryption_pubkey = [0u8; 32];
    encryption_pubkey.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    // ZK proof
    let mut zk_proof = [0u8; 32];
    zk_proof.copy_from_slice(&bytes[pos..pos + 32]);

    Ok(PrivatePayloadCommitment {
        commitment_hash,
        nullifier,
        encryption_pubkey,
        zk_proof,
    })
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commitment_hash_is_deterministic() {
        let payload = b"encrypted payload data";

        let hash1 = PrivatePayloadCommitment::compute_commitment_hash(payload);
        let hash2 = PrivatePayloadCommitment::compute_commitment_hash(payload);

        assert_eq!(hash1, hash2, "Commitment hash must be deterministic");
    }

    #[test]
    fn commitment_hash_changes_with_payload() {
        let payload1 = b"payload version 1";
        let payload2 = b"payload version 2";

        let hash1 = PrivatePayloadCommitment::compute_commitment_hash(payload1);
        let hash2 = PrivatePayloadCommitment::compute_commitment_hash(payload2);

        assert_ne!(
            hash1, hash2,
            "Different payloads must produce different hashes"
        );
    }

    #[test]
    fn nullifier_is_deterministic() {
        let secret = [0x42u8; 32];
        let counter = 1u64;

        let null1 = PrivatePayloadCommitment::compute_nullifier(&secret, counter);
        let null2 = PrivatePayloadCommitment::compute_nullifier(&secret, counter);

        assert_eq!(null1, null2, "Nullifier must be deterministic");
    }

    #[test]
    fn nullifier_changes_with_secret() {
        let secret1 = [0x01u8; 32];
        let secret2 = [0x02u8; 32];
        let counter = 1u64;

        let null1 = PrivatePayloadCommitment::compute_nullifier(&secret1, counter);
        let null2 = PrivatePayloadCommitment::compute_nullifier(&secret2, counter);

        assert_ne!(
            null1, null2,
            "Different secrets must produce different nullifiers"
        );
    }

    #[test]
    fn nullifier_changes_with_counter() {
        let secret = [0x42u8; 32];

        let null1 = PrivatePayloadCommitment::compute_nullifier(&secret, 0);
        let null2 = PrivatePayloadCommitment::compute_nullifier(&secret, 1);
        let null3 = PrivatePayloadCommitment::compute_nullifier(&secret, u64::MAX);

        assert_ne!(
            null1, null2,
            "Different counters must produce different nullifiers"
        );
        assert_ne!(
            null2, null3,
            "Different counters must produce different nullifiers"
        );
    }

    #[test]
    fn encode_decode_roundtrip() {
        let commitment = PrivatePayloadCommitment::new(
            b"test encrypted payload",
            &[0xABu8; 32],
            42,
            [0xCDu8; 32],
        );

        let encoded = encode_private_payload_commitment_v1(&commitment);
        assert_eq!(encoded.len(), PRIVATE_PAYLOAD_COMMITMENT_LEN);

        let decoded = decode_private_payload_commitment_v1(&encoded).unwrap();

        assert_eq!(commitment.commitment_hash, decoded.commitment_hash);
        assert_eq!(commitment.nullifier, decoded.nullifier);
        assert_eq!(commitment.encryption_pubkey, decoded.encryption_pubkey);
        assert_eq!(commitment.zk_proof, decoded.zk_proof);
    }

    #[test]
    fn decode_bad_version() {
        let mut bytes = [0u8; PRIVATE_PAYLOAD_COMMITMENT_LEN];
        bytes[0] = 99; // Invalid version

        let result = decode_private_payload_commitment_v1(&bytes);
        assert!(matches!(
            result,
            Err(PrivatePayloadDecodeError::BadVersion {
                expected: 1,
                got: 99
            })
        ));
    }

    #[test]
    fn decode_too_short() {
        let bytes = [1u8; 50]; // Too short

        let result = decode_private_payload_commitment_v1(&bytes);
        assert!(matches!(
            result,
            Err(PrivatePayloadDecodeError::UnexpectedEof {
                expected: 129,
                got: 50
            })
        ));
    }

    #[test]
    fn encoding_is_deterministic() {
        let commitment =
            PrivatePayloadCommitment::new(b"determinism test", &[0x11u8; 32], 100, [0x22u8; 32]);

        let enc1 = encode_private_payload_commitment_v1(&commitment);
        let enc2 = encode_private_payload_commitment_v1(&commitment);

        assert_eq!(enc1, enc2, "Encoding must be deterministic");
    }

    #[test]
    fn commitment_hides_payload() {
        // Same logical payload, encrypted with different randomness
        // (simulated by using different byte sequences)
        let payload1 = b"encrypted_with_random_1";
        let payload2 = b"encrypted_with_random_2";

        let hash1 = PrivatePayloadCommitment::compute_commitment_hash(payload1);
        let hash2 = PrivatePayloadCommitment::compute_commitment_hash(payload2);

        // Different encrypted payloads produce different commitments
        // This is the "hiding" property - can't tell what the original was
        assert_ne!(
            hash1, hash2,
            "Different encryptions must produce different commitments"
        );
    }

    #[test]
    fn nullifier_prevents_reuse() {
        // Same secret and counter should produce same nullifier
        // (used to detect double-spend)
        let secret = [0x42u8; 32];
        let counter = 1u64;

        let null1 = PrivatePayloadCommitment::compute_nullifier(&secret, counter);
        let null2 = PrivatePayloadCommitment::compute_nullifier(&secret, counter);

        assert_eq!(
            null1, null2,
            "Same secret+counter must produce same nullifier (for detection)"
        );

        // Different counter = different nullifier (valid new spend)
        let null3 = PrivatePayloadCommitment::compute_nullifier(&secret, counter + 1);
        assert_ne!(null1, null3, "Different counter = different nullifier");
    }

    #[test]
    fn from_parts_creates_commitment() {
        let commitment = PrivatePayloadCommitment::from_parts(
            [0x11u8; 32],
            [0x22u8; 32],
            [0x33u8; 32],
            [0x44u8; 32],
        );

        assert_eq!(commitment.commitment_hash, [0x11u8; 32]);
        assert_eq!(commitment.nullifier, [0x22u8; 32]);
        assert_eq!(commitment.encryption_pubkey, [0x33u8; 32]);
        assert_eq!(commitment.zk_proof, [0x44u8; 32]);
    }

    #[test]
    fn default_commitment_is_zeroed() {
        let commitment = PrivatePayloadCommitment::default();

        assert_eq!(commitment.commitment_hash, [0u8; 32]);
        assert_eq!(commitment.nullifier, [0u8; 32]);
        assert_eq!(commitment.encryption_pubkey, [0u8; 32]);
        assert_eq!(commitment.zk_proof, [0u8; 32]);
    }

    #[test]
    fn encoded_length_is_correct() {
        assert_eq!(
            PRIVATE_PAYLOAD_COMMITMENT_LEN,
            1 + 32 + 32 + 32 + 32,
            "Length constant must match actual encoding"
        );
    }
}
