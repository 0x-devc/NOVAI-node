#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]

//! Integration tests for the Agent Discovery Registry create flow
//! (Week 29, Phase 2).
//!
//! Each test publishes a `ServiceDescriptor` memory object through the
//! normal `CreateMemoryObject` signal path and verifies the per-type
//! validation rules + the by-category index that backs discovery RPC
//! queries (Phase 5). The set covers:
//!
//! - Happy path: descriptor lands in both the by_type and by_category
//!   indexes, the memory object is decodable, and the entity's memory
//!   count is incremented.
//! - Per-entity cap: 16th descriptor succeeds, 17th is rejected with
//!   `ServiceDescriptorLimitExceeded`.
//! - Validation rejections: bad category (above RESERVED_MAX), bad
//!   status (above STATUS_MAX), reputation requirement above
//!   `MAX_REPUTATION_SCORE`, non-zero `reserved` bytes, bad version
//!   byte, bad length.

use novai_ai_entities::{
    AiEntity, AutonomyMode, Capabilities, MemoryObjectType, ServiceDescriptorData,
    MAX_REPUTATION_SCORE, MAX_SERVICE_DESCRIPTORS_PER_ENTITY, SERVICE_CATEGORY_DATA_ORACLE,
    SERVICE_CATEGORY_INFERENCE, SERVICE_CATEGORY_RESERVED_MAX, SERVICE_DESCRIPTOR_SIZE,
    SERVICE_DESCRIPTOR_V1, SERVICE_STATUS_ACTIVE, SERVICE_STATUS_MAX,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_update_memory_object_tx,
    encode_create_memory_object_payload_v1, encode_update_memory_object_payload_v1,
    service_descriptor_by_category_key, write_ai_entity_op, CreateMemoryObjectPayloadV1, ExecError,
    UpdateMemoryObjectPayloadV1, KEY_PREFIX_AI_SERVICE_DESCRIPTORS_BY_CATEGORY,
};
use novai_state::{
    ai_entity_by_address_key, ai_memory_by_type_key, ai_memory_object_key, Kv, KvBatch, MemKv,
    WriteOp,
};
use novai_types::{TxV1, TxVersion};

const PUBLISHER_BALANCE: u128 = 1_000_000;
const CREATE_FEE: u64 = 1_000;
const HEIGHT: u64 = 500;

// ============================================================================
// Helpers
// ============================================================================

fn publisher_caps() -> Capabilities {
    // The CreateMemoryObject handler gates on `read_memory_objects`;
    // emit_proposals is needed for the wider memory-object tx dispatch.
    Capabilities {
        read_public_chain: true,
        read_memory_objects: true,
        emit_proposals: true,
        request_execution: false,
        read_nnpx_derived: false,
        submit_reputation_updates: false,
        _reserved: [false; 2],
    }
}

fn build_entity(code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    AiEntity::new(
        code_hash,
        creator,
        AutonomyMode::Gated,
        publisher_caps(),
        1000,
    )
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[
        write_ai_entity_op(entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
}

fn make_publisher(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut publisher = build_entity(code_hash, creator);
    publisher.economic_balance = PUBLISHER_BALANCE;
    store_entity(db, &publisher);
    publisher
}

fn sample_descriptor(category: u8, price: u64) -> ServiceDescriptorData {
    ServiceDescriptorData {
        version: SERVICE_DESCRIPTOR_V1,
        service_name_hash: [0xA1u8; 32],
        service_url_hash: [0xA2u8; 32],
        description_hash: [0xA3u8; 32],
        category,
        price_per_call: price,
        subscription_rate_per_block: 0,
        min_reputation_score: 50,
        min_stake: 1_000_000,
        capability_tags: 0x0F,
        status: SERVICE_STATUS_ACTIVE,
        reserved: [0u8; 7],
    }
}

fn make_create_tx(publisher: &AiEntity, nonce: u64, descriptor: &ServiceDescriptorData) -> TxV1 {
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ServiceDescriptor,
        data: descriptor.encode().to_vec(),
    });
    TxV1 {
        version: TxVersion::V1,
        from: publisher.id,
        pubkey: publisher.id,
        nonce,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    }
}

