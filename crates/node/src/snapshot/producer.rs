//! Gate F5 Stage 2: the demand-driven snapshot producer.
//!
//! THE SPLIT, which is the whole point of this module.
//!
//! On the commit path, holding the database lock, [`SnapshotProducer::on_commit`]
//! does at most ONE thing: create a RocksDB checkpoint (a memtable flush plus
//! hard links, cost independent of database size). It never scans, never
//! classifies, never rebuilds a tree, never audits. Off the lock, on a
//! background thread, [`SnapshotProducer::run_pending_production`] opens that
//! checkpoint by PATH and does all of the expensive work. The producing code
//! in `super::produce` is given a path and nothing else, so it has no handle
//! to the live database and could not hold the commit lock even by mistake.
//!
//! That split is not a stylistic preference. The incident this whole gate
//! exists to recover from began with unbounded blocking work on the commit path
//! (the forced compaction every 5,000 heights, which runs under this same lock
//! and whose range grows without bound). Adding a second such hazard while
//! fixing the first would be absurd.
//!
//! THE COMMON CASE COSTS NOTHING. Production is demanded, not scheduled. With
//! no peer asking, `on_commit` reads two cursors at most and usually returns
//! before even that. A healthy fleet pays no checkpoint, no flush, no disk.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use novai_state::{Kv, KEY_COMMITTED_HEIGHT, KEY_EXECUTED_HEIGHT};

use crate::consensus_node::Storage;
use crate::snapshot::bundle::SnapshotBundle;
use crate::snapshot::produce::{build_bundle, ProduceError};
use crate::MutexExt;

/// How far the cached bundle may fall behind the committed tip before a fresh
/// demand triggers a new checkpoint.
///
/// A recovering node must land inside the fleet's retention window with margin,
/// and the design's go/no-go gate is `tip - H <= 10_000` blocks. Refreshing at
/// a fifth of that keeps a cached bundle comfortably inside the gate while
/// still reusing it across the several requests one recovery makes.
pub const STALE_SNAPSHOT_BLOCKS: u64 = 2_000;

/// What the commit-path hook did. Every variant except `CheckpointTaken` means
/// the hook touched nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOutcome {
    /// Nobody asked, or a cached bundle is fresh enough to answer. This is the
    /// steady state on a healthy fleet.
    Skipped,
    /// A checkpoint is already waiting to be produced. One at a time.
    AlreadyPending,
    /// The cursors disagree, so this is mid-batch rather than a clean block
    /// boundary. The next commit will be one.
    NotAtBoundary,
    /// A checkpoint was created. All remaining work happens off this lock.
    CheckpointTaken,
    /// The storage backend cannot checkpoint (in-memory storage).
    Unsupported,
    /// The checkpoint itself failed. Logged, never fatal: a node that cannot
    /// produce a snapshot is still a healthy validator.
    Failed,
}

pub struct SnapshotProducer {
    /// Directory that holds transient checkpoints. Never inside the live
    /// database directory.
    work_dir: PathBuf,
    /// Set when a snapshot is asked for. Cleared when one is cached. Stage 4
    /// wires a peer request to this; Stage 2 exposes it so the demand path is
    /// exercised and so an operator-triggered production has a seam.
    demand: AtomicBool,
    cached: Mutex<Option<Arc<SnapshotBundle>>>,
    pending: Mutex<Option<PathBuf>>,
    /// Microseconds spent UNDER THE DATABASE LOCK on the last checkpoint. This
    /// is the commit-path cost, and it is deliberately a separate number from
    /// the background time so it cannot hide inside a total.
    last_checkpoint_micros: AtomicU64,
    /// Microseconds spent OFF the lock on the last production.
    last_background_micros: AtomicU64,
    seq: AtomicU64,
}

impl SnapshotProducer {
    #[must_use]
    pub fn new(work_dir: PathBuf) -> Self {
        Self {
            work_dir,
            demand: AtomicBool::new(false),
            cached: Mutex::new(None),
            pending: Mutex::new(None),
            last_checkpoint_micros: AtomicU64::new(0),
            last_background_micros: AtomicU64::new(0),
            seq: AtomicU64::new(0),
        }
    }

