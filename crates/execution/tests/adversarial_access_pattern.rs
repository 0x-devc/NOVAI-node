//! Week 26: A26.3 Access Pattern Analysis Attack Tests.
//!
//! PURPOSE: Test whether monitoring AI entity key queries can reveal
//! private information. An attacker with chain visibility observes which
//! keys an AI entity reads and attempts to infer private data.
//!
//! ATTACK VECTORS:
//! - AI entity queries various NNPX key sub-paths to enumerate private data
//! - AI entity uses key prefix manipulation to bypass NNPX boundary
//! - Audit log entries leak query content or private data
//! - Derived view schemas expose individual records instead of aggregates
//! - AI entity forges capability bits to gain derived view access
//!
//! EXPECTED RESULTS:
//! - AI entities blocked from ALL nnpx/ key sub-paths
//! - Audit log records only (entity_id, height) → view_id, no data content
//! - Derived view schemas only expose aggregates, not individual records
//! - Capability forgery detected and rejected
//!
//! MITIGATION: AI cannot query NNPX keys (hard boundary in execution layer).

#![allow(clippy::doc_markdown)]

use novai_ai_entities::{
    ActivityCountData, AggregateVolumeData, AiEntity, AutonomyMode, Capabilities,
    DerivedSourceType, DerivedView, DerivedViewSchema, PoolSizeData,
};
use novai_execution::{
    create_derived_view_audit_entry, is_derived_view, is_private_key, read_derived_view_with_audit,
    validate_derived_view_access, validate_nnpx_access, write_derived_view_ops, Caller, ExecError,
};
use novai_state::{
    derived_view_audit_key, KvBatch, MemKv, WriteOp, KEY_PREFIX_NNPX, KEY_PREFIX_NNPX_COMMITMENTS,
    KEY_PREFIX_NNPX_ENCRYPTED, KEY_PREFIX_NNPX_NULLIFIERS,
};

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Create an AI entity WITH derived view read capability.
fn entity_with_derived_cap() -> AiEntity {
    let caps = Capabilities {
        read_nnpx_derived: true,
        read_public_chain: true,
        read_memory_objects: true,
        emit_proposals: true,
        request_execution: true,
        _reserved: [false; 3],
    };
    AiEntity::new([0xAAu8; 32], [0xBBu8; 32], AutonomyMode::Gated, caps, 1000)
}

/// Create an AI entity WITHOUT derived view read capability.
fn entity_without_derived_cap() -> AiEntity {
    AiEntity::new(
        [0xCCu8; 32],
        [0xDDu8; 32],
        AutonomyMode::Gated,
        Capabilities::gated(),
        1000,
    )
}

/// Create and store a test derived view, returning (view, view_id).
fn store_test_view(db: &mut MemKv, schema_id: u32, creator: [u8; 32], height: u64) -> DerivedView {
    let data: Vec<u8> = match schema_id {
        1 => AggregateVolumeData {
            start_height: 100,
            end_height: 200,
            total_volume: 1_000_000,
        }
        .encode(),
        2 => ActivityCountData {
            start_height: 100,
            end_height: 200,
            tx_count: 500,
        }
        .encode(),
        3 => PoolSizeData {
            snapshot_height: 200,
            pool_size: 5_000_000,
        }
        .encode(),
        _ => panic!("Unknown schema_id {schema_id}"),
    };

    let view = DerivedView::new(
        DerivedSourceType::ChainAggregate,
        schema_id,
        height,
        creator,
        data,
    )
    .expect("Valid derived view");

    let ops = write_derived_view_ops(&view);
    db.apply_batch(&ops).unwrap();

    view
}

// ============================================================================
// A26.3-T1: AI CANNOT QUERY ANY NNPX PREFIX
// ============================================================================

