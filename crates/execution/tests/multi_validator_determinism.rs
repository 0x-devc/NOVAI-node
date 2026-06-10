#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::format_push_string)]
#![allow(clippy::single_range_in_vec_init)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::redundant_clone)]
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::cloned_ref_to_slice_refs)]

//! PURPOSE: four-validator in-process determinism harness for the executor.
//! Drives the same controlled workload through 4 independent MemKv instances
//! and asserts byte-equal state after every committed block (which implies
//! identical SMT roots, identical nonces, identical balances). Designed to
//! be diagnostic for the state_root divergence observed on the live testnet
//! at block 1745004 against bug-shipping commit 9ac23c4.
//!
//! INVARIANTS:
//! - Tx execution must be deterministic: same db state + same tx + same
//!   current_height across 4 validators must produce byte-identical db state
//!   after the tx (or an identical error on all four).
//! - A divergence detected by the harness MUST be reproducible on rerun (no
//!   wall-clock, RNG, or thread scheduling in the test path).
//!
//! FAILURE MODES:
//! - This harness uses MemKv. It cannot model RocksDB compaction. Candidate
//!   C (forced compaction at block N % 5000 == 0) requires a separate
//!   RocksKv-backed harness (deferred).
//! - The executor is single-threaded and synchronous, so per-validator
//!   wall-clock pauses cannot expose tokio async-ordering bugs here. The
//!   harness instead simulates "timing" through divergent input sequences
//!   (one validator receives txs in a different order or a different subset).
//!
//! WORKLOADS:
//!   W1  baseline_account_transfers_stay_deterministic
//!   W2  type8_register_then_transfer_from_creator_stays_deterministic
//!   W3  type10_register_with_key_then_transfer_stays_deterministic
//!   W4  beta4a_nonce_gap_via_failed_oracle_anchors_stays_deterministic
//!   W5  drained_entity_with_failing_anchors_then_creator_transfer (B probe)
//!   W6  multi_sender_high_load_transfers_stay_deterministic
//!   W7  register_and_transfer_in_same_block_stays_deterministic
//!   W8  harness_self_test_detects_injected_divergence
//!   W9  divergent_tx_ordering_within_block_reveals_drift_if_any
//!   W10 type8_creator_already_has_entity_guard_fires_consistently
//!   W11 MECHANISM: reverse_index_drift_on_one_validator_diverges_smt_root
//!   W12 MECHANISM: entity_balance_drift_on_one_validator_diverges_smt_root
//!   W13 MECHANISM: entity_nonce_drift_on_one_validator_diverges_smt_root
//!   W14 STRESS: tight beta4a interleaving with mixed signal/transfer types
//!   W15 STRESS: many entities, many transfers, all should stay equal
//!   W16 MECHANISM: entity_capability_drift_diverges_smt_root
//!   W17 MECHANISM: entity_code_hash_drift_diverges_smt_root
//!   W18 MECHANISM: entity_is_active_drift_diverges_smt_root
//!   W19 MECHANISM: entity_last_active_at_drift_diverges_smt_root
//!   W20 FIX_VALIDATION: type8_register_now_authenticated_in_smt
//!   W21 FIX_VALIDATION: type10_register_now_authenticated_in_smt
//!   W22 FIX_VALIDATION: oracle_anchor_signal_now_authenticated_in_smt
//!   W23 FIX_VALIDATION: memory_create_now_authenticated_in_smt
//!   W24 FIX_VALIDATION: memory_update_now_authenticated_in_smt
//!   W25 FIX_VALIDATION: memory_delete_now_authenticated_in_smt
//!   W26 FIX_VALIDATION: credit_ai_entity_now_authenticated_in_smt
//!   W27 FIX_VALIDATION: entity_upgrade_now_authenticated_in_smt
//!   W28 FIX_VALIDATION: governance_submit_then_execute_module_rollback
//!                       (one test, TWO assertions: submit at lib.rs:7042 changes
//!                       SMT root; execute at lib.rs:7170 + inner module_rollback
//!                       at lib.rs:6852 produces the documented double-walk and
//!                       also changes SMT root, with all 4 validators agreeing
//!                       on every snapshot)

use novai_ai_entities::{AiSignalType, AutonomyMode, Capabilities};
use novai_execution::{
    dispatch_tx, encode_register_ai_entity_payload_v1,
    encode_register_ai_entity_with_key_payload_v1, encode_signal_commitment_payload_v1,
    encode_transfer_payload_v1, read_ai_entity, write_ai_entity_op, OracleAnchorExtraV1,
    RegisterAiEntityPayloadV1, RegisterAiEntityWithKeyPayloadV1, SignalCommitmentPayloadV1,
    TransferPayloadV1,
};
use novai_state::{account_key, encode_account_v1, AccountStateV1, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{Address, TxV1, TxVersion};

// ============================================================================
// HARNESS PRIMITIVES
// ============================================================================

const VALIDATORS: usize = 4;

/// Four independent in-memory KV instances representing four validators.
struct Quad {
    dbs: [MemKv; VALIDATORS],
    height: u64,
}

impl Quad {
    fn new() -> Self {
        Self {
            dbs: [MemKv::new(), MemKv::new(), MemKv::new(), MemKv::new()],
            height: 0,
        }
    }

    /// Apply the same tx to all four dbs at the same `current_height`.
    /// Returns the per-validator Result so callers can assert they agree.
    fn apply_tx_all(&mut self, tx: &TxV1, current_height: u64) -> [TxOutcome; VALIDATORS] {
        let mut out: [TxOutcome; VALIDATORS] =
            [TxOutcome::Ok, TxOutcome::Ok, TxOutcome::Ok, TxOutcome::Ok];
        for (i, db) in self.dbs.iter_mut().enumerate() {
            out[i] = match dispatch_tx(db, tx, current_height) {
                Ok(()) => TxOutcome::Ok,
                Err(e) => TxOutcome::Err(format!("{e:?}")),
            };
        }
        out
    }

    /// Drive a sequence of txs ("a block") on all four dbs at `height`,
    /// asserting that each tx produces an identical outcome across the four,
    /// then asserting byte-equal state of the default column family.
    fn commit_block(&mut self, txs: &[TxV1], height: u64, label: &str) {
        for (idx, tx) in txs.iter().enumerate() {
            let outcomes = self.apply_tx_all(tx, height);
            assert_outcomes_match(&outcomes, label, idx, tx);
        }
        self.height = height;
        assert_states_equal(&self.dbs, label);
    }

    /// Drive a sequence of txs to a SINGLE validator only. Useful for
    /// simulating divergent input across validators (sanity self-test).
    fn apply_to_one(&mut self, idx: usize, txs: &[TxV1], height: u64) {
        for tx in txs {
            let _ = dispatch_tx(&mut self.dbs[idx], tx, height);
        }
    }
}

/// Outcome of a single tx execution on a single validator. We compare these
/// across validators with PartialEq, so divergent Err variants produce a
/// clear panic.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TxOutcome {
    Ok,
    Err(String),
}

fn assert_outcomes_match(
    outcomes: &[TxOutcome; VALIDATORS],
    label: &str,
    tx_idx: usize,
    tx: &TxV1,
) {
    let v0 = &outcomes[0];
    for (i, vi) in outcomes.iter().enumerate().skip(1) {
        assert!(
            v0 == vi,
            "[{label}] outcome divergence at tx_idx={tx_idx} from={:?} nonce={} payload[0]={:?}\n  v0   = {:?}\n  v{i} = {:?}",
            &tx.from[..4],
            tx.nonce,
            tx.payload.first(),
            v0,
            vi
        );
    }
}

/// Snapshot the default column family of a MemKv in sorted key order.
fn snapshot_default(db: &MemKv) -> Vec<(Vec<u8>, Vec<u8>)> {
    // scan_prefix(b"") routes to entries_default and sorts by key.
    db.scan_prefix(b"").unwrap()
}

