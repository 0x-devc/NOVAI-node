//! Disk budget guard for throughput validation runs (gate G0).
//!
//! PURPOSE: compute how long a run may last before it fills the disk, from
//! MEASURED free space, BEFORE the run starts, and refuse anything longer.
//!
//! A run that fills the disk is not a measurement, it is an outage. This
//! module is the difference between the two, and it sits in G0 rather than in
//! G3 because G2 validation at 32 TPS already burns 2.65 MB/s.
//!
//! WHY THE PER-TRANSACTION COST IS SO LARGE. The SMT is 256 levels with no
//! path compression, over blake3-hashed keys that are uniform across the full
//! space, so an update walk writes one node per level with no collapse: 256
//! nodes per key updated. A transfer touches three state keys and the SMT op
//! builder walks once per op rather than once per batch, so 768 nodes at 108
//! bytes each. That 82,944 bytes is PERMANENT. The node store is content
//! addressed, so a changed subtree yields a NEW key and never overwrites its
//! predecessor, and the 50k prune deletes only `consensus/blocks/` and
//! `consensus/qcs/`. Nothing collects the rest. With the account rows and the
//! amortised block body the total is 83,410 bytes per applied transaction, of
//! which 99.4 percent never comes back.
//!
//! The consequence that makes this a guard and not a note: disk burn scales
//! with APPLIED TPS, and the throughput plan exists to raise applied TPS.
//!
//! FAILURE MODES: every one of them refuses. An unreadable filesystem, an
//! unparseable `df`, a non-positive or non-finite rate, and a requested run
//! longer than the budget all return an error rather than a number. A guard
//! that guesses is worse than no guard, because the caller believes it is
//! protected.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// Bytes written per APPLIED transaction, established from source rather than
/// from docs:
///
/// ```text
/// 768 SMT nodes x 108 bytes = 82,944   permanent, never collected
/// account + fee + root rows =    198   overwritten in place
/// block body share          =    268   pruned at h+50,000
///                             -------
///                             83,410
/// ```
///
/// Denominated in APPLIED transactions, not submitted or included ones.
/// Re-inclusions commit as skipped and write nothing, so charging them would
/// overstate the burn by the duplication factor.
pub const BYTES_PER_APPLIED_TX: u64 = 83_410;

/// Fraction of free space a run is allowed to spend. The last tenth is left
/// standing because a full disk is an outage, and because the burn estimate is
/// a model rather than a guarantee.
pub const DISK_HOLDBACK: f64 = 0.9;

/// Seconds a run may last before it spends its share of the free space.
///
/// ```text
/// max_run_seconds = DISK_HOLDBACK x free_bytes / (applied_tps x BYTES_PER_APPLIED_TX)
/// ```
///
/// Returns `None` when `applied_tps` is not a rate this can divide by: zero,
/// negative, NaN or infinite. Those all mean the caller does not know its own
/// load, and the honest answer to "how long may I run at an unknown rate" is
/// not a number.
#[must_use]
pub fn max_run_seconds(free_bytes: u64, applied_tps: f64) -> Option<f64> {
    if !applied_tps.is_finite() || applied_tps <= 0.0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let free = free_bytes as f64;
    #[allow(clippy::cast_precision_loss)]
    let per_second = applied_tps * BYTES_PER_APPLIED_TX as f64;
    Some(DISK_HOLDBACK * free / per_second)
}

/// Why a run was refused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BudgetRefusal {
    /// Free space could not be measured. Fail closed: an unmeasured disk is
    /// exactly the condition under which a run fills one.
    UnmeasurableFreeSpace,
    /// The applied rate was zero, negative, NaN or infinite.
    UnusableRate(f64),
    /// The requested run is longer than the disk can carry.
    ExceedsBudget { requested: f64, max: f64 },
}

/// A granted run budget: a computed ceiling and the deadline it implies.
#[derive(Debug, Clone, Copy)]
pub struct RunBudget {
    max_run_seconds: f64,
    deadline: Instant,
}