fn make_update_tx(
    publisher: &AiEntity,
    nonce: u64,
    object_id: [u8; 32],
    new_data: Vec<u8>,
) -> TxV1 {
    let payload = encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
        object_id,
        new_data,
    });
    TxV1 {
        version: TxVersion::V1,
        from: publisher.id,
        pubkey: publisher.id,
        nonce,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    }
}

fn publish_descriptor(
    db: &mut MemKv,
    publisher: &AiEntity,
    nonce: u64,
    descriptor: &ServiceDescriptorData,
) -> [u8; 32] {
    let tx = make_create_tx(publisher, nonce, descriptor);
    apply_create_memory_object_tx(db, &tx, HEIGHT).expect("publish succeeds")
}

fn read_descriptor(db: &MemKv, publisher: &AiEntity, object_id: &[u8; 32]) -> ServiceDescriptorData {
    let envelope_bytes = db
        .get(&ai_memory_object_key(&publisher.id, object_id))
        .unwrap()
        .unwrap();
    let payload_start = envelope_bytes.len() - SERVICE_DESCRIPTOR_SIZE;
    ServiceDescriptorData::decode(&envelope_bytes[payload_start..]).expect("stored bytes decode")
}

// ============================================================================
// 1. Happy path: descriptor lands in BOTH indexes
// ============================================================================

#[test]
fn service_descriptor_publish_lands_in_by_category_index() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let descriptor = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    let tx = make_create_tx(&publisher, 0, &descriptor);
    let object_id = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).expect("publish succeeds");

    // by_category index entry is present.
    let category_key = service_descriptor_by_category_key(
        SERVICE_CATEGORY_DATA_ORACLE,
        &publisher.id,
        &object_id,
    );
    assert!(
        db.get(&category_key).unwrap().is_some(),
        "by_category index entry must exist after publish"
    );

    // Prefix scan with just (prefix || category) returns exactly one entry.
    let mut category_prefix = Vec::new();
    category_prefix.extend_from_slice(KEY_PREFIX_AI_SERVICE_DESCRIPTORS_BY_CATEGORY);
    category_prefix.push(SERVICE_CATEGORY_DATA_ORACLE);
    let entries = db.scan_prefix(&category_prefix).unwrap();
    assert_eq!(entries.len(), 1, "exactly one entry in this category");

    // The key body layout matches `category[1] || owner[32] || object_id[32]`.
    let body = &entries[0].0[KEY_PREFIX_AI_SERVICE_DESCRIPTORS_BY_CATEGORY.len()..];
    assert_eq!(body[0], SERVICE_CATEGORY_DATA_ORACLE);
    assert_eq!(&body[1..33], &publisher.id);
    assert_eq!(&body[33..65], &object_id);
}

#[test]
fn service_descriptor_publish_lands_in_by_type_index() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let descriptor = sample_descriptor(SERVICE_CATEGORY_INFERENCE, 500);
    let tx = make_create_tx(&publisher, 0, &descriptor);
    let object_id = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).expect("publish succeeds");

    // The existing memory-by-type index also carries the new descriptor.
    let type_key = ai_memory_by_type_key(
        MemoryObjectType::ServiceDescriptor.to_byte(),
        &publisher.id,
        &object_id,
    );
    assert!(
        db.get(&type_key).unwrap().is_some(),
        "by_type index entry must exist after publish"
    );

    // Memory object record stores the encoded descriptor bytes verbatim
    // (envelope wraps them; we can read+decode the envelope and pull the
    // 144-byte payload out for verification).
    let primary_key = ai_memory_object_key(&publisher.id, &object_id);
    let envelope_bytes = db.get(&primary_key).unwrap().unwrap();
    // Trailing 144 bytes are the encoded ServiceDescriptorData.
    let payload_start = envelope_bytes.len() - SERVICE_DESCRIPTOR_SIZE;
    let decoded = ServiceDescriptorData::decode(&envelope_bytes[payload_start..])
        .expect("encoded descriptor decodes back");
    assert_eq!(decoded, descriptor);
}

