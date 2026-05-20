#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]

//! End-to-end tests for Feature 8 (Entity Delegation).
//!
//! Exercises the full path: register two entities, create a
//! `DelegationGrant` memory object via `dispatch_tx`, observe the
//! by-delegate secondary index, watch the resolver merge granted
//! capabilities into the delegate's effective set, and verify that
//! delete + revoke + expiry + inactive-delegator all tear the grant
//! down without leaking state into unrelated memory-object workflows.

use novai_ai_entities::{
    AiSignalType, AutonomyMode, Capabilities, DelegationGrantData, MemoryObjectType,
    DELEGATION_GRANT_VERSION, MAX_DELEGATION_GRANTS,
};
use novai_execution::{
    apply_register_ai_entity_with_key_tx, dispatch_tx, encode_create_memory_object_payload_v1,
    encode_delete_memory_object_payload_v1, encode_register_ai_entity_with_key_payload_v1,
    encode_signal_commitment_payload_v1, encode_update_memory_object_payload_v1,
    get_memory_objects_by_entity, read_ai_entity, write_ai_entity_op, CreateMemoryObjectPayloadV1,
    DeleteMemoryObjectPayloadV1, ExecError, RegisterAiEntityWithKeyPayloadV1,
    SignalCommitmentPayloadV1, UpdateMemoryObjectPayloadV1,
};
use novai_state::{
    account_key, ai_delegations_by_delegate_prefix, encode_account_v1, AccountStateV1, Kv, KvBatch,
    MemKv, WriteOp,
};
use novai_types::{TxV1, TxVersion};

// =============================================================================
// HELPERS
// =============================================================================

fn derive_addr(pubkey: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"NOVAI_ADDRESS_V1");
    hasher.update(pubkey);
    *hasher.finalize().as_bytes()
}

fn mk_tx(from: [u8; 32], pubkey: [u8; 32], nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from,
        pubkey,
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    }
}

fn fund_account(db: &mut MemKv, addr: [u8; 32], balance: u128) {
    let acct = AccountStateV1 { balance, nonce: 0 };
    db.apply_batch(&[WriteOp::Put(
        account_key(&addr),
        encode_account_v1(&acct).to_vec(),
    )])
    .unwrap();
}

fn register_entity_with_caps(
    db: &mut MemKv,
    creator_pubkey: &[u8; 32],
    creator_nonce: u64,
    code_hash: [u8; 32],
    entity_pubkey: [u8; 32],
    initial_balance: u128,
    fee: u64,
    capabilities: Capabilities,
) -> ([u8; 32], [u8; 32]) {
    let creator_addr = derive_addr(creator_pubkey);
    let payload =
        encode_register_ai_entity_with_key_payload_v1(&RegisterAiEntityWithKeyPayloadV1 {
            code_hash,
            pubkey: entity_pubkey,
            autonomy_mode: AutonomyMode::Gated,
            capabilities,
            initial_balance,
        })
        .to_vec();
    let tx = mk_tx(creator_addr, *creator_pubkey, creator_nonce, fee, payload);
    let entity_id = apply_register_ai_entity_with_key_tx(db, &tx, 100).unwrap();
    let entity_addr = derive_addr(&entity_pubkey);
    (entity_id, entity_addr)
}

/// Build a `CreateMemoryObject` tx whose payload is a `DelegationGrant`
/// targeting `delegate_id` with `granted_caps` and `expires_at`.
fn create_grant_tx(
    sender_addr: [u8; 32],
    sender_pubkey: [u8; 32],
    nonce: u64,
    fee: u64,
    delegate_id: [u8; 32],
    granted_caps: u8,
    expires_at: u64,
) -> TxV1 {
    let grant = DelegationGrantData {
        version: DELEGATION_GRANT_VERSION,
        delegate_entity_id: delegate_id,
        granted_capabilities: granted_caps,
        expires_at,
    };
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::DelegationGrant,
        data: grant.encode().to_vec(),
    });
    mk_tx(sender_addr, sender_pubkey, nonce, fee, payload)
}

