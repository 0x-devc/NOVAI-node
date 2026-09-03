//! SMT GC Phase 1, gates R2 to R8: the surgical local rebuild-and-swap.
//!
//! WHAT THIS TOOL IS FOR. Every validator's disk is 99.8 percent unreachable
//! SMT nodes: 26.5 GB holding a live tree of about 3 MB. The node store is
//! content addressed, so a changed subtree writes a NEW row and never
//! overwrites its predecessor, and nothing deletes. Phase 0 proved that bloat
//! is the mechanism behind two production strandings: the forced compaction at
//! every 5,000th height pulls in the accumulated write volume since the last
//! boundary, and under thin headroom it takes ENOSPC mid-compaction on the
//! synchronous commit path and never resumes. Reclaiming the dead nodes removes
//! both the compaction bulk and the disk-pressure driver.
//!
//! WHY THESE GATES AND NOT OTHERS. The tool's correctness does not rest on the
//! tool being careful. It rests on a derivation: the reachable set is exactly
//! what a from-scratch rebuild over the SMT-committed leaves produces, and the
//! rebuilt root is checked against a header a quorum signed before anything is
//! renamed. So the gates here pin the two things that derivation cannot cover
//! by itself. R2 pins that the rebuild really does reproduce the reachable set,
//! walked from the root rather than asserted. R3 pins that the copier is TOTAL,
//! because the one genuinely new piece of code is the part that carries the
//! 70 MB of non-SMT rows across, and a silently dropped family is the failure
//! this design is most exposed to. The rest (R4, R5, R7, R8) pin the refusals.
//!
//! THE DEFAULT IS A DRY RUN. The count-only mode is what the CLI does with no
//! flag, and `dry_run_is_the_default_and_moves_nothing` is the gate on that.
//! An operator who mistypes gets a census, not a rename.

#[path = "a0_common/mod.rs"]
mod a0_common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use a0_common::{build_fixture_at, run_a0, Evidence, FixtureFacts, FixtureSpec};
use novai_node::snapshot::reclaim::{self, ReclaimError};
use novai_smt::{Node, NodeChild};
use novai_state::{
    block_key, decode_smt_root_v1, encode_account_v1, qc_key, AccountStateV1, Kv, RocksKv,
    KEY_COMMITTED_HEIGHT, KEY_EXECUTED_HEIGHT, KEY_LOCKED_QC, KEY_PREFIX_SMT_NODE, KEY_SMT_ROOT,
    KEY_VOTED_VIEW,
};

// ===========================================================================
// Scratch layout
//
// The tool renames SIBLINGS of the data directory, so every fixture needs a
// parent it owns. Layout mirrors the node's own:
//     <root>/data/validator-0        the live data directory
//     <root>/data/snapshot-work      the F5 producer's checkpoint dir (R8)
// and <root> is removed on drop, so no rename can strew anything into /tmp.
// ===========================================================================

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "novai_smtgc_{}_{}_{}",
            std::process::id(),
            tag,
            n
        ));
        std::fs::create_dir_all(path.join("data")).expect("create scratch");
        Self(path)
    }

    fn base(&self) -> PathBuf {
        self.0.join("data")
    }

    fn live(&self) -> PathBuf {
        self.base().join("validator-0")
    }

    fn live_arg(&self) -> String {
        self.live().to_str().expect("utf8").to_string()
    }

    /// Sibling directories the tool may have created, by suffix.
    fn siblings_with(&self, needle: &str) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(self.base()).expect("read base") {
            let p = entry.expect("dir entry").path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.contains(needle) {
                out.push(p);
            }
        }
        out.sort();
        out
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(tag: &str, spec: FixtureSpec) -> (Scratch, FixtureFacts) {
    let sc = Scratch::new(tag);
    let facts = build_fixture_at(&sc.live(), spec);
    (sc, facts)
}

// ===========================================================================
// Independent instruments
//
// These read the RESULT rather than trusting the tool's own report. The
// reachability walk in particular is a second, independent count of the live
// node set: the tool derives its figure from the leaves it rebuilt, this walk
// derives it from the tree it produced. Two instruments that must agree.
// ===========================================================================

/// Every (key, value) in both column families, as an ordered set.
fn all_rows(dir: &Path) -> Vec<(Vec<u8>, Vec<u8>)> {
    let db = RocksKv::open(dir).expect("open for census");
    let mut rows = Vec::new();
    let mut take = |k: &[u8], v: &[u8]| rows.push((k.to_vec(), v.to_vec()));
    db.for_each_prefix(b"", &mut take).expect("scan default cf");
    db.for_each_prefix(b"nnpx/", &mut take).expect("scan nnpx cf");
    rows.sort();
    rows
}

fn non_smt_rows(dir: &Path) -> Vec<(Vec<u8>, Vec<u8>)> {
    all_rows(dir)
        .into_iter()
        .filter(|(k, _)| !k.starts_with(KEY_PREFIX_SMT_NODE))
        .collect()
}

fn smt_node_keys(dir: &Path) -> BTreeSet<Vec<u8>> {
    let db = RocksKv::open(dir).expect("open for smt census");
    let mut keys = BTreeSet::new();
    db.for_each_prefix(KEY_PREFIX_SMT_NODE, |k: &[u8], _v: &[u8]| {
        keys.insert(k.to_vec());
    })
    .expect("scan smt nodes");
    keys
}

