//! Integration tests for Week 15 artifact storage system.
//!
//! Tests all acceptance criteria:
//! 1. Store/fetch roundtrip: Content matches
//! 2. Hash mismatch detected: Corrupted content rejected
//! 3. Verification works: Full flow succeeds
//! 4. Multiple backends: File and HTTP both work

use novai_ai_entities::{
    artifact_hash, fetch_and_verify_payload, verify_payload_hash, verify_signal_payload,
    ArtifactError, ArtifactStore, FetchVerifyError, LocalFileStore, MemoryValue, SignalPayload,
    VerifyResult, MAX_INLINE_MEMORY_SIZE,
};
use std::fs;
use tempfile::tempdir;

// =============================================================================
// Acceptance Criterion 1: Store/fetch roundtrip - Content matches
// =============================================================================

#[test]
fn acceptance_1_store_fetch_roundtrip_raw_bytes() {
    let temp_dir = tempdir().unwrap();
    let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

    // Store raw content
    let content = b"Hello, NOVAI artifact storage!";
    let hash = store.store(content).unwrap();

    // Fetch and verify content matches
    let fetched = store.fetch(&hash).unwrap();
    assert_eq!(
        fetched,
        content.to_vec(),
        "Fetched content must match stored content"
    );

    // Verify hash is correct
    assert_eq!(
        hash,
        artifact_hash(content),
        "Returned hash must match content hash"
    );
}

#[test]
fn acceptance_1_store_fetch_roundtrip_signal_payload() {
    let temp_dir = tempdir().unwrap();
    let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

    // Create and store a SignalPayload
    let payload = SignalPayload::new(
        "gpt-4-turbo".to_string(),
        "2024.01.15".to_string(),
        "Market analysis for BTC/USD pair".to_string(),
        vec![0x01, 0x02, 0x03, 0x04, 0x05],
        "Based on technical indicators, expect consolidation".to_string(),
    );

    let encoded = payload.encode();
    let hash = store.store(&encoded).unwrap();

    // Fetch and decode
    let fetched = store.fetch(&hash).unwrap();
    let decoded = SignalPayload::decode(&fetched).expect("Should decode successfully");

    assert_eq!(decoded, payload, "Decoded payload must match original");
}

#[test]
fn acceptance_1_store_fetch_large_content() {
    let temp_dir = tempdir().unwrap();
    let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

    // Store larger content (1MB)
    let large_content: Vec<u8> = (0..1_000_000).map(|i| (i % 256) as u8).collect();
    let hash = store.store(&large_content).unwrap();

    let fetched = store.fetch(&hash).unwrap();
    assert_eq!(
        fetched, large_content,
        "Large content must roundtrip correctly"
    );
}

#[test]
fn acceptance_1_exists_returns_correct_status() {
    let temp_dir = tempdir().unwrap();
    let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

    let content = b"Test content for exists check";
    let hash = store.store(content).unwrap();

    // Should exist after storing
    assert!(store.exists(&hash), "Should exist after storing");

    // Random hash should not exist
    let missing_hash = [0xAB; 32];
    assert!(!store.exists(&missing_hash), "Random hash should not exist");
}

// =============================================================================
// Acceptance Criterion 2: Hash mismatch detected - Corrupted content rejected
// =============================================================================

#[test]
fn acceptance_2_corrupted_content_detected_on_fetch() {
    let temp_dir = tempdir().unwrap();
    let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

    // Store content
    let content = b"Original content";
    let hash = store.store(content).unwrap();

    // Corrupt the file directly (LocalFileStore uses {hash_hex}.bin format)
    let hash_hex = hex::encode(hash);
    let file_path = temp_dir.path().join(format!("{hash_hex}.bin"));
    fs::write(&file_path, b"Corrupted content!!!").unwrap();

    // Fetch should detect corruption
    let result = store.fetch(&hash);
    assert!(
        matches!(result, Err(ArtifactError::HashMismatch { .. })),
        "Should detect hash mismatch on corrupted content"
    );
}