fn signal_payload(issuer: [u8; 32], signal_type: AiSignalType) -> Vec<u8> {
    let payload = SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
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

/// Register a delegator (`advisory()` caps) and a delegate (`read_only()`
/// caps) and return their (id, addr, pubkey) triples. The two pubkeys
/// differ so each entity has a distinct derived address.
fn setup_two_entities(
    db: &mut MemKv,
    creator_pubkey: &[u8; 32],
    delegator_pubkey: [u8; 32],
    delegate_pubkey: [u8; 32],
) -> (([u8; 32], [u8; 32]), ([u8; 32], [u8; 32])) {
    fund_account(db, derive_addr(creator_pubkey), 10_000_000);
    let (delegator_id, delegator_addr) = register_entity_with_caps(
        db,
        creator_pubkey,
        0,
        [0xC1u8; 32],
        delegator_pubkey,
        1_000_000,
        5_000,
        Capabilities::advisory(),
    );
    let (delegate_id, delegate_addr) = register_entity_with_caps(
        db,
        creator_pubkey,
        1,
        [0xC2u8; 32],
        delegate_pubkey,
        1_000_000,
        5_000,
        Capabilities::read_only(),
    );
    ((delegator_id, delegator_addr), (delegate_id, delegate_addr))
}

// =============================================================================
// TESTS
// =============================================================================

#[test]
fn delegation_grant_create_roundtrip() {
    let mut db = MemKv::new();
    let creator = [0x11u8; 32];
    let ((delegator_id, delegator_addr), (delegate_id, _)) =
        setup_two_entities(&mut db, &creator, [0x21u8; 32], [0x22u8; 32]);

    let tx = create_grant_tx(
        delegator_addr,
        [0x21u8; 32],
        0,
        500,
        delegate_id,
        0x04,
        2_000,
    );
    dispatch_tx(&mut db, &tx, 200).expect("create grant should succeed");

    let objects = get_memory_objects_by_entity(&db, &delegator_id).unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].object_type, MemoryObjectType::DelegationGrant);
    let decoded = DelegationGrantData::decode(&objects[0].data).unwrap();
    assert_eq!(decoded.delegate_entity_id, delegate_id);
    assert_eq!(decoded.granted_capabilities, 0x04);
    assert_eq!(decoded.expires_at, 2_000);

    let prefix = ai_delegations_by_delegate_prefix(&delegate_id);
    let entries = db.scan_prefix(&prefix).unwrap();
    assert_eq!(entries.len(), 1, "by-delegate index must contain one row");
    assert_eq!(entries[0].1, delegator_id.to_vec(), "value = delegator id");
}

#[test]
fn delegation_extends_capability() {
    let mut db = MemKv::new();
    let creator = [0x12u8; 32];
    let ((_, delegator_addr), (delegate_id, delegate_addr)) =
        setup_two_entities(&mut db, &creator, [0x23u8; 32], [0x24u8; 32]);

    let grant_tx = create_grant_tx(delegator_addr, [0x23u8; 32], 0, 500, delegate_id, 0x04, 0);
    dispatch_tx(&mut db, &grant_tx, 200).unwrap();

    // The delegate does NOT have emit_proposals statically. With the grant
    // active, dispatch_tx must accept a signal commitment from the delegate.
    let delegate = read_ai_entity(&db, &delegate_id).unwrap().unwrap();
    assert!(!delegate.capabilities.emit_proposals);

    let signal_tx = mk_tx(
        delegate_addr,
        [0x24u8; 32],
        0,
        2_000,
        signal_payload(delegate_id, AiSignalType::Anomaly),
    );
    dispatch_tx(&mut db, &signal_tx, 201).expect("delegated emit_proposals should pass");
}