fn stored_root(dir: &Path) -> [u8; 32] {
    let db = RocksKv::open(dir).expect("open for root");
    root_of(&db)
}

/// Read the root through an ALREADY OPEN handle.
///
/// RocksDB takes a directory lock, and a second open from the same process
/// fails, so a helper that opens the database cannot be called from inside
/// another helper that already has it open.
fn root_of(db: &RocksKv) -> [u8; 32] {
    let bytes = db.get(KEY_SMT_ROOT).expect("get root").expect("root present");
    decode_smt_root_v1(&bytes).expect("decode root")
}

fn committed_height(dir: &Path) -> u64 {
    let db = RocksKv::open(dir).expect("open for height");
    let b = db
        .get(KEY_COMMITTED_HEIGHT)
        .expect("get committed")
        .expect("committed present");
    let mut a = [0u8; 8];
    a.copy_from_slice(&b);
    u64::from_be_bytes(a)
}

/// Walk the tree from `smt/root` and return every node key it reaches.
///
/// This is the reachability set computed from the TREE, independent of the
/// leaf-driven figure the tool reports. It resolves every internal child hash
/// it meets and panics naming the hash if one is absent, so an incomplete
/// rebuild fails here loudly rather than being reported as a smaller number. A
/// walk that silently skipped a dangling child would report a live count that
/// agrees with a broken tree, which is the one way this instrument could be
/// worse than useless.
///
/// HEIGHT IS TRACKED, and it has to be. The tree is a fixed 256 levels: the
/// root sits at height 256 and a node at height H has children at H-1, so the
/// children of a height-1 node are LEAF hashes at height 0, and leaves are
/// never stored as rows (`crates/smt/src/node.rs:11`). Descending into them
/// looking for a node row finds nothing, which is correct behaviour being
/// misread as corruption.
fn reachable_node_keys(dir: &Path) -> BTreeSet<Vec<u8>> {
    let db = RocksKv::open(dir).expect("open for walk");
    let root = root_of(&db);
    let mut seen: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut stack: Vec<([u8; 32], u16)> = vec![(root, 256)];

    while let Some((h, height)) = stack.pop() {
        let mut key = KEY_PREFIX_SMT_NODE.to_vec();
        key.extend_from_slice(&h);
        if !seen.insert(key.clone()) {
            continue;
        }
        let bytes = db.get(&key).expect("get node").unwrap_or_else(|| {
            panic!(
                "dangling child hash {} at height {height} has no node row",
                hex::encode(h)
            )
        });
        let node = Node::decode(&bytes).expect("decode node");
        for child in [&node.left, &node.right] {
            if let NodeChild::Hash(ch) = child {
                // Height 0 is a leaf hash, which has no row by design.
                if height > 1 {
                    stack.push((*ch, height - 1));
                }
            }
        }
    }

    // The root of an EMPTY tree is an empty hash with no row behind it, and the
    // walk above would have panicked on it. Every fixture here has leaves, so
    // reaching this point with an empty set would itself be the bug.
    assert!(!seen.is_empty(), "the walk reached no nodes at all");
    seen
}

fn acct_pair(tag: u8, balance: u128, nonce: u64) -> (Vec<u8>, Vec<u8>) {
    (
        novai_state::account_key(&[tag; 32]),
        encode_account_v1(&AccountStateV1 { balance, nonce }).to_vec(),
    )
}

/// A fixture that has ORPHANED nodes: the same account key written repeatedly,
/// so each rewrite abandons a full 256-node root path. Without this the source
/// has no dead nodes and every reclaim assertion would pass on a tree that
/// needed no reclaiming.
fn churned_spec() -> FixtureSpec {
    let mut pre = a0_common::default_pre_state();
    for round in 1..=6u128 {
        pre.push(acct_pair(0xE5, round * 1_000, round as u64));
    }
    FixtureSpec {
        pre_state: pre,
        ..FixtureSpec::default()
    }
}

// ===========================================================================
// The dry run: the default, and M1's instrument
// ===========================================================================

#[test]
fn dry_run_is_the_default_and_moves_nothing() {
    let (sc, facts) = fixture("dryrun_default", churned_spec());
    let before = all_rows(&sc.live());

    let (code, stdout, stderr) = run_a0(&["reclaim", "--db", &sc.live_arg()]);

    assert_eq!(
        code, 0,
        "a plain reclaim must be a successful census; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("PLAN "),
        "the dry run must print a PLAN line; stdout:\n{stdout}"
    );
    // Nothing renamed, nothing staged, nothing changed. This is the assertion
    // that makes the default safe to mistype.
    assert!(
        sc.siblings_with(".reclaim-staging").is_empty(),
        "the dry run must not create a staging directory"
    );
    assert!(
        sc.siblings_with(".preinstall").is_empty(),
        "the dry run must not set the live directory aside"
    );
    assert_eq!(
        all_rows(&sc.live()),
        before,
        "the dry run must leave every row byte-identical"
    );
    assert_eq!(committed_height(&sc.live()), facts.t);
}

