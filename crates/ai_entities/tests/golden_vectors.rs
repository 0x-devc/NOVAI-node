//! Golden vector tests for AI entity encoding stability.
//!
//! Run with UPDATE_VECTORS=1 to regenerate vectors:
//! ```
//! UPDATE_VECTORS=1 cargo test -p novai-ai-entities --test golden_vectors
//! ```

use novai_ai_entities::{AiEntity, AutonomyMode, Capabilities};
// Golden vector tests intentionally use v1 codec to verify backward compatibility
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
