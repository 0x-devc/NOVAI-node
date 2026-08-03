//! Gate wedge-276272 RED tests: bind the durable vote to execution.
//!
//! Reproduces the wedge-276272 class in-process. A block is durably voted while
//! its header carries a PRE-execution state root; then the block beneath it
//! executes and moves the root, so the next pipelined block (already voted, still
//! carrying the stale pre-state root) fails the catch-up pre-commit check and the
//! chain wedges the instant real state changes.
//!
//! Header semantics today: `header(H).state_root` is stamped from `KEY_SMT_ROOT`
//! at propose time, which equals `post-state(H-1)` only while every uncommitted
//! ancestor is root-neutral. The fix makes it `post-state(H)` by executing the
//! block in a non-persisting overlay before the vote. These tests are written
//! RED-first against the fixed behavior: Test W must flip GREEN without edits when
//! the five-surface flip lands; Test P captures the pre-fix fingerprint and is
//! rewritten to the new impossibility at fix time.
//!
//! Harness: a single validator (n=1, quorum 1). One node is its own leader,
//! voter, and committer, so it builds every header through the REAL propose path
//! (which stamps `KEY_SMT_ROOT` today and the computed post-root after the fix, so
//! the fixture tracks the flip with no test edit), durably votes through
//! `handle_proposal` (exercising `verify_block`, gate 9, and the synced
//! `persist_voted_view`), accumulates its own pending state, and commits through
//! `handle_qc`. The wedge is identity-independent, so collapsing proposer and
//! voter into one identity reproduces it faithfully while avoiding leader rotation
//! and cross-node pending-state shuttling.
//!
//! Commit granularity is load-bearing: `cache_qc_and_check_commit` commits the
//! range `committed+1 ..= qc_height-2`, and the pre-commit check inspects only
//! `to_commit[0]`. So H=1 must commit on `qc3` (executing the transfer, R0 to R1)
//! in a SEPARATE call before H=2 is attempted on `qc4`; only then does H=2's stale
//! R0 header meet the advanced R1 root and halt. A single `qc4` at committed 0
//! would commit [H1,H2] together and check only H1, hiding the bug.

use std::collections::HashMap;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use novai_consensus_types::codec::{
    encode_proposal_v1_unsigned, encode_vote_v1_unsigned, encode_voted_view_v1,
};
use novai_consensus_types::{block_hash, Block, Proposal, SignedProposal, Vote, QC};
use novai_crypto::{address_from_pubkey, sign_bytes, sign_tx_v1};
use novai_execution::{
    append_smt_ops_for_state_ops, dispatch_tx, empty_smt_root, encode_transfer_payload_v1,
    TransferPayloadV1,
};
use novai_node::consensus_node::{CommitCallback, ConsensusNode, Storage};
use novai_state::{
    account_key, decode_smt_root_v1, encode_account_v1, encode_fee_pool_v1, AccountStateV1,
    FeePoolV1, Kv, KvBatch, WriteOp, KEY_FEE_POOL, KEY_SMT_ROOT, KEY_VOTED_VIEW,
};
use novai_types::{Address, TxV1, TxVersion};

/// The sender's deterministic key. Not a validator, just a funded account.
const SENDER_SEED: [u8; 32] = [9u8; 32];
const RECIPIENT: Address = [7u8; 32];
const TRANSFER_AMOUNT: u64 = 1_000;
const TRANSFER_FEE: u64 = 1_000;
const SENDER_BALANCE: u128 = 1_000_000;

/// The commit callback that actually executes committed transactions, the
/// documented seam (`set_commit_callback`). Mirrors `on_commit`'s per-tx
/// `dispatch_tx` loop and its skip-on-failure semantics; no nonce map, index, or
/// metrics (commit-only concerns the vote path must never touch).
struct TestExec;

impl CommitCallback for TestExec {
    fn on_commit(&self, db: &mut Storage, blocks: &[Block]) {
        for block in blocks {
            for tx in &block.txs {
                // Failed txs are skipped root-neutrally, exactly like on_commit.
                let _ = dispatch_tx(db, tx, block.height);
            }
        }
    }
}