#[test]
fn test_ai_cannot_query_any_nnpx_prefix() {
    // ATTACK: AI entity attempts to read every known NNPX key sub-path
    // to enumerate private data (commitments, nullifiers, encrypted payloads).
    //
    // EXPECTED: Every access attempt returns NnpxAccessDenied.

    let ai_caller = Caller::AiEntity([0x42u8; 32]);

    // Exhaustive list of NNPX key prefixes and example keys
    let nnpx_keys: Vec<Vec<u8>> = vec![
        // Bare prefix
        KEY_PREFIX_NNPX.to_vec(),
        // Commitments
        KEY_PREFIX_NNPX_COMMITMENTS.to_vec(),
        [KEY_PREFIX_NNPX_COMMITMENTS, &[0xABu8; 32]].concat(),
        // Nullifiers
        KEY_PREFIX_NNPX_NULLIFIERS.to_vec(),
        [KEY_PREFIX_NNPX_NULLIFIERS, &[0xCDu8; 32]].concat(),
        // Encrypted payloads
        KEY_PREFIX_NNPX_ENCRYPTED.to_vec(),
        [KEY_PREFIX_NNPX_ENCRYPTED, &[0xEFu8; 32]].concat(),
        // Hypothetical future sub-paths
        b"nnpx/proofs/zk_proof_1".to_vec(),
        b"nnpx/metadata/tx_meta".to_vec(),
        b"nnpx/indices/user_index".to_vec(),
    ];

    for key in &nnpx_keys {
        let result: Result<(), ExecError<()>> = validate_nnpx_access(key, &ai_caller);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "AI must be denied access to NNPX key: {:?}",
            String::from_utf8_lossy(key),
        );

        // Confirm key is classified as private
        assert!(
            is_private_key(key),
            "Key must be classified as private: {:?}",
            String::from_utf8_lossy(key),
        );
    }

    // Human account can access all the same keys
    let account_caller = Caller::Account([0x01u8; 32]);
    for key in &nnpx_keys {
        let result: Result<(), ExecError<()>> = validate_nnpx_access(key, &account_caller);
        assert!(
            result.is_ok(),
            "Human account must be allowed access to NNPX key: {:?}",
            String::from_utf8_lossy(key),
        );
    }
}

// ============================================================================
// A26.3-T2: AI CANNOT ENUMERATE NNPX KEYS
// ============================================================================

#[test]
fn test_ai_cannot_enumerate_nnpx_keys() {
    // ATTACK: AI entity attempts to enumerate NNPX keys by constructing
    // scan-prefix-style keys. Even if scan_prefix is not directly exposed
    // to AI, the access check on individual keys must block all NNPX paths.
    //
    // EXPECTED: Any key starting with b"nnpx/" is blocked for AI callers,
    // regardless of what follows the prefix.

    let ai_caller = Caller::AiEntity([0x42u8; 32]);

    // Attempt to enumerate by guessing key patterns
    for suffix_byte in 0u8..=255 {
        let mut key = KEY_PREFIX_NNPX.to_vec();
        key.push(suffix_byte);

        let result: Result<(), ExecError<()>> = validate_nnpx_access(&key, &ai_caller);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "AI must be denied access to nnpx/ + byte 0x{suffix_byte:02x}",
        );
    }

    // Attempt with long random suffixes
    for i in 0..20 {
        let mut key = KEY_PREFIX_NNPX.to_vec();
        // Append deterministic "random" bytes
        let suffix: Vec<u8> = (0..64)
            .map(|j: i32| u8::try_from((i * 37 + j * 13) % 256).expect("mod 256 fits in u8"))
            .collect();
        key.extend_from_slice(&suffix);

        let result: Result<(), ExecError<()>> = validate_nnpx_access(&key, &ai_caller);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "AI must be denied access to nnpx/ with random suffix #{i}",
        );
    }
}

// ============================================================================
// A26.3-T3: AI CANNOT READ NNPX VIA KEY PREFIX MANIPULATION
// ============================================================================

