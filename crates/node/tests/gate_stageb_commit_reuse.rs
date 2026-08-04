//! Gate ACCEL Stage B, test (a) arms a1 and a2: cached-hit reuse end to end,
//! and the cache-miss re-execution fallback
//! (docs/gate-accel-stageB-execution-batching-design.md, section 4, test a).
//!
//! Harness: the single-validator pattern (n=1, quorum 1) from the Stage A gate
//! test. Every header is built through the REAL propose path (stamping the
//! post-execution root and caching the write set in `pending_exec`), voted
//! through `handle_proposal` (which re-executes, verifies, and re-caches), and
//! committed through `handle_qc` (where the commit site takes the cached entry
//! and the callback applies it as one batch).
//!
//! The callback drives `resolve_and_apply_block`, the SAME production core the
//! node binary's `ExecutionCommitCallback` calls, and records whether each
//! commit was a cached hit or a re-execution miss. The oracle is the design's
//! per-tx executor: `dispatch_tx` over a `MemKv` seeded with the pre-block
//! state families, compared byte for byte against the node's state families
//! after commit (accounts, the whole smt/ namespace including the root record,
//! and the fee pool; consensus rows are outside execution's write surface).
//!
//! a1: the commit consumes the cache (hit recorded, entry gone from
//! `pending_exec` while higher pending entries survive), applies as one batch
//! (cursor rode the batch), and the resulting bytes equal the per-tx oracle.
//! a2: with `pending_exec` cleared before the committing QC (the restart
//! simulation), the same drive takes the re-execution fallback and produces
//! identical bytes. Non-vacuousness in both arms: the transfer really applied
//! (root moved, outcomes all Applied).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;
use novai_consensus_types::codec::encode_proposal_v1_unsigned;
use novai_consensus_types::{block_hash, Block, Proposal, SignedProposal, Vote, QC};
use novai_consensus_types::codec::encode_vote_v1_unsigned;
use novai_crypto::{address_from_pubkey, sign_bytes, sign_tx_v1};
use novai_execution::{
    append_smt_ops_for_state_ops, dispatch_tx, empty_smt_root, encode_transfer_payload_v1,
    TransferPayloadV1, TxOutcome,
};
use novai_node::consensus_node::{CommitCallback, ConsensusNode, Storage};
use novai_node::exec_apply::{resolve_and_apply_block, CachedExec};
use novai_state::{
    account_key, decode_smt_root_v1, encode_account_v1, encode_fee_pool_v1, AccountStateV1,
    FeePoolV1, Kv, KvBatch, MemKv, WriteOp, KEY_EXECUTED_HEIGHT, KEY_FEE_POOL, KEY_SMT_ROOT,
};
use novai_types::{Address, TxV1, TxVersion};

const SENDER_SEED: [u8; 32] = [9u8; 32];
const RECIPIENT: Address = [7u8; 32];
const TRANSFER_AMOUNT: u64 = 1_000;
const TRANSFER_FEE: u64 = 1_000;
const SENDER_BALANCE: u128 = 1_000_000;

/// The state families execution writes: everything the oracle comparison
/// covers. `smt/` spans both the node records and the root record.
const STATE_FAMILY_PREFIXES: [&[u8]; 3] = [b"accounts/", b"smt/", b"fee_pool"];

/// Commit callback driving the production commit core and recording the
/// resolve decisions and outcomes for the assertions.
struct RecordingExec {
    cached_hits: AtomicUsize,
    misses: AtomicUsize,
    outcomes: Mutex<Vec<(u64, Vec<TxOutcome>)>>,
}

impl RecordingExec {
    fn new() -> Self {
        Self {
            cached_hits: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
            outcomes: Mutex::new(Vec::new()),
        }
    }
}

