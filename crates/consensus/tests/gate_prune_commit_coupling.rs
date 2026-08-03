//! Pruning-requires-commit coupling (incident WEDGE-20260718): no
//! consensus block or QC row is ever deleted except inside an atomic batch
//! that also advances the committed-height cursor, and the deletion floor
//! is measured from the COMMITTED clock, never the consensus/QC clock.
//!
//! This property is why the 20260718 wedge was recoverable at all: commits
//! froze at the floor, so pruning froze with them, and the committed
//! window plus the floor QC row survived five days of frontier runaway.
//! At the tree this test was written against, the property holds by
//! structure: the only deleter of `consensus/blocks/` and `consensus/qcs/`
//! rows in the workspace is step 6 of `persist_commit_atomic`
//! (crates/consensus/src/lib.rs), whose floor is
//! `new_committed_height - PRUNE_RETAIN_BLOCKS`, whose per-block delete is
//! `block.height - PRUNE_RETAIN_BLOCKS`, and whose Delete ops ride the
//! same `ops` vector as the `KEY_COMMITTED_HEIGHT` Put, applied in one
//! `apply_batch` call. The read paths cannot delete by construction:
//! `cache_qc_and_check_commit` takes `db: &K` with only the read trait in
//! scope for writes at all.
//!
//! Structure is not a pin. A future refactor (a background GC for
//! compaction smoothing, a startup sweeper, an off-thread retention task,
//! or a floor measured from the QC height) would compile fine and pass
//! every other test, and it would delete exactly the rows a stalled fleet
//! needs to recover. These tests are the pin:
//!
//! - the no-commit drive: the full consensus machinery (proposals, votes,
//!   QC formation, adoption persists, vote-mark persists, timeout churn)
//!   runs a wedge-shaped stall to the commit-window park with ZERO commits,
//!   through a recording KV; the recorder must observe zero deletions on
//!   the two families, from any path, batched or direct.
//! - the commit drive: committing batches across the retention boundary
//!   must delete ONLY inside batches that advance the cursor, ONLY heights
//!   in lockstep with the committed blocks in that same batch, and never
//!   above the committed floor. The trigger QC rides two heights above the
//!   committed height, so a floor measured from the QC clock is caught.
//!
//! RED discipline for a regression pin: the tree is currently correct, so
//! the proof that these tests detect the bug class is by mutation. Each
//! feared refactor was applied to the working tree in turn (a delete on
//! the QC-adoption persist path; the prune deletes split into a second
//! batch; the floor measured from the trigger QC height), the relevant
//! test was proven to FAIL for the stated reason, and the mutation was
//! reverted before commit.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::{ConsensusState, PRUNE_RETAIN_BLOCKS};
use novai_consensus_types::codec::hash_block_v1;
use novai_consensus_types::{Block, Vote, QC};
use novai_crypto::address_from_pubkey;
use novai_state::{block_key, qc_key, Kv, KvBatch, MemKv, WriteOp, KEY_COMMITTED_HEIGHT};
use novai_types::Address;

/// KV wrapper that records every mutation while delegating to MemKv, so a
/// test can assert what the consensus engine wrote and deleted, and in
/// which atomic unit.
struct RecordingKv {
    inner: MemKv,
    batches: Vec<Vec<WriteOp>>,
    direct_deletes: Vec<Vec<u8>>,
}

impl RecordingKv {
    fn new() -> Self {
        Self {
            inner: MemKv::new(),
            batches: Vec::new(),
            direct_deletes: Vec::new(),
        }
    }

    /// Every deleted key across both surfaces: batched and direct.
    fn all_deleted_keys(&self) -> Vec<Vec<u8>> {
        let mut keys: Vec<Vec<u8>> = self.direct_deletes.clone();
        for batch in &self.batches {
            for op in batch {
                if let WriteOp::Delete(k) = op {
                    keys.push(k.clone());
                }
            }
        }
        keys
    }
}