#[test]
fn the_dry_run_counts_the_live_set_and_the_dead_set_separately() {
    // M1 asks for the exact live SMT node count, which nothing in this repo
    // measured before. It falls out of the rebuild by construction: the rebuild
    // over the leaves produces exactly the reachable set. This pins that the
    // reported figure is that set and not the family total, which is the
    // mistake that would look entirely plausible on a report line.
    let (sc, _facts) = fixture("dryrun_counts", churned_spec());

    let counts = reclaim::plan(&sc.live()).expect("plan a healthy directory");
    let on_disk = smt_node_keys(&sc.live());
    let reachable = reachable_node_keys(&sc.live());

    assert_eq!(
        counts.smt_node_rows,
        on_disk.len() as u64,
        "the family total must equal the rows actually on disk"
    );
    // TWO INSTRUMENTS, one answer. The tool derives its live figure by
    // rebuilding from the leaves and walking the result; this walks the
    // SOURCE tree, which is the same live set buried in millions of dead
    // rows. They must agree, and if they ever stop agreeing the reclaim is
    // not reproducing the tree it claims to.
    assert_eq!(
        counts.live_node_rows,
        reachable.len() as u64,
        "the live figure must equal the set an independent walk reaches"
    );
    // What the rebuilt directory will hold is slightly MORE than the live set,
    // because the rebuild replays leaves one at a time and strands the shared
    // top of each walk as it goes. Small, bounded by the leaf count, and
    // reported rather than folded into the live figure.
    assert!(
        counts.staged_node_rows >= counts.live_node_rows,
        "staged {} cannot be below live {}",
        counts.staged_node_rows,
        counts.live_node_rows
    );
    assert!(
        counts.staged_node_rows < counts.smt_node_rows,
        "and must still be far below what the source holds: staged {} source {}",
        counts.staged_node_rows,
        counts.smt_node_rows
    );
    assert!(
        counts.reclaimed_rows() > 0,
        "the run must actually free rows: source {} staged {}",
        counts.smt_node_rows,
        counts.staged_node_rows
    );
    // THE anti-relabelling assertion. A plan that reports the family total under
    // the live label reclaims nothing and fails right here.
    assert!(
        counts.dead_node_rows() > 0,
        "a churned fixture must carry orphans: total {} live {}",
        counts.smt_node_rows,
        counts.live_node_rows
    );
    assert!(
        counts.live_node_rows < counts.smt_node_rows,
        "the live set must be a proper subset of what is on disk"
    );
    assert_eq!(
        counts.leaf_count as usize,
        // Six rewrites of one key collapse to one leaf, so the leaf count is the
        // DISTINCT authenticated key count, not the write count.
        {
            let db = RocksKv::open(&sc.live()).expect("open");
            novai_node::snapshot::produce::extract_leaf_set(&db)
                .expect("leaves")
                .len()
        },
        "the leaf count must be the authenticated key set"
    );
}

// ===========================================================================
// R2: the rebuild reproduces the reachable set
// ===========================================================================

#[test]
fn r2_rebuild_reproduces_the_reachable_set() {
    // Every SMT-committed family represented, so a family missing from the
    // classification table drops a leaf, changes the root, and fails here. This
    // is the same exemplar set gate_a0_families_red.rs uses, for the same
    // reason: the classifier is the thing that decides what the rebuild sees.
    let mut pre = a0_common::default_pre_state();
    pre.extend(family_exemplars());
    for round in 1..=4u128 {
        pre.push(acct_pair(0xE5, round * 1_000, round as u64));
    }
    let (sc, facts) = fixture(
        "r2_reachable",
        FixtureSpec {
            pre_state: pre,
            ..FixtureSpec::default()
        },
    );

    let source_root = stored_root(&sc.live());
    let live_before = reachable_node_keys(&sc.live());
    let on_disk_before = smt_node_keys(&sc.live());
    let outcome = reclaim::reclaim(&sc.live()).expect("reclaim a healthy directory");

    assert_eq!(
        outcome.root, source_root,
        "the rebuilt root must be byte-identical to the source root"
    );
    assert_eq!(outcome.root, facts.r1, "and to the fixture's own root");
    assert_eq!(outcome.height, facts.t);
    assert_eq!(
        stored_root(&sc.live()),
        source_root,
        "the installed directory must store that same root"
    );

    // Every hash reachable from the root resolves in the new directory. The walk
    // panics on a dangling child, so this is a positive proof of completeness,
    // not an absence of complaint.
    let reachable = reachable_node_keys(&sc.live());
    let on_disk = smt_node_keys(&sc.live());

    // THE claim: the live tree survives the swap byte for byte. Not a count
    // that happens to match, the same set of node keys. A reclaim that
    // reproduced the right root while losing a node would pass a count check
    // and fail here, and it would fail on the fleet as a missing node on the
    // first update walk that touched it.
    assert_eq!(
        reachable, live_before,
        "the reachable set must be preserved exactly"
    );
    assert!(
        reachable.is_subset(&on_disk),
        "every reachable node must be present on disk"
    );
    // What is left over is the rebuild's own transient orphans, not source
    // churn. It is reported, so the operator's expected disk figure comes from
    // the tool rather than from an assumption that the result is minimal.
    assert_eq!(
        on_disk.len() as u64,
        outcome.counts.staged_node_rows,
        "the directory must hold exactly what the tool said it would"
    );
    // A strict reduction, and deliberately no claim about its size. This
    // fixture is 26 distinct leaves against 4 rewrites, so most of its rows are
    // genuinely live and the reclaim is about a tenth. The dramatic ratios come
    // from churn, and the churned fixtures are where that claim is made.
    assert!(
        on_disk.len() < on_disk_before.len(),
        "the reclaim must remove rows: {} against {}",
        on_disk.len(),
        on_disk_before.len()
    );
}