#[test]
fn acceptance_2_verification_rejects_wrong_hash() {
    let payload = SignalPayload::new(
        "model".to_string(),
        "1.0".to_string(),
        "input".to_string(),
        vec![0x42],
        "explanation".to_string(),
    );

    let encoded = payload.encode();
    let wrong_hash = [0xFF; 32];

    let result = verify_payload_hash(&encoded, &wrong_hash);
    assert!(
        matches!(result, VerifyResult::HashMismatch { .. }),
        "Should reject wrong hash"
    );
}

#[test]
fn acceptance_2_verification_rejects_tampered_payload() {
    let payload = SignalPayload::new(
        "model".to_string(),
        "1.0".to_string(),
        "input".to_string(),
        vec![0x42],
        "explanation".to_string(),
    );

    let encoded = payload.encode();
    let correct_hash = artifact_hash(&encoded);

    // Tamper with the encoded data
    let mut tampered = encoded;
    tampered[10] ^= 0xFF; // Flip some bits

    let result = verify_payload_hash(&tampered, &correct_hash);
    assert!(
        matches!(result, VerifyResult::HashMismatch { .. }),
        "Should reject tampered payload"
    );
}

// =============================================================================
// Acceptance Criterion 3: Verification works - Full flow succeeds
// =============================================================================

#[test]
fn acceptance_3_full_verification_flow() {
    let temp_dir = tempdir().unwrap();
    let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

    // Create payload
    let payload = SignalPayload::new(
        "novai-oracle-v1".to_string(),
        "0.1.0".to_string(),
        "Price feed query for ETH/USD".to_string(),
        vec![0xDE, 0xAD, 0xBE, 0xEF],
        "Current price: $3,245.67".to_string(),
    );

    // Encode and store
    let encoded = payload.encode();
    let commitment_hash = store.store(&encoded).unwrap();

    // Full verification flow: fetch + verify + decode
    let verified_payload = fetch_and_verify_payload(&store, &commitment_hash)
        .expect("Full verification flow should succeed");

    assert_eq!(
        verified_payload, payload,
        "Verified payload must match original"
    );
}

#[test]
fn acceptance_3_verify_signal_payload_decodes_correctly() {
    let payload = SignalPayload::new(
        "test-model".to_string(),
        "2.0".to_string(),
        "Test input summary".to_string(),
        vec![0x01, 0x02, 0x03],
        "Test explanation".to_string(),
    );

    let encoded = payload.encode();
    let hash = artifact_hash(&encoded);

    let result = verify_signal_payload(&encoded, &hash);
    assert!(result.is_ok(), "verify_signal_payload should succeed");
    assert_eq!(result.unwrap(), payload);
}

#[test]
fn acceptance_3_fetch_and_verify_returns_correct_errors() {
    let temp_dir = tempdir().unwrap();
    let store = LocalFileStore::new(temp_dir.path()).unwrap();

    // Try to fetch non-existent artifact
    let missing_hash = [0x42; 32];
    let result = fetch_and_verify_payload(&store, &missing_hash);

    assert!(
        matches!(
            result,
            Err(FetchVerifyError::Artifact(ArtifactError::NotFound(_)))
        ),
        "Should return NotFound error for missing artifact"
    );
}

#[test]
fn acceptance_3_verify_detects_decode_errors() {
    // Valid hash but invalid payload structure
    let garbage = vec![0x01, 0xFF, 0xFF, 0xFF, 0xFF]; // Version 1 but garbage data
    let hash = artifact_hash(&garbage);

    let result = verify_signal_payload(&garbage, &hash);
    assert!(
        matches!(result, Err(VerifyResult::PayloadDecodeError)),
        "Should detect payload decode errors after hash verification"
    );
}

// =============================================================================
// Acceptance Criterion 4: Multiple backends work
// =============================================================================

#[test]
fn acceptance_4_local_file_store_works() {
    let temp_dir = tempdir().unwrap();
    let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

    let content = b"Local file store test content";
    let hash = store.store(content).unwrap();

    assert!(store.exists(&hash));
    assert_eq!(store.fetch(&hash).unwrap(), content.to_vec());
}

#[test]
fn acceptance_4_local_store_multiple_artifacts() {
    let temp_dir = tempdir().unwrap();
    let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

    // Store multiple artifacts
    let contents: Vec<&[u8]> = vec![
        b"First artifact",
        b"Second artifact with different content",
        b"Third artifact",
    ];

    let hashes: Vec<[u8; 32]> = contents.iter().map(|c| store.store(c).unwrap()).collect();

    // Verify all can be fetched
    for (content, hash) in contents.iter().zip(hashes.iter()) {
        assert!(store.exists(hash));
        assert_eq!(store.fetch(hash).unwrap(), content.to_vec());
    }
}