/// Always-zero nonce provider. The single funded sender starts at nonce 0 and
/// sends exactly one transfer, so a constant zero is correct for the drain.
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

/// A single-validator node under test (n=1, quorum 1), with the executing commit
/// callback wired. Deterministic key, so no randomness. Returns the node, its
/// signing key (kept to sign proposals and votes as the sole validator), and its
/// address.
fn single_validator_node() -> (ConsensusNode, SigningKey, Address) {
    let sk = SigningKey::from_bytes(&[1u8; 32]);
    let addr = address_from_pubkey(&sk.verifying_key());
    let validator_set = vec![addr];
    let mut pubkeys = HashMap::new();
    pubkeys.insert(addr, sk.verifying_key());
    let mut node = ConsensusNode::new(sk.clone(), validator_set, pubkeys, 1000);
    node.set_commit_callback(Arc::new(TestExec));
    (node, sk, addr)
}

/// Fund state through the canonical execution path so `KEY_SMT_ROOT` reflects the
/// funded rows (this is R0). Mirrors the a0 fixture pattern: per-write atomic
/// batches of flat put plus SMT node puts plus the root record.
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

/// A validly signed transfer from the funded sender (nonce 0). The pubkey is the
/// real key so `verify_block`'s per-tx signature check accepts it.
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

/// A domain-separated, validly signed vote.
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

/// The single-vote QC that certifies `block` (quorum 1 for n=1).
fn certifying_qc(sk: &SigningKey, voter: Address, block: &Block) -> QC {
    let bh = block_hash(block);
    QC {
        height: block.height,
        round: block.round,
        block_hash: bh,
        votes: vec![signed_vote(sk, voter, block.height, block.round, bh)],
    }
}

/// The genesis zero-QC that justifies a height-1 proposal.
fn genesis_qc() -> QC {
    QC {
        height: 0,
        round: 0,
        block_hash: [0u8; 32],
        votes: vec![],
    }
}

/// Wrap a block as a signed proposal from the sole validator (the leader for
/// every height when n=1).
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

/// Build the next block through the REAL propose path, so its header `state_root`
/// is whatever the production proposer stamps (KEY_SMT_ROOT today, the computed
/// post-root after the fix). Requires the node to be the leader for the intended
/// height, which always holds at n=1.
fn propose_next(node: &ConsensusNode, mempool: &mut mempool::TxMempool, np: &ZeroNonce) -> Block {
    let vset = node.validator_set.clone();
    let mut state = node.state.lock().unwrap();
    let db = node.db.lock().unwrap();
    state
        .propose_block_with_budget(mempool, np, &*db, &vset, novai_types::MAX_BLOCK_SIZE)
        .expect("propose next block")
}

/// Observables captured from one faithful drive. Test W reads `qc4_result` and
/// `committed_final`; the rest document the drive's intermediate state.
#[allow(dead_code)]
struct Observations {
    r0: [u8; 32],
    r1: [u8; 32],
    voted_view_mem_after_votes: Option<(u64, u64)>,
    voted_view_durable_after_votes: Option<Vec<u8>>,
    committed_after_votes: u64,
    committed_after_qc3: u64,
    qc4_result: Result<(), String>,
    committed_final: u64,
}

