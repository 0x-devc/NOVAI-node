//! Week 26: A26.2 Size Leak Attack Tests.
//!
//! PURPOSE: Test whether commitment sizes leak transaction value information.
//! An attacker observing on-chain data attempts to infer transaction values,
//! types, or other private information from the byte sizes of commitments.
//!
//! ATTACK VECTORS:
//! - Observe encoded commitment sizes for different payload values
//! - Compare commitment sizes across transaction types
//! - Look for size variations that correlate with value ranges
//! - Check if any sub-field has variable length
//!
//! EXPECTED RESULTS:
//! - ALL commitments encode to exactly 129 bytes (PRIVATE_PAYLOAD_COMMITMENT_LEN)
//! - Commitment hashes are always 32 bytes (blake3 output)
//! - Nullifiers are always 32 bytes (blake3 output)
//! - No variable-length fields exist in the commitment structure
//!
//! MITIGATION: Normalized sizes - all commitments are identical in size.

#![allow(clippy::doc_markdown)]

use novai_ai_entities::{
    encode_private_payload_commitment_v1, PrivatePayloadCommitment,
    PRIVATE_PAYLOAD_COMMITMENT_LEN,
};

// ============================================================================
// A26.2-T1: ALL COMMITMENTS SAME ENCODED SIZE
// ============================================================================

#[test]
fn test_all_commitments_same_encoded_size() {
    // ATTACK: Create commitments from a wide variety of payloads and check
    // if any produce a different encoded size. Even a single-byte difference
    // could leak information about the underlying transaction.
    //
    // EXPECTED: Every commitment encodes to exactly 129 bytes.

    let test_payloads: Vec<Vec<u8>> = vec![
        vec![],                          // empty
        vec![0x00],                      // single zero byte
        vec![0xFF],                      // single max byte
        vec![0x42; 32],                  // 32 bytes (hash-sized)
        vec![0xAA; 64],                  // 64 bytes
        vec![0xBB; 128],                 // 128 bytes
        vec![0xCC; 256],                 // 256 bytes
        vec![0xDD; 1024],               // 1 KB
        vec![0xEE; 4096],               // 4 KB
        vec![0xFF; 65536],              // 64 KB
        vec![0x11; 1_000_000],          // 1 MB
        b"transfer:100:alice->bob".to_vec(),
        b"transfer:999999999999:whale->exchange".to_vec(),
        b"swap:1:token_a:token_b".to_vec(),
        b"swap:1000000:token_a:token_b".to_vec(),
        (0u8..=255).collect(),           // all byte values
    ];

    for (i, payload) in test_payloads.iter().enumerate() {
        let mut secret = [0u8; 32];
        secret[0] = i as u8;
        let pubkey = [0x42u8; 32];

        let commitment =
            PrivatePayloadCommitment::new(payload, &secret, i as u64, pubkey);
        let encoded = encode_private_payload_commitment_v1(&commitment);

        assert_eq!(
            encoded.len(),
            PRIVATE_PAYLOAD_COMMITMENT_LEN,
            "Payload #{} (len={}) produced commitment of {} bytes, expected {}",
            i,
            payload.len(),
            encoded.len(),
            PRIVATE_PAYLOAD_COMMITMENT_LEN,
        );
    }
}

// ============================================================================
// A26.2-T2: SMALL PAYLOAD SAME SIZE AS LARGE
// ============================================================================

#[test]
fn test_small_payload_same_size_as_large() {
    // ATTACK: Compare the encoded size of a commitment from a 1-byte payload
    // against one from a 1 MB payload. If sizes differ, an observer can
    // estimate the transaction data size.
    //
    // EXPECTED: Both produce exactly 129 bytes. The blake3 hash compresses
    // any input to a fixed 32-byte output, so payload size is hidden.

    let tiny_payload = b"x";
    let large_payload = vec![0xFFu8; 1_000_000]; // 1 MB

    let secret = [0x01u8; 32];
    let pubkey = [0xAAu8; 32];

    let tiny_commitment =
        PrivatePayloadCommitment::new(tiny_payload, &secret, 0, pubkey);
    let large_commitment =
        PrivatePayloadCommitment::new(&large_payload, &secret, 1, pubkey);

    let tiny_encoded = encode_private_payload_commitment_v1(&tiny_commitment);
    let large_encoded = encode_private_payload_commitment_v1(&large_commitment);

    assert_eq!(
        tiny_encoded.len(),
        large_encoded.len(),
        "1-byte and 1MB payloads must produce same-size commitments"
    );
    assert_eq!(tiny_encoded.len(), 129);
    assert_eq!(large_encoded.len(), 129);

    // Verify the size difference between inputs is completely hidden
    let input_size_ratio = large_payload.len() as f64 / tiny_payload.len() as f64;
    assert!(
        input_size_ratio > 999_999.0,
        "Input sizes differ by 1,000,000x but output sizes are identical"
    );
}

// ============================================================================
// A26.2-T3: DIFFERENT VALUE AMOUNTS SAME COMMITMENT SIZE
// ============================================================================

