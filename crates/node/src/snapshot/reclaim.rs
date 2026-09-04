//! SMT GC Phase 1: the surgical local rebuild-and-swap (plan option D).
//!
//! THE PROBLEM. The SMT node store is content addressed and nothing ever
//! deletes from it, so every state update orphans its entire 256-node root path
//! and writes a fresh one. On the production fleet that is 26.5 GB per
//! validator spanning a live tree of about 3 MB. Phase 0 measured the
//! consequence: the forced compaction at every 5,000th height rewrites the
//! accumulated write volume since the last boundary, and under thin headroom it
//! takes ENOSPC mid-compaction on the synchronous commit path and commits never
//! resume. Two for two in production, two for two in the local reproduction.
//! Reclaiming the dead rows removes the compaction bulk and the disk-pressure
//! driver together, so this LOWERS the stranding risk rather than adding to it.
//!
//! WHY THIS SHAPE AND NOT MARK-AND-SWEEP. Sweeping in place writes 356 million
//! tombstones and then needs a compaction to free anything, and a compaction
//! needs transient free space on a box that has 9.5 GB of it. It fits the disk
//! worst at exactly the moment disk is the problem. This rebuilds beside the
//! live directory instead: peak extra allocation is about 34 MB, measured on a
//! copy of the production node0 directory at height 7,150,693 (31.16 MB of
//! non-SMT rows plus 3.10 MB of rebuilt tree, roughly 24 MB on disk).
//!
//! WHAT MAKES IT SAFE. Not care, a derivation. `Smt::with_root` has exactly one
//! production caller and it always passes the single mutable `KEY_SMT_ROOT`,
//! that root never moves backwards, and nothing anywhere reads at a historical
//! root (there is no proof endpoint and no state-at-height RPC). So the set
//! reachable from the root IS what a from-scratch rebuild over the
//! SMT-committed leaves produces, and every other `smt/node/` row is
//! unreachable by every code path that exists. The tool then declines to trust
//! even that: it runs the full A0 audit against the exact bytes about to be
//! installed, where A5 recomputes the root from the leaves and A7 checks it
//! against a header a quorum signed, and only then does it rename.
//!
//! THE ORDER OF STEPS 3 AND 4 IS LOAD BEARING, and it is the one thing here a
//! reader is likely to think is backwards. The tree is rebuilt FIRST, into an
//! empty directory, and the verbatim copy runs SECOND, on top of it. Two
//! reasons. The rebuild walks from the empty root, so it must not find an
//! inherited `smt/root` pointing at nodes that are not there yet; copying first
//! would make the very first walk fail with a missing node. And copying second
//! means the source's own `smt/root` lands on top of the rebuilt one, so the
//! audit's A5 (rebuilt equals stored) compares the rebuild against the SOURCE
//! root rather than against a value this tool just wrote. Copy first and A5
//! would be a tautology that passes on a tree it never checked.
//!
//! IT CANNOT RUN AGAINST A RUNNING NODE. RocksDB takes a directory lock on
//! open, so the first open here fails while the node holds it. That is a
//! structural guarantee rather than a documented procedure.
//!
//! NOTHING IS EVER DELETED. The replaced directory is renamed to
//! `.preinstall-<height>`, a staging directory that fails its audit is renamed
//! to `.staging-rejected-*`, and reclaiming either is an operator decision
//! taken with the node's history in hand.

use std::path::{Path, PathBuf};

use novai_execution::append_smt_ops_for_state_ops;
use novai_state::{
    decode_smt_root_v1, Kv, KvBatch, RocksKv, WriteOp, KEY_COMMITTED_HEIGHT, KEY_EXECUTED_HEIGHT,
    KEY_PREFIX_SMT_NODE, KEY_SMT_ROOT,
};

use crate::snapshot::audit;
use crate::snapshot::install::{free_path, fsync_dir};
use crate::snapshot::produce::extract_leaf_set;
use crate::snapshot::rebuild::{rebuild_tree, walk_reachable};

/// Staging directory suffix, beside the live database directory.
///
/// Deliberately NOT F5's `.snapshot-staging`. The boot installer picks up any
/// `.snapshot-staging` carrying an `INSTALL_READY` marker and installs it, so
/// sharing the suffix would let a reclaim directory left behind by a failure be
/// adopted by the next boot. The two mechanisms must not be able to see each
/// other's work in progress. This tool also writes no ready marker: it renames
/// synchronously against a stopped node and has no boot handoff at all.
pub const RECLAIM_STAGING_SUFFIX: &str = ".reclaim-staging";

/// Where the F5 producer keeps transient checkpoints, beside the database
/// directory (`crates/node/src/main.rs`, `snapshot_work_dir`).
const SNAPSHOT_WORK_DIR: &str = "snapshot-work";

/// Rows per write batch during the verbatim copy. Bounded so the copy of a real
/// 70 MB non-SMT set never materialises as one batch.
const COPY_BATCH_ROWS: usize = 8_192;