/// Run the full wedge drive on a fresh node and capture every observable. The
/// harness-soundness checks (H=1 must commit and the transfer must move the root)
/// are asserted here so a silently-skipped tx cannot make either test vacuous.
fn run_wedge_drive(node: &ConsensusNode, sk: &SigningKey, addr: Address) -> Observations {
    let np = ZeroNonce;
    let mut mempool = mempool::TxMempool::new(1, 1000);

    // Fund and record R0 (pre-state before any mutation).
    fund(node);
    let r0 = read_root(node);

    // Queue exactly one successful transfer for H=1.
    mempool.insert(transfer_tx(), &np).expect("insert transfer");

    // H=1: propose (stamps R0 today), vote durably, then adopt the certifying QC
    // to advance the view for the next propose (no commit yet: qc height 1 < 2).
    let block1 = propose_next(node, &mut mempool, &np);
    assert_eq!(block1.height, 1, "first proposal must be height 1");
    assert_eq!(block1.txs.len(), 1, "H=1 must carry the single transfer");
    node.handle_proposal(signed_proposal(block1.clone(), genesis_qc(), addr, sk))
        .expect("vote H=1");
    let qc1 = certifying_qc(sk, addr, &block1);
    node.handle_qc(qc1).expect("adopt qc1 (no commit)");

    // H=2: empty successor. Today its header freezes R0 (KEY_SMT_ROOT is still R0
    // because H=1 has not committed). This is the block that later wedges.
    let block2 = propose_next(node, &mut mempool, &np);
    assert_eq!(block2.height, 2, "second proposal must be height 2");
    node.handle_proposal(signed_proposal(
        block2.clone(),
        certifying_qc(sk, addr, &block1),
        addr,
        sk,
    ))
    .expect("vote H=2");
    let qc2 = certifying_qc(sk, addr, &block2);
    node.handle_qc(qc2).expect("adopt qc2 (no commit)");

    // H=3: empty successor.
    let block3 = propose_next(node, &mut mempool, &np);
    assert_eq!(block3.height, 3, "third proposal must be height 3");
    node.handle_proposal(signed_proposal(
        block3.clone(),
        certifying_qc(sk, addr, &block2),
        addr,
        sk,
    ))
    .expect("vote H=3");
    let qc3 = certifying_qc(sk, addr, &block3);

    // Poison capture point: the node has durably voted (1,0),(2,0),(3,0) while it
    // has committed and executed NOTHING. Votes precede execution.
    let voted_view_mem_after_votes = node.state.lock().unwrap().voted_view;
    let voted_view_durable_after_votes = node
        .db
        .lock()
        .unwrap()
        .get(KEY_VOTED_VIEW)
        .expect("get voted_view");
    let committed_after_votes = node.state.lock().unwrap().committed_height;

    // Deliver qc3: the 3-chain commits H=1 in its OWN batch, executing the
    // transfer (R0 to R1). This must NOT halt: H=1's header R0 matches the current
    // root R0 (it is the first commit).
    node.handle_qc(qc3).expect("commit H=1 (executes the transfer)");
    let committed_after_qc3 = node.state.lock().unwrap().committed_height;
    let r1 = read_root(node);

    // Harness soundness / non-vacuousness: the transfer must have executed and
    // moved the root, or the wedge cannot trigger and both tests are vacuous.
    assert_eq!(
        committed_after_qc3, 1,
        "H=1 must commit cleanly (committed 0 to 1); got {committed_after_qc3}"
    );
    assert_ne!(
        r1, r0,
        "the transfer must move the SMT root (R1 != R0); if equal, the tx was \
         skipped and the wedge repro is vacuous. r0={r0:02x?} r1={r1:02x?}"
    );

    // H=4: an empty carrier so qc4's 3-chain can reach H=2. Built through the
    // propose path but NOT voted, so the durable vote mark stays at (3,0).
    let block4 = propose_next(node, &mut mempool, &np);
    assert_eq!(block4.height, 4, "carrier proposal must be height 4");
    node.state
        .lock()
        .unwrap()
        .cache_block(block4.clone())
        .expect("cache H=4");
    let qc4 = certifying_qc(sk, addr, &block4);

    // Deliver qc4: commit_target = 2, committed = 1, so to_commit = [block2].
    // Today block2's header R0 meets the advanced root R1, so the catch-up commit
    // halts. After the fix block2's header carries R1 (post-state) and it commits.
    let qc4_result = node.handle_qc(qc4);
    let committed_final = node.state.lock().unwrap().committed_height;

    Observations {
        r0,
        r1,
        voted_view_mem_after_votes,
        voted_view_durable_after_votes,
        committed_after_votes,
        committed_after_qc3,
        qc4_result,
        committed_final,
    }
}

