#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Integration tests for the cross-entity composition protocol (Feature 4).
//!
//! Covers:
//! - CompositionGraph create/update via the memory-object tx flow:
//!   roundtrip, max-10-dependency capacity, self-dependency rejection
//!   on both create and update.
//! - CompositionCheck signal: auto-pause on each of the four verified
//!   failure reasons (inactive, low reputation, low stake, missing source).
//! - Optional dependencies do NOT auto-pause (only emit reputation event).
//! - Rejection paths: failure not verified, missing capability, missing
//!   graph, invalid dependency index, self-check.
//! - Reputation event emit (REP_EVENT_COMPOSITION_FAILURE, delta -1).
//! - Idempotent re-pause on already-inactive target.
//! - Regression: non-composition signals still work.
//! - Golden vector: 100-byte payload with frozen field offsets.

use novai_ai_entities::{
    encode_memory_object_v1, AiEntity, AiSignalType, AutonomyMode, Capabilities,
    CompositionDependency, CompositionGraphData, MemoryObject, MemoryObjectType,
    MAX_COMPOSITION_DEPENDENCIES,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_signal_commitment_tx,
    apply_update_memory_object_tx, encode_create_memory_object_payload_v1,
    encode_signal_commitment_payload_v1, encode_update_memory_object_payload_v1,
    read_ai_entity, write_ai_entity_op, CompositionCheckExtraV1, CreateMemoryObjectPayloadV1,
    ExecError, SignalCommitmentPayloadV1, UpdateMemoryObjectPayloadV1,
    COMPOSITION_FAILURE_REPUTATION_BELOW_MIN, COMPOSITION_FAILURE_SOURCE_INACTIVE,
    COMPOSITION_FAILURE_SOURCE_NOT_FOUND, COMPOSITION_FAILURE_STAKE_BELOW_MIN,
    REP_EVENT_COMPOSITION_FAILURE, SIGNAL_COMMITMENT_PAYLOAD_V1_COMPOSITION_CHECK_LEN,
};
use novai_state::{
    ai_entity_by_address_key, ai_memory_by_type_key, ai_memory_object_key, Kv, KvBatch, MemKv,
    WriteOp,
};
use novai_types::{TxV1, TxVersion};

const ORACLE_BALANCE: u128 = 1_000_000;
const TARGET_BALANCE: u128 = 1_000_000;
const SOURCE_BALANCE: u128 = 1_000_000;
const SIGNAL_FEE: u64 = 1_000;
const CREATE_FEE: u64 = 10;
const HEIGHT: u64 = 100;

// ============================================================================
// Helpers
// ============================================================================

fn oracle_caps() -> Capabilities {
    Capabilities {
        read_public_chain: true,
        read_memory_objects: false,
        emit_proposals: true,
        request_execution: false,
        read_nnpx_derived: false,
        submit_reputation_updates: true,
        _reserved: [false; 2],
    }
}

fn composer_caps() -> Capabilities {
    // Entities that publish their own CompositionGraph need read_memory_objects
    // (memory CRUD gate) and emit_proposals (signal-commitment dispatch gate).
    // submit_reputation_updates is intentionally false so the same role cannot
    // also issue CompositionCheck signals.
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

fn build_entity(code_hash: [u8; 32], creator: [u8; 32], caps: Capabilities) -> AiEntity {
    AiEntity::new(code_hash, creator, AutonomyMode::Gated, caps, 1)
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[
        write_ai_entity_op(entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
}

fn make_oracle(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut e = build_entity(code_hash, creator, oracle_caps());
    e.economic_balance = ORACLE_BALANCE;
    store_entity(db, &e);
    e
}

fn make_target(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut e = build_entity(code_hash, creator, composer_caps());
    e.economic_balance = TARGET_BALANCE;
    e.reputation_score = 50;
    store_entity(db, &e);
    e
}

/// Build a source entity with the given health attributes, persisted to state.
fn make_source(
    db: &mut MemKv,
    code_hash: [u8; 32],
    creator: [u8; 32],
    reputation_score: u16,
    stake_balance: u128,
    is_active: bool,
) -> AiEntity {
    let mut e = build_entity(code_hash, creator, composer_caps());
    e.economic_balance = SOURCE_BALANCE;
    e.reputation_score = reputation_score;
    e.stake_balance = stake_balance;
    e.is_active = is_active;
    store_entity(db, &e);
    e
}

fn sample_dep(
    source: [u8; 32],
    required_signal_type: u8,
    min_reputation: u16,
    min_stake: u64,
    is_required: bool,
) -> CompositionDependency {
    CompositionDependency {
        source_entity_id: source,
        required_signal_type,
        min_reputation,
        min_stake,
        is_required,
    }
}

/// Persist a CompositionGraph memory object directly to state. Mirrors
/// `marketplace_system::seed_catalog` — bypasses the create-memory-object tx
/// flow and writes the object record + the by-type presence index that
/// `get_memory_objects_by_entity_and_type` scans.
fn seed_composition_graph(
    db: &mut MemKv,
    target: &AiEntity,
    graph: &CompositionGraphData,
) -> [u8; 32] {
    let data = graph.encode();
    let obj = MemoryObject::new(
        target.id,
        MemoryObjectType::CompositionGraph,
        HEIGHT - 1,
        data,
    );
    let object_id = obj.object_id;
    let encoded = encode_memory_object_v1(&obj);

    db.apply_batch(&[
        WriteOp::Put(ai_memory_object_key(&target.id, &object_id), encoded),
        WriteOp::Put(
            ai_memory_by_type_key(
                MemoryObjectType::CompositionGraph.to_byte(),
                &target.id,
                &object_id,
            ),
            Vec::new(),
        ),
    ])
    .unwrap();
    object_id
}

fn build_composition_check_payload(
    issuer: [u8; 32],
    target: [u8; 32],
    failed_dependency_idx: u8,
    failure_reason: u8,
) -> Vec<u8> {
    encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xC4u8; 32],
        signal_type: AiSignalType::CompositionCheck,
        issuer_entity_id: issuer,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: Some(CompositionCheckExtraV1 {
            target_entity_id: target,
            failed_dependency_idx,
            failure_reason,
        }),
    })
}

