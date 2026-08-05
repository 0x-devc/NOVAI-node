//! Gate ACCEL Stage B, test (b): the one-batch-per-block invariant of the
//! commit-path executor (docs/gate-accel-stageB-execution-batching-design.md,
//! section 4, test b).
//!
//! RED-first record (Phase 0, 2026-08-04): arm b1 first ran as the pre-fix
//! capture, asserting the Stage B invariant (one atomic `apply_batch`, zero
//! loose writes) against a faithful transcription of the pre-Stage-B
//! commit-path executor. It FAILED for the right reason with: "observed 3
//! batches, 1 loose puts, 0 loose deletes (per-tx SMT sessions plus the
//! separate cursor put)". That failure is the recorded pre-fix behavior this
//! gate changed. At Phase 1 b1 flipped into its permanent form below: the
//! detector proof that the counting instrument distinguishes per-tx from
//! per-block writes, so arm b2's one-batch pin cannot pass vacuously.
//!
//! Arm b2 is the permanent regression trap: the Stage B applier
//! (`apply_block_execution`) must land a committed block's execution as
//! exactly ONE batch containing the write set plus exactly one
//! `KEY_EXECUTED_HEIGHT` cursor put and nothing else, with exactly one
//! `KEY_SMT_ROOT` put carrying the post root when the block mutates state,
//! and a cursor-only batch when it does not. A future edit that quietly
//! returns the commit path to per-tx application, or splits the cursor out of
//! the batch, fails b2's count or content assertions.
//!
//! The `CountingKv` wrapper is the shared instrument for both arms: it counts
//! `apply_batch` invocations, records each batch's ops, and counts loose
//! (non-batched) `put` and `delete` calls, while delegating storage to a real
//! `MemKv` so execution behaves identically to the persisting path.

use novai_execution::{
    append_smt_ops_for_state_ops, dispatch_tx, encode_transfer_payload_v1, execute_block_to_root,
    TransferPayloadV1, TxOutcome,
};
use novai_node::exec_apply::{apply_block_execution, resolve_and_apply_block};
use novai_state::{
    account_key, encode_account_v1, encode_fee_pool_v1, encode_smt_root_v1, AccountStateV1,
    FeePoolV1, Kv, KvBatch, MemKv, WriteOp, KEY_EXECUTED_HEIGHT, KEY_FEE_POOL,
    KEY_PREFIX_SMT_NODE, KEY_SMT_ROOT,
};
use novai_types::{Address, TxV1, TxVersion};

const HEIGHT: u64 = 1_000;
const SENDER: Address = [0x11; 32];
const RECIPIENT: Address = [0x22; 32];

/// Counting store: delegates all storage to an inner `MemKv`, recording every
/// `apply_batch` (invocation count plus the full op list) and counting every
/// loose (non-batched) `put`/`delete`. The instrument for the one-batch pin.
struct CountingKv {
    inner: MemKv,
    batches: Vec<Vec<WriteOp>>,
    loose_puts: usize,
    loose_deletes: usize,
}

impl CountingKv {
    fn new(inner: MemKv) -> Self {
        Self {
            inner,
            batches: Vec::new(),
            loose_puts: 0,
            loose_deletes: 0,
        }
    }
}

impl Kv for CountingKv {
    type Error = ();

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        self.inner.get(key)
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Self::Error> {
        self.loose_puts += 1;
        self.inner.put(key, value)
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), Self::Error> {
        self.loose_deletes += 1;
        self.inner.delete(key)
    }

    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, Self::Error> {
        self.inner.scan_prefix(prefix)
    }
}

impl KvBatch for CountingKv {
    fn apply_batch(&mut self, ops: &[WriteOp]) -> Result<(), Self::Error> {
        self.batches.push(ops.to_vec());
        self.inner.apply_batch(ops)
    }
}

/// Execution does not verify signatures (verify_block does, separately), so a
/// dummy pubkey and empty signature are correct for exercising `dispatch_tx`.
fn transfer(from: Address, nonce: u64, fee: u64, to: Address, amount: u64) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from,
        pubkey: from,
        nonce,
        fee,
        payload: encode_transfer_payload_v1(&TransferPayloadV1 { to, amount }).to_vec(),
        sig: [0u8; 64],
    }
}

fn acct(balance: u128, nonce: u64) -> Vec<u8> {
    encode_account_v1(&AccountStateV1 { balance, nonce }).to_vec()
}