    /// Ask for a snapshot. Idempotent.
    pub fn request(&self) {
        self.demand.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn demanded(&self) -> bool {
        self.demand.load(Ordering::Relaxed)
    }

    /// The currently servable bundle, if one has been produced AND audited.
    #[must_use]
    pub fn cached(&self) -> Option<Arc<SnapshotBundle>> {
        self.cached.lock_or_recover().clone()
    }

    /// Height of the cached bundle, 0 when there is none. Feeds
    /// `novai_snapshot_height`.
    #[must_use]
    pub fn cached_height(&self) -> u64 {
        self.cached
            .lock_or_recover()
            .as_ref()
            .map_or(0, |b| b.manifest.height)
    }

    /// Seconds spent under the database lock on the last checkpoint. Feeds
    /// `novai_snapshot_produce_seconds`, which measures the COMMIT-PATH cost
    /// only.
    #[must_use]
    pub fn last_checkpoint_seconds(&self) -> f64 {
        self.last_checkpoint_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }

    /// Seconds spent off the lock on the last production. Feeds
    /// `novai_snapshot_background_seconds`. Exposed alongside the commit-path
    /// number precisely so the two can never be confused for each other.
    #[must_use]
    pub fn last_background_seconds(&self) -> f64 {
        self.last_background_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }

    #[must_use]
    pub fn last_checkpoint_micros(&self) -> u64 {
        self.last_checkpoint_micros.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn last_background_micros(&self) -> u64 {
        self.last_background_micros.load(Ordering::Relaxed)
    }

    /// Is a checkpoint waiting for background production?
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.pending.lock_or_recover().is_some()
    }

    /// Should this commit take a checkpoint? Consumes the demand when a cached
    /// bundle already answers it, so a fresh cache costs the commit path
    /// nothing on every subsequent block.
    fn wants_checkpoint(&self, committed_height: u64) -> bool {
        if !self.demand.load(Ordering::Relaxed) {
            return false;
        }
        let cached_height = self.cached_height();
        if cached_height > 0 && committed_height.saturating_sub(cached_height) <= STALE_SNAPSHOT_BLOCKS
        {
            // Already answerable. Clear the demand rather than re-producing.
            self.demand.store(false, Ordering::Relaxed);
            return false;
        }
        true
    }

    /// THE COMMIT-PATH HOOK. Called with the database lock held.
    ///
    /// Everything here is O(1) reads plus, at most, one checkpoint create. It
    /// must never grow a scan, a classification pass, an audit or a tree
    /// rebuild: those run in [`Self::run_pending_production`], against a path,
    /// with no access to this database.
    pub fn on_commit(&self, db: &Storage, committed_height: u64) -> HookOutcome {
        if !self.wants_checkpoint(committed_height) {
            return HookOutcome::Skipped;
        }
        if self.has_pending() {
            return HookOutcome::AlreadyPending;
        }

        // Clean-boundary check. Inside a multi-block commit batch the committed
        // cursor is already at the last block while the executed cursor trails,
        // so only the final block of a batch is a capture point.
        let committed = read_cursor(db, KEY_COMMITTED_HEIGHT);
        let executed = read_cursor(db, KEY_EXECUTED_HEIGHT);
        if committed.is_none() || committed != executed {
            return HookOutcome::NotAtBoundary;
        }

        let Storage::Rocks(rocks) = db else {
            return HookOutcome::Unsupported;
        };

        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let path = self
            .work_dir
            .join(format!("checkpoint-{committed_height}-{seq}"));
        if let Err(e) = std::fs::create_dir_all(&self.work_dir) {
            tracing::warn!(error = %e, dir = %self.work_dir.display(), "Snapshot work dir unavailable");
            return HookOutcome::Failed;
        }
        // RocksDB requires the target not to exist. Only ever a path this
        // producer itself composed, under its own work directory.
        if path.exists() {
            if !path.starts_with(&self.work_dir) {
                tracing::error!(path = %path.display(), "refusing to clear a path outside the work dir");
                return HookOutcome::Failed;
            }
            let _ = std::fs::remove_dir_all(&path);
        }

        let started = Instant::now();
        let result = rocks.create_checkpoint(&path);
        let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.last_checkpoint_micros.store(micros, Ordering::Relaxed);

        match result {
            Ok(()) => {
                *self.pending.lock_or_recover() = Some(path);
                tracing::info!(
                    height = committed_height,
                    under_lock_micros = micros,
                    "Snapshot checkpoint taken; production continues off the commit path"
                );
                HookOutcome::CheckpointTaken
            }
            Err(e) => {
                tracing::warn!(
                    height = committed_height,
                    error = ?e,
                    "Snapshot checkpoint failed; the node is unaffected"
                );
                let _ = std::fs::remove_dir_all(&path);
                HookOutcome::Failed
            }
        }
    }

    /// THE OFF-LOCK STEP. Runs on a background thread (or directly, in tests).
    ///
    /// Opens the pending checkpoint by path, runs the mandatory A0 self-audit,
    /// extracts and chunks the leaf set, and caches the result. Returns `None`
    /// when nothing was pending. The checkpoint directory is removed either
    /// way, so a failure cannot leak disk.
    pub fn run_pending_production(&self) -> Option<Result<u64, ProduceError>> {
        let path = self.pending.lock_or_recover().take()?;
        let started = Instant::now();
        let outcome = build_bundle(&path);
        let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.last_background_micros.store(micros, Ordering::Relaxed);

        if path.starts_with(&self.work_dir) {
            let _ = std::fs::remove_dir_all(&path);
        }

        match outcome {
            Ok(bundle) => {
                let height = bundle.manifest.height;
                let leaves = bundle.manifest.leaf_count;
                let bytes = bundle.payload_bytes();
                *self.cached.lock_or_recover() = Some(Arc::new(bundle));
                // The demand is answered only now, when a bundle exists AND has
                // passed its own audit.
                self.demand.store(false, Ordering::Relaxed);
                tracing::info!(
                    height,
                    leaves,
                    bytes,
                    background_micros = micros,
                    "Snapshot bundle produced and self-audited"
                );
                Some(Ok(height))
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "Snapshot production refused; nothing was cached and nothing will be served"
                );
                Some(Err(e))
            }
        }
    }

    /// The path this producer would checkpoint into next. Test seam.
    #[must_use]
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }
}

fn read_cursor(db: &Storage, key: &[u8]) -> Option<u64> {
    match db.get(key) {
        Ok(Some(b)) if b.len() == 8 => {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b);
            Some(u64::from_be_bytes(a))
        }
        _ => None,
    }
}
