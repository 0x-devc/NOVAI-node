//! Canonical encoding for AI entity types.
//!
//! Supports three versions:
//! - V1 (0x01): 203 bytes, no is_active field (legacy)
//! - V2 (0x02): 204 bytes, includes is_active field
//! - V3 (0x03): 236 bytes, includes pubkey field (current)
//!
//! Decoding auto-detects version. V1/V2 entities decode with pubkey = [0u8; 32].

use novai_ai_entities::{AiEntity, AutonomyMode, Capabilities, CodeHash};
use novai_types::Address;

/// Version byte for AiEntity encoding V1 (legacy, no is_active).
pub const AI_ENTITY_V1: u8 = 0x01;

/// Version byte for AiEntity encoding V2 (includes is_active).
pub const AI_ENTITY_V2: u8 = 0x02;

/// Version byte for AiEntity encoding V3 (current, includes pubkey).
pub const AI_ENTITY_V3: u8 = 0x03;

/// Encoded size of AiEntity v1 in bytes (legacy).
pub const AI_ENTITY_V1_SIZE: usize = 203;

/// Encoded size of AiEntity v2 in bytes.
pub const AI_ENTITY_V2_SIZE: usize = 204;

/// Encoded size of AiEntity v3 in bytes (current).
pub const AI_ENTITY_V3_SIZE: usize = 236;

/// Errors during AI entity encoding/decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiEntityCodecError {
    /// Input buffer is too short.
    BufferTooShort,
    /// Unknown or unsupported version byte.
    InvalidVersion(u8),
    /// Invalid autonomy mode value.
    InvalidAutonomyMode(u8),
}

impl std::fmt::Display for AiEntityCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiEntityCodecError::BufferTooShort => write!(f, "buffer too short"),
            AiEntityCodecError::InvalidVersion(v) => write!(f, "invalid version: {v}"),
            AiEntityCodecError::InvalidAutonomyMode(m) => {
                write!(f, "invalid autonomy mode: {m}")
            }
        }
    }
}

impl std::error::Error for AiEntityCodecError {}

/// Encode an AiEntity to canonical bytes (V2 format).
///
/// # Layout (204 bytes, little-endian integers)
///
/// | Offset | Size | Field            |
/// |--------|------|------------------|
/// | 0      | 1    | version (0x02)   |
/// | 1      | 32   | id               |
/// | 33     | 32   | code_hash        |
/// | 65     | 32   | creator          |
/// | 97     | 1    | autonomy_mode    |
/// | 98     | 1    | capabilities     |
/// | 99     | 16   | economic_balance |
/// | 115    | 8    | nonce            |
/// | 123    | 32   | memory_root      |
/// | 155    | 32   | params_root      |
/// | 187    | 8    | registered_at    |
/// | 195    | 8    | last_active_at   |
/// | 203    | 1    | is_active        |
pub fn encode_ai_entity_v2(entity: &AiEntity) -> Vec<u8> {
    let mut buf = Vec::with_capacity(AI_ENTITY_V2_SIZE);

    // Version (V2)
    buf.push(AI_ENTITY_V2);

    // Fixed-size byte arrays
    buf.extend_from_slice(&entity.id);
    buf.extend_from_slice(&entity.code_hash);
    buf.extend_from_slice(&entity.creator);

    // Enums/flags as single bytes
    buf.push(entity.autonomy_mode.to_byte());
    buf.push(entity.capabilities.to_byte());

    // Integers in little-endian
    buf.extend_from_slice(&entity.economic_balance.to_le_bytes());
    buf.extend_from_slice(&entity.nonce.to_le_bytes());

    // More fixed-size byte arrays
    buf.extend_from_slice(&entity.memory_root);
    buf.extend_from_slice(&entity.params_root);

    // More integers in little-endian
    buf.extend_from_slice(&entity.registered_at.to_le_bytes());
    buf.extend_from_slice(&entity.last_active_at.to_le_bytes());

    // V2: is_active flag
    buf.push(u8::from(entity.is_active));

    debug_assert_eq!(buf.len(), AI_ENTITY_V2_SIZE);
    buf
}

