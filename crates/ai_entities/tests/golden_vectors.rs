//! Golden vector tests for AI entity encoding stability.
//!
//! Run with UPDATE_VECTORS=1 to regenerate vectors:
//! ```
//! UPDATE_VECTORS=1 cargo test -p novai-ai-entities --test golden_vectors
//! ```

use novai_ai_entities::{AiEntity, AutonomyMode, Capabilities, DEFAULT_REPUTATION_SCORE};
// Golden vector tests intentionally use v1 codec to verify backward compatibility
use novai_codec::ai_entity_codec::{
    decode_ai_entity, encode_ai_entity_v2, encode_ai_entity_v3, encode_ai_entity_v4,
    encode_ai_entity_v5, AI_ENTITY_V4_SIZE, AI_ENTITY_V5_SIZE,
};
#[allow(deprecated)]
use novai_codec::ai_entity_codec::{decode_ai_entity_v1, encode_ai_entity_v1};
use std::fs;
use std::path::PathBuf;

fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors")
}

fn should_update_vectors() -> bool {
    std::env::var("UPDATE_VECTORS").is_ok()
}

/// Standard test entity used for golden vectors.
fn golden_test_entity() -> AiEntity {
    AiEntity::new(
        [0x42u8; 32], // code_hash
        [0x01u8; 32], // creator
        AutonomyMode::Gated,
        Capabilities::gated(),
        1000, // registered_at
    )
}

#[test]
#[allow(deprecated)] // Intentionally tests v1 codec for backward compatibility
fn golden_ai_entity_v1() {
    let entity = golden_test_entity();
    let bytes = encode_ai_entity_v1(&entity);

    let path = vectors_dir().join("ai_entity_v1.bin");

    if should_update_vectors() {
        fs::create_dir_all(vectors_dir()).expect("failed to create vectors dir");
        fs::write(&path, &bytes).expect("failed to write golden vector");
        println!("Updated golden vector: {path:?}");
        println!("Vector length: {} bytes", bytes.len());
        println!("First 16 bytes: {:02x?}", &bytes[..16]);
    } else {
        let expected = fs::read(&path)
            .expect("Golden vector file missing. Run with UPDATE_VECTORS=1 to generate.");
        assert_eq!(
            bytes, expected,
            "AI entity encoding drifted from golden vector!"
        );
    }

    // Always verify roundtrip
    let decoded = decode_ai_entity_v1(&bytes).expect("decode failed");
    assert_eq!(entity.id, decoded.id);
    assert_eq!(entity.code_hash, decoded.code_hash);
    assert_eq!(entity.creator, decoded.creator);
    assert_eq!(entity.autonomy_mode, decoded.autonomy_mode);
    assert_eq!(entity.registered_at, decoded.registered_at);
}

#[test]
#[allow(deprecated)] // Intentionally tests v1 codec for backward compatibility
fn golden_vector_is_stable_across_runs() {
    // This test verifies that encoding the same entity twice
    // produces identical bytes, catching any nondeterminism.
    let entity = golden_test_entity();

    let bytes1 = encode_ai_entity_v1(&entity);
    let bytes2 = encode_ai_entity_v1(&entity);

    assert_eq!(
        bytes1, bytes2,
        "Encoding must be deterministic across calls"
    );
}

#[test]
#[allow(deprecated)] // Intentionally tests v1 codec for backward compatibility
fn golden_vector_has_correct_length() {
    let entity = golden_test_entity();
    let bytes = encode_ai_entity_v1(&entity);

    assert_eq!(
        bytes.len(),
        203,
        "AiEntity v1 encoding must be exactly 203 bytes"
    );
}