/// One exemplar per SMT-committed execution family, matching
/// `gate_a0_families_red.rs`. Kept in step with that file deliberately: both
/// gates exist to catch a family the classification table does not carry.
fn family_exemplars() -> Vec<(Vec<u8>, Vec<u8>)> {
    use novai_execution::{
        KEY_AI_TREASURY, KEY_MARKETPLACE_TREASURY, KEY_PREFIX_AI_CHANNELS_BY_PARTY_A,
        KEY_PREFIX_AI_CHANNELS_BY_PARTY_B, KEY_PREFIX_AI_ENTITY_UPGRADES_BY_ENTITY,
        KEY_PREFIX_AI_ENTITY_UPGRADES_SUMMARY, KEY_PREFIX_AI_ORACLE_ANCHORS_BY_ENTITY,
        KEY_PREFIX_AI_ORACLE_ANCHORS_BY_HASH, KEY_PREFIX_AI_ORACLE_ANCHORS_BY_TAG,
        KEY_PREFIX_AI_ORACLE_ANCHORS_SUMMARY, KEY_PREFIX_AI_PAYMENTS_BY_HASH,
        KEY_PREFIX_AI_PAYMENTS_BY_PAYEE, KEY_PREFIX_AI_PAYMENTS_BY_PAYER,
        KEY_PREFIX_AI_PAYMENT_CONDITIONS_BY_HASH, KEY_PREFIX_AI_PAYMENT_SPLITS_BY_HASH,
        KEY_PREFIX_AI_SERVICE_DESCRIPTORS_BY_CATEGORY, KEY_PREFIX_AI_SLAS_ACTIVE_BETWEEN,
        KEY_PREFIX_AI_SLAS_BY_BUYER, KEY_PREFIX_AI_SLAS_BY_SELLER,
        KEY_PREFIX_AI_VK_REGISTRY_BY_ID, KEY_SLASH_TREASURY,
    };
    let id = [0x66; 32];
    let k = |p: &[u8]| {
        let mut v = p.to_vec();
        v.extend_from_slice(&id);
        v
    };
    vec![
        (k(KEY_PREFIX_AI_ORACLE_ANCHORS_BY_HASH), b"rec".to_vec()),
        (k(KEY_PREFIX_AI_ORACLE_ANCHORS_BY_ENTITY), vec![]),
        (k(KEY_PREFIX_AI_ORACLE_ANCHORS_BY_TAG), vec![]),
        (k(KEY_PREFIX_AI_ORACLE_ANCHORS_SUMMARY), b"sum".to_vec()),
        (k(KEY_PREFIX_AI_PAYMENTS_BY_HASH), b"pay".to_vec()),
        (k(KEY_PREFIX_AI_PAYMENTS_BY_PAYER), vec![]),
        (k(KEY_PREFIX_AI_PAYMENTS_BY_PAYEE), vec![]),
        (k(KEY_PREFIX_AI_PAYMENT_SPLITS_BY_HASH), b"spl".to_vec()),
        (k(KEY_PREFIX_AI_PAYMENT_CONDITIONS_BY_HASH), b"cnd".to_vec()),
        (k(KEY_PREFIX_AI_SLAS_ACTIVE_BETWEEN), b"sla".to_vec()),
        (k(KEY_PREFIX_AI_SLAS_BY_BUYER), vec![]),
        (k(KEY_PREFIX_AI_SLAS_BY_SELLER), vec![]),
        (k(KEY_PREFIX_AI_CHANNELS_BY_PARTY_A), vec![]),
        (k(KEY_PREFIX_AI_CHANNELS_BY_PARTY_B), vec![]),
        (k(KEY_PREFIX_AI_VK_REGISTRY_BY_ID), b"vk".to_vec()),
        (k(KEY_PREFIX_AI_ENTITY_UPGRADES_SUMMARY), b"up".to_vec()),
        (k(KEY_PREFIX_AI_ENTITY_UPGRADES_BY_ENTITY), vec![]),
        (k(KEY_PREFIX_AI_SERVICE_DESCRIPTORS_BY_CATEGORY), vec![]),
        (KEY_AI_TREASURY.to_vec(), 7u128.to_be_bytes().to_vec()),
        (KEY_MARKETPLACE_TREASURY.to_vec(), 8u128.to_be_bytes().to_vec()),
        (KEY_SLASH_TREASURY.to_vec(), 9u128.to_be_bytes().to_vec()),
    ]
}

// ===========================================================================
// R3: the copier is total
//
// Three mutations must turn this red independently: dropping a block row, a QC
// row, or a flat account row from the copier. The exhaustive test is the real
// guard; the three named ones exist so a failure says WHICH family went
// missing instead of just reporting a set difference of one.
// ===========================================================================