/// Seed rows through the canonical SMT path so the starting state carries a
/// consistent `KEY_SMT_ROOT`, exactly as a live chain would. Seeding happens on
/// the bare `MemKv` BEFORE the counting wrapper, so counters start at zero.
fn seeded_state() -> MemKv {
    let mut db = MemKv::new();
    for (k, v) in [
        (account_key(&SENDER), acct(100_000_000, 0)),
        (account_key(&RECIPIENT), acct(1_000_000, 0)),
        (
            KEY_FEE_POOL.to_vec(),
            encode_fee_pool_v1(&FeePoolV1 { balance: 0 }).to_vec(),
        ),
    ] {
        let ops = vec![WriteOp::Put(k, v)];
        let mut all = ops.clone();
        append_smt_ops_for_state_ops(&db, &ops, &mut all).expect("append smt ops");
        db.apply_batch(&all).expect("apply seed batch");
    }
    db
}

fn block_txs() -> Vec<TxV1> {
    vec![
        transfer(SENDER, 0, 10_000, RECIPIENT, 1_000),
        transfer(SENDER, 1, 10_000, RECIPIENT, 2_000),
        transfer(SENDER, 2, 10_000, RECIPIENT, 3_000),
    ]
}

/// Arm b1, permanent form: the detector proof.
///
/// The body is a faithful transcription of the PRE-Stage-B commit executor for
/// one committed 3-tx block: per-tx `dispatch_tx` against the store, then the
/// separate `KEY_EXECUTED_HEIGHT` cursor put. The assertions pin the
/// split-write signature that executor produces (one batch per tx plus a loose
/// cursor put), proving the counting instrument distinguishes per-tx from
/// per-block application. This is what makes arm b2's one-batch assertion
/// non-vacuous: if `CountingKv` could not see the split, b2 could not fail on
/// a regression. The Phase 0 RED capture ran this same body against the Stage
/// B invariant and failed with exactly this signature (see the file header).
#[test]
fn stageb_detector_pertx_transcription_shows_split_writes() {
    let mut db = CountingKv::new(seeded_state());
    let txs = block_txs();

    for (i, tx) in txs.iter().enumerate() {
        dispatch_tx(&mut db, tx, HEIGHT)
            .unwrap_or_else(|e| panic!("tx {i} must apply for a non-vacuous capture: {e:?}"));
    }
    db.put(KEY_EXECUTED_HEIGHT, &HEIGHT.to_be_bytes())
        .expect("cursor put");

    assert_eq!(
        db.batches.len(),
        3,
        "the per-tx executor writes one batch per applied tx"
    );
    assert_eq!(db.loose_puts, 1, "the separate cursor put is a loose write");
    assert_eq!(db.loose_deletes, 0, "no loose deletes on this path");
}

