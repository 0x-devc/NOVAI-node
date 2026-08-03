//! Test G (gate wedge-276272): golden equivalence of the non-persisting overlay
//! executor and the persisting executor.
//!
//! `execute_block_to_root` (overlay) and the commit-path executor run the
//! IDENTICAL `dispatch_tx` per-tx pipeline; the only difference is the backend (a
//! read-through `BlockOverlay` over a view versus a real `MemKv`). This pins the
//! two-backend byte-identity the wedge-276272 fix depends on: a block's
//! post-execution root computed at vote time in the overlay must equal the root
//! computed at commit time in the persisting store, and the buffered write set
//! must reproduce the persisting store's rows exactly. If they can drift,
//! determinism is not proven, so this is the permanent guard the primitive (and
//! accelerate Stage B) is measured against.
//!
//! Coverage: `golden_..._mixed` exercises read-through `get`, a same-sender
//! read-across-tx pair, and a second handler family (AI entity registration),
//! comparing root and full rows byte-for-byte. `golden_..._governance_two_walk`
//! pins the two-batches-per-tx handler (governance execute). `overlay_scan_prefix
//! _matches_direct` pins the `scan_prefix` merge (overrides, tombstones, order)
//! against a `MemKv` oracle.

use novai_ai_entities::{AiEntity, ApprovalGate, AutonomyMode, Capabilities, GateType};
use novai_codec::encode_approval_gate_v1;
use novai_execution::{
    append_smt_ops_for_state_ops, apply_governance_submit_tx, dispatch_tx,
    encode_execute_proposal_payload_v1, encode_register_ai_entity_payload_v1,
    encode_submit_proposal_payload_v1, encode_transfer_payload_v1, execute_block_to_root,
    write_ai_entity_op, BlockOverlay, ExecuteProposalPayloadV1, RegisterAiEntityPayloadV1,
    SubmitProposalPayloadV1, TransferPayloadV1, TxOutcome,
};
use novai_governance::ProposalType;
use novai_state::{
    account_key, ai_entity_by_address_key, approval_gate_key, decode_smt_root_v1,
    encode_account_v1, encode_fee_pool_v1, AccountStateV1, FeePoolV1, Kv, KvBatch, MemKv, WriteOp,
    KEY_FEE_POOL, KEY_SMT_ROOT,
};
use novai_types::{Address, TxV1, TxVersion};

const HEIGHT: u64 = 1_000;

/// Execution does not verify signatures (verify_block does, separately), so a
/// dummy pubkey and empty signature are correct for exercising `dispatch_tx`.
fn tx(from: Address, nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
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

fn transfer(from: Address, nonce: u64, fee: u64, to: Address, amount: u64) -> TxV1 {
    tx(
        from,
        nonce,
        fee,
        encode_transfer_payload_v1(&TransferPayloadV1 { to, amount }).to_vec(),
    )
}

fn acct(balance: u128, nonce: u64) -> Vec<u8> {
    encode_account_v1(&AccountStateV1 { balance, nonce }).to_vec()
}

/// Seed rows through the canonical SMT path so the starting state carries a
/// consistent `KEY_SMT_ROOT`, exactly as a live chain would.
fn seed(db: &mut MemKv, pairs: &[(Vec<u8>, Vec<u8>)]) {
    for (k, v) in pairs {
        let ops = vec![WriteOp::Put(k.clone(), v.clone())];
        let mut all = ops.clone();
        append_smt_ops_for_state_ops(db, &ops, &mut all).expect("append smt ops");
        db.apply_batch(&all).expect("apply seed batch");
    }
}

fn read_root(db: &MemKv) -> [u8; 32] {
    match db.get(KEY_SMT_ROOT).unwrap() {
        Some(b) => decode_smt_root_v1(&b).unwrap(),
        None => novai_execution::empty_smt_root(),
    }
}

/// Every row across both column families, deterministically sorted. `MemKv`
/// routes scans by the prefix namespace, so scanning the empty prefix and the
/// nnpx prefix together covers the whole store.
fn all_rows(db: &MemKv) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut rows = db.scan_prefix(b"").unwrap();
    rows.extend(db.scan_prefix(b"nnpx/").unwrap());
    rows.sort();
    rows
}