impl Kv for RecordingKv {
    type Error = ();

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, ()> {
        self.inner.get(key)
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), ()> {
        self.inner.put(key, value)
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), ()> {
        self.direct_deletes.push(key.to_vec());
        self.inner.delete(key)
    }

    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, ()> {
        self.inner.scan_prefix(prefix)
    }
}

impl KvBatch for RecordingKv {
    fn apply_batch(&mut self, ops: &[WriteOp]) -> Result<(), ()> {
        self.batches.push(ops.to_vec());
        self.inner.apply_batch(ops)
    }
}

fn make_validators(count: usize) -> Vec<(Address, SigningKey, VerifyingKey)> {
    (0..count)
        .map(|i| {
            let sk = SigningKey::from_bytes(&[i as u8; 32]);
            let pk = sk.verifying_key();
            (address_from_pubkey(&pk), sk, pk)
        })
        .collect()
}

fn make_block(height: u64, parent_hash: [u8; 32]) -> Block {
    Block {
        height,
        round: 0,
        parent_hash,
        state_root: novai_execution::empty_smt_root(),
        txs: vec![],
    }
}

fn make_vote(height: u64, block_hash: [u8; 32], voter: Address) -> Vote {
    Vote {
        height,
        round: 0,
        block_hash,
        voter,
        signature: [0u8; 64],
        ai_signal_commitment: None,
    }
}

/// The two consensus row families the retention contract governs
/// (crates/state/src/lib.rs `block_key` / `qc_key`): prefix derived from
/// the key helpers themselves so a key-layout change cannot silently
/// detach this test from the real families.
fn family_prefixes() -> (Vec<u8>, Vec<u8>) {
    let block_prefix = {
        let k = block_key(0);
        k[..k.len() - 8].to_vec()
    };
    let qc_prefix = {
        let k = qc_key(0);
        k[..k.len() - 8].to_vec()
    };
    (block_prefix, qc_prefix)
}

fn family_height(key: &[u8]) -> Option<(&'static str, u64)> {
    let (block_prefix, qc_prefix) = family_prefixes();
    let (family, prefix) = if key.starts_with(&block_prefix) {
        ("blocks", block_prefix)
    } else if key.starts_with(&qc_prefix) {
        ("qcs", qc_prefix)
    } else {
        return None;
    };
    let suffix = &key[prefix.len()..];
    if suffix.len() != 8 {
        return None;
    }
    let mut be = [0u8; 8];
    be.copy_from_slice(suffix);
    Some((family, u64::from_be_bytes(be)))
}