#[test]
fn test_ai_cannot_read_nnpx_via_account_key_prefix_manipulation() {
    // ATTACK: AI entity crafts keys that attempt to "escape" the nnpx/
    // namespace using path traversal or similar tricks.
    //
    // EXPECTED: The is_nnpx_key check is a simple starts_with("nnpx/"),
    // so any key starting with that prefix is blocked. Keys NOT starting
    // with that prefix are allowed (they don't access private data).

    let ai_caller = Caller::AiEntity([0x42u8; 32]);

    // Keys that START with nnpx/ are always blocked
    let blocked_keys: Vec<&[u8]> = vec![
        b"nnpx/",
        b"nnpx/commitments/../accounts/alice",
        b"nnpx/../../etc/passwd",
        b"nnpx/\x00\x00\x00",
        b"nnpx/\xff\xff\xff",
        b"nnpx//double/slash",
        b"nnpx/nullifiers/\x00",
    ];

    for key in &blocked_keys {
        let result: Result<(), ExecError<()>> = validate_nnpx_access(key, &ai_caller);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "AI must be denied for key starting with nnpx/: {:?}",
            String::from_utf8_lossy(key),
        );
    }

    // Keys that do NOT start with nnpx/ are allowed (even if they mention "nnpx")
    let allowed_keys: Vec<&[u8]> = vec![
        b"accounts/nnpx/something",      // "nnpx" is in value, not prefix
        b"ai/entities/nnpx_reader",      // mentions nnpx but not prefix
        b"derived_views/nnpx_aggregate", // derived view namespace
        b"nnp",                          // incomplete prefix
        b"nnpx",                         // missing trailing slash
        b"NNPX/commitments/abc",         // wrong case
        b"xnnpx/commitments/abc",        // wrong prefix
        b" nnpx/commitments/abc",        // leading space
    ];

    for key in &allowed_keys {
        let result: Result<(), ExecError<()>> = validate_nnpx_access(key, &ai_caller);
        assert!(
            result.is_ok(),
            "AI should be allowed for non-nnpx key: {:?}",
            String::from_utf8_lossy(key),
        );
    }
}

// ============================================================================
// A26.3-T4: AI READS ONLY REVEAL DERIVED VIEW IDS
// ============================================================================

#[test]
fn test_ai_reads_only_reveal_derived_view_ids() {
    // ATTACK: Monitor the audit log to determine what data an AI entity read.
    // If the audit log leaks view content, an observer could reconstruct
    // private information.
    //
    // EXPECTED: The audit log entry contains ONLY:
    //   Key: derived_views/audit/{entity_id}/{height}
    //   Value: {view_id} (32 bytes)
    // No view content, schema data, or query parameters are recorded.

    let mut db = MemKv::new();
    let entity = entity_with_derived_cap();
    let creator = [0x42u8; 32];

    // Store a view with actual aggregate data
    let view = store_test_view(&mut db, 1, creator, 1000);
    let view_id = view.view_id;

    // Read the view (generates audit entry)
    let (read_view, audit_op) = read_derived_view_with_audit(&db, &entity, &view_id, 5000).unwrap();

    // Verify we got the correct view back
    assert_eq!(read_view.view_id, view_id);

    // Inspect the audit WriteOp
    match audit_op {
        WriteOp::Put(key, value) => {
            // Key format: derived_views/audit/{entity_id32}/{height_be8}
            assert!(
                key.starts_with(b"derived_views/audit/"),
                "Audit key must start with derived_views/audit/"
            );

            // Key contains entity_id
            let entity_id_start = b"derived_views/audit/".len();
            let entity_id_end = entity_id_start + 32;
            assert_eq!(
                &key[entity_id_start..entity_id_end],
                &entity.id,
                "Audit key must contain entity ID"
            );

            // Key contains height
            assert_eq!(key[entity_id_end], b'/');
            let height_bytes = &key[entity_id_end + 1..];
            assert_eq!(height_bytes.len(), 8, "Height must be 8 bytes");
            let recorded_height = u64::from_be_bytes(height_bytes.try_into().unwrap());
            assert_eq!(recorded_height, 5000, "Audit must record correct height");

            // Value is ONLY the view_id (32 bytes) - NO view content
            assert_eq!(
                value.len(),
                32,
                "Audit value must be exactly 32 bytes (view_id only)"
            );
            assert_eq!(
                value.as_slice(),
                &view_id,
                "Audit value must be the view_id"
            );

            // The audit log does NOT contain:
            // - The view's data field (aggregate values)
            // - The view's schema_id
            // - The view's source_type
            // - Any query parameters
        }
        WriteOp::Delete(_) => panic!("Audit must be a Put, not Delete"),
    }
}