/// Run `txs` at `HEIGHT` through the persisting executor (A) and the overlay
/// executor (B) from the same starting state, and assert byte-identity of the
/// post root and the full row set, and that every tx applied on both.
fn assert_golden(s0: &MemKv, txs: &[TxV1]) {
    // Backend A: persisting. Same dispatch_tx, real MemKv. Every tx must apply.
    let mut db_a = s0.clone();
    for (i, t) in txs.iter().enumerate() {
        dispatch_tx(&mut db_a, t, HEIGHT)
            .unwrap_or_else(|e| panic!("persisting dispatch of tx {i} must apply: {e:?}"));
    }
    let root_a = read_root(&db_a);
    let rows_a = all_rows(&db_a);

    // Backend B: overlay. Same dispatch_tx, read-through BlockOverlay over s0.
    let exec = execute_block_to_root(s0, txs, HEIGHT).expect("overlay execute");
    let mut db_b = s0.clone();
    db_b.apply_batch(&exec.write_ops())
        .expect("apply overlay write set");
    let rows_b = all_rows(&db_b);

    assert_eq!(
        root_a, exec.post_root,
        "two-backend ROOT drift: overlay and persisting executors disagree.\n a={root_a:02x?}\n b={:02x?}",
        exec.post_root
    );
    assert_eq!(
        rows_a, rows_b,
        "two-backend ROW drift: overlay write set does not reproduce the persisting store"
    );
    assert!(
        exec.outcomes.iter().all(|o| *o == TxOutcome::Applied),
        "every tx must apply for the golden comparison to be non-vacuous; outcomes={:?}",
        exec.outcomes
    );
}

#[test]
fn golden_overlay_matches_persisting_mixed() {
    let sender: Address = [0x11; 32];
    let recipient: Address = [0x22; 32];
    let creator: Address = [0x33; 32];

    let mut s0 = MemKv::new();
    seed(
        &mut s0,
        &[
            (account_key(&sender), acct(100_000_000, 0)),
            (account_key(&recipient), acct(1_000_000, 0)),
            (account_key(&creator), acct(100_000_000, 0)),
            (
                KEY_FEE_POOL.to_vec(),
                encode_fee_pool_v1(&FeePoolV1 { balance: 0 }).to_vec(),
            ),
        ],
    );

    let register_payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash: [0x42; 32],
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::gated(),
        initial_balance: 500,
    })
    .to_vec();

    // Fees clear every handler's minimum (registration requires 5000).
    let txs = vec![
        transfer(sender, 0, 10_000, recipient, 1_000), // simple transfer
        transfer(sender, 1, 10_000, recipient, 2_000), // read-across-tx: reads tx0's nonce/balance
        tx(creator, 0, 10_000, register_payload),       // a second handler family
    ];

    assert_golden(&s0, &txs);
}