fn build_create_graph_payload(graph: &CompositionGraphData) -> Vec<u8> {
    encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::CompositionGraph,
        data: graph.encode(),
    })
}

fn build_update_graph_payload(object_id: [u8; 32], graph: &CompositionGraphData) -> Vec<u8> {
    encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
        object_id,
        new_data: graph.encode(),
    })
}

fn make_tx(from: [u8; 32], nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from,
        pubkey: from,
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    }
}

// ============================================================================
// 1. CompositionGraph create/update via tx flow
// ============================================================================

#[test]
fn composition_graph_create_and_decode_roundtrip() {
    let mut db = MemKv::new();
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let source = make_source(&mut db, [0x13u8; 32], [0x23u8; 32], 80, 50_000, true);

    let graph = CompositionGraphData {
        dependencies: vec![sample_dep(source.id, 2, 10, 1_000, true)],
    };

    let tx = make_tx(target.id, 0, CREATE_FEE, build_create_graph_payload(&graph));
    let object_id = apply_create_memory_object_tx(&mut db, &tx, HEIGHT)
        .expect("create CompositionGraph succeeds");

    let stored = db
        .get(&ai_memory_object_key(&target.id, &object_id))
        .unwrap()
        .expect("memory object stored");
    let memobj = novai_ai_entities::decode_memory_object_v1(&stored).unwrap();
    assert_eq!(memobj.object_type, MemoryObjectType::CompositionGraph);
    assert_eq!(memobj.owner_entity, target.id);

    let decoded = CompositionGraphData::decode(&memobj.data).expect("graph decodes");
    assert_eq!(decoded, graph);
}

#[test]
fn composition_graph_max_10_dependencies() {
    let mut db = MemKv::new();
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let mut deps = Vec::with_capacity(MAX_COMPOSITION_DEPENDENCIES);
    for i in 0..MAX_COMPOSITION_DEPENDENCIES {
        let mut id = [0u8; 32];
        id[0] = i as u8;
        // Fund a real source so the chain accepts the dep regardless of who
        // looks it up later — though codec-side this only checks bytes.
        let _ = make_source(&mut db, id, [0x33u8; 32], 50, 0, true);
        deps.push(sample_dep(id, 0, 0, 0, true));
    }
    let graph = CompositionGraphData { dependencies: deps };
    assert_eq!(graph.encode().len(), 1 + MAX_COMPOSITION_DEPENDENCIES * 44);
    assert_eq!(graph.encode().len(), 441);

    let tx = make_tx(target.id, 0, CREATE_FEE, build_create_graph_payload(&graph));
    apply_create_memory_object_tx(&mut db, &tx, HEIGHT)
        .expect("max-capacity graph accepted");
}

#[test]
fn composition_graph_create_rejects_self_dependency() {
    let mut db = MemKv::new();
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let graph = CompositionGraphData {
        dependencies: vec![sample_dep(target.id, 2, 0, 0, true)],
    };

    let tx = make_tx(target.id, 0, CREATE_FEE, build_create_graph_payload(&graph));
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT)
        .expect_err("self-dependency must be rejected at create");
    assert!(matches!(err, ExecError::SelfDependency), "got {err:?}");
}