// ============================================================================
// 2. Per-entity cap: 16th OK, 17th rejected
// ============================================================================

#[test]
fn service_descriptor_per_entity_cap_enforced() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x13u8; 32], [0x23u8; 32]);

    // Publish 16 descriptors; vary `price_per_call` so each descriptor
    // hashes to a different object_id.
    for i in 0..MAX_SERVICE_DESCRIPTORS_PER_ENTITY {
        let descriptor = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100 + u64::from(i));
        let tx = make_create_tx(&publisher, u64::from(i), &descriptor);
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT + u64::from(i)).unwrap_or_else(|e| {
            panic!("publish #{i} should succeed, got {e:?}");
        });
    }

    // 17th publish must fail.
    let descriptor = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 9_999);
    let tx = make_create_tx(
        &publisher,
        u64::from(MAX_SERVICE_DESCRIPTORS_PER_ENTITY),
        &descriptor,
    );
    let err = apply_create_memory_object_tx(
        &mut db,
        &tx,
        HEIGHT + u64::from(MAX_SERVICE_DESCRIPTORS_PER_ENTITY),
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::ServiceDescriptorLimitExceeded { current, max }
                if current == MAX_SERVICE_DESCRIPTORS_PER_ENTITY
                    && max == MAX_SERVICE_DESCRIPTORS_PER_ENTITY
        ),
        "got {err:?}"
    );

    // Prefix scan returns exactly 16 entries.
    let mut category_prefix = Vec::new();
    category_prefix.extend_from_slice(KEY_PREFIX_AI_SERVICE_DESCRIPTORS_BY_CATEGORY);
    category_prefix.push(SERVICE_CATEGORY_DATA_ORACLE);
    let entries = db.scan_prefix(&category_prefix).unwrap();
    assert_eq!(entries.len(), MAX_SERVICE_DESCRIPTORS_PER_ENTITY as usize);
}

// ============================================================================
// 3-7. Validation rejections
// ============================================================================

#[test]
fn service_descriptor_invalid_category_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x14u8; 32], [0x24u8; 32]);

    let mut descriptor = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    descriptor.category = SERVICE_CATEGORY_RESERVED_MAX + 1; // 16: governance-only
    let tx = make_create_tx(&publisher, 0, &descriptor);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::ServiceDescriptorInvalidCategory { byte }
                if byte == SERVICE_CATEGORY_RESERVED_MAX + 1
        ),
        "got {err:?}"
    );
}

#[test]
fn service_descriptor_invalid_status_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x15u8; 32], [0x25u8; 32]);

    let mut descriptor = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    descriptor.status = SERVICE_STATUS_MAX + 1; // 3: unknown
    let tx = make_create_tx(&publisher, 0, &descriptor);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::ServiceDescriptorInvalidStatus { byte }
                if byte == SERVICE_STATUS_MAX + 1
        ),
        "got {err:?}"
    );
}

#[test]
fn service_descriptor_reputation_over_max_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x16u8; 32], [0x26u8; 32]);

    let mut descriptor = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    descriptor.min_reputation_score = MAX_REPUTATION_SCORE + 1;
    let tx = make_create_tx(&publisher, 0, &descriptor);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::ServiceDescriptorReputationOverMax { score }
                if score == MAX_REPUTATION_SCORE + 1
        ),
        "got {err:?}"
    );
}

#[test]
fn service_descriptor_non_zero_reserved_bytes_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x17u8; 32], [0x27u8; 32]);

    let mut descriptor = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    descriptor.reserved[3] = 1; // any non-zero byte trips the lock
    let tx = make_create_tx(&publisher, 0, &descriptor);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::InvalidServiceDescriptor),
        "got {err:?}"
    );
}

