#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Week 24 End-to-End Tests for NOVAI Core Observer Reference Module.
//!
//! D24.2 - Reference AI Module "NOVAI Core Observer"
//!
//! Acceptance Criteria:
//! 1. Core Observer registered at genesis with correct properties
//! 2. Can emit Anomaly signals
//! 3. Can emit CongestionForecast signals
//! 4. Can create ChainSummary memory objects
//! 5. Can create StatisticsSnapshot memory objects

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, ChainSummaryData, MemoryObjectType,
    StatisticsSnapshotData, CORE_OBSERVER_CODE_HASH, PROTOCOL_CREATOR,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_signal_commitment_tx,
    encode_create_memory_object_payload_v1, encode_signal_commitment_payload_v1,
    get_memory_objects_by_entity, get_signals_by_issuer, get_signals_by_type, read_ai_entity,
    write_ai_entity_op, CreateMemoryObjectPayloadV1, SignalCommitmentPayloadV1,
};
use novai_state::{ai_entity_by_address_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Create the Core Observer entity with well-known identifiers.
fn create_core_observer(balance: u128, nonce: u64) -> AiEntity {
    let mut entity = AiEntity::new(
        CORE_OBSERVER_CODE_HASH,
        PROTOCOL_CREATOR,
        AutonomyMode::Advisory,
        Capabilities::advisory(), // read_public_chain, read_memory_objects, emit_proposals
        0,                        // registered_at (genesis)
    );
    entity.economic_balance = balance;
    entity.nonce = nonce;
    entity
}

/// Create a signal commitment payload.
fn create_signal_payload(
    signal_hash: [u8; 32],
    signal_type: AiSignalType,
    issuer: [u8; 32],
) -> Vec<u8> {
    let payload = SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type,
        issuer_entity_id: issuer,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: None,
        subscription_cancel: None,
        payment_request: None,
        service_attestation: None,
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
    };
    encode_signal_commitment_payload_v1(&payload)
}

/// Create a memory object payload.
fn create_memory_object_payload(object_type: MemoryObjectType, data: Vec<u8>) -> Vec<u8> {
    let payload = CreateMemoryObjectPayloadV1 { object_type, data };
    encode_create_memory_object_payload_v1(&payload)
}

/// Create a test transaction from the Core Observer.
fn create_core_observer_tx(entity_id: [u8; 32], nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from: entity_id,
        pubkey: entity_id, // Simplified for test
        nonce,
        fee,
        payload,
        sig: [0u8; 64], // Signature validation not in scope
    }
}

// ============================================================================
// Test 1: Core Observer entity has correct properties
// ============================================================================

#[test]
fn core_observer_has_well_known_identifiers() {
    // Verify the code hash and creator are deterministic
    let entity = create_core_observer(1_000_000_000, 0);

    // Entity ID should be deterministically computed
    let expected_id = AiEntity::compute_id(&CORE_OBSERVER_CODE_HASH, &PROTOCOL_CREATOR);
    assert_eq!(entity.id, expected_id, "Entity ID must match computed ID");

    // Code hash must match well-known constant
    assert_eq!(
        entity.code_hash, CORE_OBSERVER_CODE_HASH,
        "Code hash must match CORE_OBSERVER_CODE_HASH"
    );

    // Creator must match well-known constant
    assert_eq!(
        entity.creator, PROTOCOL_CREATOR,
        "Creator must match PROTOCOL_CREATOR"
    );
}

#[test]
fn core_observer_has_advisory_capabilities() {
    let entity = create_core_observer(1_000_000_000, 0);

    // Must have these capabilities
    assert!(
        entity.has_capability("read_public_chain"),
        "Core Observer must have read_public_chain"
    );
    assert!(
        entity.has_capability("read_memory_objects"),
        "Core Observer must have read_memory_objects"
    );
    assert!(
        entity.has_capability("emit_proposals"),
        "Core Observer must have emit_proposals"
    );

    // Must NOT have these capabilities (advisory mode)
    assert!(
        !entity.has_capability("request_execution"),
        "Core Observer must NOT have request_execution (advisory mode)"
    );

    // Autonomy mode must be Advisory
    assert_eq!(
        entity.autonomy_mode,
        AutonomyMode::Advisory,
        "Core Observer must be in Advisory mode"
    );
}