/// Arm b2: the Stage B applier lands exactly one batch with exact content.
#[test]
fn stageb_applier_one_batch_exact_content() {
    let s0 = seeded_state();
    let txs = block_txs();

    let exec = execute_block_to_root(&s0, &txs, HEIGHT).expect("overlay execute");
    assert!(
        exec.outcomes.iter().all(|o| *o == TxOutcome::Applied),
        "every tx must apply for a non-vacuous pin; outcomes={:?}",
        exec.outcomes
    );
    let write_ops = exec.write_ops();
    assert!(!write_ops.is_empty(), "a mutating block has a write set");

    let mut db = CountingKv::new(s0);
    apply_block_execution(&mut db, HEIGHT, write_ops.clone()).expect("applier");

    // Count: one atomic batch, nothing beside it.
    assert_eq!(db.batches.len(), 1, "one atomic batch per committed block");
    assert_eq!(db.loose_puts, 0, "no loose puts beside the block batch");
    assert_eq!(db.loose_deletes, 0, "no loose deletes beside the block batch");

    // Content: the write set plus exactly the cursor, nothing else.
    let batch = &db.batches[0];
    assert_eq!(
        batch.len(),
        write_ops.len() + 1,
        "batch is the write set plus exactly one cursor op"
    );
    for op in &write_ops {
        assert!(batch.contains(op), "write-set op missing from the batch: {op:?}");
    }

    // Exactly one KEY_SMT_ROOT put, carrying the post root.
    let root_values: Vec<&Vec<u8>> = batch
        .iter()
        .filter_map(|op| match op {
            WriteOp::Put(k, v) if k.as_slice() == KEY_SMT_ROOT => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(root_values.len(), 1, "exactly one root record put in the batch");
    assert_eq!(
        root_values[0].as_slice(),
        encode_smt_root_v1(&exec.post_root),
        "the root put must carry the block's post-execution root"
    );

    // Exactly one KEY_EXECUTED_HEIGHT put, carrying the block height.
    let cursor_values: Vec<&Vec<u8>> = batch
        .iter()
        .filter_map(|op| match op {
            WriteOp::Put(k, v) if k.as_slice() == KEY_EXECUTED_HEIGHT => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(cursor_values.len(), 1, "exactly one cursor put in the batch");
    assert_eq!(
        cursor_values[0].as_slice(),
        HEIGHT.to_be_bytes(),
        "the cursor put must carry the block height"
    );

    // SMT node records ride the same batch as the rows they authenticate.
    let node_puts = batch
        .iter()
        .filter(|op| matches!(op, WriteOp::Put(k, _) if k.starts_with(KEY_PREFIX_SMT_NODE)))
        .count();
    assert!(node_puts > 0, "SMT node records must ride the block batch");

    // The row ops for both touched accounts and the fee pool ride it too.
    for key in [
        account_key(&SENDER),
        account_key(&RECIPIENT),
        KEY_FEE_POOL.to_vec(),
    ] {
        assert!(
            batch
                .iter()
                .any(|op| matches!(op, WriteOp::Put(k, _) if *k == key)),
            "row op missing from the batch for key {key:?}"
        );
    }
}

/// The miss-path pre-apply refusal: a committed block whose header does not
/// match the re-executed post root must halt BEFORE anything is applied,
/// leaving rows, root record, and cursor untouched. Pins
/// `resolve_and_apply_block`'s refusal directly (mutation m3 of the Stage B
/// checklist proves this test fails when the refusal is skipped).
#[test]
fn stageb_miss_path_refuses_corrupted_header_before_applying() {
    let mut db = seeded_state();
    let txs = block_txs();
    let pre_rows = {
        let mut rows = db.scan_prefix(b"").unwrap();
        rows.extend(db.scan_prefix(b"nnpx/").unwrap());
        rows.sort();
        rows
    };

    // A committed block carrying a CORRUPTED header root (not the post-state
    // of its txs, not the pre-state either).
    let block = novai_consensus_types::Block {
        height: HEIGHT,
        round: 0,
        parent_hash: [0u8; 32],
        state_root: [0xAA; 32],
        txs,
    };

    let err = resolve_and_apply_block(&mut db, &block, None)
        .expect_err("a corrupted header must refuse on the miss path");
    assert!(
        err.contains("CONSENSUS SAFETY HALT: pre-apply state root mismatch"),
        "the refusal must be the pre-apply halt; got: {err}"
    );

    // Nothing was applied: the store is byte-identical to before the call.
    let post_rows = {
        let mut rows = db.scan_prefix(b"").unwrap();
        rows.extend(db.scan_prefix(b"nnpx/").unwrap());
        rows.sort();
        rows
    };
    assert_eq!(
        pre_rows, post_rows,
        "the refused apply must leave the store byte-identical (no rows, no root, no cursor)"
    );
}

/// Arm b2, empty-block form: an empty write set yields a cursor-only batch,
/// with no root rewrite, preserving today's byte behavior for empty blocks.
#[test]
fn stageb_applier_empty_write_set_cursor_only_batch() {
    let mut db = CountingKv::new(seeded_state());

    apply_block_execution(&mut db, HEIGHT, Vec::new()).expect("applier");

    assert_eq!(db.batches.len(), 1, "still exactly one batch");
    assert_eq!(db.loose_puts, 0);
    assert_eq!(db.loose_deletes, 0);

    let batch = &db.batches[0];
    assert_eq!(batch.len(), 1, "cursor put alone for an empty write set");
    match &batch[0] {
        WriteOp::Put(k, v) => {
            assert_eq!(k.as_slice(), KEY_EXECUTED_HEIGHT, "the one op is the cursor");
            assert_eq!(v.as_slice(), HEIGHT.to_be_bytes(), "cursor carries the height");
        }
        WriteOp::Delete(_) => panic!("expected a cursor put, found a delete"),
    }
}
