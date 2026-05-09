//! ZK Proof Verification Hooks (D20.4)
//!
//! PURPOSE: Define interface for future ZK verification.
//! This is a STUB implementation that always returns true.
//!
//! IMPORTANT: Real ZK verification is deferred to post-Week 30.
//! This stub enables signal commitment tests and protocol integration
//! without blocking on ZK circuit development.
//!
//! INVARIANTS:
//! - Trait signature is stable WITHIN a major version. The current shape
//!   (proof, public_inputs, proof_type, code_hash) is v2; v1 (proof,
//!   public_inputs) was extended for the ProofSubmission signal handler.
//!   Future additions (e.g. computation_hash as a separate param) are a
//!   breaking change requiring a v3.
//! - Stub always returns true (for testing/development)
//! - Real implementation will have same trait signature
//!
//! FAILURE MODES:
//! - Stub cannot fail (always returns true)
//! - Real implementation may return false for invalid proofs

/// Trait for ZK proof verification.
///
/// Implementations verify that a proof is valid for given public inputs.
///
/// # Future Implementation
///
/// Post-Week 30, this will be implemented with actual ZK circuits for:
/// - AI signal validity proofs
/// - State transition proofs
/// - Computation integrity proofs
///
/// # Example
///
/// ```
/// use novai_crypto::{ZkVerifier, StubZkVerifier};
///
/// // Using the stub verifier (always returns true)
/// let proof = b"mock_proof_data";
/// let inputs = b"public_inputs";
/// let code_hash = [0u8; 32];
/// assert!(StubZkVerifier::verify_proof(proof, inputs, 0, &code_hash));
/// ```
pub trait ZkVerifier {
    /// Verify a ZK proof against public inputs.
    ///
    /// # Arguments
    ///
    /// * `proof` - The serialized proof bytes (format depends on implementation)
    /// * `public_inputs` - The public inputs the proof claims to satisfy
    /// * `proof_type` - Discriminant identifying the proof system (see
    ///   `PROOF_TYPE_*` in `novai-execution`). Lets a single verifier
    ///   route to a different backend per proof system.
    /// * `code_hash` - Hash of the AI module code/weights the proof attests
    ///   to. Bound separately from `public_inputs` so backends can use it
    ///   as a circuit selector (e.g. picking the right verifying key per
    ///   code_hash) without parsing `public_inputs`.
    ///
    /// # Returns
    ///
    /// `true` if the proof is valid for the given public inputs, `false` otherwise.
    ///
    /// # Note
    ///
    /// The stub implementation always returns `true`. Real implementations
    /// will perform cryptographic verification.
    fn verify_proof(
        proof: &[u8],
        public_inputs: &[u8],
        proof_type: u8,
        code_hash: &[u8; 32],
    ) -> bool;
}

/// Stub ZK verifier that always returns true.
///
/// # WARNING
///
/// This is a placeholder implementation for development and testing.
/// **DO NOT** use in production without replacing with a real ZK verifier.
///
/// # Logging
///
/// When compiled with the `zk-logging` feature, this stub logs proof
/// verification attempts to stderr for debugging purposes.
///
/// # Example
///
/// ```
/// use novai_crypto::{ZkVerifier, StubZkVerifier};
///
/// // Stub always returns true regardless of input
/// let code_hash = [0u8; 32];
/// assert!(StubZkVerifier::verify_proof(&[], &[], 0, &code_hash));
/// assert!(StubZkVerifier::verify_proof(b"any_proof", b"any_inputs", 0, &code_hash));
/// ```
/// M-08: WARNING — This stub verifier accepts ALL proofs as valid.
/// It exists ONLY for development and testing. It **MUST** be replaced with a
/// real ZK verifier before mainnet. Using this in production means any
/// entity can forge ZK proofs.
///
/// # Safety (NOT safe for production)
/// The stub returns `true` for all inputs. Do NOT deploy to mainnet without
/// replacing this with a real SNARK/STARK verifier.
pub struct StubZkVerifier;

impl ZkVerifier for StubZkVerifier {
    fn verify_proof(
        proof: &[u8],
        public_inputs: &[u8],
        proof_type: u8,
        code_hash: &[u8; 32],
    ) -> bool {
        // Log proof details when zk-logging feature is enabled
        #[cfg(feature = "zk-logging")]
        eprintln!(
            "[ZK STUB] verify_proof: proof_size={} bytes, inputs_size={} bytes, \
             proof_type={proof_type}, code_hash={:02x?} -> true",
            proof.len(),
            public_inputs.len(),
            &code_hash[..8],
        );

        // Suppress unused variable warnings when logging is disabled
        #[cfg(not(feature = "zk-logging"))]
        let _ = (proof, public_inputs, proof_type, code_hash);

        // Stub always returns true — NOT SAFE for production
        true
    }
}

/// Marker type for future real ZK verifier implementations.
///
/// This type is not yet implemented. It serves as documentation
/// for the planned post-Week 30 implementation.
///
/// # Planned Features
///
/// - Groth16 or PLONK proof verification
/// - Support for multiple circuit types
/// - Batch verification for efficiency
/// - Hardware acceleration support
#[doc(hidden)]
pub struct PlaceholderRealVerifier {
    _private: (), // Prevent construction
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZERO_CODE_HASH: [u8; 32] = [0u8; 32];

    #[test]
    fn stub_returns_true_for_empty_inputs() {
        assert!(StubZkVerifier::verify_proof(&[], &[], 0, &ZERO_CODE_HASH));
    }

    #[test]
    fn stub_returns_true_for_any_inputs() {
        let proof = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let inputs = vec![0x01, 0x02, 0x03];
        assert!(StubZkVerifier::verify_proof(
            &proof,
            &inputs,
            0,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn stub_returns_true_for_large_inputs() {
        let proof = vec![0x42u8; 1024];
        let inputs = vec![0x99u8; 256];
        assert!(StubZkVerifier::verify_proof(
            &proof,
            &inputs,
            0,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn stub_is_deterministic() {
        let proof = b"test_proof";
        let inputs = b"test_inputs";

        let result1 = StubZkVerifier::verify_proof(proof, inputs, 0, &ZERO_CODE_HASH);
        let result2 = StubZkVerifier::verify_proof(proof, inputs, 0, &ZERO_CODE_HASH);
        let result3 = StubZkVerifier::verify_proof(proof, inputs, 0, &ZERO_CODE_HASH);

        assert_eq!(result1, result2);
        assert_eq!(result2, result3);
        assert!(result1); // All should be true
    }

    #[test]
    fn stub_ignores_proof_type_and_code_hash() {
        // Stub returns true regardless of proof_type or code_hash. A real
        // verifier would route on proof_type and bind to code_hash.
        let proof = b"x";
        let inputs = b"y";
        assert!(StubZkVerifier::verify_proof(
            proof,
            inputs,
            0,
            &ZERO_CODE_HASH
        ));
        assert!(StubZkVerifier::verify_proof(
            proof,
            inputs,
            1,
            &[0xFFu8; 32]
        ));
        assert!(StubZkVerifier::verify_proof(
            proof,
            inputs,
            255,
            &[0xAAu8; 32]
        ));
    }
}