// ============================================================================
// A26.3-T5: AUDIT LOG DOES NOT LEAK QUERY CONTENT
// ============================================================================

#[test]
fn test_audit_log_does_not_leak_query_content() {
    // ATTACK: Examine multiple audit log entries to determine if the
    // cumulative pattern reveals query content or frequency details
    // beyond what is intentionally recorded.
    //
    // EXPECTED: Each audit entry is a fixed-size record (32-byte value)
    // that reveals only WHICH view was read and WHEN. No additional
    // metadata is included.

    let entity_id = [0xAAu8; 32];

    // Simulate reading different views at different heights
    let view_ids: Vec<[u8; 32]> = (0..10)
        .map(|i| {
            let mut id = [0u8; 32];
            id[0] = i;
            id
        })
        .collect();

    let heights: Vec<u64> = vec![100, 200, 300, 400, 500, 600, 700, 800, 900, 1000];

    for (view_id, height) in view_ids.iter().zip(heights.iter()) {
        let audit_op = create_derived_view_audit_entry(&entity_id, view_id, *height);

        match audit_op {
            WriteOp::Put(key, value) => {
                // All audit keys have the same structure
                let expected_key = derived_view_audit_key(&entity_id, *height);
                assert_eq!(key, expected_key, "Audit key must match expected format");

                // All audit values are exactly 32 bytes (view_id)
                assert_eq!(
                    value.len(),
                    32,
                    "Audit value must be 32 bytes at height {height}",
                );
                assert_eq!(value.as_slice(), view_id.as_slice());

                // Verify the key is NOT classified as private
                assert!(
                    !is_private_key(&key),
                    "Audit keys must NOT be in NNPX namespace"
                );
                // But it IS a derived view key
                assert!(
                    is_derived_view(&key),
                    "Audit keys must be in derived_views/ namespace"
                );
            }
            WriteOp::Delete(_) => panic!("Audit must be a Put"),
        }
    }
}

// ============================================================================
// A26.3-T6: DERIVED VIEW DATA IS AGGREGATE ONLY
// ============================================================================