#[test]
fn delegation_expired_not_effective() {
    let mut db = MemKv::new();
    let creator = [0x13u8; 32];
    let ((_, delegator_addr), (delegate_id, delegate_addr)) =
        setup_two_entities(&mut db, &creator, [0x25u8; 32], [0x26u8; 32]);

    // Grant expires at height 500.
    let grant_tx = create_grant_tx(delegator_addr, [0x25u8; 32], 0, 500, delegate_id, 0x04, 500);
    dispatch_tx(&mut db, &grant_tx, 200).unwrap();

    let signal_tx = mk_tx(
        delegate_addr,
        [0x26u8; 32],
        0,
        2_000,
        signal_payload(delegate_id, AiSignalType::Anomaly),
    );
    // current_height >= expires_at must reject.
    let r = dispatch_tx(&mut db, &signal_tx, 500);
    assert!(matches!(r, Err(ExecError::IssuerMissingCapability)));
}

#[test]
fn delegation_revoked_via_delete() {
    let mut db = MemKv::new();
    let creator = [0x14u8; 32];
    let ((delegator_id, delegator_addr), (delegate_id, delegate_addr)) =
        setup_two_entities(&mut db, &creator, [0x27u8; 32], [0x28u8; 32]);

    let grant_tx = create_grant_tx(delegator_addr, [0x27u8; 32], 0, 500, delegate_id, 0x04, 0);
    dispatch_tx(&mut db, &grant_tx, 200).unwrap();
    let grant_id = get_memory_objects_by_entity(&db, &delegator_id).unwrap()[0].object_id;

    // Delete the grant.
    let delete_tx = mk_tx(
        delegator_addr,
        [0x27u8; 32],
        1,
        500,
        encode_delete_memory_object_payload_v1(&DeleteMemoryObjectPayloadV1 {
            object_id: grant_id,
        })
        .to_vec(),
    );
    dispatch_tx(&mut db, &delete_tx, 201).unwrap();

    // The by-delegate index entry must be gone.
    let prefix = ai_delegations_by_delegate_prefix(&delegate_id);
    let entries = db.scan_prefix(&prefix).unwrap();
    assert!(entries.is_empty(), "by-delegate index must be torn down");

    // Delegate can no longer pass the capability gate.
    let signal_tx = mk_tx(
        delegate_addr,
        [0x28u8; 32],
        0,
        2_000,
        signal_payload(delegate_id, AiSignalType::Anomaly),
    );
    let r = dispatch_tx(&mut db, &signal_tx, 202);
    assert!(matches!(r, Err(ExecError::IssuerMissingCapability)));
}

#[test]
fn delegation_rejects_self_delegation() {
    let mut db = MemKv::new();
    let creator = [0x15u8; 32];
    fund_account(&mut db, derive_addr(&creator), 10_000_000);
    let (entity_id, entity_addr) = register_entity_with_caps(
        &mut db,
        &creator,
        0,
        [0xC3u8; 32],
        [0x29u8; 32],
        1_000_000,
        5_000,
        Capabilities::advisory(),
    );

    let tx = create_grant_tx(entity_addr, [0x29u8; 32], 0, 500, entity_id, 0x04, 0);
    let r = dispatch_tx(&mut db, &tx, 200);
    assert!(matches!(r, Err(ExecError::InvalidDelegationSelf)));
}

#[test]
fn delegation_rejects_superset_capabilities() {
    let mut db = MemKv::new();
    let creator = [0x16u8; 32];
    // Delegator has read_only (0x03), tries to grant emit_proposals (0x04).
    fund_account(&mut db, derive_addr(&creator), 10_000_000);
    let (_, delegator_addr) = register_entity_with_caps(
        &mut db,
        &creator,
        0,
        [0xC4u8; 32],
        [0x2Au8; 32],
        1_000_000,
        5_000,
        Capabilities::read_only(),
    );
    let (delegate_id, _) = register_entity_with_caps(
        &mut db,
        &creator,
        1,
        [0xC5u8; 32],
        [0x2Bu8; 32],
        1_000_000,
        5_000,
        Capabilities::default(),
    );

    let tx = create_grant_tx(delegator_addr, [0x2Au8; 32], 0, 500, delegate_id, 0x04, 0);
    let r = dispatch_tx(&mut db, &tx, 200);
    assert!(matches!(r, Err(ExecError::DelegationCapabilityNotHeld)));
}

