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
pub const MIN_SAMPLES: u64 = 20;

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

    /// Fold the current window into a new level and start a fresh window.
    /// Returns the new level.
    pub fn sample(&self) -> u32 {
        let accepted = self.accepted.swap(0, Ordering::Relaxed);
        let rejected = self.rejected.swap(0, Ordering::Relaxed);
        let next = next_level(self.level(), accepted, rejected);
        self.level.store(next, Ordering::Relaxed);
        next
    }
}