#[test]
fn golden_ai_entity_v2() {
    let entity = golden_test_entity();
    let bytes = encode_ai_entity_v2(&entity);

    let path = vectors_dir().join("ai_entity_v2.bin");

    if should_update_vectors() {
        fs::create_dir_all(vectors_dir()).expect("failed to create vectors dir");
        fs::write(&path, &bytes).expect("failed to write golden vector");
        println!("Updated golden vector: {path:?}");
        println!("Vector length: {} bytes", bytes.len());
        println!("First 16 bytes: {:02x?}", &bytes[..16]);
    } else {
        let expected = fs::read(&path)
            .expect("Golden vector file missing. Run with UPDATE_VECTORS=1 to generate.");
        assert_eq!(
            bytes, expected,
            "AI entity v2 encoding drifted from golden vector!"
        );
    }

    // Verify correct size (204 bytes: v1 203 + 1 byte is_active)
    assert_eq!(
        bytes.len(),
        204,
        "AiEntity v2 encoding must be exactly 204 bytes"
    );

    // Verify version byte
    assert_eq!(
        bytes[0], 0x02,
        "V2 encoding must start with version byte 0x02"
    );

    // Verify roundtrip through version-dispatching decoder
    let decoded = decode_ai_entity(&bytes).expect("v2 decode failed");
    assert_eq!(entity.id, decoded.id);
    assert_eq!(entity.code_hash, decoded.code_hash);
    assert_eq!(entity.creator, decoded.creator);
    assert_eq!(entity.autonomy_mode, decoded.autonomy_mode);
    assert_eq!(
        entity.capabilities.to_byte(),
        decoded.capabilities.to_byte()
    );
    assert_eq!(entity.economic_balance, decoded.economic_balance);
    assert_eq!(entity.nonce, decoded.nonce);
    assert_eq!(entity.memory_root, decoded.memory_root);
    assert_eq!(entity.params_root, decoded.params_root);
    assert_eq!(entity.registered_at, decoded.registered_at);
    assert_eq!(entity.last_active_at, decoded.last_active_at);
    assert_eq!(entity.is_active, decoded.is_active);
}

#[test]
fn golden_ai_entity_v2_is_stable_across_runs() {
    let entity = golden_test_entity();
    let bytes1 = encode_ai_entity_v2(&entity);
    let bytes2 = encode_ai_entity_v2(&entity);
    assert_eq!(
        bytes1, bytes2,
        "V2 encoding must be deterministic across calls"
    );
}

// ============================================================================
// V3 Golden Vector Tests (includes pubkey)
// ============================================================================

#[test]
fn golden_ai_entity_v3_zero_pubkey() {
    let entity = golden_test_entity(); // pubkey = [0u8; 32]
    let bytes = encode_ai_entity_v3(&entity);

    let path = vectors_dir().join("ai_entity_v3_zero_pubkey.bin");

    if should_update_vectors() {
        fs::create_dir_all(vectors_dir()).expect("failed to create vectors dir");
        fs::write(&path, &bytes).expect("failed to write golden vector");
        println!("Updated golden vector: {path:?}");
        println!("Vector length: {} bytes", bytes.len());
    } else {
        let expected = fs::read(&path)
            .expect("Golden vector file missing. Run with UPDATE_VECTORS=1 to generate.");
        assert_eq!(
            bytes, expected,
            "AI entity v3 (zero pubkey) encoding drifted from golden vector!"
        );
    }

    assert_eq!(bytes.len(), 236, "AiEntity v3 must be exactly 236 bytes");
    assert_eq!(bytes[0], 0x03, "V3 must start with version byte 0x03");

    // Verify pubkey is at offset 123 and is all zeros
    assert_eq!(
        &bytes[123..155],
        &[0u8; 32],
        "pubkey at offset 123 must be zero"
    );

    let decoded = decode_ai_entity(&bytes).expect("v3 decode failed");
    assert_eq!(entity.id, decoded.id);
    assert_eq!(entity.code_hash, decoded.code_hash);
    assert_eq!(entity.creator, decoded.creator);
    assert_eq!(entity.autonomy_mode, decoded.autonomy_mode);
    assert_eq!(entity.pubkey, decoded.pubkey);
    assert_eq!(entity.pubkey, [0u8; 32]);
    assert_eq!(entity.is_active, decoded.is_active);
}

