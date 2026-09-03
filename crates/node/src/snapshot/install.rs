//! Gate F5 Stage 3: staging a snapshot and installing it at boot.
//!
//! This is the only module in the gate that mutates state a node boots from,
//! so its shape is dictated entirely by what happens if the process dies
//! halfway.
//!
//! THE COMMIT POINT IS ONE RENAME. Nothing touches the live directory until a
//! single `rename`. Before it, the live directory is exactly as it was and the
//! staging directory is disposable. After it, the install is done. There is no
//! window in which a node can boot from a partially written database.
//!
//! NOTHING IS EVER DELETED. Every directory this module sets aside is renamed:
//! `.preinstall-*` for the replaced state, `.staging-rejected-*` for a staging
//! directory that failed its audit, `.staging-abandoned-*` for one that was
//! never marked ready. Reclaiming them is an operator decision, taken with the
//! node's history in hand, never a side effect of a boot.
//!
//! THE MARKS ARE MERGED, NOT REPLACED. `KEY_VOTED_VIEW` and `KEY_LOCKED_QC`
//! are taken as `max(own, donor)`. `may_vote` is a strict lexicographic gate,
//! so a higher mark can only ever FORBID votes; there is no path by which a
//! foreign mark permits one. Merging at boot rather than at stage time is what
//! makes it sound: a node can vote between staging and restarting, and the
//! boot merge sees those votes. That is what makes the rollback-equivocation
//! hazard structurally impossible rather than procedurally avoided.
//!
//! THE AUDIT RUNS AGAIN HERE. A receive-time PASS is not trusted. The full A0
//! audit runs at boot, after the merge, against the exact bytes about to be
//! installed, immediately before the irreversible rename.

use std::path::{Path, PathBuf};

use novai_consensus_types::codec::{decode_qc_v1, decode_voted_view_v1, encode_voted_view_v1};
use novai_consensus_types::QC;
use novai_state::{
    Kv, KvBatch, RocksKv, WriteOp, KEY_COMMITTED_HEIGHT, KEY_LOCKED_QC, KEY_VOTED_VIEW,
};

use crate::snapshot::audit;
use crate::snapshot::bundle::SnapshotBundle;
use crate::snapshot::stage::{materialize, StageError};

/// Marker written last inside a staging directory. Its presence is the commit
/// point for "this staging directory is complete"; the boot path does nothing
/// with a staging directory that lacks it.
pub const INSTALL_READY: &str = "INSTALL_READY";

/// Suffix of the staging directory, beside the live database directory.
pub const STAGING_SUFFIX: &str = ".snapshot-staging";

#[derive(Debug)]
pub enum InstallError {
    Io(String),
    Stage(StageError),
    /// The ready marker disagrees with the staging database it labels.
    MarkerMismatch(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "snapshot install io: {e}"),
            Self::Stage(e) => write!(f, "snapshot install: {e}"),
            Self::MarkerMismatch(e) => write!(f, "snapshot install: {e}"),
        }
    }
}

/// What the boot path did.
#[derive(Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    /// No staging directory carried a ready marker. The overwhelmingly common
    /// case: an ordinary boot.
    Nothing,
    /// A staging directory existed without a ready marker, so it was an
    /// interrupted stage. Set aside, never used, never deleted.
    AbandonedIncomplete(PathBuf),
    /// The staging directory failed its boot audit. Set aside; the node boots
    /// normally on the directory it already had.
    RejectedByAudit { moved_to: PathBuf, result: String },
    /// Installed. The node will boot from the snapshot.
    Installed { height: u64, root_hex: String },
}

/// The node's own durable anti-equivocation marks, read from a database.
#[derive(Debug, Default, Clone)]
pub struct OwnMarks {
    pub voted_view: Option<(u64, u64)>,
    pub locked_qc: Option<QC>,
}

/// Read the marks a node must never regress.
///
/// # Errors
/// Returns an IO error if the database cannot be opened. An unreadable or
/// undecodable mark is treated as ABSENT rather than as an error, because the
/// merge below only ever raises: a mark that cannot be read cannot be
/// preserved, and failing the whole install over it would be worse than
/// carrying the donor's, which the height ordering already makes safe.
pub fn read_own_marks(db_dir: &Path) -> Result<OwnMarks, InstallError> {
    let db = RocksKv::open(db_dir).map_err(|e| InstallError::Io(format!("open {}: {e:?}", db_dir.display())))?;
    let voted_view = db
        .get(KEY_VOTED_VIEW)
        .ok()
        .flatten()
        .and_then(|b| decode_voted_view_v1(&b).ok());
    let locked_qc = db
        .get(KEY_LOCKED_QC)
        .ok()
        .flatten()
        .and_then(|b| decode_qc_v1(&b).ok());
    Ok(OwnMarks {
        voted_view,
        locked_qc,
    })
}

