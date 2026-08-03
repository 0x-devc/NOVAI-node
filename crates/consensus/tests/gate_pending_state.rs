//! Gate wedge-276272 Phase 2: the speculative pending-state machinery.
//!
//! `resolve_parent_state` reconstructs post-state(parent) as a `Kv` view by
//! layering the pending ancestors' write sets over the committed database, so
//! the propose and vote paths can execute a child block without persisting.
//! These tests drive the riskiest new surface adversarially, not on the happy
//! path only.
//!
//! Fixtures are built WITHOUT `resolve_parent_state` (a scratch `MemKv`
//! accumulates each block's writes) so the tests exercise `resolve` against an
//! independent oracle rather than against itself.
//!
//! Honest coverage notes:
//! - View-change interleaving manifests on this chain as a same-height sibling
//!   from a later round (a distinct block hash). `abandoned_sibling_*` drives
//!   exactly that shape: the walk is by hash, so the sibling is never on the
//!   resolved chain, and it evicts when its height commits. There is no separate
//!   in-round view-change surface in `resolve_parent_state` to drive.
//! - The degraded regime (commits stalled far below the frontier) is driven by
//!   `deep_lag_beyond_bound_recomputes`: past the in-memory write-set bound, the
//!   dropped ancestors are recomputed from stored blocks, post-root-verified.
//! - The two self-consistency asserts and the eviction boundary each carry a
//!   break-it/watch-it-fail/revert non-vacuousness proof, recorded in the
//!   session log rather than left as untested presence.

use novai_consensus::ConsensusState;
use novai_consensus_types::codec::encode_block_v1;
use novai_consensus_types::{block_hash, Block};
use novai_execution::{
    append_smt_ops_for_state_ops, empty_smt_root, encode_transfer_payload_v1, execute_block_to_root,
    BlockOverlay, TransferPayloadV1, TxOutcome,
};
use novai_state::{
    account_key, block_key, decode_smt_root_v1, encode_account_v1, encode_fee_pool_v1,
    encode_smt_root_v1, AccountStateV1, FeePoolV1, Kv, KvBatch, MemKv, WriteOp, KEY_FEE_POOL,
    KEY_SMT_ROOT,
};
use novai_types::{Address, TxV1, TxVersion};

const SENDER: Address = [0x11; 32];
const RECIPIENT: Address = [0x22; 32];

fn transfer(nonce: u64) -> TxV1 {
    let payload = encode_transfer_payload_v1(&TransferPayloadV1 {
        to: RECIPIENT,
        amount: 1,
    })
    .to_vec();
    TxV1 {
        version: TxVersion::V1,
        from: SENDER,
        pubkey: SENDER,
        nonce,
        fee: 1_000,
        payload,
        sig: [0u8; 64],
    }
}

fn read_root_db(db: &MemKv) -> [u8; 32] {
    match db.get(KEY_SMT_ROOT).unwrap() {
        Some(b) => decode_smt_root_v1(&b).unwrap(),
        None => empty_smt_root(),
    }
}

fn overlay_root(view: &BlockOverlay<'_, MemKv>) -> [u8; 32] {
    match view.get(KEY_SMT_ROOT).unwrap() {
        Some(b) => decode_smt_root_v1(&b).unwrap(),
        None => empty_smt_root(),
    }
}

fn persist_block(db: &mut MemKv, block: &Block) {
    let bytes = encode_block_v1(block).unwrap();
    db.put(&block_key(block.height), &bytes).unwrap();
}

/// A committed base at height `h0` (post-state convention: the tip header carries
/// the committed root) plus `n` pending blocks, each a transfer so the root moves
/// at every height. Everything is cached, persisted to the DB, and recorded in
/// `pending_exec`, exactly as the wired propose/vote paths will.
struct Pipeline {
    state: ConsensusState,
    db: MemKv,
    h0: u64,
    tip: Block,
    pending: Vec<Block>,
    roots: Vec<[u8; 32]>,
    r0: [u8; 32],
}