/// The census a reclaim produces, in the dry run and in the real run alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReclaimCounts {
    pub height: u64,
    pub source_root: [u8; 32],
    /// Rows outside `smt/node/`: blocks, QCs, cursors, marks, and every flat
    /// state row. These are copied verbatim.
    pub keep_rows: u64,
    pub keep_bytes: u64,
    /// Rows under `smt/node/` in the SOURCE, live and dead together. This is
    /// the number A3 reports as part of its operational total.
    pub smt_node_rows: u64,
    pub smt_node_bytes: u64,
    /// Distinct authenticated state keys. The leaf count, not a node count.
    pub leaf_count: u64,
    /// Rows reachable from the root. THE live set, walked from the root of the
    /// rebuilt tree rather than inferred. Nothing in this repo measured this
    /// before: A3 counts the family live and dead together, and the G0 gauge
    /// measures bytes in a key range without separating the two.
    pub live_node_rows: u64,
    pub live_node_bytes: u64,
    /// What the REBUILT directory will actually hold under `smt/node/`.
    ///
    /// Slightly above `live_node_rows`, and stating it separately is the point.
    /// The rebuild replays leaves one at a time, matching the node's
    /// per-transaction batching, so each walk strands the shared top of the
    /// walks before it. The excess is bounded by the leaf count rather than by
    /// the churn being reclaimed, so it is a rounding error against the figures
    /// here (12 rows against 1,268 live on a 5 leaf tree), but reporting the
    /// live count as the post-reclaim disk figure would be wrong, and quietly
    /// wrong in the direction that flatters the tool.
    pub staged_node_rows: u64,
}

impl ReclaimCounts {
    /// Rows the source holds that its own root does not reach.
    ///
    /// Measured against the LIVE set, so it is the honest size of the problem.
    /// What a run actually frees is [`Self::reclaimed_rows`], which is a little
    /// smaller because the rebuilt directory carries the rebuild's own
    /// transient orphans.
    #[must_use]
    pub const fn dead_node_rows(&self) -> u64 {
        self.smt_node_rows.saturating_sub(self.live_node_rows)
    }

    #[must_use]
    pub const fn dead_node_bytes(&self) -> u64 {
        self.smt_node_bytes.saturating_sub(self.live_node_bytes)
    }

    /// Rows a run actually removes: what the source holds, minus what the
    /// rebuilt directory will hold.
    #[must_use]
    pub const fn reclaimed_rows(&self) -> u64 {
        self.smt_node_rows.saturating_sub(self.staged_node_rows)
    }

    /// The report lines, in the A0 auditor's shape so the two read alike.
    ///
    /// Byte counts are LOGICAL key plus value bytes. On-disk bytes are smaller,
    /// measured at about 0.69 of logical on this tree, and the honest instrument
    /// for the disk figure is the `novai_db_bytes_smt_nodes` gauge rather than
    /// an assumed ratio applied here.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        vec![
            format!("D4 PASS keep_rows={} keep_bytes={}", self.keep_rows, self.keep_bytes),
            format!(
                "D5 PASS smt_node_rows={} smt_node_bytes={} (source family, live and dead)",
                self.smt_node_rows, self.smt_node_bytes
            ),
            format!(
                "D6 PASS leaves={} live_node_rows={} live_node_bytes={} staged_node_rows={}",
                self.leaf_count, self.live_node_rows, self.live_node_bytes, self.staged_node_rows
            ),
            format!(
                "PLAN reclaim_rows={} dead_rows={} keep_rows={} live_node_rows={}",
                self.reclaimed_rows(),
                self.dead_node_rows(),
                self.keep_rows,
                self.live_node_rows
            ),
        ]
    }
}

#[derive(Debug)]
pub enum ReclaimError {
    Io(String),
    /// The cursors disagree, so the directory is not at a clean block boundary.
    /// A1's own condition, checked here first so the leaf set is never read
    /// from a state that is half a block old.
    Torn {
        committed: Option<u64>,
        executed: Option<u64>,
    },
    /// A checkpoint directory is present. Its hard links pin the old SST files,
    /// so the bytes this tool exists to reclaim would not actually free.
    CheckpointOutstanding {
        dir: String,
        entries: usize,
    },
    /// A key is unknown to the classification table, or defined with no
    /// production writer. Refuse rather than drop: a dropped leaf is a wrong
    /// root, which is a silent corruption.
    Unclassifiable(String),
    /// The staging path is occupied. Never cleared automatically.
    StagingOccupied(String),
    /// The rebuild did not reproduce the source root. Fail closed BEFORE
    /// anything is written.
    RootMismatch {
        source: String,
        rebuilt: String,
    },
    /// The tree that landed on disk is not the tree that was verified in
    /// memory. Raised by the on-disk reachability walk, which exists because
    /// the A0 audit cannot see `smt/node/` rows at all.
    StagedTreeIncomplete(String),
    /// The staged directory does not carry every row the census counted outside
    /// `smt/node/`. The copier is the only writer of the blocks, the QCs, the
    /// cursors and the anti-equivocation marks, and no other check in this
    /// pipeline reads them, so this is the only place a lost family is seen.
    StagedKeepSetIncomplete {
        staged_rows: u64,
        staged_bytes: u64,
        expect_rows: u64,
        expect_bytes: u64,
    },
    /// The staged directory failed its audit. It is preserved for diagnosis and
    /// the live directory was never touched.
    AuditFailed {
        moved_to: String,
        result: String,
    },
}

