//! Adaptive fee policy.
//!
//! The generator used to send a fee chosen once on the command line. The node
//! does not have a fixed fee. It admits a transaction only if
//! `tx.fee >= max(min_fee, dynamic_fee_floor)`, and the congestion responder
//! raises the dynamic half whenever the mempool comes under pressure. A fixed
//! fee is therefore correct until the first congestion episode and wrong for
//! every submission after it, which on 2026-08-28 read as
//! `novai_mempool_rejects_fee_too_low` at 13,669 and climbing.
//!
//! The floor does not need to be published anywhere new for the generator to
//! find it. The node already states it in the refusal itself:
//!
//! ```text
//! FeeTooLow: minimum 5000, got 1000
//! ```
//!
//! It does not arrive in that shape. Code -32011 has no dedicated arm in
//! `submit_with_retry`, so it falls through to the generic RPC arm and the
//! generator sees the node's sentence wrapped in its own:
//!
//! ```text
//! RPC error -32011: FeeTooLow: minimum 5000, got 1000
//! ```
//!
//! The parser therefore looks for the marker anywhere in the reason rather
//! than at the front of it. Anchoring at the front passes every unit test
//! written against the node's wording and then silently learns nothing in
//! production, which is a worse failure than not having the parser at all.
//!
//! So the loop is closed with no change to the node: offer, read the floor
//! out of the refusal, offer above it. The alternative, probing blindly
//! upward until something sticks, would burn a nonce per probe, and burning
//! nonces against a chain that is not advancing is the thing that made the
//! reconciliation sweep look broken in the first place.
//!
//! Two properties are load bearing:
//!
//! - It converges upward in one step. One refusal is enough to learn the
//!   floor exactly, so the generator does not bounce off it repeatedly.
//! - It comes back down. The floor decays on the node once congestion clears.
//!   A policy that only ratchets up would overpay for the rest of the run,
//!   and a load generator that quietly changes what it is paying is no longer
//!   measuring the thing it was pointed at.

use std::sync::atomic::{AtomicU64, Ordering};

/// The marker the node uses for a fee refusal. Matched anywhere in the
/// reason, because the generator wraps the node's message before this sees
/// it. Matching on this rather than on the whole string keeps the parser tied
/// to one specific rejection.
const FEE_TOO_LOW_MARKER: &str = "FeeTooLow:";

/// Extra paid over the learned floor, as a fraction: floor / this. A margin
/// is not politeness, it is headroom. The floor is a moving target and a
/// submission is in flight for some time before it is judged, so paying it
/// exactly means losing a race with the next congestion step.
const MARGIN_DIVISOR: u64 = 4;

/// Accepted submissions between one relaxation step. Counted in submissions
/// rather than seconds so the decay tracks how much evidence the generator
/// has actually gathered, not how long it has been idle.
const DECAY_AFTER_ACCEPTED: u64 = 256;

/// Fraction of the current premium given back at each relaxation step:
/// current / this. Geometric, so the return to base is quick once the node
/// is calm but never a single cliff back onto a floor that may still be up.
const DECAY_DIVISOR: u64 = 8;

/// Read the effective fee floor out of a node rejection.
///
/// Returns `None` for every rejection that is not about the fee. That
/// exclusion matters: a burst of nonce errors must not be allowed to inflate
/// what every later transaction pays.
pub fn parse_fee_floor(reason: &str) -> Option<u64> {
    let marker = reason.find(FEE_TOO_LOW_MARKER)?;
    let rest = &reason[marker + FEE_TOO_LOW_MARKER.len()..];
    let rest = rest.trim_start().strip_prefix("minimum")?;
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// What the generator currently offers, and how it learns.
///
/// Shared across every worker, so all of them pay the same fee and one
/// worker's refusal teaches all of them.
#[derive(Debug)]
pub struct FeePolicy {
    /// The fee asked for on the command line. The policy never offers less.
    base: u64,
    current: AtomicU64,
    /// Accepted submissions since the last relaxation step.
    clean_run: AtomicU64,
}

impl FeePolicy {
    pub fn new(base: u64) -> Self {
        Self {
            base,
            current: AtomicU64::new(base),
            clean_run: AtomicU64::new(0),
        }
    }

    /// The fee to put on the next transaction.
    pub fn current(&self) -> u64 {
        self.current.load(Ordering::Relaxed)
    }

    /// The configured floor this policy will never go below.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// Fold one rejection into the policy. Anything that is not a fee refusal
    /// leaves the fee exactly where it was.
    pub fn observe_rejection(&self, reason: &str) {
        let Some(floor) = parse_fee_floor(reason) else {
            return;
        };
        let target = floor
            .saturating_add(floor / MARGIN_DIVISOR)
            .saturating_add(1)
            .max(self.base);
        // Only ever raise on a refusal. Two workers can be refused against
        // different floors during a ramp, and the lower of the two is stale
        // the moment it arrives.
        self.current.fetch_max(target, Ordering::Relaxed);
        self.clean_run.store(0, Ordering::Relaxed);
    }

    /// Fold one accepted submission into the policy. A long enough clean run
    /// gives back part of the premium.
    pub fn observe_accepted(&self) {
        let run = self.clean_run.fetch_add(1, Ordering::Relaxed) + 1;
        if run < DECAY_AFTER_ACCEPTED {
            return;
        }
        self.clean_run.store(0, Ordering::Relaxed);

        let current = self.current.load(Ordering::Relaxed);
        if current <= self.base {
            return;
        }
        let relaxed = current
            .saturating_sub((current / DECAY_DIVISOR).max(1))
            .max(self.base);
        self.current.store(relaxed, Ordering::Relaxed);
    }
}