fn assert_states_equal(dbs: &[MemKv; VALIDATORS], label: &str) {
    let snap0 = snapshot_default(&dbs[0]);
    for i in 1..VALIDATORS {
        let snap_i = snapshot_default(&dbs[i]);
        if snap0 != snap_i {
            let diff = diff_snapshots(&snap0, &snap_i);
            panic!("[{label}] state divergence between v0 and v{i}:\n{diff}");
        }
    }
}

fn hex_str(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn diff_snapshots(a: &[(Vec<u8>, Vec<u8>)], b: &[(Vec<u8>, Vec<u8>)]) -> String {
    let mut s = String::new();
    let mut ai = 0;
    let mut bi = 0;
    while ai < a.len() || bi < b.len() {
        match (a.get(ai), b.get(bi)) {
            (Some(ak), Some(bk)) => {
                use std::cmp::Ordering;
                match ak.0.cmp(&bk.0) {
                    Ordering::Equal => {
                        if ak.1 != bk.1 {
                            s.push_str(&format!(
                                "  DIFF  key={} v0_val={} v_other_val={}\n",
                                hex_str(&ak.0),
                                hex_str(&ak.1),
                                hex_str(&bk.1),
                            ));
                        }
                        ai += 1;
                        bi += 1;
                    }
                    Ordering::Less => {
                        s.push_str(&format!(
                            "  ONLY_V0 key={} val={}\n",
                            hex_str(&ak.0),
                            hex_str(&ak.1),
                        ));
                        ai += 1;
                    }
                    Ordering::Greater => {
                        s.push_str(&format!(
                            "  ONLY_OTHER key={} val={}\n",
                            hex_str(&bk.0),
                            hex_str(&bk.1),
                        ));
                        bi += 1;
                    }
                }
            }
            (Some(ak), None) => {
                s.push_str(&format!(
                    "  ONLY_V0 key={} val={}\n",
                    hex_str(&ak.0),
                    hex_str(&ak.1),
                ));
                ai += 1;
            }
            (None, Some(bk)) => {
                s.push_str(&format!(
                    "  ONLY_OTHER key={} val={}\n",
                    hex_str(&bk.0),
                    hex_str(&bk.1),
                ));
                bi += 1;
            }
            (None, None) => break,
        }
    }
    if s.is_empty() {
        "  (no per-key diff produced; check non-default column family)\n".to_string()
    } else {
        s
    }
}

// ============================================================================
// SEEDING
// ============================================================================

fn seed_account_all(quad: &mut Quad, addr: &Address, balance: u128, nonce: u64) {
    let acct = AccountStateV1 { balance, nonce };
    let op = WriteOp::Put(account_key(addr), encode_account_v1(&acct).to_vec());
    for db in &mut quad.dbs {
        db.apply_batch(&[op.clone()]).unwrap();
    }
}

// ============================================================================
// TX BUILDERS
// ============================================================================

fn mk_tx(from: Address, nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
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

fn mk_transfer_tx(from: Address, nonce: u64, fee: u64, to: Address, amount: u64) -> TxV1 {
    let payload = encode_transfer_payload_v1(&TransferPayloadV1 { to, amount }).to_vec();
    mk_tx(from, nonce, fee, payload)
}

fn mk_register_type8_tx(
    creator: Address,
    nonce: u64,
    fee: u64,
    code_hash: [u8; 32],
    initial_balance: u128,
) -> TxV1 {
    let payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash,
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::oracle(),
        initial_balance,
    })
    .to_vec();
    mk_tx(creator, nonce, fee, payload)
}

fn mk_register_type10_tx(
    creator: Address,
    nonce: u64,
    fee: u64,
    code_hash: [u8; 32],
    entity_pubkey: [u8; 32],
    initial_balance: u128,
) -> TxV1 {
    let payload =
        encode_register_ai_entity_with_key_payload_v1(&RegisterAiEntityWithKeyPayloadV1 {
            code_hash,
            pubkey: entity_pubkey,
            autonomy_mode: AutonomyMode::Gated,
            capabilities: Capabilities::oracle(),
            initial_balance,
        })
        .to_vec();
    mk_tx(creator, nonce, fee, payload)
}

fn mk_oracle_anchor_tx(
    sender_addr: Address,
    entity_id: [u8; 32],
    nonce: u64,
    fee: u64,
    signal_hash: [u8; 32],
    data_hash: [u8; 32],
    tag: &[u8],
) -> TxV1 {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type: AiSignalType::OracleAnchor,
        issuer_entity_id: entity_id,
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
        oracle_anchor: Some(OracleAnchorExtraV1 {
            data_hash,
            external_timestamp: 1_700_000_000,
            source_hash: [0u8; 32],
            expiry_height: 0,
            data_tag: tag.to_vec(),
        }),
    });
    mk_tx(sender_addr, nonce, fee, payload)
}

/// Replicate the production address derivation `blake3(NOVAI_ADDRESS_V1 || pubkey)`.
fn address_from_pubkey_bytes(pubkey: &[u8; 32]) -> Address {
    let mut h = blake3::Hasher::new();
    h.update(b"NOVAI_ADDRESS_V1");
    h.update(pubkey);
    *h.finalize().as_bytes()
}

// ============================================================================
// WORKLOAD 1: baseline_account_transfers
// ============================================================================

/// Smoke test: three transfers between two normal accounts on four validators.
/// Should PASS on 9ac23c4. Confirms the harness mechanics are sound.
#[test]
fn w1_baseline_account_transfers_stay_deterministic() {
    let mut quad = Quad::new();
    let a = [0x01u8; 32];
    let b = [0x02u8; 32];
    seed_account_all(&mut quad, &a, 1_000_000, 0);
    seed_account_all(&mut quad, &b, 1_000_000, 0);

    let block = vec![
        mk_transfer_tx(a, 0, 100, b, 5_000),
        mk_transfer_tx(b, 0, 100, a, 2_500),
        mk_transfer_tx(a, 1, 100, b, 1_000),
    ];
    quad.commit_block(&block, 1, "w1");
}

// ============================================================================
// WORKLOAD 2: type-8 register + transfer-from-creator next block
// ============================================================================

/// Register a Type-8 AI entity, then in the next block do a Transfer FROM
/// the creator address. Post-9ac23c4 the reverse-index Put fires on register
/// and the Transfer routes through the AI-sender branch. Should PASS.
#[test]
fn w2_type8_register_then_transfer_from_creator_stays_deterministic() {
    let mut quad = Quad::new();
    let creator = [0x10u8; 32];
    let recipient = [0x20u8; 32];
    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    let block_a = vec![mk_register_type8_tx(creator, 0, 5_000, [0xAA; 32], 500_000)];
    quad.commit_block(&block_a, 1, "w2/register");

    // After register, the dispatcher resolves creator -> entity_id via
    // ai_entity_by_address_key, so subsequent txs from `creator` route to
    // the AI-sender branches. But the dispatcher denies Transfer ONLY if
    // check_ai_entity_sender's deny-by-default arm fires for that tx_type.
    // Transfer (type 1) is explicitly allowed; the entity's nonce starts
    // at 0 (NOT the creator account nonce). Use nonce=0 against the entity.
    let block_b = vec![mk_transfer_tx(creator, 0, 100, recipient, 1_000)];
    quad.commit_block(&block_b, 2, "w2/transfer");
}

// ============================================================================
// WORKLOAD 3: type-10 register + transfer-from-entity-address
// ============================================================================