impl std::fmt::Display for ReclaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "reclaim io: {e}"),
            Self::Torn {
                committed,
                executed,
            } => write!(
                f,
                "reclaim refused: committed={committed:?} executed={executed:?}, the directory \
                 is torn or was captured inside the one-block crash window; start the node, let \
                 it close the window, stop it again"
            ),
            Self::CheckpointOutstanding { dir, entries } => write!(
                f,
                "reclaim refused: {entries} checkpoint directory entries under {dir}; their hard \
                 links pin the old SST files, so the reclaimed bytes would not free and the run \
                 would look successful while achieving nothing"
            ),
            Self::Unclassifiable(k) => write!(
                f,
                "reclaim refused: {k}; refusing to drop it, because a dropped leaf is a wrong root"
            ),
            Self::StagingOccupied(p) => write!(
                f,
                "reclaim refused: {p} already exists; it is never cleared automatically, because \
                 it may be the only copy of a previous run"
            ),
            Self::RootMismatch { source, rebuilt } => write!(
                f,
                "reclaim refused: the rebuild produced {rebuilt} but the source stores {source}; \
                 the leaf set does not span the live tree and nothing has been written"
            ),
            Self::StagedTreeIncomplete(why) => write!(
                f,
                "reclaim refused: the rebuilt tree on disk is not the tree that was verified in \
                 memory ({why}); nothing has been renamed, and note that the A0 audit cannot \
                 catch this because it rebuilds from the leaves and never reads the node store"
            ),
            Self::StagedKeepSetIncomplete {
                staged_rows,
                staged_bytes,
                expect_rows,
                expect_bytes,
            } => write!(
                f,
                "reclaim refused: the staged directory holds {staged_rows} rows and \
                 {staged_bytes} bytes outside smt/node/, but the source census counted \
                 {expect_rows} and {expect_bytes}; the copier is the only writer of the blocks, \
                 QCs, cursors and anti-equivocation marks, and neither the audit nor the \
                 staged-tree walk reads them, so nothing else would see this loss"
            ),
            Self::AuditFailed { moved_to, result } => write!(
                f,
                "reclaim refused: the staged directory failed its audit ({result}); it is \
                 preserved at {moved_to} and the live directory is untouched"
            ),
        }
    }
}

/// What a completed reclaim did.
#[derive(Debug)]
pub struct ReclaimOutcome {
    pub counts: ReclaimCounts,
    pub height: u64,
    pub root: [u8; 32],
    /// The preserved previous directory. The operator deletes it, after the
    /// node passes the rejoin gate, and never before.
    pub preinstall: PathBuf,
}

/// What a completed staging run produced, with nothing renamed.
///
/// [`stage`] is every step of a reclaim except the swap, so this is the whole
/// correctness claim standing on disk and waiting for a decision. Phase 2
/// proves the tool on a real validator copy through this type, because proving
/// the rebuild and proving the swap are separate questions and the swap belongs
/// to a live node.
#[derive(Debug)]
pub struct StagedOutcome {
    pub counts: ReclaimCounts,
    pub height: u64,
    pub root: [u8; 32],
    /// The rebuilt, walked and audited directory, sitting beside the source.
    /// It is NEVER removed automatically: a staging directory left behind is
    /// either the artefact of a proof or the evidence of a failure, and this
    /// tool does not decide which.
    pub staging: PathBuf,
    /// The staged audit's own result line, so a caller reporting a staging run
    /// quotes the verifier rather than paraphrasing it.
    pub audit_result: String,
}

// ===========================================================================
// The read-only census. This is the default mode.
// ===========================================================================

/// Count what a reclaim would do, writing nothing.
///
/// This is a full rehearsal of the correctness claim minus the writes: it reads
/// the same cursors, extracts the same leaf set, rebuilds the same tree and
/// compares the same roots. An operator who sees a PASS here knows the real run
/// will not fail its root check, and gets the exact live-node figure (M1) as a
/// side effect, since the rebuilt tree IS the reachable set.
///
/// # Errors
/// Every refusal the mutating path can raise before it writes anything: a torn
/// directory, an outstanding checkpoint, an unclassifiable key, or a rebuilt
/// root that disagrees with the stored one.
pub fn plan(db_dir: &Path) -> Result<ReclaimCounts, ReclaimError> {
    checkpoint_guard(db_dir)?;
    let db = open(db_dir)?;
    let (height, source_root) = preflight_cursors(&db)?;
    let (keep_rows, keep_bytes, smt_node_rows, smt_node_bytes) = census(&db)?;
    let leaves = extract_leaf_set(&db).map_err(|e| ReclaimError::Unclassifiable(e.to_string()))?;

    let tree = rebuild_tree(&leaves).map_err(ReclaimError::Io)?;
    if tree.root != source_root {
        return Err(ReclaimError::RootMismatch {
            source: hex::encode(source_root),
            rebuilt: hex::encode(tree.root),
        });
    }

    Ok(ReclaimCounts {
        height,
        source_root,
        keep_rows,
        keep_bytes,
        smt_node_rows,
        smt_node_bytes,
        leaf_count: leaves.len() as u64,
        live_node_rows: tree.live_rows,
        live_node_bytes: tree.live_bytes,
        staged_node_rows: tree.stored_rows,
    })
}