#[test]
fn golden_overlay_matches_persisting_governance_two_walk() {
    // S0 carries a module (inactive), a governance submitter, a timelock gate,
    // and a ModuleActivation proposal already submitted at height 1. The golden
    // block EXECUTES that proposal (the two-walk, two-batches-per-tx handler)
    // after the timelock has elapsed.
    let gate_id = *blake3::hash(b"NOVAI_TESTNET_GATE_V1").as_bytes();

    let mut module = AiEntity::new(
        *blake3::hash(b"GOLDEN_MODULE_V1").as_bytes(),
        *blake3::hash(b"GOLDEN_MODULE_CREATOR").as_bytes(),
        AutonomyMode::Gated,
        Capabilities::gated(),
        0,
    );
    module.economic_balance = 10_000_000;
    module.is_active = false;
    let module_id = module.id;

    let gov_account = *blake3::hash(b"GOLDEN_GOVERNANCE").as_bytes();
    let gov_entity = AiEntity::new(
        *blake3::hash(b"GOLDEN_GOV_MODULE").as_bytes(),
        gov_account,
        AutonomyMode::Gated,
        Capabilities::gated(),
        0,
    );

    let gate = ApprovalGate {
        gate_id,
        gate_type: GateType::TimelockOnly,
        required_approvers: Vec::new(),
        threshold: 0,
        timelock_blocks: 10,
        expiry_blocks: 100_000,
        veto_enabled: false,
        freeze_enabled: false,
    };

    let mut s0 = MemKv::new();
    s0.apply_batch(&[
        write_ai_entity_op(&module),
        WriteOp::Put(ai_entity_by_address_key(&module.id), module.id.to_vec()),
        write_ai_entity_op(&gov_entity),
        WriteOp::Put(
            ai_entity_by_address_key(&gov_entity.id),
            gov_entity.id.to_vec(),
        ),
        WriteOp::Put(approval_gate_key(&gate.gate_id), encode_approval_gate_v1(&gate)),
        WriteOp::Put(account_key(&gov_account), acct(100_000_000, 0)),
        WriteOp::Put(KEY_FEE_POOL.to_vec(), encode_fee_pool_v1(&FeePoolV1 { balance: 0 }).to_vec()),
    ])
    .unwrap();

    let submit_payload = encode_submit_proposal_payload_v1(&SubmitProposalPayloadV1 {
        proposal_type: ProposalType::ModuleActivation,
        gate_id,
        proposal_data: module_id.to_vec(),
    });
    let submit_tx = tx(gov_account, 0, 1_000, submit_payload);
    let proposal_id = apply_governance_submit_tx(&mut s0, &submit_tx, 1).expect("submit proposal");

    let execute_payload =
        encode_execute_proposal_payload_v1(&ExecuteProposalPayloadV1 { proposal_id }).to_vec();
    let execute_tx = tx(gov_account, 1, 1_000, execute_payload);

    assert_golden(&s0, &[execute_tx]);
}

#[test]
fn overlay_scan_prefix_matches_direct() {
    // A view with rows in and out of the target prefix.
    let mut view = MemKv::new();
    for (k, v) in [
        (b"acc/1".to_vec(), b"a".to_vec()),
        (b"acc/2".to_vec(), b"b".to_vec()),
        (b"acc/3".to_vec(), b"c".to_vec()),
        (b"zzz/1".to_vec(), b"z".to_vec()),
    ] {
        view.put(&k, &v).unwrap();
    }
    // Oracle: a MemKv that receives the same base plus the same ops.
    let mut mirror = view.clone();

    let ops = vec![
        WriteOp::Put(b"acc/2".to_vec(), b"B2".to_vec()), // override a view row
        WriteOp::Put(b"acc/4".to_vec(), b"d".to_vec()),  // new row within prefix
        WriteOp::Delete(b"acc/1".to_vec()),              // tombstone hides a view row
    ];
    let mut overlay = BlockOverlay::new(&view);
    overlay.apply_batch(&ops).unwrap();
    mirror.apply_batch(&ops).unwrap();

    assert_eq!(
        overlay.scan_prefix(b"acc/").unwrap(),
        mirror.scan_prefix(b"acc/").unwrap(),
        "scan_prefix over the target prefix must match the oracle (overrides, tombstone, order)"
    );
    assert_eq!(
        overlay.scan_prefix(b"").unwrap(),
        mirror.scan_prefix(b"").unwrap(),
        "scan_prefix over the whole default space must match the oracle"
    );
    for k in [
        b"acc/1".as_slice(),
        b"acc/2",
        b"acc/3",
        b"acc/4",
        b"zzz/1",
        b"nope",
    ] {
        assert_eq!(
            overlay.get(k).unwrap(),
            mirror.get(k).unwrap(),
            "get must match the oracle at {k:?} (including the tombstone)"
        );
    }
}
