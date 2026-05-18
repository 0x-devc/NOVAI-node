//! ZK Proof Verification
//!
//! PURPOSE: Define the `ZkVerifier` interface and provide both a stub
//! (development-only) and a real Groth16/BN254 implementation used by
//! the `ProofSubmission` signal handler.
//!
//! INVARIANTS:
//! - Trait signature is stable within a major version. The current shape
//!   `(proof, vk, public_inputs, proof_type, code_hash) -> bool` is v3.
//!   History: v1 was `(proof, public_inputs)`; v2 added `proof_type` and
//!   `code_hash` for backend dispatch; v3 adds `vk` so per-submission
//!   verifying keys (required by Groth16) can be supplied through the
//!   on-chain signal payload. Further additions are a breaking change.
//! - `StubZkVerifier` always returns `true`. It exists ONLY to exercise
//!   the surrounding signal-handler plumbing in development tests.
//! - `Groth16Verifier` is deterministic: no RNG, no clock, no thread
//!   scheduling. It runs on BN254 with arkworks' `parallel` feature
//!   disabled, so verification is single-threaded and reproducible.
//! - Both impls return `false` (never panic) on malformed input.
//!
//! FAILURE MODES:
//! - `StubZkVerifier` cannot fail.
//! - `Groth16Verifier` returns `false` on: malformed VK bytes, malformed
//!   proof bytes, `public_inputs.len() != 64`, VK-vs-circuit shape
//!   mismatch, or invalid pairing equation.

use ark_bn254::{Bn254, Fr};
use ark_groth16::{prepare_verifying_key, Groth16, Proof, VerifyingKey};
use ark_serialize::CanonicalDeserialize;

/// Trait for ZK proof verification.
///
/// # Example
///
/// ```
/// use novai_crypto::{StubZkVerifier, ZkVerifier};
///
/// let code_hash = [0u8; 32];
/// assert!(StubZkVerifier::verify_proof(b"proof", b"vk", b"inputs", 0, &code_hash));
/// ```
pub trait ZkVerifier {
    /// Verify a ZK proof.
    ///
    /// # Arguments
    ///
    /// * `proof` - Serialized proof bytes (format depends on implementation).
    /// * `vk` - Serialized verifying-key bytes. For Groth16/BN254 this is
    ///   ark-serialize's compressed form of `VerifyingKey<Bn254>`. Ignored
    ///   by `StubZkVerifier`.
    /// * `public_inputs` - The public inputs the proof claims to satisfy.
    ///   The `ProofSubmission` handler supplies the 64-byte concatenation
    ///   `code_hash || computation_hash`.
    /// * `proof_type` - Discriminant identifying the proof system (see
    ///   `PROOF_TYPE_*` in `novai-execution`).
    /// * `code_hash` - Hash of the AI module code/weights the proof attests to.
    ///
    /// # Returns
    ///
    /// `true` if the proof is valid, `false` otherwise. Implementations MUST
    /// NOT panic on malformed input — return `false` instead.
    fn verify_proof(
        proof: &[u8],
        vk: &[u8],
        public_inputs: &[u8],
        proof_type: u8,
        code_hash: &[u8; 32],
    ) -> bool;
}

/// Stub ZK verifier that always returns true.
///
/// # WARNING
///
/// Placeholder for development and testing. Do **not** use in production.
/// While this stub is the active verifier for `PROOF_TYPE_STUB`, any entity
/// can forge proofs of type 0. Real proofs go through `Groth16Verifier`.
///
/// # Example
///
/// ```
/// use novai_crypto::{StubZkVerifier, ZkVerifier};
///
/// let code_hash = [0u8; 32];
/// assert!(StubZkVerifier::verify_proof(&[], &[], &[], 0, &code_hash));
/// assert!(StubZkVerifier::verify_proof(b"any_proof", b"any_vk", b"any_inputs", 0, &code_hash));
/// ```
pub struct StubZkVerifier;

impl ZkVerifier for StubZkVerifier {
    fn verify_proof(
        proof: &[u8],
        vk: &[u8],
        public_inputs: &[u8],
        proof_type: u8,
        code_hash: &[u8; 32],
    ) -> bool {
        #[cfg(feature = "zk-logging")]
        eprintln!(
            "[ZK STUB] verify_proof: proof={} vk={} inputs={} proof_type={proof_type} \
             code_hash={:02x?} -> true",
            proof.len(),
            vk.len(),
            public_inputs.len(),
            &code_hash[..8],
        );

        #[cfg(not(feature = "zk-logging"))]
        let _ = (proof, vk, public_inputs, proof_type, code_hash);

        true
    }
}