// ===========================================================================
// The mutating path.
// ===========================================================================

/// Rebuild `db_dir` beside itself, prove the result, and stop before the swap.
///
/// This is every step of [`reclaim`] except the two renames, and [`reclaim`]
/// calls it rather than repeating it, so the path proven here is the path that
/// runs in production. Splitting it out is what lets the rebuild be proven on a
/// real validator copy offline: the rebuild and the audit are safe anywhere,
/// the swap belongs to a stopped node that is about to restart.
///
/// On success the staging directory is left in place, audited and unrenamed.
/// On failure it is left in place too, and a later run refuses rather than
/// clearing it, because it may be the only copy of what went wrong.
///
/// # Errors
/// Refuses, without touching the live directory, on any of: a running node
/// holding the database lock, a torn cursor pair, an outstanding checkpoint, an
/// occupied staging path, an unclassifiable key, a rebuilt root that disagrees
/// with the stored root, a staged tree that does not match the verified
/// rebuild, a staged keep set short of the census, or a staged directory that
/// fails the full A0 audit.
pub fn stage(db_dir: &Path) -> Result<StagedOutcome, ReclaimError> {
    let (base, subdir) = split(db_dir)?;
    let staging = base.join(format!("{subdir}{RECLAIM_STAGING_SUFFIX}"));

    checkpoint_guard(db_dir)?;
    if staging.exists() {
        return Err(ReclaimError::StagingOccupied(staging.display().to_string()));
    }

    let src = open(db_dir)?;
    let (height, source_root) = preflight_cursors(&src)?;
    let (keep_rows, keep_bytes, smt_node_rows, smt_node_bytes) = census(&src)?;
    let leaves = extract_leaf_set(&src).map_err(|e| ReclaimError::Unclassifiable(e.to_string()))?;

    // The correctness check runs against an in-memory rebuild BEFORE a single
    // byte is written. It costs the size of the live tree, which is megabytes,
    // and it converts every root disagreement from a staged directory that has
    // to be set aside into a refusal that leaves the disk exactly as it was.
    let tree = rebuild_tree(&leaves).map_err(ReclaimError::Io)?;
    if tree.root != source_root {
        return Err(ReclaimError::RootMismatch {
            source: hex::encode(source_root),
            rebuilt: hex::encode(tree.root),
        });
    }
    let counts = ReclaimCounts {
        height,
        source_root,
        keep_rows,
        keep_bytes,
        smt_node_rows,
        smt_node_bytes,
        leaf_count: leaves.len() as u64,
        live_node_rows: tree.live_rows,
        live_node_bytes: tree.live_bytes,
        staged_node_rows: tree.stored_rows,
    };

    // Step 3 of the plan's 3.1, and it runs before the copy. See the module
    // header: the walk must start from an empty root, and the copy must land
    // the SOURCE root on top so the audit's A5 is a real comparison.
    materialize_tree(&staging, &leaves)?;

    // Step 2, the one genuinely new piece of code. Mechanical and total: one
    // exclusion rule, applied to both column families, so it cannot silently
    // drop a family the way a classification-driven copier could.
    copy_non_smt_rows(&src, &staging)?;
    drop(src);

    // Step 4a, and it is NOT what the plan assumed. The plan's 3.3 says a
    // deleted live node is "caught before the rename by A5". It is not: A4
    // rebuilds the root from the LEAVES into a fresh store and A5 compares that
    // to the stored root, so neither ever reads this directory's own
    // `smt/node/` rows. A staged directory with an empty or truncated node
    // store audits PASS at the right height and the right root, installs, and
    // then halts the node on the first update walk that touches a node that is
    // not there. So the tree that actually landed is walked here, on disk,
    // before the audit that cannot see it.
    verify_staged_tree(&staging, source_root, tree.live_rows)?;

    // Step 4b, and it is the same class of gap as 4a: a check the design
    // assumed was somewhere else. The copier is the ONLY writer of the blocks,
    // the QCs, the cursors and the anti-equivocation marks, and it already
    // returned a row count that this pipeline discarded while the census beside
    // it held the expected one. A dropped `KEY_VOTED_VIEW` is invisible to the
    // audit (A3 classifies it Operational and no check reads it) and invisible
    // to the staged-tree walk (it is not a node), so before this it was caught
    // by nothing outside a unit test. Counting the staged keep set and
    // comparing it to the census gates every non-node family at once, rather
    // than naming the two marks and leaving the next family uncovered.
    verify_staged_keep_set(&staging, keep_rows, keep_bytes)?;

    // Step 4. The same verifier the producer, the receiver and the boot
    // installer run, against the exact bytes about to be installed, pinned to
    // the source height so A1 checks it rather than merely reporting it.
    let staging_str = staging
        .to_str()
        .ok_or_else(|| ReclaimError::Io("staging path is not valid utf8".to_string()))?;
    let report = audit::audit(staging_str, Some(height)).map_err(ReclaimError::Io)?;
    let root_agrees = report.root == Some(source_root);
    if !report.ok || !root_agrees {
        let aside = free_path(&base, &format!("{subdir}.staging-rejected"));
        std::fs::rename(&staging, &aside)
            .map_err(|e| ReclaimError::Io(format!("set aside rejected staging: {e}")))?;
        let result = if root_agrees {
            report.result_line()
        } else {
            format!(
                "{} (rebuilt root {:?} is not the source root {})",
                report.result_line(),
                report.root.map(hex::encode),
                hex::encode(source_root)
            )
        };
        return Err(ReclaimError::AuditFailed {
            moved_to: aside.display().to_string(),
            result,
        });
    }

    Ok(StagedOutcome {
        counts,
        height,
        root: source_root,
        staging,
        audit_result: report.result_line(),
    })
}