impl CommitCallback for RecordingExec {
    fn on_commit(
        &self,
        db: &mut Storage,
        block: &Block,
        cached: Option<CachedExec>,
    ) -> Result<(), String> {
        if cached.is_some() {
            self.cached_hits.fetch_add(1, Ordering::SeqCst);
        } else {
            self.misses.fetch_add(1, Ordering::SeqCst);
        }
        let outs = resolve_and_apply_block(db, block, cached)?;
        self.outcomes.lock().unwrap().push((block.height, outs));
        Ok(())
    }
}

/// Always-zero nonce provider: the single funded sender sends one transfer.
struct ZeroNonce;

impl mempool::NonceProvider for ZeroNonce {
    fn expected_nonce(&self, _from: &Address) -> u64 {
        0
    }
}

fn sender_key() -> SigningKey {
    SigningKey::from_bytes(&SENDER_SEED)
}

fn sender_address() -> Address {
    address_from_pubkey(&sender_key().verifying_key())
}

fn single_validator_node() -> (ConsensusNode, Arc<RecordingExec>, SigningKey, Address) {
    let sk = SigningKey::from_bytes(&[1u8; 32]);
    let addr = address_from_pubkey(&sk.verifying_key());
    let validator_set = vec![addr];
    let mut pubkeys = HashMap::new();
    pubkeys.insert(addr, sk.verifying_key());
    let mut node = ConsensusNode::new(sk.clone(), validator_set, pubkeys, 1000);
    let exec = Arc::new(RecordingExec::new());
    node.set_commit_callback(Arc::clone(&exec) as Arc<dyn CommitCallback>);
    (node, exec, sk, addr)
}

/// Fund through the canonical execution path so `KEY_SMT_ROOT` reflects the
/// funded rows.
fn fund(node: &ConsensusNode) {
    let pairs: Vec<(Vec<u8>, Vec<u8>)> = vec![
        (
            account_key(&sender_address()),
            encode_account_v1(&AccountStateV1 {
                balance: SENDER_BALANCE,
                nonce: 0,
            })
            .to_vec(),
        ),
        (
            account_key(&RECIPIENT),
            encode_account_v1(&AccountStateV1 {
                balance: 10_000,
                nonce: 0,
            })
            .to_vec(),
        ),
        (
            KEY_FEE_POOL.to_vec(),
            encode_fee_pool_v1(&FeePoolV1 { balance: 0 }).to_vec(),
        ),
    ];
    let mut db = node.db.lock().unwrap();
    for (k, v) in &pairs {
        let ops = vec![WriteOp::Put(k.clone(), v.clone())];
        let mut all = ops.clone();
        append_smt_ops_for_state_ops(&*db, &ops, &mut all).expect("append smt ops");
        db.apply_batch(&all).expect("apply funding batch");
    }
}

fn read_root(node: &ConsensusNode) -> [u8; 32] {
    let db = node.db.lock().unwrap();
    match db.get(KEY_SMT_ROOT).expect("get smt root") {
        Some(b) => decode_smt_root_v1(&b).expect("decode smt root"),
        None => empty_smt_root(),
    }
}

fn read_cursor(node: &ConsensusNode) -> Option<u64> {
    let db = node.db.lock().unwrap();
    db.get(KEY_EXECUTED_HEIGHT).expect("get cursor").map(|b| {
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&b);
        u64::from_be_bytes(arr)
    })
}

/// Snapshot the state families from the node's storage into a `MemKv` (the
/// per-tx oracle's starting state) or into a sorted row list (the comparison).
fn state_family_rows(db: &Storage) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut rows = Vec::new();
    for prefix in STATE_FAMILY_PREFIXES {
        rows.extend(db.scan_prefix(prefix).expect("scan state family"));
    }
    rows.sort();
    rows
}

