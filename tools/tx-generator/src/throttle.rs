//! Adaptive submission throttle.
//!
//! Nothing in the generator reads its own rejection rate. When the node
//! starts refusing traffic, the generator keeps offering at full rate, which
//! sustains the pressure that caused the refusals and, before the phase 1
//! fix, was what broke its own nonce resync.
//!
//! This is a hysteresis controller, not a proportional one, and the choice is
//! deliberate. A controller with a single threshold flaps: the moment it slows
//! down the rejection rate falls below the threshold, so it speeds up, so the
//! rate rises again. The dead band between `LOOSEN_BELOW` and `TIGHTEN_ABOVE`
//! is what makes a steady input produce a steady output.
//!
//! Two properties are load bearing and both are pinned:
//!
//! - It cannot oscillate on a steady signal. Any rejection ratio inside the
//!   dead band leaves the level exactly where it is.
//! - It cannot starve. The level is capped, so the slowdown is bounded and
//!   the generator never stops; and any quiet window steps it back down, so
//!   it always returns to full speed once the node recovers.
//! - It cannot latch. This one was learned the hard way. The controller sits
//!   inside a loop: it decides how fast the generator sends, and how fast the
//!   generator sends decides how many outcomes land in the next window. A
//!   fixed sample floor against a fixed wall clock window therefore asks for
//!   evidence that the controller has itself forbidden. On 2026-08-28 the
//!   throttle reached level 3 and stayed there for sixteen hours against a
//!   healthy node, because at one eighth speed a ten second window could not
//!   hold the twenty outcomes that de-escalation required. A request for 3
//!   TPS delivered 0.375.
//!
//! The fix is to stop treating a window as a fixed unit of time. A window
//! that is too thin to judge is carried into the next one instead of being
//! thrown away, so the controller always decides on a full sample regardless
//! of how slow it has made the generator. A run of thin windows with nothing
//! refused in them is itself the evidence that the node is fine, and steps
//! the level down on its own.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Rejection ratio strictly above which the throttle tightens one step.
pub const TIGHTEN_ABOVE: f64 = 0.50;

/// Rejection ratio strictly below which the throttle loosens one step.
/// The gap up to `TIGHTEN_ABOVE` is the dead band.
pub const LOOSEN_BELOW: f64 = 0.20;

/// Highest throttle level. The delay multiplier is `2^level`, so the slowest
/// the generator can ever run is 32 times its configured interval. Bounded on
/// purpose: a soak that silently throttles itself to nothing looks identical
/// to a soak that died.
pub const MAX_LEVEL: u32 = 5;

/// A window with fewer outcomes than this is too sparse to act on. Without
/// it, one rejection in a quiet window reads as a 100 percent rejection rate.
///
/// This is a floor on evidence, not on time. Outcomes accumulate across
/// windows until the floor is met, so a throttled generator still gets to a
/// full sample; it just takes more windows to do it.
pub const MIN_SAMPLES: u64 = 20;

/// How many consecutive windows may be too thin to judge before a clean run
/// of them is itself treated as evidence that the node is healthy.
///
/// This is the anti-latch escape and it only opens when nothing at all was
/// refused. Thin evidence of trouble is never enough to tighten on, but thin
/// evidence repeated with zero refusals is a node that is not refusing
/// anything, and staying slow in front of it is the bug this closes.
pub const MAX_SPARSE_WINDOWS: u32 = 6;

/// Decide the next throttle level from one window of outcomes.
///
/// Pure so the controller's behaviour can be pinned without timing or a
/// network.
pub fn next_level(current: u32, accepted: u64, rejected: u64) -> u32 {
    let total = accepted + rejected;
    if total < MIN_SAMPLES {
        return current;
    }

    let ratio = rejected as f64 / total as f64;
    if ratio > TIGHTEN_ABOVE {
        current.saturating_add(1).min(MAX_LEVEL)
    } else if ratio < LOOSEN_BELOW {
        current.saturating_sub(1)
    } else {
        // Inside the dead band: hold. This is the whole anti-oscillation
        // mechanism, so it is deliberately the widest branch.
        current
    }
}

/// Shared throttle state. Workers record outcomes, a sampler folds each
/// window into a level, and the generator reads the level to pace itself.
#[derive(Debug, Default)]
pub struct Throttle {
    accepted: AtomicU64,
    rejected: AtomicU64,
    level: AtomicU32,
    /// Consecutive windows that were too thin to judge. Reset by any window
    /// that reaches `MIN_SAMPLES` and by the escape itself.
    sparse_windows: AtomicU32,
}

impl Throttle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_accepted(&self) {
        self.accepted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_rejected(&self) {
        self.rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub fn level(&self) -> u32 {
        self.level.load(Ordering::Relaxed)
    }

    /// How many times the configured interval the generator should wait.
    /// Always at least 1, never more than `2^MAX_LEVEL`.
    pub fn delay_multiplier(&self) -> u32 {
        1u32 << self.level()
    }

    /// Fold the current window into a new level. Returns the new level.
    ///
    /// A window that holds enough outcomes to judge is judged and cleared. A
    /// window that does not is kept, so the next window adds to it rather
    /// than starting over. That is what stops the controller latching: at
    /// level 3 a ten second window holds about four outcomes, and demanding
    /// twenty of them per window is demanding a send rate that level 3 has
    /// ruled out. Six such windows hold twenty between them.
    ///
    /// The escape at the bottom is for the case where even that is not
    /// enough. If several windows in a row were too thin AND not one
    /// submission was refused in any of them, the node is plainly not
    /// refusing anything, and the level steps down on that.
    pub fn sample(&self) -> u32 {
        let accepted = self.accepted.load(Ordering::Relaxed);
        let rejected = self.rejected.load(Ordering::Relaxed);
        let current = self.level();

        if accepted + rejected >= MIN_SAMPLES {
            self.reset_window();
            let next = next_level(current, accepted, rejected);
            self.level.store(next, Ordering::Relaxed);
            return next;
        }

        let held = self.sparse_windows.fetch_add(1, Ordering::Relaxed) + 1;
        if rejected == 0 && held >= MAX_SPARSE_WINDOWS {
            let next = current.saturating_sub(1);
            self.reset_window();
            self.level.store(next, Ordering::Relaxed);
            return next;
        }

        // Not enough to judge and not yet quiet for long enough. Hold the
        // level and keep the outcomes: they count toward the next window.
        current
    }

    /// Clear the window and the sparse run together. They are always reset as
    /// a pair: a carried window and a carried count describe the same window.
    fn reset_window(&self) {
        self.accepted.store(0, Ordering::Relaxed);
        self.rejected.store(0, Ordering::Relaxed);
        self.sparse_windows.store(0, Ordering::Relaxed);
    }
}