/// Rebuild `db_dir` beside itself and swap it in.
///
/// The proof is [`stage`] and this is the commit point: two renames and an
/// fsync, and nothing else. Keeping the swap this thin is deliberate, because
/// everything above it can be rehearsed offline and this cannot.
///
/// # Errors
/// Every refusal [`stage`] can raise, none of which touches the live directory,
/// plus an IO failure on one of the two renames.
pub fn reclaim(db_dir: &Path) -> Result<ReclaimOutcome, ReclaimError> {
    let staged = stage(db_dir)?;
    let (base, subdir) = split(db_dir)?;
    let StagedOutcome {
        counts,
        height,
        root,
        staging,
        audit_result: _,
    } = staged;

    // Step 5. The live directory is set aside, never removed, and named for the
    // height it holds so an operator choosing one for a rollback is not
    // guessing. Then one rename, and the swap is done.
    let preinstall = free_path(&base, &format!("{subdir}.preinstall-{height}"));
    std::fs::rename(db_dir, &preinstall)
        .map_err(|e| ReclaimError::Io(format!("set the live directory aside: {e}")))?;
    std::fs::rename(&staging, db_dir)
        .map_err(|e| ReclaimError::Io(format!("move the rebuilt directory into place: {e}")))?;
    fsync_dir(&base);

    tracing::warn!(
        height,
        root = %hex::encode(root),
        reclaimed_rows = counts.dead_node_rows(),
        preserved = %preinstall.display(),
        "SMT reclaim installed; the previous directory is preserved and is the rollback"
    );

    Ok(ReclaimOutcome {
        counts,
        height,
        root,
        preinstall,
    })
}

// ===========================================================================
// Steps
// ===========================================================================

fn split(db_dir: &Path) -> Result<(PathBuf, String), ReclaimError> {
    let base = db_dir
        .parent()
        .ok_or_else(|| {
            ReclaimError::Io(format!(
                "{} has no parent directory, so there is nowhere beside it to stage",
                db_dir.display()
            ))
        })?
        .to_path_buf();
    let subdir = db_dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| ReclaimError::Io(format!("{} has no usable name", db_dir.display())))?
        .to_string();
    Ok((base, subdir))
}

fn open(db_dir: &Path) -> Result<RocksKv, ReclaimError> {
    RocksKv::open(db_dir).map_err(|e| {
        ReclaimError::Io(format!(
            "open {}: {e:?} (a running node holds this lock; stop it first)",
            db_dir.display()
        ))
    })
}

/// A1's condition, and the source root the whole run is checked against.
fn preflight_cursors(db: &RocksKv) -> Result<(u64, [u8; 32]), ReclaimError> {
    let committed = read_cursor(db, KEY_COMMITTED_HEIGHT)?;
    let executed = read_cursor(db, KEY_EXECUTED_HEIGHT)?;
    match (committed, executed) {
        (Some(c), Some(e)) if c == e => {
            let root = match db
                .get(KEY_SMT_ROOT)
                .map_err(|e| ReclaimError::Io(format!("get root: {e:?}")))?
            {
                Some(bytes) => decode_smt_root_v1(&bytes)
                    .map_err(|e| ReclaimError::Io(format!("decode stored root: {e:?}")))?,
                None => novai_execution::empty_smt_root(),
            };
            Ok((c, root))
        }
        (committed, executed) => Err(ReclaimError::Torn {
            committed,
            executed,
        }),
    }
}