#[test]
fn core_observer_registered_in_state() {
    let mut db = MemKv::new();

    let entity = create_core_observer(1_000_000_000, 0);
    let entity_id = entity.id;

    // Register entity in state (simulating genesis)
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Read back from state
    let loaded = read_ai_entity(&db, &entity_id)
        .expect("DB read should succeed")
        .expect("Entity should exist");

    assert_eq!(loaded.id, entity_id);
    assert_eq!(loaded.code_hash, CORE_OBSERVER_CODE_HASH);
    assert_eq!(loaded.creator, PROTOCOL_CREATOR);
    assert_eq!(loaded.autonomy_mode, AutonomyMode::Advisory);
    assert_eq!(loaded.economic_balance, 1_000_000_000);
    assert!(loaded.is_active, "Genesis entity should be active");
}

// ============================================================================
// Test 2: Core Observer can emit Anomaly signals
// ============================================================================

#[test]
fn core_observer_emits_anomaly_signal() {
    let mut db = MemKv::new();

    // Setup: Register Core Observer
    let entity = create_core_observer(1_000_000_000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Create anomaly signal
    // In production, this would be blake3(off-chain_anomaly_report)
    let anomaly_hash = blake3::hash(b"anomaly:tx_spike:height=1000:severity=high").into();
    let payload = create_signal_payload(anomaly_hash, AiSignalType::Anomaly, entity_id);

    let tx = create_core_observer_tx(entity_id, 0, 100, payload);
    let result = apply_signal_commitment_tx(&mut db, &tx, 1000);

    assert!(result.is_ok(), "Anomaly signal should be accepted");

    // Verify signal is indexed
    let signals = get_signals_by_type(&db, AiSignalType::Anomaly, 0, 2000).unwrap();
    assert_eq!(signals.len(), 1, "Should have 1 anomaly signal");
    assert_eq!(signals[0].issuer, entity_id);
    assert_eq!(signals[0].commitment_hash, anomaly_hash);
}

// ============================================================================
// Test 3: Core Observer can emit CongestionForecast signals
// ============================================================================

#[test]
fn core_observer_emits_congestion_forecast_signal() {
    let mut db = MemKv::new();

    // Setup: Register Core Observer
    let entity = create_core_observer(1_000_000_000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Create congestion forecast signal
    let forecast_hash =
        blake3::hash(b"forecast:congestion:next_100_blocks:level=medium:confidence=85").into();
    let payload = create_signal_payload(forecast_hash, AiSignalType::CongestionForecast, entity_id);

    let tx = create_core_observer_tx(entity_id, 0, 100, payload);
    let result = apply_signal_commitment_tx(&mut db, &tx, 1000);

    assert!(
        result.is_ok(),
        "CongestionForecast signal should be accepted"
    );

    // Verify signal is indexed
    let signals = get_signals_by_type(&db, AiSignalType::CongestionForecast, 0, 2000).unwrap();
    assert_eq!(signals.len(), 1, "Should have 1 congestion forecast signal");
    assert_eq!(signals[0].issuer, entity_id);
    assert_eq!(signals[0].signal_type, AiSignalType::CongestionForecast);
}

#[test]
fn core_observer_emits_multiple_signal_types() {
    let mut db = MemKv::new();

    // Setup: Register Core Observer with enough balance
    let entity = create_core_observer(1_000_000_000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Emit Anomaly signal
    let anomaly_hash = blake3::hash(b"anomaly:1").into();
    let payload1 = create_signal_payload(anomaly_hash, AiSignalType::Anomaly, entity_id);
    let tx1 = create_core_observer_tx(entity_id, 0, 100, payload1);
    apply_signal_commitment_tx(&mut db, &tx1, 1000).unwrap();

    // Emit CongestionForecast signal (nonce is now 1)
    let forecast_hash = blake3::hash(b"forecast:1").into();
    let payload2 =
        create_signal_payload(forecast_hash, AiSignalType::CongestionForecast, entity_id);
    let tx2 = create_core_observer_tx(entity_id, 1, 100, payload2);
    apply_signal_commitment_tx(&mut db, &tx2, 1001).unwrap();

    // Verify both signals indexed by issuer
    let all_signals = get_signals_by_issuer(&db, &entity_id, 0, 2000).unwrap();
    assert_eq!(all_signals.len(), 2, "Core Observer should have 2 signals");

    // Verify nonce was incremented
    let updated_entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(updated_entity.nonce, 2, "Nonce should be 2 after 2 signals");
}

// ============================================================================
// Test 4: Core Observer can create ChainSummary memory objects
// ============================================================================

#[test]
fn core_observer_creates_chain_summary_memory_object() {
    let mut db = MemKv::new();

    // Setup: Register Core Observer
    let entity = create_core_observer(1_000_000_000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Create ChainSummary data
    let summary = ChainSummaryData {
        start_height: 0,
        end_height: 999,
        tx_count: 5000,
        fee_total: 500_000,
        avg_block_fullness: 65,
    };
    let data = summary.encode();

    // Create memory object
    let payload = create_memory_object_payload(MemoryObjectType::ChainSummary, data);
    let tx = create_core_observer_tx(entity_id, 0, 100, payload);

    let result = apply_create_memory_object_tx(&mut db, &tx, 1000);
    assert!(result.is_ok(), "ChainSummary creation should succeed");

    // Verify memory object exists
    let objects = get_memory_objects_by_entity(&db, &entity_id).unwrap();
    assert_eq!(objects.len(), 1, "Should have 1 memory object");
    assert_eq!(objects[0].object_type, MemoryObjectType::ChainSummary);
    assert_eq!(objects[0].owner_entity, entity_id);

    // Verify data can be decoded
    let decoded = ChainSummaryData::decode(&objects[0].data).expect("Data should decode");
    assert_eq!(decoded.start_height, 0);
    assert_eq!(decoded.end_height, 999);
    assert_eq!(decoded.tx_count, 5000);
    assert_eq!(decoded.fee_total, 500_000);
    assert_eq!(decoded.avg_block_fullness, 65);
}

// ============================================================================
// Test 5: Core Observer can create StatisticsSnapshot memory objects
// ============================================================================

#[test]
fn core_observer_creates_statistics_snapshot_memory_object() {
    let mut db = MemKv::new();

    // Setup: Register Core Observer
    let entity = create_core_observer(1_000_000_000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Create StatisticsSnapshot data
    let snapshot = StatisticsSnapshotData {
        height: 1000,
        mempool_size: 150,
        avg_fee: 100,
        fee_p95: 450,
        validator_count: 5,
        avg_block_fullness: 70,
    };
    let data = snapshot.encode();

    // Create memory object
    let payload = create_memory_object_payload(MemoryObjectType::StatisticsSnapshot, data);
    let tx = create_core_observer_tx(entity_id, 0, 100, payload);

    let result = apply_create_memory_object_tx(&mut db, &tx, 1000);
    assert!(result.is_ok(), "StatisticsSnapshot creation should succeed");

    // Verify memory object exists
    let objects = get_memory_objects_by_entity(&db, &entity_id).unwrap();
    assert_eq!(objects.len(), 1, "Should have 1 memory object");
    assert_eq!(objects[0].object_type, MemoryObjectType::StatisticsSnapshot);

    // Verify data can be decoded
    let decoded = StatisticsSnapshotData::decode(&objects[0].data).expect("Data should decode");
    assert_eq!(decoded.height, 1000);
    assert_eq!(decoded.mempool_size, 150);
    assert_eq!(decoded.avg_fee, 100);
    assert_eq!(decoded.fee_p95, 450);
    assert_eq!(decoded.validator_count, 5);
    assert_eq!(decoded.avg_block_fullness, 70);
}

// ============================================================================
// Test 6: Full E2E - Observer produces signals AND memory objects
// ============================================================================

#[test]
fn core_observer_full_workflow() {
    let mut db = MemKv::new();

    // 1. Register Core Observer at "genesis"
    let entity = create_core_observer(10_000_000_000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    let mut current_nonce = 0u64;

    // 2. Create initial ChainSummary for epoch 0
    let epoch0_summary = ChainSummaryData {
        start_height: 0,
        end_height: 999,
        tx_count: 4500,
        fee_total: 450_000,
        avg_block_fullness: 60,
    };
    let payload =
        create_memory_object_payload(MemoryObjectType::ChainSummary, epoch0_summary.encode());
    let tx = create_core_observer_tx(entity_id, current_nonce, 100, payload);
    apply_create_memory_object_tx(&mut db, &tx, 1000).unwrap();
    current_nonce += 1;

    // 3. Emit anomaly signal (detected unusual activity)
    let anomaly_hash = blake3::hash(b"anomaly:unusual_tx_pattern:epoch0").into();
    let payload = create_signal_payload(anomaly_hash, AiSignalType::Anomaly, entity_id);
    let tx = create_core_observer_tx(entity_id, current_nonce, 100, payload);
    apply_signal_commitment_tx(&mut db, &tx, 1001).unwrap();
    current_nonce += 1;

    // 4. Create statistics snapshot
    let snapshot = StatisticsSnapshotData {
        height: 1000,
        mempool_size: 200,
        avg_fee: 120,
        fee_p95: 500,
        validator_count: 5,
        avg_block_fullness: 75,
    };
    let payload =
        create_memory_object_payload(MemoryObjectType::StatisticsSnapshot, snapshot.encode());
    let tx = create_core_observer_tx(entity_id, current_nonce, 100, payload);
    apply_create_memory_object_tx(&mut db, &tx, 1002).unwrap();
    current_nonce += 1;

    // 5. Emit congestion forecast
    let forecast_hash = blake3::hash(b"forecast:high_load_expected:next_epoch").into();
    let payload = create_signal_payload(forecast_hash, AiSignalType::CongestionForecast, entity_id);
    let tx = create_core_observer_tx(entity_id, current_nonce, 100, payload);
    apply_signal_commitment_tx(&mut db, &tx, 1003).unwrap();

    // Verify final state
    let final_entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(
        final_entity.nonce, 4,
        "Should have processed 4 transactions"
    );
    assert!(final_entity.is_active, "Entity should still be active");

    // Verify 2 signals emitted
    let all_signals = get_signals_by_issuer(&db, &entity_id, 0, 2000).unwrap();
    assert_eq!(all_signals.len(), 2, "Should have 2 signals");

    // Verify 2 memory objects created
    let all_objects = get_memory_objects_by_entity(&db, &entity_id).unwrap();
    assert_eq!(all_objects.len(), 2, "Should have 2 memory objects");

    // Verify signal types
    let anomalies = get_signals_by_type(&db, AiSignalType::Anomaly, 0, 2000).unwrap();
    let forecasts = get_signals_by_type(&db, AiSignalType::CongestionForecast, 0, 2000).unwrap();
    assert_eq!(anomalies.len(), 1, "Should have 1 anomaly");
    assert_eq!(forecasts.len(), 1, "Should have 1 forecast");
}

// ============================================================================
// Test 7: Entity ID is deterministic from well-known constants
// ============================================================================

#[test]
fn core_observer_entity_id_is_deterministic() {
    // Create the entity twice
    let entity1 = create_core_observer(1_000_000_000, 0);
    let entity2 = create_core_observer(2_000_000_000, 5);

    // Entity ID should be the same regardless of balance/nonce
    assert_eq!(
        entity1.id, entity2.id,
        "Entity ID must be deterministic from code_hash + creator"
    );

    // Verify it matches manual computation
    let computed_id = AiEntity::compute_id(&CORE_OBSERVER_CODE_HASH, &PROTOCOL_CREATOR);
    assert_eq!(entity1.id, computed_id);
}