#[test]
fn composition_graph_update_rejects_self_dependency() {
    let mut db = MemKv::new();
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let source = make_source(&mut db, [0x13u8; 32], [0x23u8; 32], 50, 0, true);

    // Create a clean graph first (no self-dep).
    let clean_graph = CompositionGraphData {
        dependencies: vec![sample_dep(source.id, 2, 0, 0, true)],
    };
    let create_tx = make_tx(target.id, 0, CREATE_FEE, build_create_graph_payload(&clean_graph));
    let object_id =
        apply_create_memory_object_tx(&mut db, &create_tx, HEIGHT).expect("clean create");

    // Attempt to update the graph to include a self-dependency.
    let bad_graph = CompositionGraphData {
        dependencies: vec![sample_dep(target.id, 2, 0, 0, true)],
    };
    let update_tx = make_tx(
        target.id,
        1,
        CREATE_FEE,
        build_update_graph_payload(object_id, &bad_graph),
    );
    let err = apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1)
        .expect_err("self-dependency must be rejected at update");
    assert!(matches!(err, ExecError::SelfDependency), "got {err:?}");
}

// ============================================================================
// 2. CompositionCheck auto-pause behavior
// ============================================================================

#[test]
fn composition_check_auto_pauses_on_inactive_source() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let source = make_source(&mut db, [0x13u8; 32], [0x23u8; 32], 80, 50_000, false);

    let graph = CompositionGraphData {
        dependencies: vec![sample_dep(source.id, 2, 10, 1_000, true)],
    };
    seed_composition_graph(&mut db, &target, &graph);

    let payload = build_composition_check_payload(
        oracle.id,
        target.id,
        0,
        COMPOSITION_FAILURE_SOURCE_INACTIVE,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(oracle.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .expect("inactive-source check verified");

    let target_after = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert!(!target_after.is_active, "required-dep failure auto-pauses");
}

#[test]
fn composition_check_auto_pauses_on_low_reputation() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let source = make_source(&mut db, [0x13u8; 32], [0x23u8; 32], 5, 50_000, true);

    // min_reputation = 50; source has 5 → below threshold.
    let graph = CompositionGraphData {
        dependencies: vec![sample_dep(source.id, 2, 50, 1_000, true)],
    };
    seed_composition_graph(&mut db, &target, &graph);

    let payload = build_composition_check_payload(
        oracle.id,
        target.id,
        0,
        COMPOSITION_FAILURE_REPUTATION_BELOW_MIN,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(oracle.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .expect("low-reputation check verified");

    let target_after = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert!(!target_after.is_active);
}

#[test]
fn composition_check_auto_pauses_on_low_stake() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let source = make_source(&mut db, [0x13u8; 32], [0x23u8; 32], 80, 100, true);

    // min_stake = 10_000; source has 100 → below threshold.
    let graph = CompositionGraphData {
        dependencies: vec![sample_dep(source.id, 2, 0, 10_000, true)],
    };
    seed_composition_graph(&mut db, &target, &graph);

    let payload = build_composition_check_payload(
        oracle.id,
        target.id,
        0,
        COMPOSITION_FAILURE_STAKE_BELOW_MIN,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(oracle.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .expect("low-stake check verified");

    let target_after = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert!(!target_after.is_active);
}

#[test]
fn composition_check_auto_pauses_on_missing_source() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);

    // Source ID is referenced but the entity is never persisted — it does
    // not exist in state.
    let phantom_source = [0xDDu8; 32];
    let graph = CompositionGraphData {
        dependencies: vec![sample_dep(phantom_source, 2, 0, 0, true)],
    };
    seed_composition_graph(&mut db, &target, &graph);

    let payload = build_composition_check_payload(
        oracle.id,
        target.id,
        0,
        COMPOSITION_FAILURE_SOURCE_NOT_FOUND,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(oracle.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .expect("missing-source check verified");

    let target_after = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert!(!target_after.is_active);
}

#[test]
fn composition_check_does_not_pause_optional_dependency() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let source = make_source(&mut db, [0x13u8; 32], [0x23u8; 32], 80, 50_000, false);

    // is_required = false → reputation event still fires but is_active stays true.
    let graph = CompositionGraphData {
        dependencies: vec![sample_dep(source.id, 2, 10, 1_000, false)],
    };
    seed_composition_graph(&mut db, &target, &graph);

    let payload = build_composition_check_payload(
        oracle.id,
        target.id,
        0,
        COMPOSITION_FAILURE_SOURCE_INACTIVE,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(oracle.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .expect("optional-dep check verified");

    let target_after = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert!(target_after.is_active, "optional-dep failure must NOT pause");
    assert_eq!(
        target_after.reputation_events_count, 1,
        "rep event still fires for optional-dep failures"
    );
}

// ============================================================================
// 3. Rejection paths
// ============================================================================

#[test]
fn composition_check_rejected_failure_not_verified() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    // Source is HEALTHY (active, high rep, high stake) → claim of inactivity is false.
    let source = make_source(&mut db, [0x13u8; 32], [0x23u8; 32], 80, 50_000, true);

    let graph = CompositionGraphData {
        dependencies: vec![sample_dep(source.id, 2, 10, 1_000, true)],
    };
    seed_composition_graph(&mut db, &target, &graph);

    let payload = build_composition_check_payload(
        oracle.id,
        target.id,
        0,
        COMPOSITION_FAILURE_SOURCE_INACTIVE,
    );
    let err = apply_signal_commitment_tx(
        &mut db,
        &make_tx(oracle.id, 0, SIGNAL_FEE, payload),
        HEIGHT,
    )
    .expect_err("must reject false claim");
    assert!(
        matches!(err, ExecError::DependencyFailureNotVerified),
        "got {err:?}"
    );

    // Target state unchanged: still active, no rep event recorded.
    let target_after = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert!(target_after.is_active);
    assert_eq!(target_after.reputation_events_count, 0);
}

#[test]
fn composition_check_rejected_without_capability() {
    let mut db = MemKv::new();
    // Issuer with composer_caps (no submit_reputation_updates) — should fail.
    let mut bad_issuer = build_entity([0x11u8; 32], [0x21u8; 32], composer_caps());
    bad_issuer.economic_balance = ORACLE_BALANCE;
    store_entity(&mut db, &bad_issuer);

    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let source = make_source(&mut db, [0x13u8; 32], [0x23u8; 32], 80, 50_000, false);

    let graph = CompositionGraphData {
        dependencies: vec![sample_dep(source.id, 2, 0, 0, true)],
    };
    seed_composition_graph(&mut db, &target, &graph);

    let payload = build_composition_check_payload(
        bad_issuer.id,
        target.id,
        0,
        COMPOSITION_FAILURE_SOURCE_INACTIVE,
    );
    let err = apply_signal_commitment_tx(
        &mut db,
        &make_tx(bad_issuer.id, 0, SIGNAL_FEE, payload),
        HEIGHT,
    )
    .expect_err("missing capability must reject");
    assert!(
        matches!(err, ExecError::IssuerMissingCapability),
        "got {err:?}"
    );
}

#[test]
fn composition_check_rejected_graph_not_found() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    // No graph seeded.

    let payload = build_composition_check_payload(
        oracle.id,
        target.id,
        0,
        COMPOSITION_FAILURE_SOURCE_INACTIVE,
    );
    let err = apply_signal_commitment_tx(
        &mut db,
        &make_tx(oracle.id, 0, SIGNAL_FEE, payload),
        HEIGHT,
    )
    .expect_err("must reject when no graph exists");
    assert!(
        matches!(err, ExecError::CompositionGraphNotFound),
        "got {err:?}"
    );
}

#[test]
fn composition_check_rejected_invalid_dependency_index() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let source = make_source(&mut db, [0x13u8; 32], [0x23u8; 32], 80, 50_000, false);

    // Graph has 1 dependency (index 0). Probe index 5.
    let graph = CompositionGraphData {
        dependencies: vec![sample_dep(source.id, 2, 0, 0, true)],
    };
    seed_composition_graph(&mut db, &target, &graph);

    let payload = build_composition_check_payload(
        oracle.id,
        target.id,
        5,
        COMPOSITION_FAILURE_SOURCE_INACTIVE,
    );
    let err = apply_signal_commitment_tx(
        &mut db,
        &make_tx(oracle.id, 0, SIGNAL_FEE, payload),
        HEIGHT,
    )
    .expect_err("invalid index must reject");
    assert!(
        matches!(err, ExecError::InvalidDependencyIndex { index: 5, max: 1 }),
        "got {err:?}"
    );
}

#[test]
fn composition_check_rejects_self_target() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);

    // Self-check: oracle reports failure on itself.
    let payload = build_composition_check_payload(
        oracle.id,
        oracle.id,
        0,
        COMPOSITION_FAILURE_SOURCE_INACTIVE,
    );
    let err = apply_signal_commitment_tx(
        &mut db,
        &make_tx(oracle.id, 0, SIGNAL_FEE, payload),
        HEIGHT,
    )
    .expect_err("self-check must reject");
    assert!(
        matches!(err, ExecError::SelfCompositionCheck),
        "got {err:?}"
    );
}