fn read_cursor(db: &RocksKv, key: &[u8]) -> Result<Option<u64>, ReclaimError> {
    match db
        .get(key)
        .map_err(|e| ReclaimError::Io(format!("get cursor: {e:?}")))?
    {
        Some(b) if b.len() == 8 => {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b);
            Ok(Some(u64::from_be_bytes(a)))
        }
        _ => Ok(None),
    }
}

/// Refuse while a checkpoint directory exists beside the database.
///
/// The tool runs offline, so it cannot read the producer's `has_pending` flag;
/// it reads the filesystem trace of the same thing, which is the part that
/// actually causes the harm. A checkpoint is a memtable flush plus HARD LINKS
/// to the SST files, so while one exists the old files are pinned and the space
/// does not come back when the previous directory is finally removed.
fn checkpoint_guard(db_dir: &Path) -> Result<(), ReclaimError> {
    let Some(base) = db_dir.parent() else {
        return Ok(());
    };
    let work = base.join(SNAPSHOT_WORK_DIR);
    let Ok(entries) = std::fs::read_dir(&work) else {
        // Absent is the normal case and is not an error.
        return Ok(());
    };
    let n = entries.count();
    if n > 0 {
        return Err(ReclaimError::CheckpointOutstanding {
            dir: work.display().to_string(),
            entries: n,
        });
    }
    Ok(())
}

/// Row and byte counts, split at the one boundary that matters.
fn census(db: &RocksKv) -> Result<(u64, u64, u64, u64), ReclaimError> {
    let (mut keep_rows, mut keep_bytes, mut smt_rows, mut smt_bytes) = (0u64, 0u64, 0u64, 0u64);
    {
        let mut count = |k: &[u8], v: &[u8]| {
            let bytes = (k.len() + v.len()) as u64;
            if k.starts_with(KEY_PREFIX_SMT_NODE) {
                smt_rows += 1;
                smt_bytes += bytes;
            } else {
                keep_rows += 1;
                keep_bytes += bytes;
            }
        };
        // An empty prefix reaches only the default column family, because the
        // nnpx routing prefix does not match it; the second scan reaches the
        // other one. Same two-scan idiom the auditor and the leaf extractor
        // use, so all three see the same key set.
        db.for_each_prefix(b"", &mut count)
            .map_err(|e| ReclaimError::Io(format!("scan default cf: {e:?}")))?;
        db.for_each_prefix(b"nnpx/", &mut count)
            .map_err(|e| ReclaimError::Io(format!("scan nnpx cf: {e:?}")))?;
    }
    Ok((keep_rows, keep_bytes, smt_rows, smt_bytes))
}

/// Rebuild the tree into a FRESH directory, one walk per leaf.
///
/// Identical in shape to `stage.rs`'s materialisation, and for the same reason:
/// the walk is driven through `append_smt_ops_for_state_ops`, the canonical
/// execution path every state handler and genesis use, so the resulting root is
/// produced by the node's own code rather than by a parallel implementation
/// that could drift. The final `KEY_SMT_ROOT` falls out of the last walk and is
/// never written by hand, so it cannot disagree with the tree it labels.
fn materialize_tree(staging: &Path, leaves: &[(Vec<u8>, Vec<u8>)]) -> Result<(), ReclaimError> {
    if staging.join("CURRENT").exists() {
        return Err(ReclaimError::StagingOccupied(
            staging.display().to_string(),
        ));
    }
    std::fs::create_dir_all(staging)
        .map_err(|e| ReclaimError::Io(format!("create {}: {e}", staging.display())))?;
    let mut db = RocksKv::open(staging)
        .map_err(|e| ReclaimError::Io(format!("open staging: {e:?}")))?;

    for (k, v) in leaves {
        let state_ops = vec![WriteOp::Put(k.clone(), v.clone())];
        let mut all_ops = state_ops.clone();
        append_smt_ops_for_state_ops(&db, &state_ops, &mut all_ops)
            .map_err(|e| ReclaimError::Io(format!("smt walk for a leaf: {e:?}")))?;
        db.apply_batch(&all_ops)
            .map_err(|e| ReclaimError::Io(format!("apply leaf batch: {e:?}")))?;
    }
    Ok(())
}