#[test]
fn service_descriptor_bad_version_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x18u8; 32], [0x28u8; 32]);

    let mut descriptor = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    descriptor.version = SERVICE_DESCRIPTOR_V1 + 1; // not yet defined
    let tx = make_create_tx(&publisher, 0, &descriptor);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::InvalidServiceDescriptor),
        "got {err:?}"
    );
}

#[test]
fn service_descriptor_bad_length_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x19u8; 32], [0x29u8; 32]);

    // Hand-roll a CreateMemoryObject payload with a 143-byte data field
    // (one less than SERVICE_DESCRIPTOR_SIZE). The validator's decode
    // step must reject before any other check fires.
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ServiceDescriptor,
        data: vec![0u8; SERVICE_DESCRIPTOR_SIZE - 1],
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: publisher.id,
        pubkey: publisher.id,
        nonce: 0,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    };
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::InvalidServiceDescriptor),
        "got {err:?}"
    );
}

// ============================================================================
// 8. End-to-end golden bytes: descriptor stored in state matches the
//    Phase 1 wire layout, byte-for-byte.
// ============================================================================

#[test]
fn service_descriptor_publish_record_bytes_match_golden_layout() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x1Au8; 32], [0x2Au8; 32]);

    let descriptor = ServiceDescriptorData {
        version: SERVICE_DESCRIPTOR_V1,
        service_name_hash: [0xB1u8; 32],
        service_url_hash: [0xB2u8; 32],
        description_hash: [0xB3u8; 32],
        category: SERVICE_CATEGORY_INFERENCE,
        price_per_call: 0x0102_0304_0506_0708,
        subscription_rate_per_block: 0x1112_1314_1516_1718,
        min_reputation_score: 75,
        min_stake: 0x2122_2324_2526_2728_2A2B_2C2D_2E2F_3031,
        capability_tags: 0x4142_4344,
        status: SERVICE_STATUS_ACTIVE,
        reserved: [0u8; 7],
    };
    let tx = make_create_tx(&publisher, 0, &descriptor);
    let object_id = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).expect("publish succeeds");

    let envelope_bytes = db
        .get(&ai_memory_object_key(&publisher.id, &object_id))
        .unwrap()
        .unwrap();
    let payload = &envelope_bytes[envelope_bytes.len() - SERVICE_DESCRIPTOR_SIZE..];
    assert_eq!(payload.len(), 144);

    // Lock the byte offsets so a future change to the encoder is caught
    // here in addition to the Phase 1 unit test.
    assert_eq!(payload[0], SERVICE_DESCRIPTOR_V1);
    assert_eq!(&payload[1..33], &[0xB1u8; 32]);
    assert_eq!(&payload[33..65], &[0xB2u8; 32]);
    assert_eq!(&payload[65..97], &[0xB3u8; 32]);
    assert_eq!(payload[97], SERVICE_CATEGORY_INFERENCE);
    assert_eq!(
        &payload[98..106],
        &0x0102_0304_0506_0708u64.to_be_bytes()
    );
    assert_eq!(
        &payload[106..114],
        &0x1112_1314_1516_1718u64.to_be_bytes()
    );
    assert_eq!(&payload[114..116], &75u16.to_be_bytes());
    assert_eq!(
        &payload[116..132],
        &0x2122_2324_2526_2728_2A2B_2C2D_2E2F_3031u128.to_be_bytes()
    );
    assert_eq!(&payload[132..136], &0x4142_4344u32.to_be_bytes());
    assert_eq!(payload[136], SERVICE_STATUS_ACTIVE);
    assert_eq!(&payload[137..144], &[0u8; 7]);
}

// ============================================================================
// 9. Update preserves object_id and reflects new field values
// ============================================================================