/// BN254 Groth16 verifier.
///
/// Verifies SNARK proofs against a verifying key supplied per submission.
/// All inputs are byte slices owned by the caller; this type holds no state.
///
/// # Encoding contract
///
/// - `vk` MUST be the ark-serialize compressed form of `VerifyingKey<Bn254>`.
/// - `proof` MUST be the ark-serialize compressed form of `Proof<Bn254>`.
/// - `public_inputs` MUST be exactly 64 bytes, interpreted as
///   `code_hash (32) || computation_hash (32)`. The verifier splits this
///   into 4 BN254 scalars via big-endian 16-byte halves: each scalar is
///   `Fr::from(u128::from_be_bytes(chunk))`. The circuit's VK must have
///   been generated for exactly 4 public inputs in this canonical order.
///
/// # Errors
///
/// Returns `false` (never panics) on: malformed VK or proof bytes, wrong
/// public-inputs length, VK-vs-proof structural mismatch, or invalid pairing.
pub struct Groth16Verifier;

impl ZkVerifier for Groth16Verifier {
    fn verify_proof(
        proof: &[u8],
        vk: &[u8],
        public_inputs: &[u8],
        _proof_type: u8,
        _code_hash: &[u8; 32],
    ) -> bool {
        let Ok(vk_decoded) = VerifyingKey::<Bn254>::deserialize_compressed(vk) else {
            return false;
        };
        let Ok(proof_decoded) = Proof::<Bn254>::deserialize_compressed(proof) else {
            return false;
        };
        let Some(fr_inputs) = split_public_inputs_64(public_inputs) else {
            return false;
        };
        let pvk = prepare_verifying_key(&vk_decoded);
        Groth16::<Bn254>::verify_proof(&pvk, &proof_decoded, &fr_inputs).unwrap_or(false)
    }
}

impl Groth16Verifier {
    /// Structural validity check for a Groth16/BN254 verification key.
    ///
    /// Returns `true` iff `vk` deserializes as a `VerifyingKey<Bn254>` in
    /// the canonical ark-serialize compressed form. Used by the VK
    /// Registry create handler (Week 30) so malformed registrations are
    /// rejected at publish time rather than failing on every future
    /// proof submission. Pairing-level soundness is the verifier's
    /// concern; this check only confirms the bytes parse.
    #[must_use]
    pub fn is_valid_vk(vk: &[u8]) -> bool {
        VerifyingKey::<Bn254>::deserialize_compressed(vk).is_ok()
    }
}