impl RunBudget {
    /// Plan a run against measured free space, refusing if it does not fit.
    ///
    /// # Errors
    /// Returns [`BudgetRefusal`] if the rate is unusable or the requested run
    /// exceeds what the disk can carry.
    pub fn plan(
        free_bytes: u64,
        applied_tps: f64,
        requested_seconds: f64,
    ) -> Result<Self, BudgetRefusal> {
        Self::plan_at(free_bytes, applied_tps, requested_seconds, Instant::now())
    }

    /// `plan` against a free-space reading that may have failed.
    ///
    /// # Errors
    /// Returns [`BudgetRefusal::UnmeasurableFreeSpace`] when `free_bytes` is
    /// `None`, plus everything [`RunBudget::plan`] can return.
    pub fn plan_measured(
        free_bytes: Option<u64>,
        applied_tps: f64,
        requested_seconds: f64,
    ) -> Result<Self, BudgetRefusal> {
        let free = free_bytes.ok_or(BudgetRefusal::UnmeasurableFreeSpace)?;
        Self::plan(free, applied_tps, requested_seconds)
    }

    /// `plan` with an explicit start instant, so tests are deterministic.
    ///
    /// # Errors
    /// See [`RunBudget::plan`].
    pub fn plan_at(
        free_bytes: u64,
        applied_tps: f64,
        requested_seconds: f64,
        start: Instant,
    ) -> Result<Self, BudgetRefusal> {
        let max = max_run_seconds(free_bytes, applied_tps)
            .ok_or(BudgetRefusal::UnusableRate(applied_tps))?;
        if requested_seconds > max {
            return Err(BudgetRefusal::ExceedsBudget {
                requested: requested_seconds,
                max,
            });
        }
        Ok(Self {
            max_run_seconds: max,
            deadline: start + Duration::from_secs_f64(max),
        })
    }

    /// The computed ceiling in seconds.
    #[must_use]
    pub fn max_run_seconds(&self) -> f64 {
        self.max_run_seconds
    }

    /// Whether the run must stop now.
    ///
    /// The deadline is the COMPUTED budget, not the requested length, so a
    /// caller that overruns its own request is still stopped by the disk.
    #[must_use]
    pub fn expired_at(&self, now: Instant) -> bool {
        now >= self.deadline
    }

    /// `expired_at` against the current clock.
    #[must_use]
    pub fn expired(&self) -> bool {
        self.expired_at(Instant::now())
    }
}

/// Extract the Available column, in bytes, from POSIX `df -Pk` output.
///
/// `-P` guarantees one line per filesystem and a fixed six-column layout, so
/// the fourth field is Available in 1024-byte blocks on both Linux and macOS.
///
/// Returns `None` for anything that is not unambiguously that, including an
/// error message on stdout, a header with no data row, a short row, and a
/// non-numeric column.
#[must_use]
pub fn parse_df_available_bytes(df_output: &str) -> Option<u64> {
    let row = df_output.lines().nth(1)?;
    let fields: Vec<&str> = row.split_whitespace().collect();
    // Filesystem, 1024-blocks, Used, Available, Capacity, Mounted on.
    if fields.len() < 6 {
        return None;
    }
    fields[3].parse::<u64>().ok()?.checked_mul(1024)
}

/// Free bytes on the filesystem holding `path`, or `None` if it cannot be
/// measured.
///
/// Shells out to `df -Pk` rather than calling `statvfs`. There is no stable
/// std API for this, the workspace forbids `unsafe`, and the alternative is a
/// new dependency for one number read once per run. POSIX `-P` output is
/// stable across Linux and macOS, and every parse failure fails closed, so the
/// worst case is a refused run rather than a wrong budget.
#[must_use]
pub fn free_bytes(path: &Path) -> Option<u64> {
    let output = Command::new("df").arg("-Pk").arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_df_available_bytes(&String::from_utf8_lossy(&output.stdout))
}