/// The higher of two vote high-water marks under the lexicographic
/// `(height, round)` order `may_vote` uses.
#[must_use]
pub fn max_voted_view(a: Option<(u64, u64)>, b: Option<(u64, u64)>) -> Option<(u64, u64)> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if x >= y { x } else { y }),
        (Some(x), None) => Some(x),
        (None, y) => y,
    }
}

/// The higher of two locks by `(height, round)`.
///
/// A wedged node's `locked_qc` can sit far ABOVE a fresh snapshot's, because QC
/// adoption is deliberately ungated by the commit window. Installing the
/// donor's alone would REGRESS the lock, and a regressed lock plus a
/// same-height higher-round proposal is a case the vote mark alone does not
/// cover.
#[must_use]
pub fn max_locked_qc(a: Option<QC>, b: Option<QC>) -> Option<QC> {
    match (a, b) {
        (Some(x), Some(y)) => {
            if (x.height, x.round) >= (y.height, y.round) {
                Some(x)
            } else {
                Some(y)
            }
        }
        (Some(x), None) => Some(x),
        (None, y) => y,
    }
}

/// Write `max(own, donor)` for both marks into a staging directory.
///
/// Idempotent: `max` applied twice is `max`. That is what lets the boot path
/// re-run after a crash without reasoning about how far it got.
///
/// # Errors
/// Returns an IO error if the staging database cannot be opened or written.
pub fn merge_marks_into_staging(staging: &Path, own: &OwnMarks) -> Result<(), InstallError> {
    let mut db = RocksKv::open(staging)
        .map_err(|e| InstallError::Io(format!("open staging {}: {e:?}", staging.display())))?;
    let donor = OwnMarks {
        voted_view: db
            .get(KEY_VOTED_VIEW)
            .ok()
            .flatten()
            .and_then(|b| decode_voted_view_v1(&b).ok()),
        locked_qc: db
            .get(KEY_LOCKED_QC)
            .ok()
            .flatten()
            .and_then(|b| decode_qc_v1(&b).ok()),
    };

    let mut ops = Vec::new();
    if let Some((h, r)) = max_voted_view(own.voted_view, donor.voted_view) {
        ops.push(WriteOp::Put(
            KEY_VOTED_VIEW.to_vec(),
            encode_voted_view_v1(h, r),
        ));
    }
    if let Some(qc) = max_locked_qc(own.locked_qc.clone(), donor.locked_qc) {
        let bytes = novai_consensus_types::codec::encode_qc_v1(&qc)
            .map_err(|e| InstallError::Io(format!("encode merged lock: {e:?}")))?;
        ops.push(WriteOp::Put(KEY_LOCKED_QC.to_vec(), bytes));
    }
    if ops.is_empty() {
        return Ok(());
    }
    // One batch: the two marks move together or not at all.
    db.apply_batch(&ops)
        .map_err(|e| InstallError::Io(format!("write merged marks: {e:?}")))
}

/// Build the staging directory for a bundle and mark it ready.
///
/// Runtime side. Writes nothing outside `{base}/{db_subdir}.snapshot-staging`
/// and never touches the live directory. The marks are merged again at boot,
/// so a node that votes between staging and restarting is still safe.
///
/// # Errors
/// Propagates materialisation and IO failures. On any failure the staging
/// directory is left WITHOUT a ready marker, so the boot path will set it aside
/// rather than install it.
pub fn stage_bundle(base: &Path, db_subdir: &str, bundle: &SnapshotBundle) -> Result<PathBuf, InstallError> {
    let staging = staging_path(base, db_subdir);
    if staging.exists() {
        let aside = free_path(base, &format!("{db_subdir}.staging-abandoned"));
        std::fs::rename(&staging, &aside)
            .map_err(|e| InstallError::Io(format!("set aside a previous staging dir: {e}")))?;
    }
    materialize(bundle, &staging).map_err(InstallError::Stage)?;

    // The marker is written LAST and names what it labels, so a boot can tell a
    // complete staging directory from an interrupted one and can cross-check
    // the marker against the database it sits in.
    let marker = format!(
        "version={}\nheight={}\nroot={}\n",
        bundle.manifest.version,
        bundle.manifest.height,
        hex::encode(bundle.manifest.state_root)
    );
    let marker_path = staging.join(INSTALL_READY);
    std::fs::write(&marker_path, marker.as_bytes())
        .map_err(|e| InstallError::Io(format!("write ready marker: {e}")))?;
    fsync_dir(&staging);
    Ok(staging)
}