#[test]
fn service_descriptor_update_price_preserves_object_id() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x31u8; 32], [0x41u8; 32]);
    let original = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    let object_id = publish_descriptor(&mut db, &publisher, 0, &original);

    // Update only the price; everything else stays the same.
    let mut updated = original;
    updated.price_per_call = 999_999;
    let update_tx = make_update_tx(&publisher, 1, object_id, updated.encode().to_vec());
    apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1).expect("update succeeds");

    // object_id must be the same key after the update.
    let stored = read_descriptor(&db, &publisher, &object_id);
    assert_eq!(stored.price_per_call, 999_999, "new price persisted");
    assert_eq!(stored.category, original.category, "category unchanged");
    assert_eq!(
        stored.service_url_hash, original.service_url_hash,
        "url hash unchanged"
    );

    // by_category index entry under the SAME object_id is still present
    // (we did not need to rewrite it, since category did not change).
    let category_key = service_descriptor_by_category_key(
        original.category,
        &publisher.id,
        &object_id,
    );
    assert!(
        db.get(&category_key).unwrap().is_some(),
        "by_category index entry survives update"
    );
}

#[test]
fn service_descriptor_update_status_preserves_object_id() {
    use novai_ai_entities::SERVICE_STATUS_PAUSED;
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x32u8; 32], [0x42u8; 32]);
    let original = sample_descriptor(SERVICE_CATEGORY_INFERENCE, 100);
    let object_id = publish_descriptor(&mut db, &publisher, 0, &original);

    let mut updated = original;
    updated.status = SERVICE_STATUS_PAUSED;
    let update_tx = make_update_tx(&publisher, 1, object_id, updated.encode().to_vec());
    apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1).expect("update succeeds");

    let stored = read_descriptor(&db, &publisher, &object_id);
    assert_eq!(stored.status, SERVICE_STATUS_PAUSED);
}

#[test]
fn service_descriptor_update_all_mutable_fields_at_once() {
    use novai_ai_entities::SERVICE_STATUS_DEPRECATED;
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x33u8; 32], [0x43u8; 32]);
    let original = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    let object_id = publish_descriptor(&mut db, &publisher, 0, &original);

    // Every field except `category` and `reserved` can change in one
    // update. This locks the boundary between mutable and immutable
    // fields end-to-end.
    let mut updated = original;
    updated.service_name_hash = [0xD1u8; 32];
    updated.service_url_hash = [0xD2u8; 32];
    updated.description_hash = [0xD3u8; 32];
    updated.price_per_call = 12_345;
    updated.subscription_rate_per_block = 42;
    updated.min_reputation_score = 80;
    updated.min_stake = 5_000_000;
    updated.capability_tags = 0xFF;
    updated.status = SERVICE_STATUS_DEPRECATED;
    let update_tx = make_update_tx(&publisher, 1, object_id, updated.encode().to_vec());
    apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1).expect("update succeeds");

    let stored = read_descriptor(&db, &publisher, &object_id);
    assert_eq!(stored, updated);
    assert_eq!(stored.category, original.category, "category MUST be unchanged");
}

// ============================================================================
// 10. Category-immutability is enforced
// ============================================================================

#[test]
fn service_descriptor_update_category_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x34u8; 32], [0x44u8; 32]);
    let original = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    let object_id = publish_descriptor(&mut db, &publisher, 0, &original);

    let mut updated = original;
    updated.category = SERVICE_CATEGORY_INFERENCE; // valid category, but DIFFERENT
    let update_tx = make_update_tx(&publisher, 1, object_id, updated.encode().to_vec());
    let err =
        apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1).unwrap_err();
    assert!(
        matches!(err, ExecError::ServiceDescriptorCategoryImmutable),
        "got {err:?}"
    );

    // Stored descriptor is unchanged.
    let stored = read_descriptor(&db, &publisher, &object_id);
    assert_eq!(stored.category, SERVICE_CATEGORY_DATA_ORACLE);

    // by_category index for the ORIGINAL category is still present;
    // no entry was created under the attempted-new category.
    let original_key = service_descriptor_by_category_key(
        SERVICE_CATEGORY_DATA_ORACLE,
        &publisher.id,
        &object_id,
    );
    let attempted_key = service_descriptor_by_category_key(
        SERVICE_CATEGORY_INFERENCE,
        &publisher.id,
        &object_id,
    );
    assert!(db.get(&original_key).unwrap().is_some());
    assert!(db.get(&attempted_key).unwrap().is_none());
}