// ============================================================================
// 4. Reputation event semantics
// ============================================================================

#[test]
fn composition_check_updates_reputation() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let source = make_source(&mut db, [0x13u8; 32], [0x23u8; 32], 80, 50_000, false);

    let graph = CompositionGraphData {
        dependencies: vec![sample_dep(source.id, 2, 0, 0, true)],
    };
    seed_composition_graph(&mut db, &target, &graph);

    let target_before = read_ai_entity(&db, &target.id).unwrap().unwrap();
    let rep_before = target_before.reputation_score;
    let events_before = target_before.reputation_events_count;

    let payload = build_composition_check_payload(
        oracle.id,
        target.id,
        0,
        COMPOSITION_FAILURE_SOURCE_INACTIVE,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(oracle.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .unwrap();

    let target_after = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert_eq!(
        target_after.reputation_score,
        rep_before - 1,
        "delta -1 applied"
    );
    assert_eq!(
        target_after.reputation_events_count,
        events_before + 1,
        "events incremented"
    );
    // Sanity check the constant — REP_EVENT_COMPOSITION_FAILURE is the
    // event class advertised by this signal type, even though we don't
    // store the discriminant alongside the score.
    assert_eq!(REP_EVENT_COMPOSITION_FAILURE, 7);
}

#[test]
fn composition_check_idempotent_on_already_paused_target() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);

    // Target is already paused before the check fires.
    let mut target = build_entity([0x12u8; 32], [0x22u8; 32], composer_caps());
    target.economic_balance = TARGET_BALANCE;
    target.reputation_score = 30;
    target.is_active = false;
    store_entity(&mut db, &target);

    let source = make_source(&mut db, [0x13u8; 32], [0x23u8; 32], 80, 50_000, false);
    let graph = CompositionGraphData {
        dependencies: vec![sample_dep(source.id, 2, 0, 0, true)],
    };
    seed_composition_graph(&mut db, &target, &graph);

    let payload = build_composition_check_payload(
        oracle.id,
        target.id,
        0,
        COMPOSITION_FAILURE_SOURCE_INACTIVE,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(oracle.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .expect("idempotent re-pause succeeds");

    let target_after = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert!(!target_after.is_active, "still paused (no flip back)");
    assert_eq!(
        target_after.reputation_score, 29,
        "delta -1 still applied to already-paused target"
    );
    assert_eq!(target_after.reputation_events_count, 1);
}

// ============================================================================
// 5. Regression
// ============================================================================

#[test]
fn non_composition_signals_still_work() {
    let mut db = MemKv::new();
    let issuer = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let anomaly = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: AiSignalType::Anomaly,
        issuer_entity_id: issuer.id,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
    });
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(issuer.id, 0, SIGNAL_FEE, anomaly),
        HEIGHT,
    )
    .expect("base anomaly still applies; CompositionCheck doesn't break it");

    let after = read_ai_entity(&db, &issuer.id).unwrap().unwrap();
    assert_eq!(after.reputation_events_count, 0);
}