#[test]
fn r3_reclaim_preserves_every_non_smt_row() {
    let (sc, _facts) = fixture("r3_all", churned_spec());
    let before = non_smt_rows(&sc.live());
    assert!(!before.is_empty(), "the fixture must have rows to preserve");

    reclaim::reclaim(&sc.live()).expect("reclaim");

    let after = non_smt_rows(&sc.live());
    // `smt/root` is the one row the reclaim legitimately rewrites, and it must
    // rewrite it to the SAME value; it is compared here rather than excluded.
    assert_eq!(
        after, before,
        "every row outside smt/node/ must survive byte-identically"
    );
}

#[test]
fn r3_reclaim_preserves_the_block_rows() {
    let (sc, facts) = fixture("r3_blocks", churned_spec());
    let want_t = row(&sc.live(), &block_key(facts.t));
    let want_t1 = row(&sc.live(), &block_key(facts.t + 1));

    reclaim::reclaim(&sc.live()).expect("reclaim");

    assert_eq!(
        row(&sc.live(), &block_key(facts.t)),
        want_t,
        "block row at the audited height must survive; losing it strands the \
         node on its next boot replay"
    );
    assert_eq!(row(&sc.live(), &block_key(facts.t + 1)), want_t1);
}

#[test]
fn r3_reclaim_preserves_the_qc_rows() {
    let (sc, facts) = fixture("r3_qcs", churned_spec());
    let want = row(&sc.live(), &qc_key(facts.t + 1));
    assert!(want.is_some(), "the fixture must carry a certifying QC row");

    reclaim::reclaim(&sc.live()).expect("reclaim");

    assert_eq!(
        row(&sc.live(), &qc_key(facts.t + 1)),
        want,
        "the certifying QC row must survive; it is the trust anchor the audit \
         reads on the next boot"
    );
}

#[test]
fn r3_reclaim_preserves_the_flat_account_rows() {
    // HONEST SCOPE, because I checked and the obvious reading is wrong. This is
    // an end-state assertion, NOT a gate on the copier, and mutating the copier
    // to skip `accounts/` does not turn it red. The reason is the step order:
    // the rebuild runs first and drives `append_smt_ops_for_state_ops`, which
    // writes the flat leaf row alongside the tree, so every SMT-committed row
    // is written TWICE and the copy of it is pure redundancy. What actually
    // guards a lost leaf is the root: drop one from the rebuild and A5 fails.
    //
    // The rows where the copier is the only writer, and therefore the rows
    // where its totality is load bearing, are the ones that are not leaves:
    // blocks, QCs, the cursors, and the anti-equivocation marks. Those have
    // their own tests above and all four are mutation-proven.
    let (sc, _facts) = fixture("r3_accounts", churned_spec());
    let key = novai_state::account_key(&[0xA1; 32]);
    let want = row(&sc.live(), &key);
    assert!(want.is_some(), "the fixture must carry the account");

    reclaim::reclaim(&sc.live()).expect("reclaim");

    assert_eq!(
        row(&sc.live(), &key),
        want,
        "a flat authenticated row must survive the swap, whichever step wrote it"
    );
}

#[test]
fn r3_reclaim_preserves_the_cursors() {
    // Copier-only rows: nothing else writes them into the staged directory, and
    // losing them makes the result unbootable. Unlike the marks, the audit DOES
    // see this one, so it is guarded twice.
    let (sc, facts) = fixture("r3_cursors", churned_spec());

    reclaim::reclaim(&sc.live()).expect("reclaim");

    assert_eq!(
        row(&sc.live(), KEY_COMMITTED_HEIGHT),
        Some(facts.t.to_be_bytes().to_vec()),
        "the committed cursor must survive"
    );
    assert_eq!(
        row(&sc.live(), KEY_EXECUTED_HEIGHT),
        Some(facts.t.to_be_bytes().to_vec()),
        "and so must the executed cursor; equal cursors are what keep the boot \
         replay out of its fatal missing-block path"
    );
}

#[test]
fn r3_reclaim_preserves_the_anti_equivocation_marks() {
    // THE row the audit cannot see, and therefore the one place where the
    // copier's totality is the only thing standing between a reclaim and a
    // safety bug.
    //
    // Every other R3 mutation is caught twice: the assertion fires, and so does
    // the staged audit, because dropping blocks or QCs destroys the
    // certification evidence A6 and A8 read. The marks are different. A3
    // classifies them as Operational and no other check reads them at all, so a
    // copier that dropped KEY_VOTED_VIEW would produce a directory that audits
    // PASS at the right height and the right root, install cleanly, and boot a
    // node that has forgotten what it already voted for. That is an
    // equivocation vector, and it is exactly the hazard install.rs merges marks
    // to avoid. Nothing but this assertion would catch it.
    let (sc, facts) = fixture("r3_marks", churned_spec());
    let mark = (facts.t, 3u64);
    let lock = a0_common::make_qc(&facts.block_t1, &[0, 1, 3]);
    {
        let mut db = RocksKv::open(&sc.live()).expect("reopen");
        db.put(
            KEY_VOTED_VIEW,
            &novai_consensus_types::codec::encode_voted_view_v1(mark.0, mark.1),
        )
        .expect("plant the durable vote mark");
        db.put(
            KEY_LOCKED_QC,
            &novai_consensus_types::codec::encode_qc_v1(&lock).expect("encode lock"),
        )
        .expect("plant the lock");
    }
    let want_mark = row(&sc.live(), KEY_VOTED_VIEW);
    let want_lock = row(&sc.live(), KEY_LOCKED_QC);
    assert!(want_mark.is_some() && want_lock.is_some());

    reclaim::reclaim(&sc.live()).expect("reclaim");

    assert_eq!(
        row(&sc.live(), KEY_VOTED_VIEW),
        want_mark,
        "the durable vote high-water mark must survive; a node that loses it \
         can vote again at a view it already voted at"
    );
    assert_eq!(
        row(&sc.live(), KEY_LOCKED_QC),
        want_lock,
        "and so must the lock: a regressed lock plus a same-height higher-round \
         proposal is a case the vote mark alone does not cover"
    );
}