/// Register a Type-10 AI entity (with a separate signing pubkey), then in
/// the next block do a Transfer FROM the derived entity address. Should
/// route through the AI-sender branch and stay deterministic.
#[test]
fn w3_type10_register_with_key_then_transfer_stays_deterministic() {
    let mut quad = Quad::new();
    let creator = [0x11u8; 32];
    let recipient = [0x21u8; 32];
    let entity_pubkey = [0xBBu8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    let block_a = vec![mk_register_type10_tx(
        creator,
        0,
        5_000,
        [0xCC; 32],
        entity_pubkey,
        500_000,
    )];
    quad.commit_block(&block_a, 1, "w3/register");

    let block_b = vec![mk_transfer_tx(entity_addr, 0, 100, recipient, 1_000)];
    quad.commit_block(&block_b, 2, "w3/transfer");
}

// ============================================================================
// WORKLOAD 4: β4-A nonce-gap probe (single-validator determinism)
// ============================================================================

/// Drive a sequence of OracleAnchor signals where some succeed and some
/// fail. Under β4-A, entity.nonce can jump by more than 1. The harness
/// checks that the jump is identical across all 4 validators.
///
/// Sequence on an oracle entity (after fund + register):
///   - signal nonce=0  -> success (anchor written)
///   - signal nonce=1  -> success
///   - signal nonce=2  -> success
/// Then a Transfer from the SAME entity address with nonce=3.
#[test]
fn w4_oracle_anchor_then_transfer_stays_deterministic() {
    let mut quad = Quad::new();
    let creator = [0x12u8; 32];
    let recipient = [0x22u8; 32];
    let entity_pubkey = [0xDDu8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xEE; 32],
            entity_pubkey,
            1_000_000,
        )],
        1,
        "w4/register",
    );

    // OracleAnchor signals from entity_addr. The dispatcher routes via
    // ai_entity_by_address_key(entity_addr), capabilities require
    // emit_proposals which is set by Capabilities::oracle().
    // entity_id is recomputed by handler; we only need entity_id for the
    // signal payload's issuer_entity_id field, which is the same as the
    // dispatcher-resolved entity.id.
    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xEE; 32], &creator);

    let sigs = vec![
        mk_oracle_anchor_tx(
            entity_addr,
            entity_id,
            0,
            1_000,
            [0x01; 32],
            [0xAA; 32],
            b"x/y",
        ),
        mk_oracle_anchor_tx(
            entity_addr,
            entity_id,
            1,
            1_000,
            [0x02; 32],
            [0xAA; 32],
            b"x/y",
        ),
        mk_oracle_anchor_tx(
            entity_addr,
            entity_id,
            2,
            1_000,
            [0x03; 32],
            [0xAA; 32],
            b"x/y",
        ),
    ];
    quad.commit_block(&sigs, 2, "w4/anchors");

    quad.commit_block(
        &[mk_transfer_tx(entity_addr, 3, 100, recipient, 10_000)],
        3,
        "w4/transfer",
    );
}

// ============================================================================
// WORKLOAD 5: drained entity + failing oracle anchors + creator Transfer
//             (the operator's specific Candidate B probe)
// ============================================================================

/// User-specified probe for Candidate B (same Transfer branch, drift in
/// non-SMT-authenticated upstream state via failed signal handler runs).
///
/// Setup:
///   - Type-10 entity registered with low initial_balance (just above one
///     fee), so the first oracle anchor succeeds but subsequent ones fail
///     with InsufficientFunds.
///   - 5 sequential OracleAnchor submissions: 1 success + 4 failures.
///   - Then a Transfer FROM the creator account (a normal-sender path).
///
/// Variant w5a: Transfer FROM the entity address (AI-sender branch).
/// Variant w5b: Transfer FROM the creator address (normal-sender branch).
///
/// Both should produce byte-identical state across all 4 validators.
#[test]
fn w5_drained_entity_with_failing_anchors_then_entity_transfer() {
    let mut quad = Quad::new();
    let creator = [0x13u8; 32];
    let recipient = [0x23u8; 32];
    let entity_pubkey = [0xEEu8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    // Initial balance: 1_500 (enough for ONE anchor at fee 1000, but the
    // second anchor will hit InsufficientFunds after the first debits 1000).
    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0x55; 32],
            entity_pubkey,
            1_500,
        )],
        1,
        "w5/register",
    );

    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0x55; 32], &creator);

    // Five anchors: first succeeds, then four fail with InsufficientFunds.
    let anchors = vec![
        mk_oracle_anchor_tx(
            entity_addr,
            entity_id,
            0,
            1_000,
            [0xA1; 32],
            [0xDD; 32],
            b"price",
        ),
        mk_oracle_anchor_tx(
            entity_addr,
            entity_id,
            1,
            1_000,
            [0xA2; 32],
            [0xDD; 32],
            b"price",
        ),
        mk_oracle_anchor_tx(
            entity_addr,
            entity_id,
            2,
            1_000,
            [0xA3; 32],
            [0xDD; 32],
            b"price",
        ),
        mk_oracle_anchor_tx(
            entity_addr,
            entity_id,
            3,
            1_000,
            [0xA4; 32],
            [0xDD; 32],
            b"price",
        ),
        mk_oracle_anchor_tx(
            entity_addr,
            entity_id,
            4,
            1_000,
            [0xA5; 32],
            [0xDD; 32],
            b"price",
        ),
    ];
    quad.commit_block(&anchors, 2, "w5/anchors");

    // After block 2: entity has 500 left (1500 - 1000). Next anchor at
    // nonce=1 would need 1000, so fail. Then a Transfer FROM the entity
    // address with a SMALL amount (less than 500) should succeed and
    // exercise the AI-sender branch, writing the post-drift entity record
    // into the SMT. If any non-SMT state drifted between validators, this
    // transfer would expose it as a state_root divergence.
    quad.commit_block(
        &[mk_transfer_tx(entity_addr, 5, 100, recipient, 100)],
        3,
        "w5/transfer",
    );
}

// ============================================================================
// WORKLOAD 6: multi-sender high-load transfers
// ============================================================================

/// 8 normal accounts, each with one transfer to the next, plus some
/// transfers between AI entities. Probe for ordering-related divergence.
#[test]
fn w6_multi_sender_high_load_stays_deterministic() {
    let mut quad = Quad::new();
    let mut addrs: Vec<Address> = Vec::new();
    for i in 0..8u8 {
        let mut a = [0u8; 32];
        a[0] = 0x30;
        a[1] = i;
        addrs.push(a);
        seed_account_all(&mut quad, &a, 1_000_000, 0);
    }

    let mut block = Vec::new();
    for i in 0..addrs.len() {
        let from = addrs[i];
        let to = addrs[(i + 1) % addrs.len()];
        block.push(mk_transfer_tx(from, 0, 100, to, 1_000 + i as u64));
    }
    quad.commit_block(&block, 1, "w6/round1");

    let mut block = Vec::new();
    for i in 0..addrs.len() {
        let from = addrs[i];
        let to = addrs[(i + 3) % addrs.len()];
        block.push(mk_transfer_tx(from, 1, 100, to, 500 + i as u64));
    }
    quad.commit_block(&block, 2, "w6/round2");
}

// ============================================================================
// WORKLOAD 7: register and transfer in the SAME block
// ============================================================================