/// THE BOOT PATH. Call before any database is opened, single threaded.
///
/// # Errors
/// Returns an IO error only when the filesystem refuses a rename. Every other
/// failure is an outcome, not an error: a failed audit sets the staging
/// directory aside and lets the node boot normally.
pub fn complete_install_at_boot(base: &Path, db_subdir: &str) -> Result<InstallOutcome, InstallError> {
    let staging = staging_path(base, db_subdir);
    let live = base.join(db_subdir);
    let ready = staging.join(INSTALL_READY);

    if !ready.exists() {
        if staging.exists() {
            // Staged but never marked ready: an interrupted stage. Disposable,
            // but set aside rather than removed.
            let aside = free_path(base, &format!("{db_subdir}.staging-abandoned"));
            std::fs::rename(&staging, &aside)
                .map_err(|e| InstallError::Io(format!("set aside incomplete staging: {e}")))?;
            tracing::warn!(moved_to = %aside.display(), "Incomplete snapshot staging directory set aside");
            return Ok(InstallOutcome::AbandonedIncomplete(aside));
        }
        return Ok(InstallOutcome::Nothing);
    }

    // 1. Merge this node's own anti-equivocation marks. Skipped when the live
    //    directory is absent, which is the crash-between-the-renames case: the
    //    merge already ran in the attempt that moved it aside.
    if live.exists() {
        let own = read_own_marks(&live)?;
        merge_marks_into_staging(&staging, &own)?;
        tracing::info!(
            voted_view = ?own.voted_view,
            locked_qc_height = ?own.locked_qc.as_ref().map(|q| q.height),
            "Merged this node's own vote and lock marks into the staged snapshot"
        );
    }

    // 2. G3. The full audit, against the exact bytes about to be installed,
    //    after the merge and immediately before the irreversible rename. A
    //    receive-time PASS is not trusted and is not reused.
    let staging_str = staging
        .to_str()
        .ok_or_else(|| InstallError::Io("staging path is not valid utf8".to_string()))?;
    let report = audit::audit(staging_str, None).map_err(InstallError::Io)?;
    let marker_height = read_marker_height(&ready);
    let height_agrees = match (marker_height, report.height) {
        (Some(m), Some(r)) => m == r,
        _ => false,
    };
    if !report.ok || !height_agrees {
        let aside = free_path(base, &format!("{db_subdir}.staging-rejected"));
        std::fs::rename(&staging, &aside)
            .map_err(|e| InstallError::Io(format!("set aside rejected staging: {e}")))?;
        let result = if height_agrees {
            report.result_line()
        } else {
            format!(
                "{} (ready marker claims height {marker_height:?}, database says {:?})",
                report.result_line(),
                report.height
            )
        };
        tracing::error!(
            moved_to = %aside.display(),
            result = %result,
            "Staged snapshot FAILED its boot audit; booting on the existing data directory"
        );
        return Ok(InstallOutcome::RejectedByAudit {
            moved_to: aside,
            result,
        });
    }

    let height = report.height.expect("a passing audit carries a height");
    let root_hex = hex::encode(report.root.expect("a passing audit carries a root"));

    // 3. Set the live directory aside. Skipped when it is already absent,
    //    which is exactly the crash-between-the-renames case.
    if live.exists() {
        let old_height = read_committed_height(&live).unwrap_or(0);
        let aside = free_path(base, &format!("{db_subdir}.preinstall-{old_height}"));
        std::fs::rename(&live, &aside)
            .map_err(|e| InstallError::Io(format!("set the live directory aside: {e}")))?;
        tracing::warn!(moved_to = %aside.display(), "Previous data directory preserved");
    }

    // 4. THE COMMIT POINT. One rename.
    std::fs::rename(&staging, &live)
        .map_err(|e| InstallError::Io(format!("move the staged snapshot into place: {e}")))?;
    fsync_dir(base);

    // The marker has served its purpose and must not survive into the live
    // directory, or the next boot would try to install the node's own database
    // into itself. Renamed, like everything else here.
    let installed_marker = live.join(INSTALL_READY);
    if installed_marker.exists() {
        let _ = std::fs::rename(&installed_marker, live.join("INSTALL_COMPLETED"));
    }

    tracing::warn!(height, root = %root_hex, "SNAPSHOT INSTALLED; booting from installed state");
    Ok(InstallOutcome::Installed { height, root_hex })
}