fn row(dir: &Path, key: &[u8]) -> Option<Vec<u8>> {
    let db = RocksKv::open(dir).expect("open for row read");
    db.get(key).expect("get row")
}

// ===========================================================================
// R4: refuse a torn directory
// ===========================================================================

#[test]
fn r4_reclaim_refuses_a_torn_directory() {
    let (sc, facts) = fixture("r4_torn", churned_spec());
    {
        let mut db = RocksKv::open(&sc.live()).expect("reopen");
        // committed != executed: the crash-window shape. A1's own condition, and
        // the tool must not inherit that check from the audit at the far end,
        // because by then it would already have read the leaf set from a state
        // that is half a block old.
        db.put(KEY_EXECUTED_HEIGHT, &(facts.t - 1).to_be_bytes())
            .expect("tear the cursors");
    }
    let before = all_rows(&sc.live());

    match reclaim::reclaim(&sc.live()) {
        Err(ReclaimError::Torn {
            committed,
            executed,
        }) => {
            assert_eq!(committed, Some(facts.t));
            assert_eq!(executed, Some(facts.t - 1));
        }
        other => panic!("a torn directory must be refused, got {other:?}"),
    }

    assert_eq!(
        all_rows(&sc.live()),
        before,
        "a refusal must leave the directory untouched"
    );
    assert!(sc.siblings_with(".reclaim-staging").is_empty());
    assert!(sc.siblings_with(".preinstall").is_empty());
}

#[test]
fn r4_the_dry_run_refuses_a_torn_directory_too() {
    // The census reads the same state the mutating path would, so it must apply
    // the same precondition. A dry run that reported happily on a torn
    // directory would be an operator's evidence that the real run is safe.
    let (sc, facts) = fixture("r4_torn_dry", churned_spec());
    {
        let mut db = RocksKv::open(&sc.live()).expect("reopen");
        db.put(KEY_EXECUTED_HEIGHT, &(facts.t - 1).to_be_bytes())
            .expect("tear");
    }
    assert!(matches!(
        reclaim::plan(&sc.live()),
        Err(ReclaimError::Torn { .. })
    ));

    let (code, stdout, _stderr) = run_a0(&["reclaim", "--db", &sc.live_arg()]);
    assert_eq!(code, 1, "a refused census must exit non-zero; stdout:\n{stdout}");
}

// ===========================================================================
// R5: a failing audit never renames
// ===========================================================================

#[test]
fn r5_a_failing_audit_never_renames() {
    // The staged directory is made to fail A7 by corrupting the header the
    // audit checks the rebuilt root against. Preflight still passes, because
    // the cursors are intact, so this exercises the AUDIT gate specifically
    // rather than an earlier refusal standing in for it.
    let (sc, facts) = fixture("r5_audit_fail", churned_spec());
    {
        let mut db = RocksKv::open(&sc.live()).expect("reopen");
        let mut block = facts.block_t.clone();
        block.state_root = [0x77; 32];
        db.put(
            &block_key(facts.t),
            &novai_consensus_types::codec::encode_block_v1(&block).expect("encode"),
        )
        .expect("corrupt the header");
    }
    let before = all_rows(&sc.live());

    match reclaim::reclaim(&sc.live()) {
        Err(ReclaimError::AuditFailed { moved_to, result }) => {
            assert!(
                moved_to.contains(".staging-rejected"),
                "a rejected staging dir must be set aside, not deleted: {moved_to}"
            );
            assert!(result.contains("FAIL"), "result line: {result}");
        }
        other => panic!("a failing staged audit must refuse, got {other:?}"),
    }

    assert_eq!(
        all_rows(&sc.live()),
        before,
        "THE point of the audit-then-one-rename discipline: the live directory \
         is untouched when the audit fails"
    );
    assert!(
        sc.siblings_with(".preinstall").is_empty(),
        "nothing may be set aside when the rename never happened"
    );
    assert_eq!(
        sc.siblings_with(".staging-rejected").len(),
        1,
        "the rejected directory must be preserved for diagnosis"
    );
    assert!(
        sc.siblings_with(".reclaim-staging").is_empty(),
        "the staging path itself must be freed by the rename aside"
    );
}

// ===========================================================================
// R7: idempotence
// ===========================================================================