/// Split a 64-byte public-inputs buffer into 4 BN254 scalars.
///
/// Each 32-byte hash is split into a 16-byte high half and a 16-byte low
/// half; each half is interpreted as a big-endian `u128` and lifted into
/// `Fr`. The 128-bit values fit comfortably below the BN254 scalar-field
/// modulus (~2^254), so the mapping is bias-free and canonical.
fn split_public_inputs_64(bytes: &[u8]) -> Option<[Fr; 4]> {
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [Fr::from(0u64); 4];
    for (i, slot) in out.iter_mut().enumerate() {
        let chunk: [u8; 16] = bytes[i * 16..(i + 1) * 16].try_into().ok()?;
        *slot = Fr::from(u128::from_be_bytes(chunk));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    use ark_relations::{
        lc,
        r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError, Variable},
    };
    use ark_serialize::CanonicalSerialize;
    use ark_std::rand::{rngs::StdRng, SeedableRng};

    const ZERO_CODE_HASH: [u8; 32] = [0u8; 32];

    // -------- StubZkVerifier --------

    #[test]
    fn stub_returns_true_for_empty_inputs() {
        assert!(StubZkVerifier::verify_proof(
            &[],
            &[],
            &[],
            0,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn stub_returns_true_for_any_inputs() {
        let proof = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let vk = vec![0x12, 0x34];
        let inputs = vec![0x01, 0x02, 0x03];
        assert!(StubZkVerifier::verify_proof(
            &proof,
            &vk,
            &inputs,
            0,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn stub_returns_true_for_large_inputs() {
        let proof = vec![0x42u8; 1024];
        let vk = vec![0x37u8; 512];
        let inputs = vec![0x99u8; 256];
        assert!(StubZkVerifier::verify_proof(
            &proof,
            &vk,
            &inputs,
            0,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn stub_is_deterministic() {
        let proof = b"test_proof";
        let vk = b"test_vk";
        let inputs = b"test_inputs";

        let r1 = StubZkVerifier::verify_proof(proof, vk, inputs, 0, &ZERO_CODE_HASH);
        let r2 = StubZkVerifier::verify_proof(proof, vk, inputs, 0, &ZERO_CODE_HASH);
        let r3 = StubZkVerifier::verify_proof(proof, vk, inputs, 0, &ZERO_CODE_HASH);

        assert_eq!(r1, r2);
        assert_eq!(r2, r3);
        assert!(r1);
    }

    #[test]
    fn stub_ignores_all_arguments() {
        assert!(StubZkVerifier::verify_proof(
            b"x",
            b"y",
            b"z",
            0,
            &ZERO_CODE_HASH
        ));
        assert!(StubZkVerifier::verify_proof(
            b"x",
            b"y",
            b"z",
            1,
            &[0xFFu8; 32]
        ));
        assert!(StubZkVerifier::verify_proof(
            b"x",
            b"y",
            b"z",
            255,
            &[0xAAu8; 32]
        ));
    }

    // -------- Groth16Verifier --------

    /// Trivial sum circuit (4 public inputs, 1 witness) used to exercise
    /// the verifier with a real proof. Not a meaningful AI circuit.
    /// Constraint: w * 1 = c0 + c1 + c2 + c3.
    struct SumCircuit {
        public_inputs: [Option<Fr>; 4],
        witness: Option<Fr>,
    }

    impl ConstraintSynthesizer<Fr> for SumCircuit {
        fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
            let c0 = cs.new_input_variable(|| {
                self.public_inputs[0].ok_or(SynthesisError::AssignmentMissing)
            })?;
            let c1 = cs.new_input_variable(|| {
                self.public_inputs[1].ok_or(SynthesisError::AssignmentMissing)
            })?;
            let c2 = cs.new_input_variable(|| {
                self.public_inputs[2].ok_or(SynthesisError::AssignmentMissing)
            })?;
            let c3 = cs.new_input_variable(|| {
                self.public_inputs[3].ok_or(SynthesisError::AssignmentMissing)
            })?;
            let w =
                cs.new_witness_variable(|| self.witness.ok_or(SynthesisError::AssignmentMissing))?;
            cs.enforce_constraint(lc!() + w, lc!() + Variable::One, lc!() + c0 + c1 + c2 + c3)?;
            Ok(())
        }
    }

    /// Generate a fresh (vk_bytes, proof_bytes, public_inputs_bytes) triple
    /// that the production verifier will accept. `seed` controls the trusted
    /// setup so callers can produce distinct VKs.
    fn gen_valid_proof_with_seed(seed: u64) -> (Vec<u8>, Vec<u8>, [u8; 64]) {
        let mut rng = StdRng::seed_from_u64(seed);
        let setup_circuit = SumCircuit {
            public_inputs: [None; 4],
            witness: None,
        };
        let pk =
            Groth16::<Bn254>::generate_random_parameters_with_reduction(setup_circuit, &mut rng)
                .expect("setup");

        let mut public_inputs_bytes = [0u8; 64];
        for (i, b) in public_inputs_bytes.iter_mut().enumerate() {
            *b = u8::try_from(i).expect("0..64 fits in u8");
        }
        let fr_inputs = split_public_inputs_64(&public_inputs_bytes).expect("64 bytes");
        let witness = fr_inputs[0] + fr_inputs[1] + fr_inputs[2] + fr_inputs[3];

        let prove_circuit = SumCircuit {
            public_inputs: fr_inputs.map(Some),
            witness: Some(witness),
        };
        let proof =
            Groth16::<Bn254>::create_random_proof_with_reduction(prove_circuit, &pk, &mut rng)
                .expect("prove");

        let mut vk_bytes = Vec::new();
        pk.vk.serialize_compressed(&mut vk_bytes).expect("vk ser");
        let mut proof_bytes = Vec::new();
        proof
            .serialize_compressed(&mut proof_bytes)
            .expect("proof ser");

        (vk_bytes, proof_bytes, public_inputs_bytes)
    }

    fn gen_valid_proof() -> (Vec<u8>, Vec<u8>, [u8; 64]) {
        gen_valid_proof_with_seed(0)
    }

    #[test]
    fn groth16_valid_proof_accepted() {
        let (vk, proof, inputs) = gen_valid_proof();
        assert!(Groth16Verifier::verify_proof(
            &proof,
            &vk,
            &inputs,
            1,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn groth16_tampered_proof_rejected() {
        let (vk, mut proof, inputs) = gen_valid_proof();
        proof[0] ^= 0x01;
        assert!(!Groth16Verifier::verify_proof(
            &proof,
            &vk,
            &inputs,
            1,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn groth16_wrong_vk_rejected() {
        let (_, proof, inputs) = gen_valid_proof_with_seed(0);
        // Different seed -> different trusted setup -> different VK.
        let (other_vk, _, _) = gen_valid_proof_with_seed(1);
        assert!(!Groth16Verifier::verify_proof(
            &proof,
            &other_vk,
            &inputs,
            1,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn groth16_wrong_public_inputs_rejected() {
        let (vk, proof, mut inputs) = gen_valid_proof();
        inputs[0] ^= 0xFF;
        assert!(!Groth16Verifier::verify_proof(
            &proof,
            &vk,
            &inputs,
            1,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn groth16_malformed_proof_bytes_rejected() {
        let (vk, _, inputs) = gen_valid_proof();
        let bad_proof = vec![0xFFu8; 32];
        assert!(!Groth16Verifier::verify_proof(
            &bad_proof,
            &vk,
            &inputs,
            1,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn groth16_malformed_vk_bytes_rejected() {
        let (_, proof, inputs) = gen_valid_proof();
        let bad_vk = vec![0xFFu8; 32];
        assert!(!Groth16Verifier::verify_proof(
            &proof,
            &bad_vk,
            &inputs,
            1,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn groth16_wrong_public_inputs_length_rejected() {
        let (vk, proof, _) = gen_valid_proof();
        assert!(!Groth16Verifier::verify_proof(
            &proof,
            &vk,
            &[0u8; 63],
            1,
            &ZERO_CODE_HASH
        ));
        assert!(!Groth16Verifier::verify_proof(
            &proof,
            &vk,
            &[0u8; 65],
            1,
            &ZERO_CODE_HASH
        ));
        assert!(!Groth16Verifier::verify_proof(
            &proof,
            &vk,
            &[],
            1,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn groth16_empty_proof_rejected() {
        let (vk, _, inputs) = gen_valid_proof();
        assert!(!Groth16Verifier::verify_proof(
            &[],
            &vk,
            &inputs,
            1,
            &ZERO_CODE_HASH
        ));
    }

    #[test]
    fn groth16_empty_vk_rejected() {
        let (_, proof, inputs) = gen_valid_proof();
        assert!(!Groth16Verifier::verify_proof(
            &proof,
            &[],
            &inputs,
            1,
            &ZERO_CODE_HASH
        ));
    }

    // -------- helper --------

    #[test]
    fn split_public_inputs_rejects_wrong_length() {
        assert!(split_public_inputs_64(&[]).is_none());
        assert!(split_public_inputs_64(&[0u8; 63]).is_none());
        assert!(split_public_inputs_64(&[0u8; 65]).is_none());
        assert!(split_public_inputs_64(&[0u8; 64]).is_some());
    }

    #[test]
    fn split_public_inputs_maps_canonically() {
        let mut bytes = [0u8; 64];
        // First chunk: 0x0102..0x10 as a 128-bit BE int.
        for (i, b) in bytes[..16].iter_mut().enumerate() {
            *b = u8::try_from(i + 1).expect("fits");
        }
        let out = split_public_inputs_64(&bytes).expect("64 bytes");
        // Reconstruct the same value via u128.
        let mut expected_first = [0u8; 16];
        for (i, b) in expected_first.iter_mut().enumerate() {
            *b = u8::try_from(i + 1).expect("fits");
        }
        assert_eq!(out[0], Fr::from(u128::from_be_bytes(expected_first)));
        assert_eq!(out[1], Fr::from(0u64));
        assert_eq!(out[2], Fr::from(0u64));
        assert_eq!(out[3], Fr::from(0u64));
    }
}