/// Walk the tree that actually landed on disk and prove it is complete.
///
/// Two claims, and the second is the one that makes this more than a smoke
/// test. Every internal node reachable from the stored root must be present, so
/// a lost or unwritten row is named here rather than at runtime. And the count
/// must equal the in-memory rebuild's live figure, which ties the tree on disk
/// to the tree whose root was already checked against the source. A walk that
/// only checked for dangling children would pass a directory holding a
/// perfectly well formed tree of the wrong shape.
///
/// PUBLIC ON PURPOSE. It is the check the plan's step 4 was assumed to do, so
/// it has to be runnable on its own: against a staged directory before a swap,
/// and against an installed directory after one. A check reachable only from
/// inside the pipeline that calls it can be exercised only by breaking that
/// pipeline, and a gate exercisable only by its own definition site is not a
/// gate.
///
/// # Errors
/// [`ReclaimError::StagedTreeIncomplete`] if the stored root is not
/// `expect_root`, if an internal node reachable from it is absent, or if the
/// reachable count is not `expect_rows`.
pub fn verify_staged_tree(
    staging: &Path,
    expect_root: [u8; 32],
    expect_rows: u64,
) -> Result<(), ReclaimError> {
    let db = RocksKv::open(staging)
        .map_err(|e| ReclaimError::Io(format!("reopen staging to verify: {e:?}")))?;
    let stored = match db
        .get(KEY_SMT_ROOT)
        .map_err(|e| ReclaimError::Io(format!("get staged root: {e:?}")))?
    {
        Some(bytes) => decode_smt_root_v1(&bytes)
            .map_err(|e| ReclaimError::Io(format!("decode staged root: {e:?}")))?,
        None => novai_execution::empty_smt_root(),
    };
    if stored != expect_root {
        return Err(ReclaimError::StagedTreeIncomplete(format!(
            "staged root {} is not the source root {}",
            hex::encode(stored),
            hex::encode(expect_root)
        )));
    }
    let (rows, _bytes) =
        walk_reachable(&db, stored).map_err(ReclaimError::StagedTreeIncomplete)?;
    if rows != expect_rows {
        return Err(ReclaimError::StagedTreeIncomplete(format!(
            "the staged tree spans {rows} rows but the verified rebuild spans {expect_rows}"
        )));
    }
    Ok(())
}

/// Count the staged rows outside `smt/node/` and prove the copier lost none.
///
/// WHY THIS EXISTS, and it is the third instance in this gate of the same
/// defect. Phase 1 found that the plan's "caught before the rename by A5" named
/// a check that does not exist, and that the audit is blind to the
/// anti-equivocation marks. This is the same shape once more: the copier
/// already returned the number of rows it wrote, the census beside it already
/// held the number it should have written, and the pipeline compared neither.
///
/// A dropped `KEY_VOTED_VIEW` is seen by nothing else. A3 classifies it
/// Operational, no audit check reads it, and it is not a node so the staged
/// tree walk does not reach it either. The result would audit PASS at the right
/// height and the right root, install cleanly, and boot a validator that has
/// forgotten what it already voted for.
///
/// COUNTING THE STAGED DIRECTORY RATHER THAN TRUSTING THE COPIER'S RETURN
/// VALUE is the point. The copier's own count says what it decided to write;
/// this says what is actually there, so it also covers a write that was
/// batched, dropped or never flushed.
///
/// The invariant that makes the comparison exact: the only non-node rows the
/// rebuild writes are the leaf rows and `smt/root`
/// (`append_smt_ops_for_state_ops`, `crates/execution/src/lib.rs:6552-6593`),
/// and every one of those is in the source keep set already, so the staged keep
/// set is the source keep set with nothing added.
///
/// Honest limit, stated rather than implied: rows and bytes are a strong
/// checksum over a verbatim copy, not a set equality. A copier that substituted
/// one row for another of the same length would pass here. That is not a shape
/// this copier can produce, since it writes back exactly the pairs it scanned,
/// and a full key-set comparison would mean holding both key sets in memory at
/// production scale for a failure mode the code has no path to.
///
/// # Errors
/// [`ReclaimError::StagedKeepSetIncomplete`] if either figure disagrees.
pub fn verify_staged_keep_set(
    staging: &Path,
    expect_rows: u64,
    expect_bytes: u64,
) -> Result<(), ReclaimError> {
    let db = RocksKv::open(staging)
        .map_err(|e| ReclaimError::Io(format!("reopen staging to count: {e:?}")))?;
    let (mut rows, mut bytes) = (0u64, 0u64);
    {
        let mut count = |k: &[u8], v: &[u8]| {
            if !k.starts_with(KEY_PREFIX_SMT_NODE) {
                rows += 1;
                bytes += (k.len() + v.len()) as u64;
            }
        };
        db.for_each_prefix(b"", &mut count)
            .map_err(|e| ReclaimError::Io(format!("scan staged default cf: {e:?}")))?;
        db.for_each_prefix(b"nnpx/", &mut count)
            .map_err(|e| ReclaimError::Io(format!("scan staged nnpx cf: {e:?}")))?;
    }
    if rows != expect_rows || bytes != expect_bytes {
        return Err(ReclaimError::StagedKeepSetIncomplete {
            staged_rows: rows,
            staged_bytes: bytes,
            expect_rows,
            expect_bytes,
        });
    }
    Ok(())
}

