//! Golden vector tests for AI entity encoding stability.
//!
//! Run with UPDATE_VECTORS=1 to regenerate vectors:
//! ```
//! UPDATE_VECTORS=1 cargo test -p novai-ai-entities --test golden_vectors
//! ```

use novai_ai_entities::{AiEntity, AutonomyMode, Capabilities};
// Golden vector tests intentionally use v1 codec to verify backward compatibility
use novai_codec::ai_entity_codec::{decode_ai_entity, encode_ai_entity_v2};
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
        println!("Updated golden vector: {:?}", path);
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
        println!("Updated golden vector: {:?}", path);
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