/// Encode an AiEntity to canonical bytes (V3 format, current).
///
/// # Layout (236 bytes, little-endian integers)
///
/// | Offset | Size | Field            |
/// |--------|------|------------------|
/// | 0      | 1    | version (0x03)   |
/// | 1      | 32   | id               |
/// | 33     | 32   | code_hash        |
/// | 65     | 32   | creator          |
/// | 97     | 1    | autonomy_mode    |
/// | 98     | 1    | capabilities     |
/// | 99     | 16   | economic_balance |
/// | 115    | 8    | nonce            |
/// | 123    | 32   | pubkey           |
/// | 155    | 32   | memory_root      |
/// | 187    | 32   | params_root      |
/// | 219    | 8    | registered_at    |
/// | 227    | 8    | last_active_at   |
/// | 235    | 1    | is_active        |
pub fn encode_ai_entity_v3(entity: &AiEntity) -> Vec<u8> {
    let mut buf = Vec::with_capacity(AI_ENTITY_V3_SIZE);

    buf.push(AI_ENTITY_V3);
    buf.extend_from_slice(&entity.id);
    buf.extend_from_slice(&entity.code_hash);
    buf.extend_from_slice(&entity.creator);
    buf.push(entity.autonomy_mode.to_byte());
    buf.push(entity.capabilities.to_byte());
    buf.extend_from_slice(&entity.economic_balance.to_le_bytes());
    buf.extend_from_slice(&entity.nonce.to_le_bytes());
    buf.extend_from_slice(&entity.pubkey);
    buf.extend_from_slice(&entity.memory_root);
    buf.extend_from_slice(&entity.params_root);
    buf.extend_from_slice(&entity.registered_at.to_le_bytes());
    buf.extend_from_slice(&entity.last_active_at.to_le_bytes());
    buf.push(u8::from(entity.is_active));

    debug_assert_eq!(buf.len(), AI_ENTITY_V3_SIZE);
    buf
}

/// Encode an AiEntity to canonical bytes (V1 format, legacy).
///
/// # Layout (203 bytes, little-endian integers)
///
/// | Offset | Size | Field            |
/// |--------|------|------------------|
/// | 0      | 1    | version (0x01)   |
/// | 1      | 32   | id               |
/// | 33     | 32   | code_hash        |
/// | 65     | 32   | creator          |
/// | 97     | 1    | autonomy_mode    |
/// | 98     | 1    | capabilities     |
/// | 99     | 16   | economic_balance |
/// | 115    | 8    | nonce            |
/// | 123    | 32   | memory_root      |
/// | 155    | 32   | params_root      |
/// | 187    | 8    | registered_at    |
/// | 195    | 8    | last_active_at   |
#[deprecated(note = "Use encode_ai_entity_v2 for new entities")]
pub fn encode_ai_entity_v1(entity: &AiEntity) -> Vec<u8> {
    let mut buf = Vec::with_capacity(AI_ENTITY_V1_SIZE);

    buf.push(AI_ENTITY_V1);
    buf.extend_from_slice(&entity.id);
    buf.extend_from_slice(&entity.code_hash);
    buf.extend_from_slice(&entity.creator);
    buf.push(entity.autonomy_mode.to_byte());
    buf.push(entity.capabilities.to_byte());
    buf.extend_from_slice(&entity.economic_balance.to_le_bytes());
    buf.extend_from_slice(&entity.nonce.to_le_bytes());
    buf.extend_from_slice(&entity.memory_root);
    buf.extend_from_slice(&entity.params_root);
    buf.extend_from_slice(&entity.registered_at.to_le_bytes());
    buf.extend_from_slice(&entity.last_active_at.to_le_bytes());

    debug_assert_eq!(buf.len(), AI_ENTITY_V1_SIZE);
    buf
}

/// Decode an AiEntity from canonical bytes.
///
/// Supports both V1 and V2 formats:
/// - V1 (0x01): 203 bytes, is_active defaults to true
/// - V2 (0x02): 204 bytes, is_active from encoded byte
///
/// # Errors
///
/// Returns error if buffer is too short, version is unsupported,
/// or autonomy mode is invalid.
pub fn decode_ai_entity(input: &[u8]) -> Result<AiEntity, AiEntityCodecError> {
    if input.is_empty() {
        return Err(AiEntityCodecError::BufferTooShort);
    }

    let version = input[0];
    match version {
        AI_ENTITY_V1 => decode_ai_entity_v1_impl(input),
        AI_ENTITY_V2 => decode_ai_entity_v2_impl(input),
        AI_ENTITY_V3 => decode_ai_entity_v3_impl(input),
        _ => Err(AiEntityCodecError::InvalidVersion(version)),
    }
}