#[test]
fn test_different_value_amounts_same_commitment_size() {
    // ATTACK: Create commitments for transactions with different monetary
    // values (0.01, 1, 100, 1_000_000, u128::MAX) and check if any size
    // variation reveals the value range.
    //
    // EXPECTED: All commitments are 129 bytes. Transaction value is part
    // of the encrypted payload, which is hashed to a fixed 32-byte digest.

    let values: Vec<u128> = vec![
        0,
        1,
        100,
        1_000,
        1_000_000,
        1_000_000_000,
        1_000_000_000_000,
        u128::MAX / 2,
        u128::MAX,
    ];

    for (i, value) in values.iter().enumerate() {
        // Simulate an encrypted payload containing the value
        let payload = format!("encrypted:value={}", value);
        let mut secret = [0u8; 32];
        secret[0] = i as u8;
        let pubkey = [0x42u8; 32];

        let commitment =
            PrivatePayloadCommitment::new(payload.as_bytes(), &secret, i as u64, pubkey);
        let encoded = encode_private_payload_commitment_v1(&commitment);

        assert_eq!(
            encoded.len(),
            PRIVATE_PAYLOAD_COMMITMENT_LEN,
            "Value {} produced commitment of {} bytes, expected {}",
            value,
            encoded.len(),
            PRIVATE_PAYLOAD_COMMITMENT_LEN,
        );
    }

    // Also test with raw big-endian value bytes directly (not string encoding)
    for (i, value) in values.iter().enumerate() {
        let payload = value.to_be_bytes();
        let mut secret = [0u8; 32];
        secret[0] = (i + 100) as u8;
        let pubkey = [0x42u8; 32];

        let commitment =
            PrivatePayloadCommitment::new(&payload, &secret, (i + 100) as u64, pubkey);
        let encoded = encode_private_payload_commitment_v1(&commitment);

        assert_eq!(
            encoded.len(),
            PRIVATE_PAYLOAD_COMMITMENT_LEN,
            "Raw value {} produced commitment of {} bytes, expected {}",
            value,
            encoded.len(),
            PRIVATE_PAYLOAD_COMMITMENT_LEN,
        );
    }
}

// ============================================================================
// A26.2-T4: EMPTY PAYLOAD SAME SIZE
// ============================================================================

#[test]
fn test_empty_payload_same_size() {
    // ATTACK: Check if an empty payload (no transaction data) produces a
    // different-sized commitment than a non-empty one. An empty payload
    // could indicate a special transaction type.
    //
    // EXPECTED: Empty payload produces exactly 129 bytes, same as any other.

    let empty_payload: &[u8] = b"";
    let nonempty_payload = b"some actual transaction data here";

    let secret = [0x42u8; 32];
    let pubkey = [0xAAu8; 32];

    let empty_commitment =
        PrivatePayloadCommitment::new(empty_payload, &secret, 0, pubkey);
    let nonempty_commitment =
        PrivatePayloadCommitment::new(nonempty_payload, &secret, 1, pubkey);

    let empty_encoded = encode_private_payload_commitment_v1(&empty_commitment);
    let nonempty_encoded = encode_private_payload_commitment_v1(&nonempty_commitment);

    assert_eq!(
        empty_encoded.len(),
        PRIVATE_PAYLOAD_COMMITMENT_LEN,
        "Empty payload must produce {} byte commitment",
        PRIVATE_PAYLOAD_COMMITMENT_LEN,
    );
    assert_eq!(
        nonempty_encoded.len(),
        PRIVATE_PAYLOAD_COMMITMENT_LEN,
        "Non-empty payload must produce {} byte commitment",
        PRIVATE_PAYLOAD_COMMITMENT_LEN,
    );
    assert_eq!(
        empty_encoded.len(),
        nonempty_encoded.len(),
        "Empty and non-empty payloads must produce same-size commitments"
    );

    // Verify the empty payload still produces a valid commitment hash
    // (not all zeros or some degenerate value)
    assert_ne!(
        empty_commitment.commitment_hash,
        [0u8; 32],
        "Empty payload must still produce a non-zero commitment hash"
    );
}

// ============================================================================
// A26.2-T5: NULLIFIER FIXED SIZE
// ============================================================================

#[test]
fn test_nullifier_fixed_size() {
    // ATTACK: Check if nullifiers vary in size based on the spending secret
    // or counter value. Variable-size nullifiers could leak information about
    // the number of prior spends (counter value) or secret properties.
    //
    // EXPECTED: All nullifiers are exactly 32 bytes (blake3 output).

    let secrets: Vec<[u8; 32]> = vec![
        [0x00u8; 32], // all zeros
        [0xFFu8; 32], // all ones
        [0x42u8; 32], // arbitrary
        {
            let mut s = [0u8; 32];
            s[0] = 1; // minimal nonzero
            s
        },
    ];

    let counters: Vec<u64> = vec![0, 1, 100, u64::MAX / 2, u64::MAX];

    for secret in &secrets {
        for &counter in &counters {
            let nullifier =
                PrivatePayloadCommitment::compute_nullifier(secret, counter);

            assert_eq!(
                nullifier.len(),
                32,
                "Nullifier for secret {:02x?}.. counter {} must be 32 bytes, got {}",
                &secret[..4],
                counter,
                nullifier.len(),
            );

            // Verify it's not degenerate (all zeros)
            // (zero secret + zero counter could theoretically produce zero output)
            // blake3 with domain separation should never produce all-zero output
            // for any input
            let is_all_zero = nullifier.iter().all(|&b| b == 0);
            assert!(
                !is_all_zero,
                "Nullifier must not be all zeros (secret={:02x?}.., counter={})",
                &secret[..4],
                counter,
            );
        }
    }
}