fn decode_cursor(batch: &[WriteOp]) -> Option<u64> {
    batch.iter().find_map(|op| match op {
        WriteOp::Put(k, v) if k.as_slice() == KEY_COMMITTED_HEIGHT => {
            let mut be = [0u8; 8];
            if v.len() == 8 {
                be.copy_from_slice(v);
                Some(u64::from_be_bytes(be))
            } else {
                None
            }
        }
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// The no-commit drive: a wedge-shaped stall must delete NOTHING
// ---------------------------------------------------------------------------

#[test]
fn no_commit_drive_never_deletes_consensus_rows() {
    let validators = make_validators(4);
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
    let pubkeys: Vec<(Address, VerifyingKey)> =
        validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

    let mut states: Vec<ConsensusState> = validator_set
        .iter()
        .map(|a| ConsensusState::new(*a))
        .collect();
    let mut dbs: Vec<RecordingKv> = (0..4).map(|_| RecordingKv::new()).collect();

    struct NP;
    impl mempool::NonceProvider for NP {
        fn expected_nonce(&self, _from: &Address) -> u64 {
            0
        }
    }

    // Phase 1: the incident shape. Commits stall at 0 (bodies unresolvable,
    // every commit walk fails) while the frontier climbs to the
    // commit-window park, with the node-shaped persists (highest QC and
    // vote mark) running against the recorded stores every height.
    // Reconceived (gate wedge-276272): post-fix a node cannot vote an UNRESOLVABLE
    // body, so the eliminated "vote on unresolvable data" climb is impossible. The
    // surviving stall a behind node sees: it ADOPTS the fleet frontier (ungated)
    // and advances its durable marks while commits stall (bodies not yet synced).
    // Each height adopts the fleet's QC (the commit walk finds no bodies and commits
    // nothing) and advances the durable vote mark via the engine backstop, running
    // the node-shaped cursor persists every height, committed frozen at 0.
    for h in 1..=1024u64 {
        let frontier = Block {
            height: h,
            round: 0,
            parent_hash: [0xCC; 32],
            state_root: [0u8; 32],
            txs: vec![],
        };
        let fqc = QC {
            height: h,
            round: 0,
            block_hash: hash_block_v1(&frontier).expect("hash"),
            votes: vec![],
        };
        for (state, db) in states.iter_mut().zip(dbs.iter_mut()) {
            let _ = state.cache_qc_and_check_commit(fqc.clone(), db);
            state
                .note_self_vote(h, 0)
                .unwrap_or_else(|e| panic!("self-vote inside the window at {h}: {e:?}"));
            state.persist_highest_qc(db).expect("persist hqc");
            state.persist_voted_view(db).expect("persist voted view");
            assert_eq!(state.committed_height, 0, "commits are stalled");
        }
    }

    // Phase 2: the parked fleet churns rounds on timeouts, as the wedged
    // fleet did for five days.
    for _round in 0..3 {
        let timeouts: Vec<_> = states
            .iter()
            .zip(validators.iter())
            .map(|(state, (_, sk, _))| state.create_timeout(sk).expect("create timeout"))
            .collect();
        for (state, db) in states.iter_mut().zip(dbs.iter_mut()) {
            for timeout in &timeouts {
                let _ = state.add_timeout(timeout.clone(), &pubkeys);
            }
            state.persist_highest_qc(db).expect("persist hqc");
        }
    }

    // The pin: five-days-in-miniature of commit-free consensus activity
    // deleted NOTHING from either consensus row family, on any surface.
    for (i, db) in dbs.iter().enumerate() {
        let family_deletes: Vec<String> = db
            .all_deleted_keys()
            .iter()
            .filter_map(|k| family_height(k))
            .map(|(family, height)| format!("{family}/{height}"))
            .collect();
        assert!(
            family_deletes.is_empty(),
            "node {i} deleted consensus rows {family_deletes:?} during a \
             commit-free drive; deletion without a committed advance is the \
             exact hazard that would have destroyed the WEDGE-20260718 \
             recovery window"
        );
        assert_eq!(
            states[i].committed_height, 0,
            "the drive must have been genuinely commit-free"
        );
        assert_eq!(
            states[i].highest_qc.as_ref().map(|q| q.height),
            Some(1024),
            "the drive must have genuinely climbed to the window park"
        );
    }
}

// ---------------------------------------------------------------------------
// The commit drive: deletion only inside cursor-advancing batches, floor
// on the committed clock
// ---------------------------------------------------------------------------

/// Every recorded batch that deletes from the two families must also
/// advance the committed cursor in the SAME batch, must delete only
/// heights in lockstep with that batch's committed blocks, and must stay
/// at or below the committed floor.
fn assert_deletes_are_commit_coupled(batches: &[Vec<WriteOp>], mut prev_committed: u64) {
    for (n, batch) in batches.iter().enumerate() {
        let deleted: Vec<(&'static str, u64)> = batch
            .iter()
            .filter_map(|op| match op {
                WriteOp::Delete(k) => family_height(k),
                _ => None,
            })
            .collect();
        let committed_block_heights: Vec<u64> = batch
            .iter()
            .filter_map(|op| match op {
                WriteOp::Put(k, _) => {
                    family_height(k).and_then(|(family, h)| (family == "blocks").then_some(h))
                }
                _ => None,
            })
            .collect();
        let cursor = decode_cursor(batch);

        if deleted.is_empty() {
            if let Some(c) = cursor {
                prev_committed = c;
            }
            continue;
        }

        let cursor = cursor.unwrap_or_else(|| {
            panic!(
                "batch {n} deletes consensus rows {deleted:?} without a \
                 committed-height cursor write in the same atomic batch"
            )
        });
        assert!(
            cursor > prev_committed,
            "batch {n} deletes consensus rows {deleted:?} but its cursor \
             write ({cursor}) does not advance past {prev_committed}"
        );
        let floor = cursor - PRUNE_RETAIN_BLOCKS;
        for (family, h) in &deleted {
            assert!(
                *h <= floor,
                "batch {n} deleted {family}/{h} above the committed floor \
                 {floor} (cursor {cursor} minus retention); the deletion \
                 floor must be measured from the committed clock"
            );
            assert!(
                committed_block_heights
                    .iter()
                    .any(|bh| bh.saturating_sub(PRUNE_RETAIN_BLOCKS) == *h),
                "batch {n} deleted {family}/{h}, which is not in lockstep \
                 (block height minus retention) with any block committed in \
                 that same batch {committed_block_heights:?}; the per-height \
                 delete must ride its own commit"
            );
        }
        prev_committed = cursor;
    }
}

#[test]
fn commit_batches_couple_deletion_to_cursor_advance() {
    let validators = make_validators(4);
    let mut state = ConsensusState::new(validators[0].0);
    let mut db = RecordingKv::new();

    // Single-block commit batches crossing the retention boundary. The
    // trigger QC rides two heights above the committed height (the 3-chain
    // shape), so a deletion floor measured from the QC clock instead of
    // the committed clock lands on different heights and fails the
    // lockstep assertion below.
    let mut prev_committed = 0u64;
    for h in (PRUNE_RETAIN_BLOCKS + 1)..=(PRUNE_RETAIN_BLOCKS + 3) {
        let block = make_block(h, [0xAA; 32]);
        let block_hash = hash_block_v1(&block).expect("hash");
        let cqc = QC {
            height: h,
            round: 0,
            block_hash,
            votes: vec![],
        };
        state.qc_cache.insert(h, cqc);
        let trigger = QC {
            height: h + 2,
            round: 0,
            block_hash: [0x55; 32],
            votes: vec![],
        };
        state
            .persist_commit_atomic(&mut db, &[block], &trigger, h, None)
            .expect("persist commit");
        state.committed_height = h;
        prev_committed = h;
    }

    // A multi-block catch-up batch: three blocks, one cursor write, three
    // lockstep deletions, all in one atomic unit.
    let start = prev_committed + 1;
    let blocks: Vec<Block> = (start..start + 3)
        .map(|h| make_block(h, [0xBB; 32]))
        .collect();
    for block in &blocks {
        let block_hash = hash_block_v1(block).expect("hash");
        state.qc_cache.insert(
            block.height,
            QC {
                height: block.height,
                round: 0,
                block_hash,
                votes: vec![],
            },
        );
    }
    let new_committed = start + 2;
    let trigger = QC {
        height: new_committed + 2,
        round: 0,
        block_hash: [0x66; 32],
        votes: vec![],
    };
    state
        .persist_commit_atomic(&mut db, &blocks, &trigger, new_committed, None)
        .expect("persist multi-block commit");

    // The drive must have actually pruned (no vacuous pass), with every
    // deletion commit-coupled.
    let total_family_deletes: usize = db
        .all_deleted_keys()
        .iter()
        .filter(|k| family_height(k).is_some())
        .count();
    assert_eq!(
        total_family_deletes, 12,
        "six committed heights past the boundary must prune six heights in \
         two families"
    );
    assert!(
        db.direct_deletes.is_empty(),
        "prune deletions must ride the atomic batch, never the direct \
         delete surface"
    );
    assert_deletes_are_commit_coupled(&db.batches, 0);
}