/// Decode V1 format (legacy, is_active = true).
fn decode_ai_entity_v1_impl(input: &[u8]) -> Result<AiEntity, AiEntityCodecError> {
    if input.len() < AI_ENTITY_V1_SIZE {
        return Err(AiEntityCodecError::BufferTooShort);
    }

    let mut cursor = 1; // Skip version byte

    // id: [u8; 32]
    let mut id = [0u8; 32];
    id.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    // code_hash: [u8; 32]
    let mut code_hash: CodeHash = [0u8; 32];
    code_hash.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    // creator: [u8; 32]
    let mut creator: Address = [0u8; 32];
    creator.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    // autonomy_mode: u8
    let autonomy_byte = input[cursor];
    cursor += 1;
    let autonomy_mode = AutonomyMode::from_byte(autonomy_byte)
        .ok_or(AiEntityCodecError::InvalidAutonomyMode(autonomy_byte))?;

    // capabilities: u8
    let capabilities = Capabilities::from_byte(input[cursor]);
    cursor += 1;

    // economic_balance: u128 (little-endian)
    let economic_balance = u128::from_le_bytes(
        input[cursor..cursor + 16]
            .try_into()
            .expect("slice is 16 bytes"),
    );
    cursor += 16;

    // nonce: u64 (little-endian)
    let nonce = u64::from_le_bytes(
        input[cursor..cursor + 8]
            .try_into()
            .expect("slice is 8 bytes"),
    );
    cursor += 8;

    // memory_root: [u8; 32]
    let mut memory_root = [0u8; 32];
    memory_root.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    // params_root: [u8; 32]
    let mut params_root = [0u8; 32];
    params_root.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    // registered_at: u64 (little-endian)
    let registered_at = u64::from_le_bytes(
        input[cursor..cursor + 8]
            .try_into()
            .expect("slice is 8 bytes"),
    );
    cursor += 8;

    // last_active_at: u64 (little-endian)
    let last_active_at = u64::from_le_bytes(
        input[cursor..cursor + 8]
            .try_into()
            .expect("slice is 8 bytes"),
    );

    Ok(AiEntity {
        id,
        code_hash,
        creator,
        autonomy_mode,
        capabilities,
        economic_balance,
        nonce,
        pubkey: [0u8; 32], // V1 backward compat: no key
        memory_root,
        params_root,
        registered_at,
        last_active_at,
        is_active: true, // V1 backward compat: default to active
        // V1→V4 promotion: reputation defaults
        reputation_score: novai_ai_entities::DEFAULT_REPUTATION_SCORE,
        total_transactions: 0,
        reputation_events_count: 0,
    })
}