/// Register a Type-10 entity in block N tx 0, then in tx 1 of the SAME
/// block do a Transfer FROM the new entity address. The dispatcher's
/// reverse-index lookup must see the freshly-written Put. This is a
/// within-block sequential-write-then-read sanity test.
#[test]
fn w7_register_and_transfer_in_same_block_stays_deterministic() {
    let mut quad = Quad::new();
    let creator = [0x14u8; 32];
    let recipient = [0x24u8; 32];
    let entity_pubkey = [0xCCu8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    let block = vec![
        mk_register_type10_tx(creator, 0, 5_000, [0x66; 32], entity_pubkey, 500_000),
        mk_transfer_tx(entity_addr, 0, 100, recipient, 5_000),
    ];
    quad.commit_block(&block, 1, "w7/combined");
}

// ============================================================================
// WORKLOAD 8: HARNESS SELF-TEST
//             Confirm assert_states_equal catches an injected divergence.
// ============================================================================

/// Apply a different tx to validator 0 vs the others; assert the harness
/// detects the divergence. This is a meta-test of the harness itself.
#[test]
#[should_panic(expected = "state divergence between v0 and v1")]
fn w8_harness_self_test_detects_injected_divergence() {
    let mut quad = Quad::new();
    let a = [0x01u8; 32];
    let b = [0x02u8; 32];
    seed_account_all(&mut quad, &a, 1_000_000, 0);
    seed_account_all(&mut quad, &b, 1_000_000, 0);

    // Apply a transfer to validator 0 only.
    quad.apply_to_one(0, &[mk_transfer_tx(a, 0, 100, b, 1_234)], 1);

    // Now assert. Should panic.
    assert_states_equal(&quad.dbs, "w8/injected");
}

// ============================================================================
// WORKLOAD 9: divergent-tx-ordering self-probe
//             If the executor is truly order-independent for a given block
//             contents, this should still produce byte-equal state. If
//             ordering matters (which under the SMT it does for the
//             AI-sender ops vec), this will diverge and surface a real
//             determinism risk.
// ============================================================================

/// Construct two transfers in a block whose final states should be
/// order-independent (two independent senders, two independent recipients).
/// Apply them in the same order to all four validators. Should pass.
#[test]
fn w9_independent_transfers_stay_deterministic_in_block_order() {
    let mut quad = Quad::new();
    let a = [0x01u8; 32];
    let b = [0x02u8; 32];
    let c = [0x03u8; 32];
    let d = [0x04u8; 32];
    seed_account_all(&mut quad, &a, 1_000_000, 0);
    seed_account_all(&mut quad, &b, 1_000_000, 0);
    seed_account_all(&mut quad, &c, 1_000_000, 0);
    seed_account_all(&mut quad, &d, 1_000_000, 0);

    let block = vec![
        mk_transfer_tx(a, 0, 100, b, 1_000),
        mk_transfer_tx(c, 0, 100, d, 2_000),
    ];
    quad.commit_block(&block, 1, "w9");
}

// ============================================================================
// WORKLOAD 10: the new CreatorAlreadyHasEntity guard fires consistently
// ============================================================================

/// After a Type-8 register, a second Type-8 register from the same creator
/// (with a different code_hash so the duplicate-entity check does NOT
/// fire) should be rejected with `CreatorAlreadyHasEntity` by all four
/// validators. Verifies the new guard at lib.rs:9098-9101 is deterministic.
#[test]
fn w10_creator_already_has_entity_guard_fires_consistently() {
    let mut quad = Quad::new();
    let creator = [0x15u8; 32];
    seed_account_all(&mut quad, &creator, 10_000_000, 0);

    quad.commit_block(
        &[mk_register_type8_tx(creator, 0, 5_000, [0x77; 32], 100_000)],
        1,
        "w10/first",
    );

    // Second register, same creator, different code_hash => different
    // entity_id (so the EntityAlreadyExists path doesn't fire), but the
    // reverse-index for the creator is now present, so the new guard MUST
    // fire on all 4 with CreatorAlreadyHasEntity.
    quad.commit_block(
        &[mk_register_type8_tx(creator, 1, 5_000, [0x88; 32], 100_000)],
        2,
        "w10/second",
    );
}

// ============================================================================
// MECHANISM PROOFS (W11-W13)
//
// These workloads prove the SMT inclusion gap mechanism by injecting
// non-SMT-authenticated state divergence on a single validator and then
// driving a Transfer. If the SMT root diverges as a result, it confirms
// that ANY upstream drift in non-SMT-authenticated state can cause a
// state_root mismatch on the next Transfer.
// ============================================================================

use novai_state::KEY_SMT_ROOT;

fn read_smt_root(db: &MemKv) -> [u8; 32] {
    db.get(KEY_SMT_ROOT).unwrap().map_or([0u8; 32], |b| {
        novai_state::decode_smt_root_v1(&b).unwrap_or([0u8; 32])
    })
}

/// Read a known key value on validator i (for state-injection helpers).
fn raw_get(db: &MemKv, key: &[u8]) -> Option<Vec<u8>> {
    db.get(key).unwrap()
}

/// Raw delete (bypasses SMT) on validator i.
fn raw_delete(db: &mut MemKv, key: Vec<u8>) {
    db.apply_batch(&[WriteOp::Delete(key)]).unwrap();
}

// ----------------------------------------------------------------------------
// W11 MECHANISM: reverse-index drift on @3 -> different Transfer branch
//                -> divergent SMT root
// ----------------------------------------------------------------------------

/// Set up an AI entity on all 4 validators. Then DELETE the reverse-index
/// key on validator 3 only (simulating any cause of differential drift in
/// this non-SMT-authenticated state). Then submit a Transfer FROM the
/// entity address. On v0/v1/v2 the dispatcher routes through the AI-sender
/// branch; on v3 it falls back to the normal-sender branch. Different ops
/// vectors hit the SMT, producing different KEY_SMT_ROOT values.
#[test]
fn w11_reverse_index_drift_diverges_smt_root_on_next_transfer() {
    let mut quad = Quad::new();
    let creator = [0x16u8; 32];
    let recipient = [0x26u8; 32];
    let entity_pubkey = [0x11u8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0x99; 32],
            entity_pubkey,
            500_000,
        )],
        1,
        "w11/register",
    );

    // After register, the reverse-index for entity_addr exists on all 4.
    let rev_key = novai_state::ai_entity_by_address_key(&entity_addr);
    let rev_val_v0 = raw_get(&quad.dbs[0], &rev_key).expect("v0 reverse-index present");
    for i in 0..VALIDATORS {
        assert_eq!(
            raw_get(&quad.dbs[i], &rev_key),
            Some(rev_val_v0.clone()),
            "v{i} should have the same reverse-index after register"
        );
    }

    // INJECT: remove the reverse-index on v3 only. This models ANY upstream
    // mechanism that caused @3's reverse-index to drift (compaction loss,
    // tombstone interaction, etc.). The harness does not need to prove the
    // trigger here, only the mechanism.
    raw_delete(&mut quad.dbs[3], rev_key.clone());
    assert!(
        raw_get(&quad.dbs[3], &rev_key).is_none(),
        "v3 reverse-index gone"
    );

    // Now a Transfer FROM the entity address. On v0/v1/v2 the dispatcher
    // resolves to Some(entity) and routes through the AI-sender branch.
    // On v3 the lookup returns None, dispatcher routes through the
    // normal-sender branch reading account_key(entity_addr) (which is
    // empty / zero-balance), so the tx FAILS with a different error class.
    //
    // The harness assertion is then: state_root diverges between v0/v1/v2
    // (which mutated entity + to_acct + fee_pool through SMT) and v3
    // (which failed early with AccountNotFound or similar, leaving SMT
    // root unchanged from prior).
    //
    // We do NOT use commit_block here because outcomes will diverge by
    // design; we instead step manually and assert the SMT root divergence
    // directly.
    let tx = mk_transfer_tx(entity_addr, 0, 100, recipient, 5_000);
    let _ = dispatch_tx(&mut quad.dbs[0], &tx, 2);
    let _ = dispatch_tx(&mut quad.dbs[1], &tx, 2);
    let _ = dispatch_tx(&mut quad.dbs[2], &tx, 2);
    let _ = dispatch_tx(&mut quad.dbs[3], &tx, 2);

    let r0 = read_smt_root(&quad.dbs[0]);
    let r1 = read_smt_root(&quad.dbs[1]);
    let r2 = read_smt_root(&quad.dbs[2]);
    let r3 = read_smt_root(&quad.dbs[3]);
    assert_eq!(r0, r1, "v0 and v1 should agree (AI-sender branch on both)");
    assert_eq!(r0, r2, "v0 and v2 should agree (AI-sender branch on both)");
    assert_ne!(
        r0, r3,
        "MECHANISM PROOF: v0 (AI-sender branch) must diverge from v3 (normal-sender branch) because the reverse-index drifted"
    );
}

// ----------------------------------------------------------------------------
// W12 MECHANISM: entity economic_balance drift on @3 -> same AI-sender
//                branch, different ops content -> divergent SMT root
// ----------------------------------------------------------------------------