/// Test W: RED today, GREEN after the five-surface flip (no edits).
///
/// Asserts the post-fix outcome: the pipelined successor H=2 commits cleanly
/// because its header carries the post-execution root. Today the drive wedges at
/// the catch-up halt, so this assertion fails printing the halt string, which is
/// the RED-first proof that the fix is not yet present.
#[test]
fn wedge_pipelined_successor_commits_after_mutation() {
    let (node, sk, addr) = single_validator_node();
    let obs = run_wedge_drive(&node, &sk, addr);

    assert!(
        obs.qc4_result.is_ok(),
        "RED until the fix lands: the pipelined successor H=2 was durably voted \
         carrying a pre-execution root and now cannot commit. Expected a clean \
         commit; got the catch-up halt: {:?}",
        obs.qc4_result
    );
    assert_eq!(
        obs.committed_final, 2,
        "post-fix the successor commits (committed 1 to 2); got {}",
        obs.committed_final
    );
}

/// Test P (flipped at fix time, gate wedge-276272): the post-fix impossibility.
///
/// After H=1 (a transfer) is voted with a post-state header (R1 != R0), a
/// hand-built pipelined successor carrying the STALE pre-state root R0 (the old
/// convention, the exact block that wedged the chain pre-fix) is REFUSED at
/// verify_block, and the durable vote mark does NOT advance to it. The Phase-0
/// form of this test captured that same block being durably voted and wedging the
/// catch-up commit; the pinned behaviors map cleanly across the flip: R1 != R0
/// (the mutation is real) still holds, and "votes precede execution" becomes "the
/// pre-state successor is refused before any durable mark can be written".
#[test]
fn pre_state_successor_refused_at_verify_and_unmarked() {
    let (node, sk, addr) = single_validator_node();
    let np = ZeroNonce;
    let mut mempool = mempool::TxMempool::new(1, 1000);

    fund(&node);
    let r0 = read_root(&node);
    mempool.insert(transfer_tx(), &np).expect("insert transfer");

    // H=1 carries the transfer; the fixed propose path stamps post-state(1) = R1.
    let block1 = propose_next(&node, &mut mempool, &np);
    assert_eq!(block1.txs.len(), 1, "H=1 carries the single transfer");
    assert_ne!(
        block1.state_root, r0,
        "the transfer moved the root: post-state(1) = R1 != R0, so the stale root is genuinely wrong"
    );
    node.handle_proposal(signed_proposal(block1.clone(), genesis_qc(), addr, &sk))
        .expect("H=1 with a post-state header is accepted and voted");
    assert_eq!(
        node.state.lock().unwrap().voted_view,
        Some((1, 0)),
        "H=1 is durably voted"
    );

    // Adopt qc1 so a successor at H=2 ties to block1 as its parent.
    node.handle_qc(certifying_qc(&sk, addr, &block1))
        .expect("adopt qc1 (no commit at height 1)");

    // A hand-built pipelined successor carrying the STALE pre-state root R0 (the
    // old convention) instead of post-state(2) = R1. This is the exact block that
    // wedged the chain pre-fix.
    let stale_h2 = Block {
        height: 2,
        round: 0,
        parent_hash: block_hash(&block1),
        state_root: r0, // STALE: old-convention pre-state root, not post-state(2)
        txs: vec![],
    };

    let result = node.handle_proposal(signed_proposal(
        stale_h2,
        certifying_qc(&sk, addr, &block1),
        addr,
        &sk,
    ));

    // Refused at verify_block (post-state(2) = R1 != R0), and the durable vote mark
    // does NOT advance to the refused successor.
    let err = result
        .expect_err("a pipelined successor carrying the stale pre-state root must be refused");
    assert!(
        err.contains("state root mismatch"),
        "the refusal must be the verify_block post-state check; got: {err}"
    );
    assert_eq!(
        node.state.lock().unwrap().voted_view,
        Some((1, 0)),
        "the durable vote mark must NOT advance to the refused pre-state successor"
    );
    assert_eq!(
        node.db.lock().unwrap().get(KEY_VOTED_VIEW).unwrap(),
        Some(encode_voted_view_v1(1, 0)),
        "on disk the durable mark stays at (1,0); the wedge block leaves no trace"
    );
}