fn transfer_tx() -> TxV1 {
    let sk = sender_key();
    let pk = sk.verifying_key();
    let payload = encode_transfer_payload_v1(&TransferPayloadV1 {
        to: RECIPIENT,
        amount: TRANSFER_AMOUNT,
    })
    .to_vec();
    let mut tx = TxV1 {
        version: TxVersion::V1,
        from: address_from_pubkey(&pk),
        pubkey: pk.to_bytes(),
        nonce: 0,
        fee: TRANSFER_FEE,
        payload,
        sig: [0u8; 64],
    };
    sign_tx_v1(&sk, &mut tx).expect("sign transfer");
    tx
}

fn signed_vote(sk: &SigningKey, voter: Address, height: u64, round: u64, bh: [u8; 32]) -> Vote {
    let unsigned = Vote {
        height,
        round,
        block_hash: bh,
        voter,
        signature: [0u8; 64],
        ai_signal_commitment: None,
    };
    let bytes = encode_vote_v1_unsigned(&unsigned);
    let mut to_sign = Vec::new();
    to_sign.extend_from_slice(b"NOVAI_VOTE_V1");
    to_sign.extend_from_slice(&bytes);
    Vote {
        signature: sign_bytes(sk, &to_sign),
        ..unsigned
    }
}

fn certifying_qc(sk: &SigningKey, voter: Address, block: &Block) -> QC {
    let bh = block_hash(block);
    QC {
        height: block.height,
        round: block.round,
        block_hash: bh,
        votes: vec![signed_vote(sk, voter, block.height, block.round, bh)],
    }
}

fn genesis_qc() -> QC {
    QC {
        height: 0,
        round: 0,
        block_hash: [0u8; 32],
        votes: vec![],
    }
}

fn signed_proposal(
    block: Block,
    justify_qc: QC,
    proposer: Address,
    sk: &SigningKey,
) -> SignedProposal {
    let proposal = Proposal { block, justify_qc };
    let unsigned = encode_proposal_v1_unsigned(&proposal).expect("encode proposal");
    SignedProposal {
        proposer,
        proposal,
        signature: sign_bytes(sk, &unsigned),
    }
}

fn propose_next(node: &ConsensusNode, mempool: &mut mempool::TxMempool, np: &ZeroNonce) -> Block {
    let vset = node.validator_set.clone();
    let mut state = node.state.lock().unwrap();
    let db = node.db.lock().unwrap();
    state
        .propose_block_with_budget(mempool, np, &*db, &vset, novai_types::MAX_BLOCK_SIZE)
        .expect("propose next block")
}

/// Drive the pipeline to the brink of the TX-BEARING block's commit.
///
/// Shape: H=1 is EMPTY and H=2 carries the transfer, because a height-1 block
/// can never take the cached path by design: its parent is genesis, which is
/// never persisted, so the parent binding is unavailable and
/// `execute_committed_blocks` conservatively degrades the cached entry to a
/// re-execution miss (cached bytes are never applied unchecked). H=2's parent
/// header (H=1) is loadable from the db, so its commit is the first that can
/// be a cached hit. The drive votes H=1..=H=4, delivers qc3 (committing the
/// empty H=1, the documented genesis-edge miss), and stops at the brink of
/// qc4 (which commits H=2, the block both arms assert on).
struct Drive {
    node: ConsensusNode,
    exec: Arc<RecordingExec>,
    sk: SigningKey,
    addr: Address,
    r0: [u8; 32],
    block2: Block,
    block3: Block,
    block4: Block,
    oracle_rows: Vec<(Vec<u8>, Vec<u8>)>,
    oracle_root: [u8; 32],
}