#[test]
fn golden_ai_entity_v3_with_pubkey() {
    let mut entity = golden_test_entity();
    entity.pubkey = [0xAB; 32]; // Non-zero pubkey

    let bytes = encode_ai_entity_v3(&entity);

    let path = vectors_dir().join("ai_entity_v3_with_pubkey.bin");

    if should_update_vectors() {
        fs::create_dir_all(vectors_dir()).expect("failed to create vectors dir");
        fs::write(&path, &bytes).expect("failed to write golden vector");
        println!("Updated golden vector: {path:?}");
    } else {
        let expected = fs::read(&path)
            .expect("Golden vector file missing. Run with UPDATE_VECTORS=1 to generate.");
        assert_eq!(
            bytes, expected,
            "AI entity v3 (with pubkey) encoding drifted from golden vector!"
        );
    }

    assert_eq!(bytes.len(), 236);
    assert_eq!(bytes[0], 0x03);
    assert_eq!(
        &bytes[123..155],
        &[0xAB; 32],
        "pubkey must be at offset 123"
    );

    let decoded = decode_ai_entity(&bytes).expect("v3 decode failed");
    assert_eq!(decoded.pubkey, [0xAB; 32]);
}

#[test]
fn golden_ai_entity_v3_is_stable_across_runs() {
    let mut entity = golden_test_entity();
    entity.pubkey = [0xCD; 32];

    let bytes1 = encode_ai_entity_v3(&entity);
    let bytes2 = encode_ai_entity_v3(&entity);
    assert_eq!(
        bytes1, bytes2,
        "V3 encoding must be deterministic across calls"
    );
}

#[test]
fn v2_backward_compat_decodes_with_zero_pubkey() {
    let entity = golden_test_entity();
    let v2_bytes = encode_ai_entity_v2(&entity);
    let decoded = decode_ai_entity(&v2_bytes).expect("v2 decode failed");
    assert_eq!(
        decoded.pubkey, [0u8; 32],
        "V2 entities must decode with pubkey = [0u8; 32]"
    );
}

// ============================================================================
// V4 Golden Vector Tests (includes reputation tail)
// ============================================================================

#[test]
fn golden_ai_entity_v4() {
    let mut entity = golden_test_entity();
    entity.pubkey = [0xAB; 32];
    entity.reputation_score = 73;
    entity.total_transactions = 12;
    entity.reputation_events_count = 4;

    let bytes = encode_ai_entity_v4(&entity);
    let path = vectors_dir().join("ai_entity_v4.bin");

    if should_update_vectors() {
        fs::create_dir_all(vectors_dir()).expect("failed to create vectors dir");
        fs::write(&path, &bytes).expect("failed to write golden vector");
        println!("Updated golden vector: {path:?}");
        println!("Vector length: {} bytes", bytes.len());
    } else {
        let expected = fs::read(&path)
            .expect("Golden vector file missing. Run with UPDATE_VECTORS=1 to generate.");
        assert_eq!(
            bytes, expected,
            "AI entity v4 encoding drifted from golden vector!"
        );
    }

    assert_eq!(bytes.len(), AI_ENTITY_V4_SIZE);
    assert_eq!(bytes.len(), 246);
    assert_eq!(bytes[0], 0x04, "V4 must start with version byte 0x04");

    // reputation tail at offsets 236, 238, 242
    assert_eq!(&bytes[236..238], &73u16.to_le_bytes());
    assert_eq!(&bytes[238..242], &12u32.to_le_bytes());
    assert_eq!(&bytes[242..246], &4u32.to_le_bytes());

    let decoded = decode_ai_entity(&bytes).expect("v4 decode failed");
    assert_eq!(entity.id, decoded.id);
    assert_eq!(entity.pubkey, decoded.pubkey);
    assert_eq!(decoded.reputation_score, 73);
    assert_eq!(decoded.total_transactions, 12);
    assert_eq!(decoded.reputation_events_count, 4);
}

#[test]
fn v3_backward_compat_promotes_reputation_defaults() {
    let entity = golden_test_entity();
    let v3_bytes = encode_ai_entity_v3(&entity);
    let decoded = decode_ai_entity(&v3_bytes).expect("v3 decode failed");
    assert_eq!(
        decoded.reputation_score, DEFAULT_REPUTATION_SCORE,
        "V3 entities must decode with reputation_score = DEFAULT_REPUTATION_SCORE"
    );
    assert_eq!(decoded.total_transactions, 0);
    assert_eq!(decoded.reputation_events_count, 0);
}