// ============================================================================
// 11-15. Update path revalidates every field create checks
// ============================================================================

#[test]
fn service_descriptor_update_invalid_category_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x35u8; 32], [0x45u8; 32]);
    let original = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    let object_id = publish_descriptor(&mut db, &publisher, 0, &original);

    // Try to set a category in the governance-reserved range. The
    // bad-category check fires BEFORE the category-immutability check
    // (we want the more specific error first).
    let mut updated = original;
    updated.category = SERVICE_CATEGORY_RESERVED_MAX + 1;
    let update_tx = make_update_tx(&publisher, 1, object_id, updated.encode().to_vec());
    let err =
        apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::ServiceDescriptorInvalidCategory { byte }
                if byte == SERVICE_CATEGORY_RESERVED_MAX + 1
        ),
        "got {err:?}"
    );
}

#[test]
fn service_descriptor_update_invalid_status_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x36u8; 32], [0x46u8; 32]);
    let original = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    let object_id = publish_descriptor(&mut db, &publisher, 0, &original);

    let mut updated = original;
    updated.status = SERVICE_STATUS_MAX + 1;
    let update_tx = make_update_tx(&publisher, 1, object_id, updated.encode().to_vec());
    let err =
        apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::ServiceDescriptorInvalidStatus { byte }
                if byte == SERVICE_STATUS_MAX + 1
        ),
        "got {err:?}"
    );
}

#[test]
fn service_descriptor_update_reputation_over_max_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x37u8; 32], [0x47u8; 32]);
    let original = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    let object_id = publish_descriptor(&mut db, &publisher, 0, &original);

    let mut updated = original;
    updated.min_reputation_score = MAX_REPUTATION_SCORE + 1;
    let update_tx = make_update_tx(&publisher, 1, object_id, updated.encode().to_vec());
    let err =
        apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::ServiceDescriptorReputationOverMax { score }
                if score == MAX_REPUTATION_SCORE + 1
        ),
        "got {err:?}"
    );
}

#[test]
fn service_descriptor_update_non_zero_reserved_bytes_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x38u8; 32], [0x48u8; 32]);
    let original = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    let object_id = publish_descriptor(&mut db, &publisher, 0, &original);

    let mut updated = original;
    updated.reserved[0] = 0xFF;
    let update_tx = make_update_tx(&publisher, 1, object_id, updated.encode().to_vec());
    let err =
        apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1).unwrap_err();
    assert!(
        matches!(err, ExecError::InvalidServiceDescriptor),
        "got {err:?}"
    );
}

#[test]
fn service_descriptor_update_bad_version_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x39u8; 32], [0x49u8; 32]);
    let original = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    let object_id = publish_descriptor(&mut db, &publisher, 0, &original);

    let mut updated = original;
    updated.version = SERVICE_DESCRIPTOR_V1 + 1;
    let update_tx = make_update_tx(&publisher, 1, object_id, updated.encode().to_vec());
    let err =
        apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1).unwrap_err();
    assert!(
        matches!(err, ExecError::InvalidServiceDescriptor),
        "got {err:?}"
    );
}

#[test]
fn service_descriptor_update_bad_length_rejected() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x3Au8; 32], [0x4Au8; 32]);
    let original = sample_descriptor(SERVICE_CATEGORY_DATA_ORACLE, 100);
    let object_id = publish_descriptor(&mut db, &publisher, 0, &original);

    // Hand-roll a 143-byte new_data field.
    let update_tx = make_update_tx(
        &publisher,
        1,
        object_id,
        vec![0u8; SERVICE_DESCRIPTOR_SIZE - 1],
    );
    let err =
        apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1).unwrap_err();
    assert!(
        matches!(err, ExecError::InvalidServiceDescriptor),
        "got {err:?}"
    );
}