fn build_pipeline(n: u64) -> Pipeline {
    let mut db = MemKv::new();
    for (k, v) in [
        (
            account_key(&SENDER),
            encode_account_v1(&AccountStateV1 {
                balance: 1_000_000_000,
                nonce: 0,
            })
            .to_vec(),
        ),
        (
            account_key(&RECIPIENT),
            encode_account_v1(&AccountStateV1 {
                balance: 1_000_000,
                nonce: 0,
            })
            .to_vec(),
        ),
        (
            KEY_FEE_POOL.to_vec(),
            encode_fee_pool_v1(&FeePoolV1 { balance: 0 }).to_vec(),
        ),
    ] {
        let ops = vec![WriteOp::Put(k, v)];
        let mut all = ops.clone();
        append_smt_ops_for_state_ops(&db, &ops, &mut all).unwrap();
        db.apply_batch(&all).unwrap();
    }
    let r0 = read_root_db(&db);

    let h0 = 3u64;
    let tip = Block {
        height: h0,
        round: 0,
        parent_hash: [0x55; 32],
        state_root: r0, // post-state convention: header(h0) == post-state(h0) == KEY_SMT_ROOT
        txs: vec![],
    };
    persist_block(&mut db, &tip);

    let mut state = ConsensusState::new([0x01; 32]);
    state.committed_height = h0;
    state.cache_block(tip.clone()).unwrap();

    // Scratch accumulates post-state as an independent oracle for the fixtures.
    let mut scratch = db.clone();
    let mut pending = Vec::new();
    let mut roots = Vec::new();
    let mut parent_hash = block_hash(&tip);
    for i in 0..n {
        let height = h0 + 1 + i;
        let txs = vec![transfer(i)];
        let exec = execute_block_to_root(&scratch, &txs, height).unwrap();
        assert!(
            exec.outcomes.iter().all(|o| *o == TxOutcome::Applied),
            "fixture tx at height {height} must apply"
        );
        let block = Block {
            height,
            round: 0,
            parent_hash,
            state_root: exec.post_root,
            txs,
        };
        scratch.apply_batch(&exec.write_ops()).unwrap();
        state.cache_block(block.clone()).unwrap();
        persist_block(&mut db, &block);
        state.note_pending_exec(block_hash(&block), height, exec.post_root, exec.write_set);
        roots.push(exec.post_root);
        parent_hash = block_hash(&block);
        pending.push(block);
    }

    Pipeline {
        state,
        db,
        h0,
        tip,
        pending,
        roots,
        r0,
    }
}

#[test]
fn resolve_reconstructs_parent_post_state() {
    let p = build_pipeline(3);
    for i in 0..3usize {
        let block = &p.pending[i];
        let view = p
            .state
            .resolve_parent_state(block_hash(block), block.height, &p.db)
            .expect("resolve");
        assert_eq!(
            overlay_root(&view),
            p.roots[i],
            "resolve must reconstruct post-state of pending[{i}] (walk depth {})",
            i + 1
        );
        assert_eq!(
            overlay_root(&view),
            block.state_root,
            "reconstructed post-state root must equal the block's post-state header"
        );
    }
}

#[test]
fn abandoned_sibling_ignored_then_evicted() {
    let mut p = build_pipeline(3);

    // A sibling at height h0+1 from a later (dead) round: a distinct hash and a
    // distinct root. p.db still holds the committed state (root r0), so it is the
    // correct base to execute the sibling over.
    let sib_tx = {
        let payload = encode_transfer_payload_v1(&TransferPayloadV1 {
            to: RECIPIENT,
            amount: 1,
        })
        .to_vec();
        TxV1 {
            version: TxVersion::V1,
            from: SENDER,
            pubkey: SENDER,
            nonce: 0,
            fee: 2_000, // distinct fee => distinct tx => distinct block hash
            payload,
            sig: [0u8; 64],
        }
    };
    let sib_exec = execute_block_to_root(&p.db, &[sib_tx.clone()], p.h0 + 1).unwrap();
    let sibling = Block {
        height: p.h0 + 1,
        round: 1,
        parent_hash: block_hash(&p.tip),
        state_root: sib_exec.post_root,
        txs: vec![sib_tx],
    };
    p.state.cache_block(sibling.clone()).unwrap();
    p.state.note_pending_exec(
        block_hash(&sibling),
        p.h0 + 1,
        sib_exec.post_root,
        sib_exec.write_set,
    );
    let sib_hash = block_hash(&sibling);
    let p0_hash = block_hash(&p.pending[0]);

    // The walk is by hash, so resolving the canonical tip ignores the sibling.
    let view = p
        .state
        .resolve_parent_state(block_hash(&p.pending[2]), p.pending[2].height, &p.db)
        .unwrap();
    assert_eq!(
        overlay_root(&view),
        p.roots[2],
        "the canonical chain is reconstructed; the abandoned sibling is ignored"
    );
    assert!(p.state.pending_exec.contains_key(&sib_hash));
    assert!(p.state.pending_exec.contains_key(&p0_hash));

    // Commit the canonical height h0+1; eviction drops every entry at or below it,
    // including the abandoned sibling.
    p.state.apply_commits(&[p.pending[0].clone()]).unwrap();
    assert_eq!(p.state.committed_height, p.h0 + 1);
    assert!(
        !p.state.pending_exec.contains_key(&sib_hash),
        "the abandoned sibling must evict when its height commits"
    );
    assert!(
        !p.state.pending_exec.contains_key(&p0_hash),
        "the committed block must evict"
    );
    assert!(
        p.state
            .pending_exec
            .contains_key(&block_hash(&p.pending[1])),
        "a higher pending block is retained"
    );
    assert!(
        p.state
            .pending_exec
            .contains_key(&block_hash(&p.pending[2])),
        "a higher pending block is retained"
    );
}