/// Copy every row whose key is not under `smt/node/`, verbatim, into `staging`.
///
/// ONE exclusion rule, applied mechanically to both column families. That is
/// the whole design of this function: it needs no classification judgment, so
/// unlike a copier driven by the classification table it cannot silently drop a
/// family that table does not know about. It carries blocks, QCs, cursors, the
/// anti-equivocation marks, and every flat state row, and it carries the
/// source's `smt/root` on top of the rebuilt one so the audit compares the
/// rebuild against the source rather than against this tool's own output.
fn copy_non_smt_rows(src: &RocksKv, staging: &Path) -> Result<u64, ReclaimError> {
    let mut dst = RocksKv::open(staging)
        .map_err(|e| ReclaimError::Io(format!("reopen staging for copy: {e:?}")))?;
    let mut copied = 0u64;
    let mut batch: Vec<WriteOp> = Vec::with_capacity(COPY_BATCH_ROWS);
    let mut failure: Option<String> = None;

    {
        let mut take = |k: &[u8], v: &[u8]| {
            if failure.is_some() || k.starts_with(KEY_PREFIX_SMT_NODE) {
                return;
            }
            batch.push(WriteOp::Put(k.to_vec(), v.to_vec()));
            copied += 1;
            if batch.len() >= COPY_BATCH_ROWS {
                if let Err(e) = dst.apply_batch(&batch) {
                    failure = Some(format!("copy batch: {e:?}"));
                }
                batch.clear();
            }
        };
        src.for_each_prefix(b"", &mut take)
            .map_err(|e| ReclaimError::Io(format!("scan default cf: {e:?}")))?;
        src.for_each_prefix(b"nnpx/", &mut take)
            .map_err(|e| ReclaimError::Io(format!("scan nnpx cf: {e:?}")))?;
    }

    if let Some(e) = failure {
        return Err(ReclaimError::Io(e));
    }
    if !batch.is_empty() {
        dst.apply_batch(&batch)
            .map_err(|e| ReclaimError::Io(format!("copy final batch: {e:?}")))?;
    }
    Ok(copied)
}

// ===========================================================================
// CLI
// ===========================================================================

/// What the operator asked for. The census is the default in every path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Read only. Writes nothing, stages nothing, renames nothing.
    DryRun,
    /// Rebuild, walk, count and audit beside the source. Renames nothing.
    StageOnly,
    /// All of that, and then the swap.
    Apply,
}

/// Print the census, and apply it only when asked.
///
/// `Ok(true)` = success (exit 0), `Ok(false)` = refused (exit 1), `Err` =
/// environment error (exit 2), matching `audit::run`.
///
/// # Errors
/// Never: refusals are reported as `Ok(false)` so the caller's exit codes stay
/// aligned with the auditor's. The signature keeps the `Err` arm so a future
/// environment failure has somewhere to go without changing every call site.
pub fn run(db_dir: &str, mode: Mode) -> Result<bool, String> {
    let path = Path::new(db_dir);

    if mode == Mode::Apply {
        return match reclaim(path) {
            Ok(outcome) => {
                for line in outcome.counts.lines() {
                    println!("{line}");
                }
                println!(
                    "RESULT RECLAIMED height={} root={} preserved={}",
                    outcome.height,
                    hex::encode(outcome.root),
                    outcome.preinstall.display()
                );
                Ok(true)
            }
            Err(e) => {
                println!("RESULT REFUSED {e}");
                Ok(false)
            }
        };
    }

    if mode == Mode::StageOnly {
        return match stage(path) {
            Ok(outcome) => {
                let c = &outcome.counts;
                println!("D1 PASS committed=executed height={}", c.height);
                println!("D2 PASS source_root={}", hex::encode(c.source_root));
                println!("D3 PASS checkpoint_pin=none");
                for line in c.lines() {
                    println!("{line}");
                }
                // The three checks that stand between a rebuilt directory and a
                // swap, each named so a staging report quotes them rather than
                // asserting that they ran. D7 and D8 are the two the A0 audit
                // cannot perform.
                println!(
                    "D7 PASS staged_tree_rows={} root={}",
                    c.live_node_rows,
                    hex::encode(outcome.root)
                );
                println!(
                    "D8 PASS staged_keep_rows={} staged_keep_bytes={}",
                    c.keep_rows, c.keep_bytes
                );
                println!("D9 PASS staged_audit {}", outcome.audit_result);
                println!(
                    "RESULT STAGED height={} root={} staging={} (nothing was renamed; the \
                     staging directory is never removed automatically)",
                    outcome.height,
                    hex::encode(outcome.root),
                    outcome.staging.display()
                );
                Ok(true)
            }
            Err(e) => {
                println!("RESULT REFUSED {e}");
                Ok(false)
            }
        };
    }

    match plan(path) {
        Ok(counts) => {
            // Stated as one equality rather than as two readings: `plan` only
            // returns when the two cursors are equal, so printing them as
            // separate measurements would dress a derived fact as an observed
            // one.
            println!("D1 PASS committed=executed height={}", counts.height);
            println!("D2 PASS source_root={}", hex::encode(counts.source_root));
            println!("D3 PASS checkpoint_pin=none");
            for line in counts.lines() {
                println!("{line}");
            }
            println!(
                "RESULT DRY-RUN height={} root={} (nothing was written; pass --apply to swap)",
                counts.height,
                hex::encode(counts.source_root)
            );
            Ok(true)
        }
        Err(e) => {
            println!("RESULT REFUSED {e}");
            Ok(false)
        }
    }
}