#[test]
fn r7_reclaim_is_idempotent() {
    let (sc, _facts) = fixture("r7_idempotent", churned_spec());
    let source_root = stored_root(&sc.live());

    let first = reclaim::reclaim(&sc.live()).expect("first reclaim");
    assert!(
        first.counts.reclaimed_rows() > 0,
        "the first run must have had something to reclaim"
    );
    let rows_after_first = all_rows(&sc.live());

    // A census on the reclaimed directory must find nothing LEFT to free. The
    // claim is `reclaimed_rows == 0`, not `dead_node_rows == 0`: the directory
    // still carries the first rebuild's own transient orphans, and a second
    // rebuild reproduces exactly the same ones, so there is nothing a second
    // run could remove. That is what idempotence means here, and stating it as
    // "no dead rows" would be a claim the tool does not make.
    let census = reclaim::plan(&sc.live()).expect("plan the reclaimed dir");
    assert_eq!(
        census.reclaimed_rows(),
        0,
        "a second run must free nothing: source {} staged {}",
        census.smt_node_rows,
        census.staged_node_rows
    );
    assert_eq!(
        census.live_node_rows, first.counts.live_node_rows,
        "and must see the same live set"
    );

    let second = reclaim::reclaim(&sc.live()).expect("second reclaim");
    assert_eq!(second.root, source_root, "the root cannot move");
    assert_eq!(second.root, first.root);
    assert_eq!(
        all_rows(&sc.live()),
        rows_after_first,
        "a second run over an already reclaimed directory changes nothing"
    );
    assert_eq!(
        sc.siblings_with(".preinstall").len(),
        2,
        "each run preserves its predecessor; nothing is ever deleted"
    );
}

// ===========================================================================
// R8: refuse while a checkpoint is outstanding
// ===========================================================================

#[test]
fn r8_reclaim_refuses_while_a_checkpoint_is_outstanding() {
    // The hazard is not concurrency, it is HARD LINKS. A checkpoint directory
    // left by the F5 producer pins the old SST files, so the 26.5 GB the
    // operator came to reclaim does not actually free after the rename and the
    // run looks successful while achieving nothing. The tool refuses on the
    // observable trace of that pin rather than on a runtime flag it has no way
    // to read from an offline process.
    let (sc, _facts) = fixture("r8_checkpoint", churned_spec());
    let work = sc.base().join("snapshot-work");
    std::fs::create_dir_all(work.join("checkpoint-7-0")).expect("plant a checkpoint");
    std::fs::write(work.join("checkpoint-7-0").join("CURRENT"), b"x").expect("plant");
    let before = all_rows(&sc.live());

    match reclaim::reclaim(&sc.live()) {
        Err(ReclaimError::CheckpointOutstanding { dir, entries }) => {
            assert!(dir.contains("snapshot-work"), "dir: {dir}");
            assert_eq!(entries, 1);
        }
        other => panic!("an outstanding checkpoint must be refused, got {other:?}"),
    }
    assert_eq!(all_rows(&sc.live()), before);
    assert!(sc.siblings_with(".preinstall").is_empty());

    // And it must proceed once the pin is gone, so the guard is a real
    // precondition rather than a permanent refusal.
    std::fs::remove_dir_all(&work).expect("drop the checkpoint");
    reclaim::reclaim(&sc.live()).expect("reclaim once the pin is released");
}

// ===========================================================================
// The staged tree is verified ON DISK, because the audit cannot see it
// ===========================================================================

#[test]
fn a0_is_blind_to_a_hole_in_the_node_store() {
    // THE correction to the plan. Its 3.3 says a deleted live node "presents as
    // MissingNode on the next update walk" and is "caught before the rename by
    // A5". The first half is right. The second half is not: A4 rebuilds the root
    // from the LEAVES into a fresh store and A5 compares that to the stored
    // root, so the audit never reads the directory's own smt/node/ rows at all.
    // A directory with a punctured node store audits PASS at the right height
    // and the right root, installs cleanly, and halts a validator later.
    //
    // This test states that blindness as a fact rather than leaving it implied,
    // because the whole reason `verify_staged_tree` exists is that step 4 of the
    // plan cannot do the job step 4 was assumed to do.
    let (sc, facts) = fixture("a0_blind", churned_spec());
    let source_root = stored_root(&sc.live());

    let reachable = reachable_node_keys(&sc.live());
    let victim = reachable
        .iter()
        .nth(reachable.len() / 2)
        .expect("a reachable node to remove")
        .clone();
    {
        let mut db = RocksKv::open(&sc.live()).expect("reopen");
        db.delete(&victim).expect("delete a live node");
    }

    let (code, stdout, stderr) = run_a0(&["audit", "--db", &sc.live_arg()]);
    assert_eq!(
        code, 0,
        "A0 rebuilds from the leaves and never reads the node store, so it \
         cannot see a hole. If this ever starts failing, A0 grew a real check \
         and this test should be revisited rather than patched. \
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(&format!("RESULT PASS height={}", facts.t)),
        "and it passes at the right height, which is what makes it dangerous; \
         stdout:\n{stdout}"
    );
    assert_eq!(
        a0_common::parse_result_root(&stdout),
        hex::encode(source_root),
        "and at the right root"
    );
}