// ============================================================================
// A26.2-T6: COMMITMENT HASH FIXED SIZE
// ============================================================================

#[test]
fn test_commitment_hash_fixed_size() {
    // ATTACK: Check if commitment hashes vary in size based on the payload.
    // A variable-length hash would directly leak payload size information.
    //
    // EXPECTED: All commitment hashes are exactly 32 bytes (blake3 output).

    let payloads: Vec<&[u8]> = vec![
        b"",
        b"a",
        b"ab",
        &[0u8; 32],
        &[0xFFu8; 64],
        &[0xABu8; 1000],
        &[0x00u8; 100_000],
    ];

    for payload in &payloads {
        let hash = PrivatePayloadCommitment::compute_commitment_hash(payload);

        assert_eq!(
            hash.len(),
            32,
            "Commitment hash for payload len={} must be 32 bytes, got {}",
            payload.len(),
            hash.len(),
        );

        // Verify non-degenerate output
        let is_all_zero = hash.iter().all(|&b| b == 0);
        assert!(
            !is_all_zero,
            "Commitment hash must not be all zeros for payload len={}",
            payload.len(),
        );
    }

    // Also verify that different payloads produce different hashes
    // (collision resistance)
    let hash_empty = PrivatePayloadCommitment::compute_commitment_hash(b"");
    let hash_zero = PrivatePayloadCommitment::compute_commitment_hash(&[0u8]);

    assert_ne!(
        hash_empty, hash_zero,
        "Empty payload and zero-byte payload must produce different hashes"
    );
}

// ============================================================================
// A26.2-T7: ENCODED COMMITMENT GOLDEN SIZE
// ============================================================================

#[test]
fn test_encoded_commitment_golden_size() {
    // PURPOSE: Golden vector test locking the commitment encoding size to
    // exactly 129 bytes. This prevents accidental changes to the encoding
    // format that could introduce variable-length fields.
    //
    // FORMAT:
    //   [version:1][commitment_hash:32][nullifier:32][encryption_pubkey:32][zk_proof:32]
    //   Total: 1 + 32 + 32 + 32 + 32 = 129 bytes
    //
    // If this test fails, a code change has altered the commitment encoding
    // format, which could introduce a size leak vulnerability.

    // Verify the constant itself
    assert_eq!(
        PRIVATE_PAYLOAD_COMMITMENT_LEN, 129,
        "PRIVATE_PAYLOAD_COMMITMENT_LEN must be exactly 129"
    );

    // Verify the constant matches the actual encoding formula
    let expected = 1  // version byte
        + 32           // commitment_hash (blake3)
        + 32           // nullifier (blake3)
        + 32           // encryption_pubkey (X25519)
        + 32;          // zk_proof (blake3 stub)
    assert_eq!(
        PRIVATE_PAYLOAD_COMMITMENT_LEN, expected,
        "Encoding size must equal version(1) + 4*hash(32) = 129"
    );

    // Golden vector: known commitment with deterministic inputs
    let golden_commitment = PrivatePayloadCommitment::new(
        b"NOVAI_GOLDEN_VECTOR_PAYLOAD_V1",
        &[0x42u8; 32],
        0,
        [0xAAu8; 32],
    );

    let golden_encoded = encode_private_payload_commitment_v1(&golden_commitment);

    // Lock the size
    assert_eq!(
        golden_encoded.len(),
        129,
        "Golden vector commitment must be exactly 129 bytes"
    );

    // Lock the version byte
    assert_eq!(
        golden_encoded[0], 1,
        "Golden vector version byte must be 1"
    );

    // Lock the field boundaries
    assert_eq!(
        &golden_encoded[1..33],
        &golden_commitment.commitment_hash,
        "Bytes 1..33 must be commitment_hash"
    );
    assert_eq!(
        &golden_encoded[33..65],
        &golden_commitment.nullifier,
        "Bytes 33..65 must be nullifier"
    );
    assert_eq!(
        &golden_encoded[65..97],
        &golden_commitment.encryption_pubkey,
        "Bytes 65..97 must be encryption_pubkey"
    );
    assert_eq!(
        &golden_encoded[97..129],
        &golden_commitment.zk_proof,
        "Bytes 97..129 must be zk_proof"
    );

    // No trailing bytes
    assert_eq!(
        golden_encoded.len(),
        129,
        "No trailing bytes allowed beyond 129"
    );
}