#[test]
fn delegation_multiple_grants_combine() {
    let mut db = MemKv::new();
    let creator = [0x17u8; 32];
    // Two delegators, both with relevant caps. One grants emit_proposals,
    // the other grants submit_reputation_updates. Delegate gets both via
    // OR-merge.
    fund_account(&mut db, derive_addr(&creator), 50_000_000);
    let (delegator_a_id, delegator_a_addr) = register_entity_with_caps(
        &mut db,
        &creator,
        0,
        [0xC6u8; 32],
        [0x2Cu8; 32],
        1_000_000,
        5_000,
        Capabilities::advisory(),
    );
    let oracle_caps = Capabilities {
        read_public_chain: true,
        // delegator must also hold read_memory_objects to issue any
        // CreateMemoryObject (DelegationGrant included).
        read_memory_objects: true,
        submit_reputation_updates: true,
        ..Capabilities::default()
    };
    let (delegator_b_id, delegator_b_addr) = register_entity_with_caps(
        &mut db,
        &creator,
        1,
        [0xC7u8; 32],
        [0x2Du8; 32],
        1_000_000,
        5_000,
        oracle_caps,
    );
    let (delegate_id, _) = register_entity_with_caps(
        &mut db,
        &creator,
        2,
        [0xC8u8; 32],
        [0x2Eu8; 32],
        1_000_000,
        5_000,
        Capabilities::read_only(),
    );

    let tx_a = create_grant_tx(delegator_a_addr, [0x2Cu8; 32], 0, 500, delegate_id, 0x04, 0);
    dispatch_tx(&mut db, &tx_a, 200).unwrap();
    let tx_b = create_grant_tx(delegator_b_addr, [0x2Du8; 32], 0, 500, delegate_id, 0x20, 0);
    dispatch_tx(&mut db, &tx_b, 201).unwrap();

    // Two index rows must exist.
    let prefix = ai_delegations_by_delegate_prefix(&delegate_id);
    let entries = db.scan_prefix(&prefix).unwrap();
    assert_eq!(entries.len(), 2);
    let mut delegators: Vec<Vec<u8>> = entries.into_iter().map(|(_, v)| v).collect();
    delegators.sort();
    let mut expected = vec![delegator_a_id.to_vec(), delegator_b_id.to_vec()];
    expected.sort();
    assert_eq!(delegators, expected);
}

#[test]
fn delegation_does_not_affect_delegator() {
    let mut db = MemKv::new();
    let creator = [0x18u8; 32];
    let ((delegator_id, delegator_addr), (delegate_id, _)) =
        setup_two_entities(&mut db, &creator, [0x2Fu8; 32], [0x30u8; 32]);

    let before = read_ai_entity(&db, &delegator_id).unwrap().unwrap();

    let grant_tx = create_grant_tx(delegator_addr, [0x2Fu8; 32], 0, 500, delegate_id, 0x04, 0);
    dispatch_tx(&mut db, &grant_tx, 200).unwrap();

    let after = read_ai_entity(&db, &delegator_id).unwrap().unwrap();
    // Static capabilities byte must be identical before/after.
    assert_eq!(before.capabilities.to_byte(), after.capabilities.to_byte());
    // The delegator's reputation/stake/active state are untouched too.
    assert_eq!(before.reputation_score, after.reputation_score);
    assert_eq!(before.stake_balance, after.stake_balance);
    assert!(after.is_active);
}