#[test]
fn restart_recomputes_from_stored_blocks() {
    let mut p = build_pipeline(3);

    // Simulate a restart mid-pipeline: in-memory caches empty, committed height
    // and the DB-persisted blocks survive.
    p.state.pending_exec.clear();
    p.state.block_by_hash.clear();
    p.state.block_cache.clear();

    let view = p
        .state
        .resolve_parent_state(block_hash(&p.pending[2]), p.pending[2].height, &p.db)
        .expect("resolve after restart must rebuild from stored blocks");
    assert_eq!(
        overlay_root(&view),
        p.roots[2],
        "post-state(parent) recomputed from the stored blocks after restart"
    );
}

#[test]
fn eviction_boundary_via_apply_commits() {
    let mut p = build_pipeline(3);

    // Commit heights h0+1 and h0+2 through the real commit path.
    p.state
        .apply_commits(&[p.pending[0].clone(), p.pending[1].clone()])
        .unwrap();
    assert_eq!(p.state.committed_height, p.h0 + 2);

    assert!(
        !p.state
            .pending_exec
            .contains_key(&block_hash(&p.pending[0])),
        "h0+1 is at the boundary and must evict"
    );
    assert!(
        !p.state
            .pending_exec
            .contains_key(&block_hash(&p.pending[1])),
        "h0+2 is at the boundary and must evict"
    );
    assert!(
        p.state
            .pending_exec
            .contains_key(&block_hash(&p.pending[2])),
        "h0+3 is above the boundary and must be retained"
    );
    assert_eq!(
        p.state.pending_exec.len(),
        1,
        "exactly the above-boundary entry remains"
    );
}

#[test]
fn resolve_refuses_disconnected_parent_chain() {
    let mut p = build_pipeline(1);

    // A block at h0+1 whose parent points nowhere: the walk finds it by hash but
    // cannot connect to the committed tip.
    let orphan = Block {
        height: p.h0 + 1,
        round: 0,
        parent_hash: [0xEE; 32],
        state_root: p.r0,
        txs: vec![],
    };
    p.state.cache_block(orphan.clone()).unwrap();

    let err = match p
        .state
        .resolve_parent_state(block_hash(&orphan), p.h0 + 1, &p.db)
    {
        Ok(_) => {
            panic!("resolve must refuse a parent chain that does not connect to the committed tip")
        }
        Err(e) => e,
    };
    let msg = format!("{err:?}");
    assert!(
        msg.contains("does not connect to the committed tip"),
        "self-consistency 1 must name the disconnect; got: {msg}"
    );
}

#[test]
fn resolve_refuses_local_root_divergence() {
    let mut p = build_pipeline(1);

    // Drift the local committed root away from the committed tip header.
    let drift = [0x99u8; 32];
    p.db
        .put(KEY_SMT_ROOT, &encode_smt_root_v1(&drift))
        .unwrap();

    let err = match p
        .state
        .resolve_parent_state(block_hash(&p.pending[0]), p.pending[0].height, &p.db)
    {
        Ok(_) => {
            panic!("resolve must refuse when the local root diverges from the committed tip header")
        }
        Err(e) => e,
    };
    let msg = format!("{err:?}");
    assert!(
        msg.contains("diverged from committed tip header"),
        "self-consistency 2 must name the divergence; got: {msg}"
    );
}

#[test]
fn deep_lag_beyond_bound_recomputes_dropped_write_sets() {
    // Ten pending heights with commits stalled: past the in-memory write-set
    // bound, the oldest entries keep only their post root.
    let p = build_pipeline(10);

    let with_ws = p
        .state
        .pending_exec
        .values()
        .filter(|pe| pe.write_set.is_some())
        .count();
    let without_ws = p
        .state
        .pending_exec
        .values()
        .filter(|pe| pe.write_set.is_none())
        .count();
    assert_eq!(with_ws, 8, "the bound keeps exactly 8 write sets");
    assert_eq!(without_ws, 2, "the oldest 2 are post-root-only");

    // resolve still reconstructs the deepest post-state, recomputing the dropped
    // ancestors from stored blocks and verifying each against its cached post root.
    let view = p
        .state
        .resolve_parent_state(block_hash(&p.pending[9]), p.pending[9].height, &p.db)
        .expect("deep resolve across the bound");
    assert_eq!(
        overlay_root(&view),
        p.roots[9],
        "deep-lag parent state recomputed across the write-set bound"
    );
}