fn drive_to_commit_brink() -> Drive {
    let (node, exec, sk, addr) = single_validator_node();
    let np = ZeroNonce;
    let mut mempool = mempool::TxMempool::new(1, 1000);

    fund(&node);
    let r0 = read_root(&node);

    // H=1: empty (the mempool is empty), voted.
    let block1 = propose_next(&node, &mut mempool, &np);
    assert_eq!(block1.height, 1, "first proposal must be height 1");
    assert!(block1.txs.is_empty(), "H=1 must be empty");
    node.handle_proposal(signed_proposal(block1.clone(), genesis_qc(), addr, &sk))
        .expect("vote H=1");
    node.handle_qc(certifying_qc(&sk, addr, &block1))
        .expect("adopt qc1 (no commit)");

    // H=2 carries the transfer; the vote populates pending_exec with its
    // write set.
    mempool.insert(transfer_tx(), &np).expect("insert transfer");
    let block2 = propose_next(&node, &mut mempool, &np);
    assert_eq!(block2.height, 2);
    assert_eq!(block2.txs.len(), 1, "H=2 must carry the single transfer");
    node.handle_proposal(signed_proposal(
        block2.clone(),
        certifying_qc(&sk, addr, &block1),
        addr,
        &sk,
    ))
    .expect("vote H=2");
    node.handle_qc(certifying_qc(&sk, addr, &block2))
        .expect("adopt qc2 (no commit)");

    // H=3: empty, voted.
    let block3 = propose_next(&node, &mut mempool, &np);
    assert_eq!(block3.height, 3);
    node.handle_proposal(signed_proposal(
        block3.clone(),
        certifying_qc(&sk, addr, &block2),
        addr,
        &sk,
    ))
    .expect("vote H=3");

    // Oracle seed BEFORE any commit executes: the db still holds the funded
    // pre-block state (H=1 is empty, so post-state(1) has the same state
    // families), which is exactly H=2's parent state.
    assert_eq!(node.state.lock().unwrap().committed_height, 0);
    let (oracle_rows, oracle_root) = {
        let pre_rows = {
            let db = node.db.lock().unwrap();
            state_family_rows(&db)
        };
        // Oracle: the design's per-tx executor over a MemKv seeded with the
        // pre-block state families.
        let mut oracle = MemKv::new();
        for (k, v) in &pre_rows {
            oracle.put(k, v).expect("seed oracle row");
        }
        for tx in &block2.txs {
            dispatch_tx(&mut oracle, tx, block2.height).expect("oracle tx must apply");
        }
        let root = match oracle.get(KEY_SMT_ROOT).unwrap() {
            Some(b) => decode_smt_root_v1(&b).unwrap(),
            None => empty_smt_root(),
        };
        let mut rows = Vec::new();
        for prefix in STATE_FAMILY_PREFIXES {
            rows.extend(oracle.scan_prefix(prefix).unwrap());
        }
        rows.sort();
        (rows, root)
    };

    // qc3 commits the empty H=1: the documented genesis-edge miss (a height-1
    // block's parent binding is unavailable, so the cached entry degrades to
    // a re-execution of an empty block).
    node.handle_qc(certifying_qc(&sk, addr, &block3))
        .expect("commit H=1 (empty)");
    assert_eq!(node.state.lock().unwrap().committed_height, 1);
    assert_eq!(
        exec.misses.load(Ordering::SeqCst),
        1,
        "H=1's commit is the documented genesis-edge miss"
    );
    assert_eq!(exec.cached_hits.load(Ordering::SeqCst), 0);
    assert_eq!(read_root(&node), r0, "empty H=1 leaves the root unmoved");

    // H=4: empty, voted, so qc4's 3-chain reaches H=2.
    let block4 = propose_next(&node, &mut mempool, &np);
    assert_eq!(block4.height, 4);
    node.handle_proposal(signed_proposal(
        block4.clone(),
        certifying_qc(&sk, addr, &block3),
        addr,
        &sk,
    ))
    .expect("vote H=4");

    Drive {
        node,
        exec,
        sk,
        addr,
        r0,
        block2,
        block3,
        block4,
        oracle_rows,
        oracle_root,
    }
}