#[test]
fn test_derived_view_data_is_aggregate_only() {
    // ATTACK: Inspect derived view schemas to determine if any schema
    // could expose individual records (per-address balances, individual
    // transaction amounts, etc.) rather than aggregates.
    //
    // EXPECTED: All current schemas produce only aggregate data:
    // - AggregateVolume: total_volume (sum), not individual tx amounts
    // - ActivityCount: tx_count (count), not individual tx details
    // - PoolSize: pool_size (total), not individual deposit amounts

    // Schema 1: AggregateVolume
    let vol_data = AggregateVolumeData {
        start_height: 100,
        end_height: 200,
        total_volume: 5_000_000, // Sum of all txs, not individual amounts
    };
    let vol_bytes = vol_data.encode();
    assert_eq!(vol_bytes.len(), 32, "AggregateVolume is exactly 32 bytes");

    // The schema contains:
    //   start_height (8 bytes) - public block range
    //   end_height (8 bytes) - public block range
    //   total_volume (16 bytes) - AGGREGATE sum, not individual
    // NO per-address, per-tx, or per-user data exists.
    let decoded = AggregateVolumeData::decode(&vol_bytes).unwrap();
    assert_eq!(decoded.total_volume, 5_000_000);
    // Cannot determine: how many txs contributed, individual amounts,
    // sender/receiver addresses, or tx types.

    // Schema 2: ActivityCount
    let act_data = ActivityCountData {
        start_height: 100,
        end_height: 200,
        tx_count: 500, // Total count, not per-address
    };
    let act_bytes = act_data.encode();
    assert_eq!(act_bytes.len(), 24, "ActivityCount is exactly 24 bytes");

    let decoded = ActivityCountData::decode(&act_bytes).unwrap();
    assert_eq!(decoded.tx_count, 500);
    // Cannot determine: which addresses were active, how many txs
    // each address made, or the nature of any individual tx.

    // Schema 3: PoolSize
    let pool_data = PoolSizeData {
        snapshot_height: 200,
        pool_size: 10_000_000, // Total pool, not individual deposits
    };
    let pool_bytes = pool_data.encode();
    assert_eq!(pool_bytes.len(), 24, "PoolSize is exactly 24 bytes");

    let decoded = PoolSizeData::decode(&pool_bytes).unwrap();
    assert_eq!(decoded.pool_size, 10_000_000);
    // Cannot determine: how many depositors, individual deposit amounts,
    // or deposit/withdrawal history.

    // Verify schema validation rejects wrong-sized data
    // (prevents smuggling extra fields into a schema)
    assert!(DerivedViewSchema::AggregateVolume.validate_data(&[0u8; 32]));
    assert!(!DerivedViewSchema::AggregateVolume.validate_data(&[0u8; 33]));
    assert!(!DerivedViewSchema::AggregateVolume.validate_data(&[0u8; 64]));

    assert!(DerivedViewSchema::ActivityCount.validate_data(&[0u8; 24]));
    assert!(!DerivedViewSchema::ActivityCount.validate_data(&[0u8; 25]));

    assert!(DerivedViewSchema::PoolSize.validate_data(&[0u8; 24]));
    assert!(!DerivedViewSchema::PoolSize.validate_data(&[0u8; 25]));
}

// ============================================================================
// A26.3-T7: ACCESS PATTERN ACROSS SCHEMAS REVEALS NOTHING
// ============================================================================

#[test]
fn test_access_pattern_across_schemas_reveals_nothing() {
    // ATTACK: AI entity reads views across all schemas in sequence.
    // Check if the combination of aggregate data across schemas could
    // reveal individual private records.
    //
    // EXPECTED: Each schema returns only its aggregate. Even reading all
    // schemas together does not reveal individual transactions.

    let mut db = MemKv::new();
    let entity = entity_with_derived_cap();
    let creator = [0x42u8; 32];

    // Store one view per schema
    let vol_view = store_test_view(&mut db, 1, creator, 1000);
    let act_view = store_test_view(&mut db, 2, creator, 1000);
    let pool_view = store_test_view(&mut db, 3, creator, 1000);

    // AI reads all three schemas
    let (v1, _) = read_derived_view_with_audit(&db, &entity, &vol_view.view_id, 2000).unwrap();
    let (v2, _) = read_derived_view_with_audit(&db, &entity, &act_view.view_id, 2000).unwrap();
    let (v3, _) = read_derived_view_with_audit(&db, &entity, &pool_view.view_id, 2000).unwrap();

    // Decode the data
    let vol = AggregateVolumeData::decode(&v1.data).unwrap();
    let act = ActivityCountData::decode(&v2.data).unwrap();
    let pool = PoolSizeData::decode(&v3.data).unwrap();

    // The AI now knows:
    //   - Total volume in block range [100, 200]: 1,000,000
    //   - Total tx count in block range [100, 200]: 500
    //   - Total pool size at height 200: 5,000,000
    //
    // It can compute: average tx size = 1,000,000 / 500 = 2,000
    //
    // But it CANNOT determine:
    //   - Individual transaction amounts
    //   - Which addresses were involved
    //   - The distribution of transaction sizes
    //   - Any individual deposit or withdrawal
    //
    // The average is a statistical aggregate that reveals no individual data.
    let avg = vol.total_volume / u128::from(act.tx_count);
    assert_eq!(avg, 2000, "Average is a harmless aggregate");

    // Verify each view returns data of the correct schema size only
    assert_eq!(v1.data.len(), 32); // AggregateVolume
    assert_eq!(v2.data.len(), 24); // ActivityCount
    assert_eq!(v3.data.len(), 24); // PoolSize

    // Verify pool size is independent of volume (different aggregate)
    assert_ne!(
        pool.pool_size, vol.total_volume,
        "Pool size and volume are independent aggregates"
    );
}