/// Decode V2 format (includes is_active).
fn decode_ai_entity_v2_impl(input: &[u8]) -> Result<AiEntity, AiEntityCodecError> {
    if input.len() < AI_ENTITY_V2_SIZE {
        return Err(AiEntityCodecError::BufferTooShort);
    }

    let mut cursor = 1; // Skip version byte

    // id: [u8; 32]
    let mut id = [0u8; 32];
    id.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    // code_hash: [u8; 32]
    let mut code_hash: CodeHash = [0u8; 32];
    code_hash.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    // creator: [u8; 32]
    let mut creator: Address = [0u8; 32];
    creator.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    // autonomy_mode: u8
    let autonomy_byte = input[cursor];
    cursor += 1;
    let autonomy_mode = AutonomyMode::from_byte(autonomy_byte)
        .ok_or(AiEntityCodecError::InvalidAutonomyMode(autonomy_byte))?;

    // capabilities: u8
    let capabilities = Capabilities::from_byte(input[cursor]);
    cursor += 1;

    // economic_balance: u128 (little-endian)
    let economic_balance = u128::from_le_bytes(
        input[cursor..cursor + 16]
            .try_into()
            .expect("slice is 16 bytes"),
    );
    cursor += 16;

    // nonce: u64 (little-endian)
    let nonce = u64::from_le_bytes(
        input[cursor..cursor + 8]
            .try_into()
            .expect("slice is 8 bytes"),
    );
    cursor += 8;

    // memory_root: [u8; 32]
    let mut memory_root = [0u8; 32];
    memory_root.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    // params_root: [u8; 32]
    let mut params_root = [0u8; 32];
    params_root.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    // registered_at: u64 (little-endian)
    let registered_at = u64::from_le_bytes(
        input[cursor..cursor + 8]
            .try_into()
            .expect("slice is 8 bytes"),
    );
    cursor += 8;

    // last_active_at: u64 (little-endian)
    let last_active_at = u64::from_le_bytes(
        input[cursor..cursor + 8]
            .try_into()
            .expect("slice is 8 bytes"),
    );
    cursor += 8;

    // V2: is_active
    let is_active = input[cursor] != 0;

    Ok(AiEntity {
        id,
        code_hash,
        creator,
        autonomy_mode,
        capabilities,
        economic_balance,
        nonce,
        pubkey: [0u8; 32], // V2 backward compat: no key
        memory_root,
        params_root,
        registered_at,
        last_active_at,
        is_active,
        // V2→V4 promotion: reputation defaults
        reputation_score: novai_ai_entities::DEFAULT_REPUTATION_SCORE,
        total_transactions: 0,
        reputation_events_count: 0,
    })
}

/// Decode V3 format (includes pubkey + is_active).
fn decode_ai_entity_v3_impl(input: &[u8]) -> Result<AiEntity, AiEntityCodecError> {
    if input.len() < AI_ENTITY_V3_SIZE {
        return Err(AiEntityCodecError::BufferTooShort);
    }

    let mut cursor = 1; // Skip version byte

    let mut id = [0u8; 32];
    id.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    let mut code_hash: CodeHash = [0u8; 32];
    code_hash.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    let mut creator: Address = [0u8; 32];
    creator.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    let autonomy_byte = input[cursor];
    cursor += 1;
    let autonomy_mode = AutonomyMode::from_byte(autonomy_byte)
        .ok_or(AiEntityCodecError::InvalidAutonomyMode(autonomy_byte))?;

    let capabilities = Capabilities::from_byte(input[cursor]);
    cursor += 1;

    let economic_balance = u128::from_le_bytes(
        input[cursor..cursor + 16]
            .try_into()
            .expect("slice is 16 bytes"),
    );
    cursor += 16;

    let nonce = u64::from_le_bytes(
        input[cursor..cursor + 8]
            .try_into()
            .expect("slice is 8 bytes"),
    );
    cursor += 8;

    // V3: pubkey
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    let mut memory_root = [0u8; 32];
    memory_root.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    let mut params_root = [0u8; 32];
    params_root.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    let registered_at = u64::from_le_bytes(
        input[cursor..cursor + 8]
            .try_into()
            .expect("slice is 8 bytes"),
    );
    cursor += 8;

    let last_active_at = u64::from_le_bytes(
        input[cursor..cursor + 8]
            .try_into()
            .expect("slice is 8 bytes"),
    );
    cursor += 8;

    let is_active = input[cursor] != 0;

    Ok(AiEntity {
        id,
        code_hash,
        creator,
        autonomy_mode,
        capabilities,
        economic_balance,
        nonce,
        pubkey,
        memory_root,
        params_root,
        registered_at,
        last_active_at,
        is_active,
        // V3→V4 promotion: reputation defaults
        reputation_score: novai_ai_entities::DEFAULT_REPUTATION_SCORE,
        total_transactions: 0,
        reputation_events_count: 0,
    })
}