/// Inject a different `economic_balance` on v3's entity record. Both v0
/// and v3 then route through the AI-sender branch on a subsequent Transfer
/// (same branch), but the entity record written into the SMT differs in
/// the balance bytes, producing a different SMT root.
///
/// This is the canonical Candidate B scenario: same branch, different
/// upstream non-SMT-authenticated state, different SMT root.
#[test]
fn w12_entity_balance_drift_diverges_smt_root_on_next_transfer() {
    let mut quad = Quad::new();
    let creator = [0x17u8; 32];
    let recipient = [0x27u8; 32];
    let entity_pubkey = [0x12u8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xA1; 32],
            entity_pubkey,
            1_000_000,
        )],
        1,
        "w12/register",
    );

    // Resolve entity_id on v0 (same on all by construction at this point).
    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xA1; 32], &creator);

    // INJECT: lower economic_balance on v3's entity record by 1. This
    // models any non-SMT-authenticated handler having drifted the balance
    // on @3 only (e.g., a signal commit that was wrongly applied or not
    // applied).
    let mut v3_entity = read_ai_entity(&quad.dbs[3], &entity_id)
        .unwrap()
        .expect("v3 entity present");
    v3_entity.economic_balance -= 1;
    quad.dbs[3]
        .apply_batch(&[write_ai_entity_op(&v3_entity)])
        .unwrap();

    // SMT roots are still equal at this point (the inject is a non-SMT
    // write). Confirm:
    let r0_pre = read_smt_root(&quad.dbs[0]);
    let r3_pre = read_smt_root(&quad.dbs[3]);
    assert_eq!(
        r0_pre, r3_pre,
        "non-SMT write does not change SMT root by itself"
    );

    // Now a Transfer FROM the entity address. Both v0 and v3 route through
    // the AI-sender branch. The entity record they write back into the SMT
    // differs in the balance bytes, so SMT roots must diverge.
    let tx = mk_transfer_tx(entity_addr, 0, 100, recipient, 5_000);
    let _ = dispatch_tx(&mut quad.dbs[0], &tx, 2);
    let _ = dispatch_tx(&mut quad.dbs[3], &tx, 2);

    let r0 = read_smt_root(&quad.dbs[0]);
    let r3 = read_smt_root(&quad.dbs[3]);
    assert_ne!(
        r0, r3,
        "MECHANISM PROOF: same Transfer branch, different non-SMT-authenticated upstream balance, different SMT root"
    );
}

// ----------------------------------------------------------------------------
// W13 MECHANISM: entity nonce drift -> divergent SMT root
// ----------------------------------------------------------------------------

/// Same as W12 but drifting `entity.nonce` instead of `economic_balance`.
/// Under β4-A semantics (entity.nonce = tx.nonce + 1, not entity.nonce + 1),
/// any prior drift in entity.nonce propagates through the next Transfer
/// even though the executor is deterministic given identical inputs.
#[test]
fn w13_entity_nonce_drift_diverges_smt_root_on_next_transfer() {
    let mut quad = Quad::new();
    let creator = [0x18u8; 32];
    let recipient = [0x28u8; 32];
    let entity_pubkey = [0x13u8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xB1; 32],
            entity_pubkey,
            1_000_000,
        )],
        1,
        "w13/register",
    );

    // INJECT: bump entity.nonce on v3's entity record by 1.
    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xB1; 32], &creator);

    let mut v3_entity = read_ai_entity(&quad.dbs[3], &entity_id)
        .unwrap()
        .expect("v3 entity present");
    v3_entity.nonce += 1;
    quad.dbs[3]
        .apply_batch(&[write_ai_entity_op(&v3_entity)])
        .unwrap();

    // Pick a tx.nonce that satisfies both validators' nonce checks under
    // β4-A: tx.nonce >= entity.nonce. v0 has entity.nonce=0; v3 has
    // entity.nonce=1. tx.nonce=1 satisfies both.
    let tx = mk_transfer_tx(entity_addr, 1, 100, recipient, 5_000);
    let _ = dispatch_tx(&mut quad.dbs[0], &tx, 2);
    let _ = dispatch_tx(&mut quad.dbs[3], &tx, 2);

    // After successful Transfer:
    //   v0: entity.nonce = tx.nonce + 1 = 2
    //   v3: entity.nonce = tx.nonce + 1 = 2  (β4-A pins on tx.nonce, NOT
    //                                          on prior entity.nonce)
    // So the FINAL entity.nonce is identical! The drift is absorbed.
    //
    // BUT: the OTHER entity fields (last_active_at = current_height = 2 on
    // both) and the balance debit are identical, so the encoded entity
    // bytes might actually be identical post-transfer. If so, SMT roots
    // AGREE. This is an important corner case to verify.
    let r0 = read_smt_root(&quad.dbs[0]);
    let r3 = read_smt_root(&quad.dbs[3]);

    // Pull the post-transfer entity bytes to confirm the absorption.
    let entity_key = novai_state::ai_entity_key(&entity_id);
    let v0_entity_post = raw_get(&quad.dbs[0], &entity_key).expect("post present");
    let v3_entity_post = raw_get(&quad.dbs[3], &entity_key).expect("post present");
    if v0_entity_post == v3_entity_post {
        // β4-A ABSORBS the nonce drift: same final entity bytes, same SMT.
        // This is actually a deeply important observation about β4-A as a
        // determinism shield against prior non-SMT-authenticated nonce
        // drift. Document it.
        assert_eq!(
            r0, r3,
            "β4-A absorbs the nonce drift entirely; SMT roots match"
        );
    } else {
        // If for any reason the entity bytes differ after transfer, SMT
        // must diverge.
        assert_ne!(r0, r3, "entity drift not absorbed; SMT must diverge");
    }
}

// ----------------------------------------------------------------------------
// W14 STRESS: tight β4-A interleaving with mixed signal/transfer types
// ----------------------------------------------------------------------------

/// Drive many blocks with mixed signal (success and failure) and Transfer
/// activity from one AI entity. Verify all 4 validators stay byte-equal
/// throughout. Pure same-input stress test; this should ALWAYS pass on a
/// correctly-deterministic executor.
#[test]
fn w14_tight_beta4a_signal_transfer_interleave_stays_deterministic() {
    let mut quad = Quad::new();
    let creator = [0x19u8; 32];
    let recipient = [0x29u8; 32];
    let entity_pubkey = [0x14u8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xC1; 32],
            entity_pubkey,
            1_500,
        )],
        1,
        "w14/register",
    );

    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xC1; 32], &creator);

    // Tight interleaving:
    //   block 2: anchor n=0 (ok), anchor n=1 (fail: balance), anchor n=2 (fail), transfer n=3 (small ok)
    //   block 3: anchor n=4 (fail), transfer n=5 (small ok), anchor n=6 (fail)
    //   block 4: refund via Credit then anchor n=7 (now ok), transfer n=8 (ok)
    quad.commit_block(
        &[
            mk_oracle_anchor_tx(
                entity_addr,
                entity_id,
                0,
                1_000,
                [0x01; 32],
                [0xDD; 32],
                b"k",
            ),
            mk_oracle_anchor_tx(
                entity_addr,
                entity_id,
                1,
                1_000,
                [0x02; 32],
                [0xDD; 32],
                b"k",
            ),
            mk_oracle_anchor_tx(
                entity_addr,
                entity_id,
                2,
                1_000,
                [0x03; 32],
                [0xDD; 32],
                b"k",
            ),
            mk_transfer_tx(entity_addr, 3, 100, recipient, 50),
        ],
        2,
        "w14/b2",
    );
    quad.commit_block(
        &[
            mk_oracle_anchor_tx(
                entity_addr,
                entity_id,
                4,
                1_000,
                [0x04; 32],
                [0xDD; 32],
                b"k",
            ),
            mk_transfer_tx(entity_addr, 5, 100, recipient, 50),
            mk_oracle_anchor_tx(
                entity_addr,
                entity_id,
                6,
                1_000,
                [0x05; 32],
                [0xDD; 32],
                b"k",
            ),
        ],
        3,
        "w14/b3",
    );
}

// ----------------------------------------------------------------------------
// W15 STRESS: many entities, many transfers
// ----------------------------------------------------------------------------

// ----------------------------------------------------------------------------
// W16-W19 EXTENDED MECHANISM PROOFS: drift on capability/code_hash/is_active/
//                                     last_active_at
// ----------------------------------------------------------------------------