// ============================================================================
// 6. Golden vector
// ============================================================================

#[test]
fn golden_vector_composition_check_payload_100_bytes() {
    let issuer = [0x22u8; 32];
    let target = [0x33u8; 32];
    let payload = build_composition_check_payload(
        issuer,
        target,
        7,
        COMPOSITION_FAILURE_REPUTATION_BELOW_MIN,
    );

    assert_eq!(payload.len(), 100);
    assert_eq!(payload.len(), SIGNAL_COMMITMENT_PAYLOAD_V1_COMPOSITION_CHECK_LEN);

    // Frozen field offsets — moving any of these is a wire-format break.
    assert_eq!(payload[0], 2, "version byte");
    assert_eq!(&payload[1..33], &[0xC4u8; 32], "signal_hash at 1..33");
    assert_eq!(
        payload[33],
        AiSignalType::CompositionCheck.to_byte(),
        "signal_type byte at 33"
    );
    assert_eq!(payload[33], 12, "CompositionCheck discriminant is 12");
    assert_eq!(&payload[34..66], &issuer, "issuer_entity_id at 34..66");
    assert_eq!(&payload[66..98], &target, "target_entity_id at 66..98");
    assert_eq!(payload[98], 7, "failed_dependency_idx at 98");
    assert_eq!(
        payload[99],
        COMPOSITION_FAILURE_REPUTATION_BELOW_MIN,
        "failure_reason at 99"
    );
}
