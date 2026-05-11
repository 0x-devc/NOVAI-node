//! Integration tests for the ZK stub verifier (D20.4) and the v3 trait shape.
//!
//! These tests verify the stub implementation behaves correctly and that
//! the trait can be used as a bound. The `Groth16Verifier` is exercised
//! separately in `crates/crypto/src/zk.rs` and end-to-end in
//! `crates/execution/tests/verification_system.rs`.

use novai_crypto::{StubZkVerifier, ZkVerifier};

const ZERO_CODE_HASH: [u8; 32] = [0u8; 32];

// ============================================================================
// BASIC FUNCTIONALITY TESTS
// ============================================================================

#[test]
fn stub_returns_true_for_empty_inputs() {
    let result = StubZkVerifier::verify_proof(&[], &[], &[], 0, &ZERO_CODE_HASH);
    assert!(result, "Stub must return true for empty inputs");
}

#[test]
fn stub_returns_true_for_non_empty_inputs() {
    let proof = b"mock_zk_proof_data_here";
    let vk = b"mock_verifying_key_bytes";
    let inputs = b"public_inputs_for_verification";

    let result = StubZkVerifier::verify_proof(proof, vk, inputs, 0, &ZERO_CODE_HASH);
    assert!(result, "Stub must return true for non-empty inputs");
}

#[test]
fn stub_returns_true_for_large_proof() {
    let proof = vec![0xABu8; 256];
    let vk = vec![0xEFu8; 128];
    let inputs = vec![0xCDu8; 64];

    let result = StubZkVerifier::verify_proof(&proof, &vk, &inputs, 0, &ZERO_CODE_HASH);
    assert!(result, "Stub must return true for large proofs");
}

#[test]
fn stub_returns_true_for_proof_only() {
    let proof = b"proof_without_inputs";
    let vk: &[u8] = &[];
    let inputs: &[u8] = &[];

    let result = StubZkVerifier::verify_proof(proof, vk, inputs, 0, &ZERO_CODE_HASH);
    assert!(result, "Stub must return true even with empty vk and inputs");
}

#[test]
fn stub_returns_true_for_inputs_only() {
    let proof: &[u8] = &[];
    let vk: &[u8] = &[];
    let inputs = b"inputs_without_proof";

    let result = StubZkVerifier::verify_proof(proof, vk, inputs, 0, &ZERO_CODE_HASH);
    assert!(result, "Stub must return true even with empty proof and vk");
}

// ============================================================================
// DETERMINISM TESTS
// ============================================================================

#[test]
fn stub_is_deterministic() {
    let proof = b"deterministic_test_proof";
    let vk = b"deterministic_test_vk";
    let inputs = b"deterministic_test_inputs";

    for _ in 0..10 {
        assert!(
            StubZkVerifier::verify_proof(proof, vk, inputs, 0, &ZERO_CODE_HASH),
            "Stub must be deterministic"
        );
    }
}

#[test]
fn stub_same_result_for_same_inputs() {
    let proof = vec![0x11u8; 128];
    let vk = vec![0x33u8; 64];
    let inputs = vec![0x22u8; 32];

    let result1 = StubZkVerifier::verify_proof(&proof, &vk, &inputs, 0, &ZERO_CODE_HASH);
    let result2 = StubZkVerifier::verify_proof(&proof, &vk, &inputs, 0, &ZERO_CODE_HASH);

    assert_eq!(result1, result2, "Same inputs must produce same result");
}

// ============================================================================
// TRAIT BOUND TESTS (compile-time verification)
// ============================================================================

/// Function that accepts any `ZkVerifier` implementation. Compile-time check
/// that the trait can be used as a bound.
fn verify_with_trait<V: ZkVerifier>(
    proof: &[u8],
    vk: &[u8],
    inputs: &[u8],
    proof_type: u8,
    code_hash: &[u8; 32],
) -> bool {
    V::verify_proof(proof, vk, inputs, proof_type, code_hash)
}

#[test]
fn trait_can_be_used_as_bound() {
    let proof = b"trait_bound_test";
    let vk = b"trait_bound_vk";
    let inputs = b"trait_inputs";

    let result =
        verify_with_trait::<StubZkVerifier>(proof, vk, inputs, 0, &ZERO_CODE_HASH);
    assert!(
        result,
        "Trait bound function should work with StubZkVerifier"
    );
}

/// Demonstrates a custom (non-stub, non-Groth16) `ZkVerifier` impl. Compile-time
/// check that custom implementations match the v3 trait shape.
struct MockRealVerifier;

impl ZkVerifier for MockRealVerifier {
    fn verify_proof(
        proof: &[u8],
        vk: &[u8],
        public_inputs: &[u8],
        proof_type: u8,
        code_hash: &[u8; 32],
    ) -> bool {
        // A "real" verifier might check proof and vk shape, route on proof_type,
        // bind to code_hash, etc. For this mock, exercise every parameter so
        // a future trait change forces a test update.
        !proof.is_empty()
            && !vk.is_empty()
            && !public_inputs.is_empty()
            && proof_type == 0
            && code_hash == &ZERO_CODE_HASH
    }
}

#[test]
fn custom_implementation_works() {
    // Requires non-empty proof, non-empty vk, non-empty inputs, proof_type=0, zero code_hash.
    assert!(!MockRealVerifier::verify_proof(
        &[],
        &[],
        &[],
        0,
        &ZERO_CODE_HASH
    ));
    assert!(!MockRealVerifier::verify_proof(
        b"proof",
        &[],
        b"inputs",
        0,
        &ZERO_CODE_HASH
    ));
    assert!(!MockRealVerifier::verify_proof(
        b"proof",
        b"vk",
        &[],
        0,
        &ZERO_CODE_HASH
    ));
    assert!(MockRealVerifier::verify_proof(
        b"proof",
        b"vk",
        b"inputs",
        0,
        &ZERO_CODE_HASH
    ));
    // Wrong proof_type → reject
    assert!(!MockRealVerifier::verify_proof(
        b"proof",
        b"vk",
        b"inputs",
        1,
        &ZERO_CODE_HASH
    ));
    // Non-zero code_hash → reject
    assert!(!MockRealVerifier::verify_proof(
        b"proof",
        b"vk",
        b"inputs",
        0,
        &[0xFFu8; 32]
    ));
}

#[test]
fn trait_bound_works_with_custom_impl() {
    let result = verify_with_trait::<MockRealVerifier>(
        b"proof",
        b"vk",
        b"inputs",
        0,
        &ZERO_CODE_HASH,
    );
    assert!(result);

    let result_empty =
        verify_with_trait::<MockRealVerifier>(&[], &[], &[], 0, &ZERO_CODE_HASH);
    assert!(!result_empty);
}

// ============================================================================
// EDGE CASE TESTS
// ============================================================================

#[test]
fn stub_handles_binary_data() {
    let proof: Vec<u8> = (0u8..=255).collect();
    let vk: Vec<u8> = (0u8..=127).collect();
    let inputs: Vec<u8> = (0u8..=255).rev().collect();

    assert!(
        StubZkVerifier::verify_proof(&proof, &vk, &inputs, 0, &ZERO_CODE_HASH),
        "Stub must handle all byte values"
    );
}

#[test]
fn stub_handles_null_bytes() {
    let proof = vec![0u8; 100];
    let vk = vec![0u8; 75];
    let inputs = vec![0u8; 50];

    assert!(
        StubZkVerifier::verify_proof(&proof, &vk, &inputs, 0, &ZERO_CODE_HASH),
        "Stub must handle null bytes"
    );
}