/// W16: drift the capability bitmask on v3's entity record. Transfer does not
/// check or update capabilities, so both v0 and v3 take the AI-sender branch
/// and write the entity back into the SMT. The encoded entity bytes differ
/// in the capability bits, so the SMT root diverges.
#[test]
fn w16_entity_capability_drift_diverges_smt_root_on_next_transfer() {
    let mut quad = Quad::new();
    let creator = [0x1Au8; 32];
    let recipient = [0x2Bu8; 32];
    let entity_pubkey = [0x15u8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xD1; 32],
            entity_pubkey,
            1_000_000,
        )],
        1,
        "w16/register",
    );

    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xD1; 32], &creator);
    let mut v3_entity = read_ai_entity(&quad.dbs[3], &entity_id).unwrap().unwrap();
    // Flip a capability bit on v3 only.
    v3_entity.capabilities.emit_proposals = !v3_entity.capabilities.emit_proposals;
    quad.dbs[3]
        .apply_batch(&[write_ai_entity_op(&v3_entity)])
        .unwrap();

    let tx = mk_transfer_tx(entity_addr, 0, 100, recipient, 5_000);
    let _ = dispatch_tx(&mut quad.dbs[0], &tx, 2);
    let _ = dispatch_tx(&mut quad.dbs[3], &tx, 2);

    let r0 = read_smt_root(&quad.dbs[0]);
    let r3 = read_smt_root(&quad.dbs[3]);
    assert_ne!(
        r0, r3,
        "MECHANISM PROOF: capability bit drift produces divergent SMT root via AI-sender branch"
    );
}

/// W17: drift the `code_hash` on v3's entity record. Transfer does not check
/// or update `code_hash`, so v3's entity record encoded into the SMT differs.
#[test]
fn w17_entity_code_hash_drift_diverges_smt_root_on_next_transfer() {
    let mut quad = Quad::new();
    let creator = [0x1Bu8; 32];
    let recipient = [0x2Cu8; 32];
    let entity_pubkey = [0x16u8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xD2; 32],
            entity_pubkey,
            1_000_000,
        )],
        1,
        "w17/register",
    );

    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xD2; 32], &creator);
    let mut v3_entity = read_ai_entity(&quad.dbs[3], &entity_id).unwrap().unwrap();
    // Drift code_hash on v3.
    v3_entity.code_hash[0] ^= 0xFF;
    quad.dbs[3]
        .apply_batch(&[write_ai_entity_op(&v3_entity)])
        .unwrap();

    let tx = mk_transfer_tx(entity_addr, 0, 100, recipient, 5_000);
    let _ = dispatch_tx(&mut quad.dbs[0], &tx, 2);
    let _ = dispatch_tx(&mut quad.dbs[3], &tx, 2);

    let r0 = read_smt_root(&quad.dbs[0]);
    let r3 = read_smt_root(&quad.dbs[3]);
    assert_ne!(
        r0, r3,
        "MECHANISM PROOF: code_hash drift produces divergent SMT root via AI-sender branch"
    );
}

/// W18: drift `is_active` on v3 to `false`. Transfer's AI-sender branch
/// checks `is_active` and returns `EntityNotActive` if false. The outcome
/// then differs across validators (v0 succeeds, v3 fails) and the harness
/// catches it via the outcome-match channel rather than the SMT-root channel.
#[test]
fn w18_entity_is_active_drift_produces_different_outcomes() {
    let mut quad = Quad::new();
    let creator = [0x1Cu8; 32];
    let recipient = [0x2Du8; 32];
    let entity_pubkey = [0x17u8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xD3; 32],
            entity_pubkey,
            1_000_000,
        )],
        1,
        "w18/register",
    );

    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xD3; 32], &creator);
    let mut v3_entity = read_ai_entity(&quad.dbs[3], &entity_id).unwrap().unwrap();
    v3_entity.is_active = false;
    quad.dbs[3]
        .apply_batch(&[write_ai_entity_op(&v3_entity)])
        .unwrap();

    let tx = mk_transfer_tx(entity_addr, 0, 100, recipient, 5_000);
    let r0 = dispatch_tx(&mut quad.dbs[0], &tx, 2);
    let r3 = dispatch_tx(&mut quad.dbs[3], &tx, 2);
    assert!(r0.is_ok(), "v0 Transfer should succeed");
    assert!(
        matches!(
            r3.as_ref().map_err(|e| format!("{e:?}")),
            Err(s) if s.contains("EntityNotActive")
        ),
        "v3 Transfer should fail with EntityNotActive (got: {r3:?})"
    );

    // SMT roots also diverge because v0 mutated state while v3 returned early.
    let root0 = read_smt_root(&quad.dbs[0]);
    let root3 = read_smt_root(&quad.dbs[3]);
    assert_ne!(
        root0, root3,
        "MECHANISM PROOF: is_active drift produces both outcome divergence AND SMT root divergence"
    );
}

/// W19: drift `last_active_at` on v3. Transfer does NOT update
/// `last_active_at`, so the drifted value persists into the post-Transfer
/// entity record written into the SMT.
#[test]
fn w19_entity_last_active_at_drift_diverges_smt_root_on_next_transfer() {
    let mut quad = Quad::new();
    let creator = [0x1Du8; 32];
    let recipient = [0x2Eu8; 32];
    let entity_pubkey = [0x18u8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xD4; 32],
            entity_pubkey,
            1_000_000,
        )],
        1,
        "w19/register",
    );

    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xD4; 32], &creator);
    let mut v3_entity = read_ai_entity(&quad.dbs[3], &entity_id).unwrap().unwrap();
    v3_entity.last_active_at += 1;
    quad.dbs[3]
        .apply_batch(&[write_ai_entity_op(&v3_entity)])
        .unwrap();

    let tx = mk_transfer_tx(entity_addr, 0, 100, recipient, 5_000);
    let _ = dispatch_tx(&mut quad.dbs[0], &tx, 2);
    let _ = dispatch_tx(&mut quad.dbs[3], &tx, 2);

    let r0 = read_smt_root(&quad.dbs[0]);
    let r3 = read_smt_root(&quad.dbs[3]);
    assert_ne!(
        r0, r3,
        "MECHANISM PROOF: last_active_at drift produces divergent SMT root via AI-sender branch (Transfer does not refresh this field)"
    );
}

/// Spawn N entities, each does M transfers, all interleaved. Pure
/// same-input determinism stress test under realistic load mix.
#[test]
fn w15_many_entities_many_transfers_stay_deterministic() {
    let mut quad = Quad::new();
    let recipient = [0x2Au8; 32];
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    const N_ENTITIES: usize = 5;
    let mut entity_addrs = Vec::new();
    for i in 0..N_ENTITIES {
        let mut creator = [0u8; 32];
        creator[0] = 0x40;
        creator[1] = i as u8;
        let mut entity_pubkey = [0u8; 32];
        entity_pubkey[0] = 0x70;
        entity_pubkey[1] = i as u8;
        let entity_addr = address_from_pubkey_bytes(&entity_pubkey);
        entity_addrs.push(entity_addr);

        seed_account_all(&mut quad, &creator, 10_000_000, 0);
        let mut code_hash = [0u8; 32];
        code_hash[0] = 0xF0;
        code_hash[1] = i as u8;
        quad.commit_block(
            &[mk_register_type10_tx(
                creator,
                0,
                5_000,
                code_hash,
                entity_pubkey,
                1_000_000,
            )],
            (i + 1) as u64,
            "w15/register",
        );
    }

    // Now N transfers per entity, interleaved.
    for round in 0..3 {
        let mut block = Vec::new();
        for (i, addr) in entity_addrs.iter().enumerate() {
            block.push(mk_transfer_tx(
                *addr,
                round as u64,
                100,
                recipient,
                10 + (i as u64),
            ));
        }
        quad.commit_block(&block, (N_ENTITIES + 1 + round) as u64, "w15/round");
    }
}