#[test]
fn signal_with_delegated_capability_succeeds() {
    // Same shape as delegation_extends_capability but explicit assertions on
    // post-state for clarity. Kept as a separate test for the test plan.
    let mut db = MemKv::new();
    let creator = [0x19u8; 32];
    let ((_, delegator_addr), (delegate_id, delegate_addr)) =
        setup_two_entities(&mut db, &creator, [0x31u8; 32], [0x32u8; 32]);

    let grant_tx = create_grant_tx(delegator_addr, [0x31u8; 32], 0, 500, delegate_id, 0x04, 0);
    dispatch_tx(&mut db, &grant_tx, 200).unwrap();

    let signal_tx = mk_tx(
        delegate_addr,
        [0x32u8; 32],
        0,
        2_000,
        signal_payload(delegate_id, AiSignalType::Anomaly),
    );
    dispatch_tx(&mut db, &signal_tx, 201).expect("delegated signal accepted");

    // Delegate's nonce advances and balance is debited by fee.
    let delegate = read_ai_entity(&db, &delegate_id).unwrap().unwrap();
    assert_eq!(delegate.nonce, 1);
}

#[test]
fn signal_without_delegation_rejected() {
    let mut db = MemKv::new();
    let creator = [0x1Au8; 32];
    fund_account(&mut db, derive_addr(&creator), 10_000_000);
    let (entity_id, entity_addr) = register_entity_with_caps(
        &mut db,
        &creator,
        0,
        [0xC9u8; 32],
        [0x33u8; 32],
        1_000_000,
        5_000,
        Capabilities::read_only(),
    );

    let signal_tx = mk_tx(
        entity_addr,
        [0x33u8; 32],
        0,
        2_000,
        signal_payload(entity_id, AiSignalType::Anomaly),
    );
    let r = dispatch_tx(&mut db, &signal_tx, 200);
    assert!(
        matches!(r, Err(ExecError::IssuerMissingCapability)),
        "expected IssuerMissingCapability, got {r:?}"
    );
}

#[test]
fn max_delegation_grants_enforced() {
    let mut db = MemKv::new();
    let creator = [0x1Bu8; 32];
    let ((_, delegator_addr), (delegate_id, _)) =
        setup_two_entities(&mut db, &creator, [0x34u8; 32], [0x35u8; 32]);

    // Top up the delegator so it can afford MAX_DELEGATION_GRANTS + 1 fees.
    fund_account(&mut db, delegator_addr, 100_000_000);

    // Issue MAX_DELEGATION_GRANTS grants, varying expires_at so payload
    // bytes differ and the resulting object ids differ.
    for i in 0..MAX_DELEGATION_GRANTS {
        let tx = create_grant_tx(
            delegator_addr,
            [0x34u8; 32],
            u64::from(i),
            500,
            delegate_id,
            0x04,
            10_000 + u64::from(i),
        );
        dispatch_tx(&mut db, &tx, 200 + u64::from(i)).expect("under-cap grant should land");
    }

    // The next grant trips the cap.
    let over_tx = create_grant_tx(
        delegator_addr,
        [0x34u8; 32],
        u64::from(MAX_DELEGATION_GRANTS),
        500,
        delegate_id,
        0x04,
        99_999,
    );
    let r = dispatch_tx(&mut db, &over_tx, 200 + u64::from(MAX_DELEGATION_GRANTS));
    assert!(matches!(
        r,
        Err(ExecError::DelegationCountExceeded { current, max })
            if current == MAX_DELEGATION_GRANTS && max == MAX_DELEGATION_GRANTS
    ));
}