// ============================================================================
// A26.3-T8: AI CANNOT READ DERIVED VIEW WITH FORGED CAPABILITY
// ============================================================================

#[test]
fn test_ai_cannot_read_derived_view_with_forged_capability() {
    // ATTACK: AI entity with read_nnpx_derived=false attempts to read
    // derived views by various means (direct access, capability manipulation).
    //
    // EXPECTED: Access is denied at the validation layer. The capability
    // check reads from the entity struct, not from user-supplied data.

    let mut db = MemKv::new();
    let entity_no_cap = entity_without_derived_cap();
    let creator = [0x42u8; 32];

    // Store a view
    let view = store_test_view(&mut db, 1, creator, 1000);

    // Attempt 1: Direct read without capability
    let result = read_derived_view_with_audit(&db, &entity_no_cap, &view.view_id, 2000);
    assert!(
        matches!(result, Err(ExecError::DerivedViewAccessDenied)),
        "Entity without capability must be denied"
    );

    // Attempt 2: Entity with all OTHER capabilities but NOT read_nnpx_derived
    let max_caps_no_derived = Capabilities {
        read_public_chain: true,
        read_memory_objects: true,
        emit_proposals: true,
        request_execution: true,
        read_nnpx_derived: false, // The critical one is OFF
        _reserved: [false; 3],
    };
    let entity_max_no_derived = AiEntity::new(
        [0xEEu8; 32],
        [0xFFu8; 32],
        AutonomyMode::Autonomous,
        max_caps_no_derived,
        1000,
    );

    let result = read_derived_view_with_audit(&db, &entity_max_no_derived, &view.view_id, 2000);
    assert!(
        matches!(result, Err(ExecError::DerivedViewAccessDenied)),
        "Entity with all caps EXCEPT read_nnpx_derived must be denied"
    );

    // Attempt 3: Validate that capability byte encoding is correct
    // (ensure bit 4 being off means read_nnpx_derived=false)
    let caps_byte = max_caps_no_derived.to_byte();
    assert_eq!(
        caps_byte & (1 << 4),
        0,
        "Bit 4 must be 0 when read_nnpx_derived=false"
    );

    // Attempt 4: Decode from a byte with bit 4 set → read_nnpx_derived=true
    let forged_byte = caps_byte | (1 << 4);
    let forged_caps = Capabilities::from_byte(forged_byte);
    assert!(forged_caps.read_nnpx_derived, "Forged byte has bit 4 set");

    // But the entity's capability is what matters, not the forged byte.
    // The entity was created with read_nnpx_derived=false, so it stays denied.
    let result: Result<(), ExecError<()>> = validate_derived_view_access(&entity_max_no_derived);
    assert!(
        matches!(result, Err(ExecError::DerivedViewAccessDenied)),
        "Entity struct capability is authoritative, not external bytes"
    );

    // Attempt 5: Entity with capability=true CAN read (positive control)
    let entity_with_cap = entity_with_derived_cap();
    let result = read_derived_view_with_audit(&db, &entity_with_cap, &view.view_id, 2000);
    assert!(
        result.is_ok(),
        "Entity WITH capability should succeed (positive control)"
    );
}