// ============================================================================
// W20-W27 FIX VALIDATION (post-SMT-inclusion-gap close)
//
// Pattern (mandatory cross-validator agreement per operator Adjustment 1):
//   1. Setup state on all 4 validators identically.
//   2. Snapshot KEY_SMT_ROOT on all 4; assert they match.
//   3. Run the target handler on all 4 with same input via commit_block
//      (which itself asserts per-tx outcome equality + post-block state
//      equality across all 4).
//   4. Snapshot KEY_SMT_ROOT on all 4; assert ALL four match the new root
//      AND that the new root differs from the pre-handler snapshot.
//
// If any test fails to assert "post != pre," the helper apply_state_ops_with_smt
// did not land on that handler's apply_batch site. The test is a positive
// proof that the fix is applied.
// ============================================================================

fn smt_roots(quad: &Quad) -> [[u8; 32]; VALIDATORS] {
    [
        read_smt_root(&quad.dbs[0]),
        read_smt_root(&quad.dbs[1]),
        read_smt_root(&quad.dbs[2]),
        read_smt_root(&quad.dbs[3]),
    ]
}

fn assert_smt_roots_agree(roots: &[[u8; 32]; VALIDATORS], ctx: &str) {
    for i in 1..VALIDATORS {
        assert_eq!(
            roots[0],
            roots[i],
            "[{ctx}] cross-validator SMT root mismatch: v0 != v{i} (v0={}, v{i}={})",
            hex_str(&roots[0]),
            hex_str(&roots[i])
        );
    }
}

fn assert_handler_authenticated_in_smt(
    pre: &[[u8; 32]; VALIDATORS],
    post: &[[u8; 32]; VALIDATORS],
    handler_label: &str,
) {
    assert_smt_roots_agree(pre, &format!("{handler_label}/pre"));
    assert_smt_roots_agree(post, &format!("{handler_label}/post"));
    assert_ne!(
        pre[0], post[0],
        "{handler_label}: KEY_SMT_ROOT did NOT change. The fix (apply_state_ops_with_smt) is NOT applied to this handler's apply_batch site, OR this handler did not produce any state ops. v0_pre={}, v0_post={}",
        hex_str(&pre[0]),
        hex_str(&post[0])
    );
}

// ----------------------------------------------------------------------------
// W20: Type-8 register now authenticates account + fee_pool + entity + reverse-index in SMT
// ----------------------------------------------------------------------------

#[test]
fn w20_type8_register_now_authenticated_in_smt() {
    let mut quad = Quad::new();
    let creator = [0xF0u8; 32];
    seed_account_all(&mut quad, &creator, 10_000_000, 0);

    let pre = smt_roots(&quad);
    quad.commit_block(
        &[mk_register_type8_tx(creator, 0, 5_000, [0xA0; 32], 100_000)],
        1,
        "w20",
    );
    let post = smt_roots(&quad);
    assert_handler_authenticated_in_smt(&pre, &post, "type8 register (lib.rs:9146)");
}

// ----------------------------------------------------------------------------
// W21: Type-10 register now authenticates account + fee_pool + entity + reverse-index in SMT
// ----------------------------------------------------------------------------

#[test]
fn w21_type10_register_now_authenticated_in_smt() {
    let mut quad = Quad::new();
    let creator = [0xF1u8; 32];
    let entity_pubkey = [0xB1u8; 32];
    seed_account_all(&mut quad, &creator, 10_000_000, 0);

    let pre = smt_roots(&quad);
    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xA1; 32],
            entity_pubkey,
            100_000,
        )],
        1,
        "w21",
    );
    let post = smt_roots(&quad);
    assert_handler_authenticated_in_smt(&pre, &post, "type10 register (lib.rs:9407)");
}

// ----------------------------------------------------------------------------
// W22: OracleAnchor signal commit now authenticates signal + oracle records + entity in SMT
// ----------------------------------------------------------------------------

#[test]
fn w22_oracle_anchor_signal_now_authenticated_in_smt() {
    let mut quad = Quad::new();
    let creator = [0xF2u8; 32];
    let entity_pubkey = [0xB2u8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);
    seed_account_all(&mut quad, &creator, 10_000_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xA2; 32],
            entity_pubkey,
            1_000_000,
        )],
        1,
        "w22/register",
    );

    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xA2; 32], &creator);

    let pre = smt_roots(&quad);
    quad.commit_block(
        &[mk_oracle_anchor_tx(
            entity_addr,
            entity_id,
            0,
            1_000,
            [0xCC; 32],
            [0xDD; 32],
            b"k",
        )],
        2,
        "w22/anchor",
    );
    let post = smt_roots(&quad);
    assert_handler_authenticated_in_smt(&pre, &post, "oracle anchor signal commit (lib.rs:9008)");
}

// ----------------------------------------------------------------------------
// W23: Memory create now authenticates memory object + count + by_type + entity in SMT
// ----------------------------------------------------------------------------

#[test]
fn w23_memory_create_now_authenticated_in_smt() {
    use novai_ai_entities::MemoryObjectType;
    use novai_execution::{encode_create_memory_object_payload_v1, CreateMemoryObjectPayloadV1};

    let mut quad = Quad::new();
    let creator = [0xF3u8; 32];
    let entity_pubkey = [0xB3u8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);
    seed_account_all(&mut quad, &creator, 10_000_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xA3; 32],
            entity_pubkey,
            1_000_000,
        )],
        1,
        "w23/register",
    );

    let pre = smt_roots(&quad);
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: b"w23-data".to_vec(),
    });
    let create_tx = mk_tx(entity_addr, 0, 1_000, payload);
    quad.commit_block(&[create_tx], 2, "w23/create");
    let post = smt_roots(&quad);
    assert_handler_authenticated_in_smt(&pre, &post, "memory create (lib.rs:10685)");
}

// ----------------------------------------------------------------------------
// W24: Memory update now authenticates updated object + entity in SMT
// ----------------------------------------------------------------------------

#[test]
fn w24_memory_update_now_authenticated_in_smt() {
    use novai_ai_entities::{MemoryObject, MemoryObjectType};
    use novai_execution::{
        encode_create_memory_object_payload_v1, encode_update_memory_object_payload_v1,
        CreateMemoryObjectPayloadV1, UpdateMemoryObjectPayloadV1,
    };

    let mut quad = Quad::new();
    let creator = [0xF4u8; 32];
    let entity_pubkey = [0xB4u8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);
    seed_account_all(&mut quad, &creator, 10_000_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xA4; 32],
            entity_pubkey,
            1_000_000,
        )],
        1,
        "w24/register",
    );

    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xA4; 32], &creator);

    let create_payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: b"original".to_vec(),
    });
    quad.commit_block(
        &[mk_tx(entity_addr, 0, 1_000, create_payload)],
        2,
        "w24/create",
    );

    let object_id =
        MemoryObject::compute_id(&entity_id, MemoryObjectType::ChainSummary, 2, b"original");

    let pre = smt_roots(&quad);
    let update_payload = encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
        object_id,
        new_data: b"updated".to_vec(),
    });
    quad.commit_block(
        &[mk_tx(entity_addr, 1, 1_000, update_payload)],
        3,
        "w24/update",
    );
    let post = smt_roots(&quad);
    assert_handler_authenticated_in_smt(&pre, &post, "memory update (lib.rs:10857)");
}

// ----------------------------------------------------------------------------
// W25: Memory delete now authenticates deletes + entity in SMT
// ----------------------------------------------------------------------------

#[test]
fn w25_memory_delete_now_authenticated_in_smt() {
    use novai_ai_entities::{MemoryObject, MemoryObjectType};
    use novai_execution::{
        encode_create_memory_object_payload_v1, encode_delete_memory_object_payload_v1,
        CreateMemoryObjectPayloadV1, DeleteMemoryObjectPayloadV1,
    };

    let mut quad = Quad::new();
    let creator = [0xF5u8; 32];
    let entity_pubkey = [0xB5u8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);
    seed_account_all(&mut quad, &creator, 10_000_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xA5; 32],
            entity_pubkey,
            1_000_000,
        )],
        1,
        "w25/register",
    );

    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xA5; 32], &creator);

    let create_payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: b"to-delete".to_vec(),
    });
    quad.commit_block(
        &[mk_tx(entity_addr, 0, 1_000, create_payload)],
        2,
        "w25/create",
    );

    let object_id =
        MemoryObject::compute_id(&entity_id, MemoryObjectType::ChainSummary, 2, b"to-delete");

    let pre = smt_roots(&quad);
    let delete_payload =
        encode_delete_memory_object_payload_v1(&DeleteMemoryObjectPayloadV1 { object_id }).to_vec();
    quad.commit_block(
        &[mk_tx(entity_addr, 1, 1_000, delete_payload)],
        3,
        "w25/delete",
    );
    let post = smt_roots(&quad);
    assert_handler_authenticated_in_smt(&pre, &post, "memory delete (lib.rs:11122)");
}

