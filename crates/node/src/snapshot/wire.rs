//! Gate F5 Stage 4: the serving side's rate limit and the requesting side's
//! per-peer strike ladder.
//!
//! Both are pure: the clock is passed in, so they unit-test without sleeping,
//! in the same spirit as `sync_backoff_ms` and `sync_retry_due`.

use std::collections::HashMap;

use novai_types::Address;

/// Minimum spacing between chunk serves to a single peer.
///
/// Producing is already demand-gated and cached, so serving is a memory read
/// and a send. The limit exists so one peer cannot pull a node's whole outbound
/// budget, and so a peer that requests in a tight loop is throttled rather than
/// answered. Twenty chunks a second per peer moves a few-megabyte snapshot in
/// well under a minute, which is far inside the retention budget.
pub const SERVE_MIN_INTERVAL_MICROS: u64 = 50_000;

/// Consecutive bad answers before a peer stops being asked.
///
/// A "bad answer" is a chunk whose bytes do not match the digest the (already
/// quorum-verified) manifest claims for it. That is not a difference of
/// opinion: the manifest is trusted because a quorum signed the header it
/// commits to, so a mismatching chunk means the peer is broken or hostile.
/// Three strikes rather than one, because a single mismatch can be a truncated
/// transfer.
pub const PEER_STRIKE_LIMIT: u32 = 3;

/// Per-peer serve spacing. Not a token bucket on purpose: a bucket lets a peer
/// bank idle time and then burst, which is exactly the shape this is meant to
/// prevent.
#[derive(Debug, Default)]
pub struct ServeLimiter {
    last_served_micros: HashMap<Address, u64>,
}

impl ServeLimiter {
    /// May `peer` be served now? Records the serve when it returns true, so a
    /// caller cannot accidentally check twice and serve twice.
    pub fn allow(&mut self, peer: Address, now_micros: u64) -> bool {
        match self.last_served_micros.get(&peer) {
            Some(&last) if now_micros.saturating_sub(last) < SERVE_MIN_INTERVAL_MICROS => false,
            _ => {
                self.last_served_micros.insert(peer, now_micros);
                true
            }
        }
    }

    /// Forget peers not seen within `retain_micros` of now, so a long-lived
    /// node does not accumulate an entry per address it ever served.
    pub fn prune(&mut self, now_micros: u64, retain_micros: u64) {
        self.last_served_micros
            .retain(|_, &mut last| now_micros.saturating_sub(last) <= retain_micros);
    }

    #[must_use]
    pub fn tracked_peers(&self) -> usize {
        self.last_served_micros.len()
    }
}

/// Per-peer strike ladder for the requesting side.
#[derive(Debug, Default)]
pub struct PeerStrikes {
    strikes: HashMap<Address, u32>,
}

impl PeerStrikes {
    /// Record one bad answer. Returns the new count.
    pub fn strike(&mut self, peer: Address) -> u32 {
        let e = self.strikes.entry(peer).or_insert(0);
        *e = e.saturating_add(1);
        *e
    }

    /// Has this peer earned its way out of the rotation?
    #[must_use]
    pub fn is_shunned(&self, peer: &Address) -> bool {
        self.strikes.get(peer).is_some_and(|&n| n >= PEER_STRIKE_LIMIT)
    }

    /// A good answer clears the ladder: strikes must be CONSECUTIVE, or a peer
    /// that is fine most of the time would eventually be shunned for nothing.
    pub fn clear(&mut self, peer: &Address) {
        self.strikes.remove(peer);
    }

    #[must_use]
    pub fn count(&self, peer: &Address) -> u32 {
        self.strikes.get(peer).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Address = [0xAA; 32];
    const B: Address = [0xBB; 32];

    #[test]
    fn the_first_serve_to_a_peer_is_always_allowed() {
        let mut l = ServeLimiter::default();
        assert!(l.allow(A, 0));
    }

    #[test]
    fn a_second_serve_inside_the_interval_is_refused() {
        let mut l = ServeLimiter::default();
        assert!(l.allow(A, 1_000_000));
        assert!(!l.allow(A, 1_000_000 + SERVE_MIN_INTERVAL_MICROS - 1));
        assert!(l.allow(A, 1_000_000 + SERVE_MIN_INTERVAL_MICROS));
    }

    #[test]
    fn one_greedy_peer_does_not_throttle_another() {
        let mut l = ServeLimiter::default();
        assert!(l.allow(A, 0));
        assert!(!l.allow(A, 1));
        assert!(l.allow(B, 1), "B has its own budget");
    }

    #[test]
    fn a_refused_serve_does_not_extend_the_window() {
        // If a refusal stamped the clock, a peer requesting in a tight loop
        // would push its own next allowed serve out forever.
        let mut l = ServeLimiter::default();
        assert!(l.allow(A, 0));
        for t in 1..SERVE_MIN_INTERVAL_MICROS {
            assert!(!l.allow(A, t));
        }
        assert!(l.allow(A, SERVE_MIN_INTERVAL_MICROS));
    }

    #[test]
    fn pruning_forgets_idle_peers() {
        let mut l = ServeLimiter::default();
        l.allow(A, 0);
        l.allow(B, 10_000_000);
        l.prune(10_000_000, 1_000_000);
        assert_eq!(l.tracked_peers(), 1, "A is long idle and is forgotten");
        assert!(l.allow(A, 10_000_001), "and is served again as a newcomer");
    }

    #[test]
    fn strikes_accumulate_to_the_limit_then_shun() {
        let mut s = PeerStrikes::default();
        assert!(!s.is_shunned(&A));
        for n in 1..PEER_STRIKE_LIMIT {
            s.strike(A);
            assert!(!s.is_shunned(&A), "shunned too early at {n}");
        }
        s.strike(A);
        assert!(s.is_shunned(&A));
    }

    #[test]
    fn a_good_answer_clears_the_ladder_so_strikes_are_consecutive() {
        let mut s = PeerStrikes::default();
        s.strike(A);
        s.strike(A);
        s.clear(&A);
        assert_eq!(s.count(&A), 0);
        s.strike(A);
        assert!(
            !s.is_shunned(&A),
            "a peer that answers correctly in between must not be shunned by a tally"
        );
    }

    #[test]
    fn strikes_are_per_peer() {
        let mut s = PeerStrikes::default();
        for _ in 0..PEER_STRIKE_LIMIT {
            s.strike(A);
        }
        assert!(s.is_shunned(&A));
        assert!(!s.is_shunned(&B));
    }
}