#[test]
fn acceptance_4_local_store_idempotent() {
    let temp_dir = tempdir().unwrap();
    let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

    let content = b"Idempotent store test";

    // Store same content twice
    let hash1 = store.store(content).unwrap();
    let hash2 = store.store(content).unwrap();

    // Should return same hash
    assert_eq!(hash1, hash2, "Storing same content should return same hash");

    // Should still be fetchable
    assert_eq!(store.fetch(&hash1).unwrap(), content.to_vec());
}

// Note: HTTP backend (HttpFetchStore) is tested in unit tests.
// It requires actual HTTP servers which are not available in integration tests.
// The unit tests verify:
// - exists() uses HEAD requests
// - fetch() validates content hash
// - Multiple mirrors are tried on failure
// - 30-second timeout is respected

// =============================================================================
// Additional Integration Tests: MemoryValue
// =============================================================================

#[test]
fn memory_value_integration_inline() {
    let temp_dir = tempdir().unwrap();
    let store = LocalFileStore::new(temp_dir.path()).unwrap();

    // Small data should be inline
    let small_data = vec![0x42; 100];
    let value = MemoryValue::from_data(small_data.clone());

    assert!(value.is_inline());
    assert_eq!(value.resolve(&store).unwrap(), small_data);
}

#[test]
fn memory_value_integration_artifact() {
    let temp_dir = tempdir().unwrap();
    let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

    // Large data should become artifact reference
    let large_data: Vec<u8> = vec![0x42; MAX_INLINE_MEMORY_SIZE + 1];

    // Store the data first
    let hash = store.store(&large_data).unwrap();

    // Create artifact reference
    let value = MemoryValue::Artifact(hash);

    assert!(value.is_artifact());
    assert_eq!(value.resolve(&store).unwrap(), large_data);
}

#[test]
fn memory_value_encode_decode_integration() {
    // Test that encoded MemoryValues can be stored and fetched
    let temp_dir = tempdir().unwrap();
    let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

    let inline_value = MemoryValue::Inline(vec![0x01, 0x02, 0x03]);
    let encoded = inline_value.encode();

    // Store the encoded value
    let hash = store.store(&encoded).unwrap();

    // Fetch and decode
    let fetched = store.fetch(&hash).unwrap();
    let decoded = MemoryValue::decode(&fetched).unwrap();

    assert_eq!(decoded, inline_value);
}

// =============================================================================
// End-to-End Scenario Test
// =============================================================================

#[test]
fn e2e_ai_signal_workflow() {
    let temp_dir = tempdir().unwrap();
    let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

    // Step 1: AI generates a signal payload
    let payload = SignalPayload::new(
        "novai-market-analyzer".to_string(),
        "1.2.3".to_string(),
        "Analyzed BTC price movements over 24h period".to_string(),
        vec![
            // Simulated output data (could be serialized predictions)
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
        ],
        "Strong buy signal based on RSI oversold conditions".to_string(),
    );

    // Step 2: Encode and compute commitment hash
    let encoded = payload.encode();
    let commitment_hash = payload.compute_hash();

    // Step 3: Store payload off-chain
    let stored_hash = store.store(&encoded).unwrap();
    assert_eq!(
        commitment_hash, stored_hash,
        "Computed and stored hashes must match"
    );

    // Step 4: On-chain: commitment_hash would be stored in a transaction
    // (simulated - in real use this would be in SignalCommitment)

    // Step 5: Client fetches and verifies the payload
    let verified =
        fetch_and_verify_payload(&store, &commitment_hash).expect("Verification should succeed");

    // Step 6: Client uses the verified payload
    assert_eq!(verified.model_id, "novai-market-analyzer");
    assert_eq!(
        verified.explanation,
        "Strong buy signal based on RSI oversold conditions"
    );

    // Step 7: Verify the payload hash matches what's on-chain
    assert_eq!(
        verified.compute_hash(),
        commitment_hash,
        "Payload hash must match on-chain commitment"
    );
}