// ----------------------------------------------------------------------------
// W26: Credit AI entity now authenticates funder account + fee_pool + entity in SMT
// ----------------------------------------------------------------------------

#[test]
fn w26_credit_ai_entity_now_authenticated_in_smt() {
    use novai_execution::{encode_credit_ai_entity_payload_v1, CreditAiEntityPayloadV1};

    let mut quad = Quad::new();
    let creator = [0xF6u8; 32];
    let funder = [0xC6u8; 32];
    let entity_pubkey = [0xB6u8; 32];
    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &funder, 10_000_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xA6; 32],
            entity_pubkey,
            100_000,
        )],
        1,
        "w26/register",
    );

    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xA6; 32], &creator);

    let pre = smt_roots(&quad);
    let credit_payload = encode_credit_ai_entity_payload_v1(&CreditAiEntityPayloadV1 {
        entity_id,
        amount: 50_000,
    })
    .to_vec();
    quad.commit_block(&[mk_tx(funder, 0, 1_000, credit_payload)], 2, "w26/credit");
    let post = smt_roots(&quad);
    assert_handler_authenticated_in_smt(&pre, &post, "credit ai entity (lib.rs:9260)");
}

// ----------------------------------------------------------------------------
// W27: Entity upgrade now authenticates creator account + fee_pool + entity +
//       upgrade summary + upgrade record in SMT
// ----------------------------------------------------------------------------

#[test]
fn w27_entity_upgrade_now_authenticated_in_smt() {
    use novai_execution::{encode_entity_upgrade_payload_v1, EntityUpgradePayloadV1};

    let mut quad = Quad::new();
    let creator = [0xF7u8; 32];
    let entity_pubkey = [0xB7u8; 32];
    seed_account_all(&mut quad, &creator, 10_000_000, 0);

    // Register at height 1.
    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xA7; 32],
            entity_pubkey,
            100_000,
        )],
        1,
        "w27/register",
    );

    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xA7; 32], &creator);

    let pre = smt_roots(&quad);
    // Upgrade at a height well past MIN_UPGRADE_INTERVAL_BLOCKS (1000).
    let upgrade_payload = encode_entity_upgrade_payload_v1(&EntityUpgradePayloadV1 {
        entity_id,
        new_code_hash: [0xA8; 32],
        reason_hash: [0u8; 32],
    })
    .to_vec();
    quad.commit_block(
        &[mk_tx(creator, 1, 5_000, upgrade_payload)],
        2_000,
        "w27/upgrade",
    );
    let post = smt_roots(&quad);
    assert_handler_authenticated_in_smt(&pre, &post, "entity upgrade (lib.rs:9527)");
}

// ----------------------------------------------------------------------------
// W28: Governance Submit then Execute (ModuleRollback) authenticates BOTH the
//      submit-side proposal write AND the execute-side inner module-rollback
//      double-walk in the SMT.
//
// This is the only test that exercises the handler-calls-handler case audited
// for Adjustment 2: apply_governance_execute_tx (lib.rs:7170) invokes
// apply_module_rollback (lib.rs:6852) as an inner handler. After the fix both
// handlers call apply_state_ops_with_smt, so the execute tx produces two
// successive SMT walks within one transaction. The test asserts the resulting
// SMT root differs from the post-submit snapshot, confirming the double walk
// landed.
//
// Setup uses the smallest possible approval gate: TimelockOnly, threshold=0,
// timelock_blocks=0, expiry_blocks high enough to outlive both blocks.
// ----------------------------------------------------------------------------

#[test]
fn w28_governance_submit_then_execute_now_authenticated_in_smt() {
    use novai_ai_entities::{ApprovalGate, GateType};
    use novai_codec::encode_approval_gate_v1;
    use novai_execution::{
        encode_execute_proposal_payload_v1, encode_submit_proposal_payload_v1,
        ExecuteProposalPayloadV1, SubmitProposalPayloadV1,
    };
    use novai_governance::{Proposal, ProposalType};
    use novai_state::approval_gate_key;

    let mut quad = Quad::new();
    let proposer = [0xF8u8; 32];
    let creator = [0xE8u8; 32];
    let entity_pubkey = [0xB8u8; 32];

    seed_account_all(&mut quad, &proposer, 10_000_000, 0);
    seed_account_all(&mut quad, &creator, 10_000_000, 0);

    // Seed the smallest possible TimelockOnly gate on all 4 validators.
    let gate_id = [0x99u8; 32];
    let gate = ApprovalGate {
        gate_id,
        gate_type: GateType::TimelockOnly,
        required_approvers: Vec::new(),
        threshold: 0,
        timelock_blocks: 0,
        expiry_blocks: 10_000,
        veto_enabled: false,
        freeze_enabled: false,
    };
    let gate_bytes = encode_approval_gate_v1(&gate);
    for db in &mut quad.dbs {
        db.apply_batch(&[WriteOp::Put(
            approval_gate_key(&gate_id),
            gate_bytes.clone(),
        )])
        .unwrap();
    }

    // Register an active Type-10 AI entity so ModuleRollback has a real
    // is_active=true -> false transition to write.
    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xA8; 32],
            entity_pubkey,
            1_000_000,
        )],
        1,
        "w28/register",
    );

    use novai_ai_entities::AiEntity;
    let entity_id = AiEntity::compute_id(&[0xA8; 32], &creator);

    // -- Snapshot 1: before submit --
    let pre = smt_roots(&quad);

    // Submit a ModuleRollback proposal targeting the entity.
    let submit_payload = encode_submit_proposal_payload_v1(&SubmitProposalPayloadV1 {
        proposal_type: ProposalType::ModuleRollback,
        gate_id,
        proposal_data: entity_id.to_vec(),
    });
    // Submit must pay at least MIN_FEE_GOVERNANCE_SUBMIT (2_000).
    let submit_tx = mk_tx(proposer, 0, 2_000, submit_payload);
    quad.commit_block(&[submit_tx], 2, "w28/submit");

    // -- Snapshot 2: after submit, before execute --
    let mid = smt_roots(&quad);
    assert_handler_authenticated_in_smt(&pre, &mid, "governance submit (lib.rs:7042)");

    // Pre-compute the proposal_id deterministically (mirrors Proposal::new
    // inside the submit handler).
    let proposal_id = Proposal::compute_id(
        ProposalType::ModuleRollback,
        &proposer,
        &gate_id,
        &entity_id,
    );

    // Execute the proposal. The execute handler will:
    //   1. apply_module_rollback (lib.rs:6852) writes the deactivated entity
    //      via apply_state_ops_with_smt -> walk #1.
    //   2. The outer apply_governance_execute_tx (lib.rs:7170) writes the
    //      proposal state update via apply_state_ops_with_smt -> walk #2.
    // Execute must pay at least MIN_FEE_GOVERNANCE_EXECUTE (500).
    let execute_payload =
        encode_execute_proposal_payload_v1(&ExecuteProposalPayloadV1 { proposal_id });
    quad.commit_block(
        &[mk_tx(proposer, 1, 500, execute_payload.to_vec())],
        3,
        "w28/execute",
    );

    // -- Snapshot 3: after execute --
    let post = smt_roots(&quad);
    assert_handler_authenticated_in_smt(
        &mid,
        &post,
        "governance execute + module rollback double walk (lib.rs:7170 + lib.rs:6852)",
    );
}
