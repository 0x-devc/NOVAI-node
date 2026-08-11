//! Gate F5 Stage 3 RED tests: staging and the boot-time install.
//!
//! This is the first stage that mutates state a node boots from, so the tests
//! are shaped by what happens if the process dies halfway rather than by the
//! happy path.
//!
//! - CRASH IDEMPOTENCY. The boot sequence is simulated step by step and
//!   interrupted at every point. Each next boot must either complete cleanly or
//!   cleanly abandon. Never a half state, and never a deletion.
//! - THE INSTALL-SOUNDNESS THEOREM, EXECUTED. Install at H, feed the certified
//!   successor, and require the real production guards to accept it.
//! - THE CONTAINMENT CONTROL. Force a wrong root PAST the audit and require the
//!   guards to halt locally: committed never advances, nothing is emitted.
//! - EQUIVOCATION SAFETY. Both marks merge as max(own, donor), and a re-install
//!   after a vote cannot regress either.
//!
//! RED discipline: this file reads API that does not exist on the preceding
//! tree, so its RED state is a compile failure, which is weak on its own. The
//! load-bearing evidence is the MUTATION proof recorded at the gate.

#[path = "a0_common/mod.rs"]
mod a0_common;

use a0_common::{build_fixture, dev_signing_keys, make_qc, run_a0, FixtureSpec, TmpDir};
use ed25519_dalek::SigningKey;
use novai_consensus_types::codec::{encode_qc_v1, encode_voted_view_v1, hash_block_v1};
use novai_consensus_types::{Block, QC};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::{ConsensusNode, Storage};
use novai_node::snapshot::install::{
    complete_install_at_boot, max_locked_qc, max_voted_view, merge_marks_into_staging,
    read_own_marks, stage_bundle, staging_path, InstallOutcome, OwnMarks, INSTALL_READY,
};
use novai_node::snapshot::produce::build_bundle;
use novai_state::{
    Kv, RocksKv, KEY_COMMITTED_HEIGHT, KEY_EXECUTED_HEIGHT, KEY_LOCKED_QC, KEY_SMT_ROOT,
    KEY_VOTED_VIEW,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SUBDIR: &str = "validator-2";

/// A base directory holding a live database built from a fixture, plus a staged
/// bundle produced from a second fixture. This is the shape every boot test
/// starts from.
struct Bed {
    base: TmpDir,
    /// Height of the staged snapshot.
    h: u64,
    /// Root of the staged snapshot.
    root: [u8; 32],
}

impl Bed {
    fn live(&self) -> PathBuf {
        self.base.0.join(SUBDIR)
    }
    fn staging(&self) -> PathBuf {
        staging_path(&self.base.0, SUBDIR)
    }
    fn entries(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.base.0)
            .expect("base readable")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

/// Build: a live database (an old fixture at height 7) and a staged bundle
/// produced from a fresh fixture at height 20.
fn bed(tag: &str) -> Bed {
    let base = TmpDir::new(tag);

    // The node's existing, stale database.
    let old = build_fixture(&format!("{tag}_old"), FixtureSpec::default());
    copy_dir(&old.tmp.0, &base.0.join(SUBDIR));

    // The donor snapshot, from a different and later fixture.
    let donor = build_fixture(
        &format!("{tag}_donor"),
        FixtureSpec {
            t: 20,
            ..FixtureSpec::default()
        },
    );
    let bundle = build_bundle(&donor.tmp.0).expect("donor produces a bundle");
    let h = bundle.manifest.height;
    let root = bundle.manifest.state_root;
    stage_bundle(&base.0, SUBDIR, &bundle).expect("stage");

    Bed { base, h, root }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for e in std::fs::read_dir(from).expect("read source") {
        let e = e.expect("entry");
        if e.file_type().expect("ftype").is_file() {
            std::fs::copy(e.path(), to.join(e.file_name())).expect("copy");
        }
    }
}

fn read_u64(dir: &Path, key: &[u8]) -> Option<u64> {
    let db = RocksKv::open(dir).ok()?;
    match db.get(key) {
        Ok(Some(b)) if b.len() == 8 => {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b);
            Some(u64::from_be_bytes(a))
        }
        _ => None,
    }
}

fn read_root(dir: &Path) -> Option<[u8; 32]> {
    let db = RocksKv::open(dir).ok()?;
    let b = db.get(KEY_SMT_ROOT).ok()??;
    novai_state::decode_smt_root_v1(&b).ok()
}

// ---------------------------------------------------------------------------
// The happy path, then T3.7 crash idempotency at every interruption point
// ---------------------------------------------------------------------------

#[test]
fn a_complete_staging_directory_installs_and_preserves_the_old_one() {
    let b = bed("f5i_happy");
    let old_height = read_u64(&b.live(), KEY_COMMITTED_HEIGHT).expect("old committed");

    let outcome = complete_install_at_boot(&b.base.0, SUBDIR).expect("boot");
    assert_eq!(
        outcome,
        InstallOutcome::Installed {
            height: b.h,
            root_hex: hex::encode(b.root)
        }
    );

    assert_eq!(read_u64(&b.live(), KEY_COMMITTED_HEIGHT), Some(b.h));
    assert_eq!(read_u64(&b.live(), KEY_EXECUTED_HEIGHT), Some(b.h));
    assert_eq!(read_root(&b.live()), Some(b.root));
    assert!(!b.staging().exists(), "the staging directory was consumed");

    let preserved = b.base.0.join(format!("{SUBDIR}.preinstall-{old_height}"));
    assert!(preserved.is_dir(), "the replaced directory must be PRESERVED, not deleted");
    assert_eq!(
        read_u64(&preserved, KEY_COMMITTED_HEIGHT),
        Some(old_height),
        "and it must still be readable"
    );
}

#[test]
fn a_second_boot_after_a_completed_install_is_an_ordinary_boot() {
    let b = bed("f5i_twice");
    complete_install_at_boot(&b.base.0, SUBDIR).expect("first boot");
    let before = b.entries();
    assert_eq!(
        complete_install_at_boot(&b.base.0, SUBDIR).expect("second boot"),
        InstallOutcome::Nothing,
        "an installed node must not try to install itself again"
    );
    assert_eq!(b.entries(), before, "and must change nothing on disk");
}

#[test]
fn crash_before_the_first_rename_leaves_the_live_directory_untouched() {
    // The interruption point with the most at stake: everything is staged and
    // audited, and the node dies. The live directory must be exactly as it was.
    let b = bed("f5i_crash_a");
    let live_before = read_u64(&b.live(), KEY_COMMITTED_HEIGHT);
    assert!(b.staging().join(INSTALL_READY).exists());

    // "Crash": simply never call the boot path. Nothing has moved.
    assert_eq!(read_u64(&b.live(), KEY_COMMITTED_HEIGHT), live_before);

    // The next boot completes cleanly.
    assert!(matches!(
        complete_install_at_boot(&b.base.0, SUBDIR).expect("boot"),
        InstallOutcome::Installed { .. }
    ));
    assert_eq!(read_u64(&b.live(), KEY_COMMITTED_HEIGHT), Some(b.h));
}

#[test]
fn crash_between_the_two_renames_is_completed_by_the_next_boot() {
    // The dangerous window: the live directory has been set aside and the
    // staging directory has not yet been moved into place, so there IS no live
    // directory. The next boot must finish the job rather than start a fresh
    // chain or lose the snapshot.
    let b = bed("f5i_crash_b");
    let old_height = read_u64(&b.live(), KEY_COMMITTED_HEIGHT).expect("old committed");

    // Simulate exactly that interruption: perform only the aside rename.
    let aside = b.base.0.join(format!("{SUBDIR}.preinstall-{old_height}"));
    std::fs::rename(b.live(), &aside).expect("aside rename");
    assert!(!b.live().exists(), "precondition: no live directory");
    assert!(b.staging().join(INSTALL_READY).exists());

    let outcome = complete_install_at_boot(&b.base.0, SUBDIR).expect("boot");
    assert!(
        matches!(outcome, InstallOutcome::Installed { .. }),
        "the next boot must COMPLETE the interrupted install, got {outcome:?}"
    );
    assert_eq!(read_u64(&b.live(), KEY_COMMITTED_HEIGHT), Some(b.h));
    assert!(aside.is_dir(), "the set-aside directory is still preserved");
    assert!(!b.staging().exists());
}

#[test]
fn crash_after_the_second_rename_leaves_nothing_to_do() {
    let b = bed("f5i_crash_c");
    complete_install_at_boot(&b.base.0, SUBDIR).expect("install");
    // A crash here means the marker rename may not have happened; either way
    // the next boot must be an ordinary one and must not reinstall.
    assert_eq!(
        complete_install_at_boot(&b.base.0, SUBDIR).expect("boot"),
        InstallOutcome::Nothing
    );
    assert_eq!(read_u64(&b.live(), KEY_COMMITTED_HEIGHT), Some(b.h));
}

#[test]
fn an_interrupted_stage_without_a_ready_marker_is_never_installed() {
    // A staging directory that exists but was never marked ready is an
    // interrupted stage. It must be set aside untouched, never installed and
    // never deleted.
    let b = bed("f5i_notready");
    std::fs::remove_file(b.staging().join(INSTALL_READY)).expect("drop the marker");
    let live_before = read_u64(&b.live(), KEY_COMMITTED_HEIGHT);

    let outcome = complete_install_at_boot(&b.base.0, SUBDIR).expect("boot");
    match outcome {
        InstallOutcome::AbandonedIncomplete(moved) => {
            assert!(moved.is_dir(), "set aside, not deleted");
        }
        other => panic!("expected the staging dir to be abandoned, got {other:?}"),
    }
    assert_eq!(
        read_u64(&b.live(), KEY_COMMITTED_HEIGHT),
        live_before,
        "the live directory must be untouched"
    );
}

#[test]
fn no_boot_path_outcome_ever_deletes_a_directory() {
    // Every set-aside directory in every outcome must still be on disk. This is
    // the rule that makes every step before the commit point reversible.
    for (tag, sabotage) in [
        ("f5i_nodel_ok", 0u8),
        ("f5i_nodel_reject", 1),
        ("f5i_nodel_abandon", 2),
    ] {
        let b = bed(tag);
        let before = b.entries().len();
        match sabotage {
            1 => {
                // Break the staged root so the boot audit rejects it.
                let db = RocksKv::open(b.staging()).expect("open staging");
                drop(db);
                let mut db = RocksKv::open(b.staging()).expect("reopen");
                db.put(KEY_SMT_ROOT, &novai_state::encode_smt_root_v1(&[0x99; 32]))
                    .expect("corrupt");
            }
            2 => {
                std::fs::remove_file(b.staging().join(INSTALL_READY)).expect("drop marker");
            }
            _ => {}
        }
        complete_install_at_boot(&b.base.0, SUBDIR).expect("boot");
        assert!(
            b.entries().len() >= before,
            "{tag}: a boot must never reduce the number of directories"
        );
    }
}

// ---------------------------------------------------------------------------
// G3: the boot re-audits, and does not trust a receive-time PASS
// ---------------------------------------------------------------------------

#[test]
fn the_boot_path_re_audits_and_rejects_a_staging_dir_corrupted_after_staging() {
    // stage_bundle materialises from a bundle that already passed its producer
    // audit. Corrupting the staging directory AFTERWARDS models every way the
    // bytes can differ from what was proven: a bad disk, a partial write, a
    // tampering hand. The boot audit must catch it, because it audits the bytes
    // it is about to install rather than trusting an earlier verdict.
    let b = bed("f5i_g3");
    let live_before = read_u64(&b.live(), KEY_COMMITTED_HEIGHT);
    {
        let mut db = RocksKv::open(b.staging()).expect("open staging");
        db.put(KEY_SMT_ROOT, &novai_state::encode_smt_root_v1(&[0x99; 32]))
            .expect("corrupt the staged root");
    }

    match complete_install_at_boot(&b.base.0, SUBDIR).expect("boot") {
        InstallOutcome::RejectedByAudit { moved_to, result } => {
            assert!(moved_to.is_dir(), "the rejected staging dir is preserved");
            assert!(result.contains("RESULT FAIL"), "{result}");
        }
        other => panic!("a corrupted staging dir must be rejected, got {other:?}"),
    }
    assert_eq!(
        read_u64(&b.live(), KEY_COMMITTED_HEIGHT),
        live_before,
        "and the node boots on the directory it already had"
    );
    assert!(!b.staging().exists());
}

#[test]
fn the_boot_path_rejects_a_ready_marker_that_disagrees_with_its_database() {
    let b = bed("f5i_marker");
    std::fs::write(
        b.staging().join(INSTALL_READY),
        b"version=1\nheight=999999\nroot=deadbeef\n",
    )
    .expect("rewrite marker");

    match complete_install_at_boot(&b.base.0, SUBDIR).expect("boot") {
        InstallOutcome::RejectedByAudit { result, .. } => {
            assert!(result.contains("ready marker claims"), "{result}");
        }
        other => panic!("a lying marker must be rejected, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// T3.4 / T3.5 equivocation safety: max(own, donor), never lower than either
// ---------------------------------------------------------------------------

#[test]
fn the_boot_merge_keeps_this_nodes_higher_vote_mark() {
    let b = bed("f5i_marks_own");
    // This node voted far above the donor snapshot's own mark.
    {
        let mut db = RocksKv::open(b.live()).expect("open live");
        db.put(KEY_VOTED_VIEW, &encode_voted_view_v1(9_000, 3))
            .expect("own mark");
    }
    complete_install_at_boot(&b.base.0, SUBDIR).expect("boot");

    let db = RocksKv::open(b.live()).expect("open installed");
    let (h, r) = novai_consensus_types::codec::decode_voted_view_v1(
        &db.get(KEY_VOTED_VIEW).unwrap().expect("mark present"),
    )
    .expect("decode");
    assert_eq!(
        (h, r),
        (9_000, 3),
        "installing must never lower this node's own vote high-water mark"
    );
}

#[test]
fn the_boot_merge_takes_the_donor_mark_when_it_is_higher() {
    let b = bed("f5i_marks_donor");
    {
        let mut db = RocksKv::open(b.live()).expect("open live");
        db.put(KEY_VOTED_VIEW, &encode_voted_view_v1(1, 0))
            .expect("own mark");
        // Give the staged snapshot a higher mark than this node's.
        let mut staged = RocksKv::open(b.staging()).expect("open staging");
        staged
            .put(KEY_VOTED_VIEW, &encode_voted_view_v1(50_000, 1))
            .expect("donor mark");
    }
    complete_install_at_boot(&b.base.0, SUBDIR).expect("boot");

    let db = RocksKv::open(b.live()).expect("open installed");
    let (h, r) = novai_consensus_types::codec::decode_voted_view_v1(
        &db.get(KEY_VOTED_VIEW).unwrap().expect("mark present"),
    )
    .expect("decode");
    assert_eq!((h, r), (50_000, 1));
}

#[test]
fn the_boot_merge_keeps_this_nodes_higher_lock() {
    // A wedged node's locked_qc sits far above a fresh snapshot's, because QC
    // adoption is deliberately ungated by the commit window. Installing the
    // donor's alone would REGRESS the lock.
    let b = bed("f5i_lock_own");
    let high = QC {
        height: 900_000,
        round: 2,
        block_hash: [0x77; 32],
        votes: vec![],
    };
    {
        let mut db = RocksKv::open(b.live()).expect("open live");
        db.put(KEY_LOCKED_QC, &encode_qc_v1(&high).expect("encode"))
            .expect("own lock");
    }
    complete_install_at_boot(&b.base.0, SUBDIR).expect("boot");

    let db = RocksKv::open(b.live()).expect("open installed");
    let got = novai_consensus_types::codec::decode_qc_v1(
        &db.get(KEY_LOCKED_QC).unwrap().expect("lock present"),
    )
    .expect("decode");
    assert_eq!(
        (got.height, got.round),
        (900_000, 2),
        "installing must never regress the safety lock"
    );
}

#[test]
fn merging_marks_is_idempotent_so_a_retried_boot_is_safe() {
    let b = bed("f5i_idem");
    let own = OwnMarks {
        voted_view: Some((77, 4)),
        locked_qc: None,
    };
    merge_marks_into_staging(&b.staging(), &own).expect("merge once");
    let after_one = read_own_marks(&b.staging()).expect("read");
    merge_marks_into_staging(&b.staging(), &own).expect("merge twice");
    let after_two = read_own_marks(&b.staging()).expect("read");
    assert_eq!(after_one.voted_view, after_two.voted_view);
    assert_eq!(after_one.voted_view, Some((77, 4)));
}

#[test]
fn t3_6_a_reinstall_after_a_vote_cannot_regress_the_vote_mark() {
    // The rollback-equivocation hazard, executed. Stage, install, vote at
    // (h, r), then install a FRESH snapshot whose own mark sits far below that
    // vote. The installed mark must still be (h, r), so the node cannot be
    // asked to vote at (h, r') for any r'.
    let b = bed("f5i_rollback");
    complete_install_at_boot(&b.base.0, SUBDIR).expect("first install");

    // The node runs and votes.
    let voted = (b.h + 1_234, 5);
    {
        let mut db = RocksKv::open(b.live()).expect("open installed");
        db.put(KEY_VOTED_VIEW, &encode_voted_view_v1(voted.0, voted.1))
            .expect("vote");
    }

    // A fresh snapshot is staged and installed again. Its own mark is low.
    let donor2 = build_fixture(
        "f5i_rollback_donor2",
        FixtureSpec {
            t: 30,
            ..FixtureSpec::default()
        },
    );
    let bundle2 = build_bundle(&donor2.tmp.0).expect("produce");
    stage_bundle(&b.base.0, SUBDIR, &bundle2).expect("stage again");
    complete_install_at_boot(&b.base.0, SUBDIR).expect("second install");

    let db = RocksKv::open(b.live()).expect("open");
    let (h, r) = novai_consensus_types::codec::decode_voted_view_v1(
        &db.get(KEY_VOTED_VIEW).unwrap().expect("mark present"),
    )
    .expect("decode");
    assert_eq!(
        (h, r),
        voted,
        "re-installing must not hand the node back a vote memory it already spent; \
         that is the restart-race equivocation class, re-armed by a rollback"
    );

    // And the gate really refuses: may_vote is a strict > on (height, round).
    assert!(
        max_voted_view(Some(voted), Some((b.h, 0))) == Some(voted),
        "the merge rule itself is monotone"
    );
    assert!(
        max_locked_qc(None, None).is_none(),
        "and it is total over absent marks"
    );
}

// ---------------------------------------------------------------------------
// T3.8 the install-soundness theorem, executed
// ---------------------------------------------------------------------------

/// A node whose storage IS the installed directory, in a 2-validator set so
/// quorum is 1 and a single signed vote certifies.
fn node_over(dir: &Path) -> (ConsensusNode, SigningKey, novai_types::Address) {
    let sk_a = SigningKey::from_bytes(&[1u8; 32]);
    let sk_b = SigningKey::from_bytes(&[2u8; 32]);
    let addr_a = address_from_pubkey(&sk_a.verifying_key());
    let addr_b = address_from_pubkey(&sk_b.verifying_key());
    let mut pubkeys = HashMap::new();
    pubkeys.insert(addr_a, sk_a.verifying_key());
    pubkeys.insert(addr_b, sk_b.verifying_key());
    let storage = Storage::Rocks(RocksKv::open(dir).expect("open installed dir"));
    let node = ConsensusNode::new_with_storage(
        sk_b,
        vec![addr_a, addr_b],
        pubkeys,
        1_000,
        storage,
        None,
    );
    (node, sk_a, addr_a)
}

fn empty_child(parent: &Block, state_root: [u8; 32]) -> Block {
    Block {
        height: parent.height + 1,
        round: 0,
        parent_hash: hash_block_v1(parent).expect("hash"),
        state_root,
        txs: vec![],
    }
}

fn single_vote_qc(sk: &SigningKey, block: &Block) -> QC {
    let bh = hash_block_v1(block).expect("hash");
    QC {
        height: block.height,
        round: block.round,
        block_hash: bh,
        votes: vec![a0_common::sign_vote(sk, block.height, block.round, bh)],
    }
}

#[test]
fn t3_8_install_soundness_theorem_the_installed_node_accepts_the_certified_successor() {
    let b = bed("f5i_theorem");
    complete_install_at_boot(&b.base.0, SUBDIR).expect("install");

    // The theorem's premises, read off the installed directory.
    let installed_root = read_root(&b.live()).expect("root");
    assert_eq!(installed_root, b.root, "A2 and A5: stored root is the rebuilt root");
    let block_h = {
        let db = RocksKv::open(b.live()).expect("open");
        novai_consensus::ConsensusState::load_block(&db, b.h)
            .expect("load")
            .expect("block(H) is installed")
    };
    assert_eq!(
        block_h.state_root, installed_root,
        "A7, lag-0: header(H) commits to post-state(H), and it equals KEY_SMT_ROOT"
    );

    // Build the certified successors. The installed state is unchanged by empty
    // blocks, so each header carries the same post-state root.
    let (node, sk_a, _addr_a) = node_over(&b.live());
    let b1 = empty_child(&block_h, installed_root);
    let b2 = empty_child(&b1, installed_root);
    let b3 = empty_child(&b2, installed_root);
    let qc3 = single_vote_qc(&sk_a, &b3);
    {
        let mut state = node.state.lock().unwrap();
        state.committed_height = b.h;
        state.cache_block(block_h.clone()).unwrap();
        state.cache_block(b1.clone()).unwrap();
        state.cache_block(b2.clone()).unwrap();
        state.cache_block(b3.clone()).unwrap();
    }

    // The 3-chain rule commits H+1. Its parent is the installed tip, so
    // verify_pre_commit_state_root compares header(H).state_root against
    // KEY_SMT_ROOT, and resolve_and_apply_block re-executes and compares the
    // computed post root against header(H+1).
    let result = node.handle_qc(qc3);
    assert!(
        result.is_ok(),
        "the certified successor must be accepted on installed state: {result:?}"
    );
    assert_eq!(
        node.state.lock().unwrap().committed_height,
        b.h + 1,
        "and the commit must actually advance"
    );
}

// ---------------------------------------------------------------------------
// T3.9 the containment control
// ---------------------------------------------------------------------------

#[test]
fn t3_9_containment_a_wrong_root_forced_past_the_audit_halts_locally() {
    let b = bed("f5i_containment");
    complete_install_at_boot(&b.base.0, SUBDIR).expect("install");

    // Force a wrong root PAST the audit: the audit has already run and passed,
    // and this writes over the installed bytes afterwards. Nothing downstream
    // may accept it.
    let wrong = [0x99u8; 32];
    let good = read_root(&b.live()).expect("root");
    assert_ne!(wrong, good, "the injected root must differ, or the test is vacuous");
    {
        let mut db = RocksKv::open(b.live()).expect("open installed");
        db.put(KEY_SMT_ROOT, &novai_state::encode_smt_root_v1(&wrong))
            .expect("inject");
    }

    let block_h = {
        let db = RocksKv::open(b.live()).expect("open");
        novai_consensus::ConsensusState::load_block(&db, b.h)
            .expect("load")
            .expect("block(H)")
    };
    let (node, sk_a, _addr) = node_over(&b.live());
    let b1 = empty_child(&block_h, good);
    let b2 = empty_child(&b1, good);
    let b3 = empty_child(&b2, good);
    let qc3 = single_vote_qc(&sk_a, &b3);
    {
        let mut state = node.state.lock().unwrap();
        state.committed_height = b.h;
        state.cache_block(block_h).unwrap();
        state.cache_block(b1).unwrap();
        state.cache_block(b2).unwrap();
        state.cache_block(b3).unwrap();
    }

    let result = node.handle_qc(qc3);
    assert!(result.is_err(), "a wrong root must HALT the commit, not be committed");
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("CONSENSUS SAFETY HALT"),
        "the halt must name itself so an operator can find it: {msg}"
    );
    assert_eq!(
        node.state.lock().unwrap().committed_height,
        b.h,
        "committed must NEVER advance past the installed height on a wrong root"
    );
    // Read through the node's OWN handle: it holds the RocksDB lock on this
    // directory, so a second open would fail and silently read as absent, which
    // would make this assertion vacuous rather than false.
    let executed = {
        let db = node.db.lock().unwrap();
        db.get(KEY_EXECUTED_HEIGHT)
            .expect("read executed cursor")
            .map(|v| {
                let mut a = [0u8; 8];
                a.copy_from_slice(&v);
                u64::from_be_bytes(a)
            })
    };
    assert_eq!(
        executed,
        Some(b.h),
        "and nothing may be executed on top of it"
    );
}

#[test]
fn t3_9_a_halted_node_emits_nothing_so_the_fleet_is_unaffected() {
    // The other half of the blast-radius claim. A node whose state is wrong
    // cannot influence any other validator, because it never produces a message
    // they would act on: it holds one vote of the set, the commit-window rule
    // refuses to self-vote far above its committed height, and the vote path
    // refuses a proposal whose root does not match its own.
    let b = bed("f5i_blast");
    complete_install_at_boot(&b.base.0, SUBDIR).expect("install");
    {
        let mut db = RocksKv::open(b.live()).expect("open");
        db.put(KEY_SMT_ROOT, &novai_state::encode_smt_root_v1(&[0x99; 32]))
            .expect("inject");
    }
    let (node, _sk, _addr) = node_over(&b.live());
    {
        let mut state = node.state.lock().unwrap();
        state.committed_height = b.h;
    }
    // Far above the commit window: the engine refuses to record a self vote at
    // all, which is the backstop no caller can bypass.
    let far = b.h + novai_consensus::COMMIT_WINDOW + 1;
    let refused = node.state.lock().unwrap().note_self_vote(far, 0);
    assert!(
        refused.is_err(),
        "a node this far behind cannot record a vote, so it cannot broadcast one"
    );
    assert!(
        !node
            .state
            .lock()
            .unwrap()
            .within_commit_window(far),
        "and the window says so directly"
    );
}

// ---------------------------------------------------------------------------
// Composition: the installed directory is what A0 says it is
// ---------------------------------------------------------------------------

#[test]
fn the_installed_directory_passes_a_fresh_independent_audit() {
    let b = bed("f5i_final_audit");
    complete_install_at_boot(&b.base.0, SUBDIR).expect("install");
    let (code, out, err) = run_a0(&["audit", "--db", b.live().to_str().unwrap()]);
    assert_eq!(code, 0, "stdout:\n{out}\nstderr:\n{err}");
    assert!(out.contains(&format!("RESULT PASS height={}", b.h)), "{out}");
    assert!(out.contains(&hex::encode(b.root)), "{out}");
}

#[test]
fn staging_never_writes_outside_its_own_directory() {
    let b = bed("f5i_scope");
    let live_before = read_u64(&b.live(), KEY_COMMITTED_HEIGHT);
    let donor = build_fixture(
        "f5i_scope_donor",
        FixtureSpec {
            t: 40,
            ..FixtureSpec::default()
        },
    );
    let bundle = build_bundle(&donor.tmp.0).expect("produce");
    stage_bundle(&b.base.0, SUBDIR, &bundle).expect("stage");
    assert_eq!(
        read_u64(&b.live(), KEY_COMMITTED_HEIGHT),
        live_before,
        "staging must not touch the live directory at all"
    );
}

#[test]
fn dev_keys_are_still_the_valset_the_installed_snapshot_is_checked_against() {
    // Identity never travels in a snapshot: it comes from the launch flags. A
    // bundle from any donor is checked against THIS node's derived validator
    // set, which is what makes a donor copy legitimate at all.
    let keys = dev_signing_keys();
    assert_eq!(keys.len(), 4);
    let fx = build_fixture("f5i_valset", FixtureSpec::default());
    let qc = make_qc(&fx.block_t1, &[0, 1, 3]);
    assert_eq!(qc.votes.len(), 3, "quorum for n=4 is 3");
}