#[test]
fn a_reclaim_heals_a_punctured_node_store() {
    // The corollary, and it is a genuinely useful property rather than a
    // curiosity. The rebuild takes nothing from the source's node store, so a
    // source with a hole in its tree produces a staged tree with no hole. The
    // reclaim is therefore also the repair for the damage class it is most
    // often suspected of causing, which matters for the 4.4 rollback decision:
    // a node that halts on a missing node after a reclaim was not necessarily
    // broken BY the reclaim.
    let (sc, _facts) = fixture("heals", churned_spec());
    let source_root = stored_root(&sc.live());
    let want_live = reachable_node_keys(&sc.live());
    {
        let reachable = reachable_node_keys(&sc.live());
        let victim = reachable.iter().next().expect("a node").clone();
        let mut db = RocksKv::open(&sc.live()).expect("reopen");
        db.delete(&victim).expect("puncture the tree");
    }

    let outcome = reclaim::reclaim(&sc.live()).expect("reclaim a punctured source");

    assert_eq!(outcome.root, source_root, "the root is unchanged");
    assert_eq!(
        reachable_node_keys(&sc.live()),
        want_live,
        "and the hole is gone: the rebuilt tree is complete because it was \
         built from the leaves, not copied from the damaged store"
    );
}

// ===========================================================================
// The commit point, and the never-delete rule
// ===========================================================================

#[test]
fn the_replaced_directory_is_preserved_and_still_openable() {
    // 4.4 rollback step 4 is "rename .preinstall-* back". That is only a
    // recovery path if the preserved directory is a working database, so this
    // opens it and reads its cursor rather than asserting the path exists.
    let (sc, facts) = fixture("preinstall", churned_spec());
    let before = all_rows(&sc.live());

    let outcome = reclaim::reclaim(&sc.live()).expect("reclaim");

    assert!(
        outcome.preinstall.exists(),
        "the replaced directory must be preserved"
    );
    assert!(
        outcome
            .preinstall
            .file_name()
            .and_then(|s| s.to_str())
            .expect("name")
            .contains(&format!("preinstall-{}", facts.t)),
        "the preserved directory must name the height it holds, so an operator \
         picking one for a rollback is not guessing: {}",
        outcome.preinstall.display()
    );
    assert_eq!(
        committed_height(&outcome.preinstall),
        facts.t,
        "the preserved directory must still be an openable database"
    );
    assert_eq!(
        all_rows(&outcome.preinstall),
        before,
        "and must hold exactly what the live directory held, dead nodes included"
    );
}

#[test]
fn the_reclaimed_directory_passes_a_full_a0_audit_at_the_same_height_and_root() {
    // Phase 2's acceptance shape, run here on a synthetic fixture: A0 full PASS
    // at the SAME height and the SAME root as the source. The tool runs this
    // audit internally before its rename; this runs it again afterwards through
    // the CLI, against the bytes that actually landed, because an internal PASS
    // is a claim and the installed directory is the artefact.
    let (sc, facts) = fixture("a0_after", churned_spec());
    let source_root = stored_root(&sc.live());

    reclaim::reclaim(&sc.live()).expect("reclaim");

    let (code, stdout, stderr) = run_a0(&["audit", "--db", &sc.live_arg()]);
    assert_eq!(
        code, 0,
        "the reclaimed directory must audit clean; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(&format!("RESULT PASS height={} root=", facts.t)),
        "stdout:\n{stdout}"
    );
    assert_eq!(
        a0_common::parse_result_root(&stdout),
        hex::encode(source_root),
        "same height, same root"
    );
}

#[test]
fn the_apply_flag_is_what_moves_anything() {
    let (sc, _facts) = fixture("apply_flag", churned_spec());

    let (code, stdout, stderr) = run_a0(&["reclaim", "--db", &sc.live_arg(), "--apply"]);
    assert_eq!(
        code, 0,
        "an applied reclaim must succeed; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        sc.siblings_with(".preinstall").len(),
        1,
        "--apply is the flag that renames; stdout:\n{stdout}"
    );
    assert!(
        reachable_node_keys(&sc.live()).is_subset(&smt_node_keys(&sc.live())),
        "and the result carries the whole reachable set"
    );
}

#[test]
fn a_hqc_descent_directory_reclaims_too() {
    // The healthy-node copy shape: blocks above the committed tip stored at
    // receipt, KEY_HIGHEST_QC certifying the pipeline tip, no dense qc row at
    // T+1. This is what a real stopped validator looks like, so the tool must
    // carry those pipeline block rows across and still audit.
    let mut pre = a0_common::default_pre_state();
    for round in 1..=3u128 {
        pre.push(acct_pair(0xE5, round * 1_000, round as u64));
    }
    let (sc, facts) = fixture(
        "hqc_descent",
        FixtureSpec {
            pre_state: pre,
            evidence: Evidence::HqcDescent,
            ..FixtureSpec::default()
        },
    );
    let before = non_smt_rows(&sc.live());

    let outcome = reclaim::reclaim(&sc.live()).expect("reclaim an hqc-descent dir");

    assert_eq!(outcome.height, facts.t);
    assert_eq!(non_smt_rows(&sc.live()), before);
    let (code, stdout, _stderr) = run_a0(&["audit", "--db", &sc.live_arg()]);
    assert_eq!(code, 0, "stdout:\n{stdout}");
}