#[must_use]
pub fn staging_path(base: &Path, db_subdir: &str) -> PathBuf {
    base.join(format!("{db_subdir}{STAGING_SUFFIX}"))
}

/// The first unused `{stem}` / `{stem}-2` / `{stem}-3` path under `base`.
/// Never returns a path that exists, so a rename can never clobber a preserved
/// directory.
///
/// Shared with the SMT GC reclaim tool rather than reimplemented there: that
/// tool sets aside directories under the same never-delete rule, and two
/// implementations of "find a name that cannot clobber a preserved directory"
/// is two places the never-delete rule can be broken.
pub(crate) fn free_path(base: &Path, stem: &str) -> PathBuf {
    let first = base.join(stem);
    if !first.exists() {
        return first;
    }
    for n in 2..10_000u32 {
        let p = base.join(format!("{stem}-{n}"));
        if !p.exists() {
            return p;
        }
    }
    base.join(format!("{stem}-overflow"))
}

fn read_marker_height(marker: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(marker).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("height="))
        .and_then(|v| v.trim().parse::<u64>().ok())
}

fn read_committed_height(db_dir: &Path) -> Option<u64> {
    let db = RocksKv::open(db_dir).ok()?;
    match db.get(KEY_COMMITTED_HEIGHT) {
        Ok(Some(b)) if b.len() == 8 => {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b);
            Some(u64::from_be_bytes(a))
        }
        _ => None,
    }
}

/// Best effort directory fsync, so a rename is durable before the node starts
/// writing chain state through it. A failure is logged, never fatal: the
/// rename itself is already atomic within the filesystem.
pub(crate) fn fsync_dir(dir: &Path) {
    match std::fs::File::open(dir) {
        Ok(f) => {
            if let Err(e) = f.sync_all() {
                tracing::warn!(dir = %dir.display(), error = %e, "Directory fsync failed");
            }
        }
        Err(e) => tracing::warn!(dir = %dir.display(), error = %e, "Directory fsync open failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qc_at(height: u64, round: u64) -> QC {
        QC {
            height,
            round,
            block_hash: [0x11; 32],
            votes: vec![],
        }
    }

    #[test]
    fn max_voted_view_never_returns_lower_than_either_side() {
        assert_eq!(max_voted_view(Some((9, 1)), Some((5, 7))), Some((9, 1)));
        assert_eq!(max_voted_view(Some((5, 7)), Some((9, 1))), Some((9, 1)));
        assert_eq!(max_voted_view(Some((5, 2)), Some((5, 7))), Some((5, 7)));
        assert_eq!(max_voted_view(Some((5, 7)), None), Some((5, 7)));
        assert_eq!(max_voted_view(None, Some((5, 7))), Some((5, 7)));
        assert_eq!(max_voted_view(None, None), None);
    }

    #[test]
    fn max_voted_view_is_idempotent_so_a_retried_boot_is_safe() {
        let once = max_voted_view(Some((9, 1)), Some((5, 7)));
        let twice = max_voted_view(once, Some((5, 7)));
        assert_eq!(once, twice);
    }

    #[test]
    fn max_locked_qc_orders_by_height_then_round() {
        assert_eq!(
            max_locked_qc(Some(qc_at(9, 0)), Some(qc_at(5, 9))).map(|q| (q.height, q.round)),
            Some((9, 0))
        );
        assert_eq!(
            max_locked_qc(Some(qc_at(5, 1)), Some(qc_at(5, 4))).map(|q| (q.height, q.round)),
            Some((5, 4))
        );
        assert!(max_locked_qc(None, None).is_none());
        assert_eq!(
            max_locked_qc(Some(qc_at(3, 0)), None).map(|q| q.height),
            Some(3)
        );
    }
}