/// Shared post-commit assertions for both arms: H=2 committed, the transfer
/// applied (root moved, outcomes Applied), the cursor rode the batch, and the
/// node's state families are byte-identical to the per-tx oracle.
fn assert_committed_state_matches_oracle(d: &Drive) {
    assert_eq!(
        d.node.state.lock().unwrap().committed_height,
        2,
        "H=2 must commit"
    );

    let r1 = read_root(&d.node);
    assert_ne!(r1, d.r0, "the transfer must move the root (non-vacuous)");
    assert_eq!(
        r1, d.block2.state_root,
        "the committed root equals H=2's post-state header"
    );
    assert_eq!(
        r1, d.oracle_root,
        "the committed root equals the per-tx oracle's root"
    );

    let outcomes = d.exec.outcomes.lock().unwrap();
    let h2 = outcomes
        .iter()
        .find(|(h, _)| *h == 2)
        .expect("H=2 outcomes recorded");
    assert!(
        h2.1.iter().all(|o| *o == TxOutcome::Applied),
        "every H=2 tx applied (non-vacuous); outcomes={:?}",
        h2.1
    );
    drop(outcomes);

    assert_eq!(
        read_cursor(&d.node),
        Some(2),
        "the executed cursor rode the block's batch"
    );

    let db = d.node.db.lock().unwrap();
    let rows = state_family_rows(&db);
    assert_eq!(
        rows, d.oracle_rows,
        "the node's state families must be byte-identical to the per-tx oracle"
    );
}

/// Arm a1: the commit consumes the vote-time cache and applies it end to end.
#[test]
fn stageb_commit_reuses_vote_time_write_set() {
    let d = drive_to_commit_brink();

    // The pending cache holds the uncommitted blocks' executions.
    {
        let state = d.node.state.lock().unwrap();
        for b in [&d.block2, &d.block3, &d.block4] {
            assert!(
                state.pending_exec.contains_key(&block_hash(b)),
                "pending_exec must hold H={} before commit",
                b.height
            );
        }
    }

    // qc4 commits H=2 (the transfer) through the real commit site.
    d.node
        .handle_qc(certifying_qc(&d.sk, d.addr, &d.block4))
        .expect("commit H=2");

    assert_committed_state_matches_oracle(&d);

    // H=2's commit was a cached hit; the only miss is H=1's genesis edge.
    assert_eq!(
        d.exec.cached_hits.load(Ordering::SeqCst),
        1,
        "H=2's commit must consume the vote-time cache"
    );
    assert_eq!(
        d.exec.misses.load(Ordering::SeqCst),
        1,
        "no further fallback beyond H=1's documented genesis-edge miss"
    );

    // The cache entry was consumed (taken, not merely evicted with the rest):
    // H=2's hash is gone while the higher pending entries survive the commit.
    let state = d.node.state.lock().unwrap();
    assert!(
        !state.pending_exec.contains_key(&block_hash(&d.block2)),
        "H=2's entry must be consumed by the commit"
    );
    for b in [&d.block3, &d.block4] {
        assert!(
            state.pending_exec.contains_key(&block_hash(b)),
            "H={} stays pending above the committed height",
            b.height
        );
    }
}

/// Arm a2: with the cache cleared (restart simulation), the same commit takes
/// the re-execution fallback and produces byte-identical state.
#[test]
fn stageb_commit_cache_miss_reexecutes_identically() {
    let d = drive_to_commit_brink();

    // Restart simulation: the in-memory cache is gone before the committing QC.
    d.node.state.lock().unwrap().pending_exec.clear();

    d.node
        .handle_qc(certifying_qc(&d.sk, d.addr, &d.block4))
        .expect("commit H=2 via the fallback");

    assert_committed_state_matches_oracle(&d);

    // The commit re-executed: two misses total (H=1's genesis edge, then
    // H=2's cleared cache), no cached hit anywhere.
    assert_eq!(
        d.exec.cached_hits.load(Ordering::SeqCst),
        0,
        "no cache to consume after the clear"
    );
    assert_eq!(
        d.exec.misses.load(Ordering::SeqCst),
        2,
        "H=2's commit must take the re-execution fallback"
    );
}