/// Backward-compatible alias for decode_ai_entity.
///
/// Decodes both V1 and V2 formats.
#[deprecated(note = "Use decode_ai_entity which handles all versions")]
pub fn decode_ai_entity_v1(input: &[u8]) -> Result<AiEntity, AiEntityCodecError> {
    decode_ai_entity(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_entity() -> AiEntity {
        AiEntity::new(
            [0x42u8; 32],
            [0x01u8; 32],
            AutonomyMode::Gated,
            Capabilities::gated(),
            1000,
        )
    }

    #[test]
    fn encode_v2_produces_correct_size() {
        let entity = test_entity();
        let encoded = encode_ai_entity_v2(&entity);
        assert_eq!(encoded.len(), AI_ENTITY_V2_SIZE);
    }

    #[test]
    fn encode_v2_starts_with_version() {
        let entity = test_entity();
        let encoded = encode_ai_entity_v2(&entity);
        assert_eq!(encoded[0], AI_ENTITY_V2);
    }

    #[test]
    fn roundtrip_v2_works() {
        let entity = test_entity();
        let encoded = encode_ai_entity_v2(&entity);
        let decoded = decode_ai_entity(&encoded).unwrap();

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
    fn roundtrip_v2_preserves_is_active_false() {
        let mut entity = test_entity();
        entity.is_active = false;

        let encoded = encode_ai_entity_v2(&entity);
        let decoded = decode_ai_entity(&encoded).unwrap();

        assert!(!decoded.is_active, "is_active=false must be preserved");
    }

    #[test]
    fn roundtrip_v2_preserves_is_active_true() {
        let mut entity = test_entity();
        entity.is_active = true;

        let encoded = encode_ai_entity_v2(&entity);
        let decoded = decode_ai_entity(&encoded).unwrap();

        assert!(decoded.is_active, "is_active=true must be preserved");
    }

    #[test]
    #[allow(deprecated)]
    fn v1_backward_compat_decodes_as_active() {
        let entity = test_entity();
        let encoded = encode_ai_entity_v1(&entity);

        assert_eq!(encoded[0], AI_ENTITY_V1);
        assert_eq!(encoded.len(), AI_ENTITY_V1_SIZE);

        let decoded = decode_ai_entity(&encoded).unwrap();
        assert!(
            decoded.is_active,
            "V1 entities must decode with is_active=true"
        );
    }

    #[test]
    fn encoding_is_deterministic() {
        let entity = test_entity();
        let encoded1 = encode_ai_entity_v2(&entity);
        let encoded2 = encode_ai_entity_v2(&entity);
        assert_eq!(encoded1, encoded2, "Encoding must be deterministic");
    }

    #[test]
    fn decode_rejects_short_buffer() {
        // Use valid version byte (V2) but insufficient length
        let mut short = vec![0u8; 100];
        short[0] = AI_ENTITY_V2;
        let result = decode_ai_entity(&short);
        assert_eq!(result, Err(AiEntityCodecError::BufferTooShort));
    }

    #[test]
    fn decode_rejects_invalid_version() {
        let mut encoded = encode_ai_entity_v2(&test_entity());
        encoded[0] = 0xFF; // Invalid version
        let result = decode_ai_entity(&encoded);
        assert_eq!(result, Err(AiEntityCodecError::InvalidVersion(0xFF)));
    }

    #[test]
    fn decode_rejects_invalid_autonomy_mode() {
        let mut encoded = encode_ai_entity_v2(&test_entity());
        encoded[97] = 0xFF; // Invalid autonomy mode at offset 97
        let result = decode_ai_entity(&encoded);
        assert_eq!(result, Err(AiEntityCodecError::InvalidAutonomyMode(0xFF)));
    }

    #[test]
    fn roundtrip_with_nonzero_values() {
        let mut entity = test_entity();
        entity.economic_balance = 1_000_000_000_000;
        entity.nonce = 42;
        entity.memory_root = [0xAA; 32];
        entity.params_root = [0xBB; 32];
        entity.is_active = false;

        let encoded = encode_ai_entity_v2(&entity);
        let decoded = decode_ai_entity(&encoded).unwrap();

        assert_eq!(entity.economic_balance, decoded.economic_balance);
        assert_eq!(entity.nonce, decoded.nonce);
        assert_eq!(entity.memory_root, decoded.memory_root);
        assert_eq!(entity.params_root, decoded.params_root);
        assert_eq!(entity.is_active, decoded.is_active);
    }

    #[test]
    fn decode_empty_buffer_returns_error() {
        let result = decode_ai_entity(&[]);
        assert_eq!(result, Err(AiEntityCodecError::BufferTooShort));
    }
}
