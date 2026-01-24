//! Integration tests for ZK stub verifier (D20.4)
//!
//! These tests verify the stub implementation behaves correctly
//! and that the trait can be used as expected.

use novai_crypto::{StubZkVerifier, ZkVerifier};

// ============================================================================
// BASIC FUNCTIONALITY TESTS
// ============================================================================

#[test]
fn stub_returns_true_for_empty_inputs() {
    let result = StubZkVerifier::verify_proof(&[], &[]);
    assert!(result, "Stub must return true for empty inputs");
}

#[test]
fn stub_returns_true_for_non_empty_inputs() {
    let proof = b"mock_zk_proof_data_here";
    let inputs = b"public_inputs_for_verification";

    let result = StubZkVerifier::verify_proof(proof, inputs);
    assert!(result, "Stub must return true for non-empty inputs");
}

#[test]
fn stub_returns_true_for_large_proof() {
    // Simulate a realistic proof size (e.g., Groth16 ~200 bytes)
    let proof = vec![0xABu8; 256];
    let inputs = vec![0xCDu8; 64];

    let result = StubZkVerifier::verify_proof(&proof, &inputs);
    assert!(result, "Stub must return true for large proofs");
}

#[test]
fn stub_returns_true_for_proof_only() {
    let proof = b"proof_without_inputs";
    let inputs: &[u8] = &[];

    let result = StubZkVerifier::verify_proof(proof, inputs);
    assert!(result, "Stub must return true even with empty inputs");
}

#[test]
fn stub_returns_true_for_inputs_only() {
    let proof: &[u8] = &[];
    let inputs = b"inputs_without_proof";

    let result = StubZkVerifier::verify_proof(proof, inputs);
    assert!(result, "Stub must return true even with empty proof");
}

// ============================================================================
// DETERMINISM TESTS
// ============================================================================

#[test]
fn stub_is_deterministic() {
    let proof = b"deterministic_test_proof";
    let inputs = b"deterministic_test_inputs";

    // Call multiple times, should always return true
    for _ in 0..10 {
        assert!(
            StubZkVerifier::verify_proof(proof, inputs),
            "Stub must be deterministic"
        );
    }
}

#[test]
fn stub_same_result_for_same_inputs() {
    let proof = vec![0x11u8; 128];
    let inputs = vec![0x22u8; 32];

    let result1 = StubZkVerifier::verify_proof(&proof, &inputs);
    let result2 = StubZkVerifier::verify_proof(&proof, &inputs);

    assert_eq!(result1, result2, "Same inputs must produce same result");
}

// ============================================================================
// TRAIT BOUND TESTS (compile-time verification)
// ============================================================================

/// Function that accepts any ZkVerifier implementation.
/// This is a compile-time check that the trait can be used as a bound.
fn verify_with_trait<V: ZkVerifier>(proof: &[u8], inputs: &[u8]) -> bool {
    V::verify_proof(proof, inputs)
}

#[test]
fn trait_can_be_used_as_bound() {
    let proof = b"trait_bound_test";
    let inputs = b"trait_inputs";

    // Use the generic function with StubZkVerifier
    let result = verify_with_trait::<StubZkVerifier>(proof, inputs);
    assert!(result, "Trait bound function should work with StubZkVerifier");
}

/// Demonstrates how a future real verifier would implement the trait.
/// This is a compile-time check that custom implementations work.
struct MockRealVerifier;

impl ZkVerifier for MockRealVerifier {
    fn verify_proof(proof: &[u8], public_inputs: &[u8]) -> bool {
        // A "real" verifier might check proof length, etc.
        // For this mock, we just check that both are non-empty
        !proof.is_empty() && !public_inputs.is_empty()
    }
}

#[test]
fn custom_implementation_works() {
    // MockRealVerifier requires non-empty inputs
    assert!(!MockRealVerifier::verify_proof(&[], &[]));
    assert!(!MockRealVerifier::verify_proof(b"proof", &[]));
    assert!(!MockRealVerifier::verify_proof(&[], b"inputs"));
    assert!(MockRealVerifier::verify_proof(b"proof", b"inputs"));
}

#[test]
fn trait_bound_works_with_custom_impl() {
    // Verify the trait bound function works with custom implementations
    let result = verify_with_trait::<MockRealVerifier>(b"proof", b"inputs");
    assert!(result);

    let result_empty = verify_with_trait::<MockRealVerifier>(&[], &[]);
    assert!(!result_empty);
}

// ============================================================================
// EDGE CASE TESTS
// ============================================================================

#[test]
fn stub_handles_binary_data() {
    // Test with all possible byte values
    let proof: Vec<u8> = (0u8..=255).collect();
    let inputs: Vec<u8> = (0u8..=255).rev().collect();

    assert!(
        StubZkVerifier::verify_proof(&proof, &inputs),
        "Stub must handle all byte values"
    );
}

#[test]
fn stub_handles_null_bytes() {
    let proof = vec![0u8; 100];
    let inputs = vec![0u8; 50];

    assert!(
        StubZkVerifier::verify_proof(&proof, &inputs),
        "Stub must handle null bytes"
    );
}