#[test]
fn golden_ai_entity_v4_is_stable_across_runs() {
    let mut entity = golden_test_entity();
    entity.reputation_score = 88;
    entity.total_transactions = 999;
    entity.reputation_events_count = 7;

    let bytes1 = encode_ai_entity_v4(&entity);
    let bytes2 = encode_ai_entity_v4(&entity);
    assert_eq!(bytes1, bytes2, "V4 encoding must be deterministic");
}

// ============================================================================
// V5 Golden Vector Tests (includes stake tail)
// ============================================================================

#[test]
fn golden_ai_entity_v5() {
    let mut entity = golden_test_entity();
    entity.pubkey = [0xAB; 32];
    entity.reputation_score = 73;
    entity.total_transactions = 12;
    entity.reputation_events_count = 4;
    entity.stake_balance = 250_000;
    entity.stake_locked_until = 5_000;

    let bytes = encode_ai_entity_v5(&entity);
    let path = vectors_dir().join("ai_entity_v5.bin");

    if should_update_vectors() {
        fs::create_dir_all(vectors_dir()).expect("failed to create vectors dir");
        fs::write(&path, &bytes).expect("failed to write golden vector");
        println!("Updated golden vector: {path:?}");
        println!("Vector length: {} bytes", bytes.len());
    } else {
        let expected = fs::read(&path)
            .expect("Golden vector file missing. Run with UPDATE_VECTORS=1 to generate.");
        assert_eq!(
            bytes, expected,
            "AI entity v5 encoding drifted from golden vector!"
        );
    }

    assert_eq!(bytes.len(), AI_ENTITY_V5_SIZE);
    assert_eq!(bytes.len(), 270);
    assert_eq!(bytes[0], 0x05, "V5 must start with version byte 0x05");

    // stake tail at offsets 246, 262
    assert_eq!(&bytes[246..262], &250_000_u128.to_le_bytes());
    assert_eq!(&bytes[262..270], &5_000_u64.to_le_bytes());

    let decoded = decode_ai_entity(&bytes).expect("v5 decode failed");
    assert_eq!(entity.id, decoded.id);
    assert_eq!(entity.pubkey, decoded.pubkey);
    assert_eq!(decoded.reputation_score, 73);
    assert_eq!(decoded.total_transactions, 12);
    assert_eq!(decoded.reputation_events_count, 4);
    assert_eq!(decoded.stake_balance, 250_000);
    assert_eq!(decoded.stake_locked_until, 5_000);
}

#[test]
fn v4_backward_compat_promotes_stake_defaults() {
    let mut entity = golden_test_entity();
    entity.reputation_score = 73;
    entity.total_transactions = 12;
    entity.reputation_events_count = 4;

    let v4_bytes = encode_ai_entity_v4(&entity);
    let decoded = decode_ai_entity(&v4_bytes).expect("v4 decode failed");

    assert_eq!(
        decoded.stake_balance, 0,
        "V4 entities must decode with stake_balance = 0"
    );
    assert_eq!(
        decoded.stake_locked_until, 0,
        "V4 entities must decode with stake_locked_until = 0"
    );
    // Reputation tail still preserved on V4 → V5 promotion
    assert_eq!(decoded.reputation_score, 73);
    assert_eq!(decoded.total_transactions, 12);
    assert_eq!(decoded.reputation_events_count, 4);
}

#[test]
fn v3_backward_compat_promotes_stake_and_reputation_defaults() {
    let entity = golden_test_entity();
    let v3_bytes = encode_ai_entity_v3(&entity);
    let decoded = decode_ai_entity(&v3_bytes).expect("v3 decode failed");

    assert_eq!(decoded.stake_balance, 0);
    assert_eq!(decoded.stake_locked_until, 0);
    assert_eq!(decoded.reputation_score, DEFAULT_REPUTATION_SCORE);
}

#[test]
fn golden_ai_entity_v5_is_stable_across_runs() {
    let mut entity = golden_test_entity();
    entity.stake_balance = 12_345_678;
    entity.stake_locked_until = 9_999;

    let bytes1 = encode_ai_entity_v5(&entity);
    let bytes2 = encode_ai_entity_v5(&entity);
    assert_eq!(bytes1, bytes2, "V5 encoding must be deterministic");
}