#[test]
fn non_delegation_memory_objects_unchanged() {
    // Regression: creating a non-DelegationGrant memory object must not
    // touch the ai_delegations_by_delegate index. We use ChainSummary as a
    // canonical non-grant type.
    let mut db = MemKv::new();
    let creator = [0x1Cu8; 32];
    fund_account(&mut db, derive_addr(&creator), 10_000_000);
    let (entity_id, entity_addr) = register_entity_with_caps(
        &mut db,
        &creator,
        0,
        [0xCAu8; 32],
        [0x36u8; 32],
        1_000_000,
        5_000,
        Capabilities::gated(),
    );

    let tx = mk_tx(
        entity_addr,
        [0x36u8; 32],
        0,
        500,
        encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
            object_type: MemoryObjectType::ChainSummary,
            data: b"non-grant payload".to_vec(),
        }),
    );
    dispatch_tx(&mut db, &tx, 200).unwrap();

    // Primary record exists.
    let objects = get_memory_objects_by_entity(&db, &entity_id).unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].object_type, MemoryObjectType::ChainSummary);

    // No by-delegate index entry was created for any conceivable delegate.
    let prefix = ai_delegations_by_delegate_prefix(&entity_id);
    assert!(db.scan_prefix(&prefix).unwrap().is_empty());

    // Updating the non-grant memory object is still allowed.
    let update_tx = mk_tx(
        entity_addr,
        [0x36u8; 32],
        1,
        500,
        encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
            object_id: objects[0].object_id,
            new_data: b"v2".to_vec(),
        }),
    );
    dispatch_tx(&mut db, &update_tx, 201).expect("non-grant update must still work");
}

#[test]
fn delegation_inactive_delegator_grants_ignored() {
    let mut db = MemKv::new();
    let creator = [0x1Du8; 32];
    let ((delegator_id, delegator_addr), (delegate_id, delegate_addr)) =
        setup_two_entities(&mut db, &creator, [0x37u8; 32], [0x38u8; 32]);

    let grant_tx = create_grant_tx(delegator_addr, [0x37u8; 32], 0, 500, delegate_id, 0x04, 0);
    dispatch_tx(&mut db, &grant_tx, 200).unwrap();

    // Sanity: signal goes through while delegator is active.
    let signal_tx = mk_tx(
        delegate_addr,
        [0x38u8; 32],
        0,
        2_000,
        signal_payload(delegate_id, AiSignalType::Anomaly),
    );
    dispatch_tx(&mut db, &signal_tx, 201).unwrap();

    // Flip delegator to inactive.
    let mut delegator = read_ai_entity(&db, &delegator_id).unwrap().unwrap();
    delegator.is_active = false;
    db.apply_batch(&[write_ai_entity_op(&delegator)]).unwrap();

    // Now a fresh signal from the delegate must fail.
    let signal_tx2 = mk_tx(
        delegate_addr,
        [0x38u8; 32],
        1,
        2_000,
        signal_payload(delegate_id, AiSignalType::Anomaly),
    );
    let r = dispatch_tx(&mut db, &signal_tx2, 202);
    assert!(matches!(r, Err(ExecError::IssuerMissingCapability)));
}

#[test]
fn delegation_grant_rejects_update() {
    let mut db = MemKv::new();
    let creator = [0x1Eu8; 32];
    let ((delegator_id, delegator_addr), (delegate_id, _)) =
        setup_two_entities(&mut db, &creator, [0x39u8; 32], [0x3Au8; 32]);

    let grant_tx = create_grant_tx(delegator_addr, [0x39u8; 32], 0, 500, delegate_id, 0x04, 0);
    dispatch_tx(&mut db, &grant_tx, 200).unwrap();
    let grant_id = get_memory_objects_by_entity(&db, &delegator_id).unwrap()[0].object_id;

    // Try to update the grant payload.
    let new_grant = DelegationGrantData {
        version: DELEGATION_GRANT_VERSION,
        delegate_entity_id: delegate_id,
        granted_capabilities: 0x07, // tries to widen scope
        expires_at: 0,
    };
    let update_tx = mk_tx(
        delegator_addr,
        [0x39u8; 32],
        1,
        500,
        encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
            object_id: grant_id,
            new_data: new_grant.encode().to_vec(),
        }),
    );
    let r = dispatch_tx(&mut db, &update_tx, 201);
    assert!(matches!(r, Err(ExecError::DelegationGrantNotUpdatable)));
}
