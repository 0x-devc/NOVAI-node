//! PURPOSE: Thread-safe per-address threat scores for transaction deprioritization.
//!
//! INVARIANTS:
//! - Scores are clamped to [0, 100]
//! - `BTreeMap` is used for deterministic iteration in `snapshot()`
//! - Transactions are NEVER rejected based on threat score, only reordered
//! - Scores decay over time — entries at 0 are removed
//!
//! FAILURE MODES:
//! - Mutex poisoning is handled by callers (lock_or_recover pattern)

#![forbid(unsafe_code)]

use novai_types::Address;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Maximum threat score value (u8).
pub const MAX_THREAT_SCORE: u8 = 100;

/// Default decay amount per cycle.
pub const DEFAULT_DECAY_AMOUNT: u8 = 5;

/// Thread-safe map of per-address threat scores.
///
/// Uses `BTreeMap` for deterministic iteration order in `snapshot()`.
/// Protected by `Mutex` for concurrent access from copilot and mempool threads.
pub struct ThreatScoreMap {
    inner: Mutex<BTreeMap<Address, u8>>,
}

impl ThreatScoreMap {
    /// Create an empty threat score map.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BTreeMap::new()),
        }
    }

    /// Set threat score for an address. Clamped to `[0, MAX_THREAT_SCORE]`.
    pub fn set(&self, addr: Address, score: u8) {
        let clamped = score.min(MAX_THREAT_SCORE);
        let mut map = self.inner.lock().expect("threat score lock");
        if clamped == 0 {
            map.remove(&addr);
        } else {
            map.insert(addr, clamped);
        }
    }

    /// Get threat score for an address. Returns 0 if absent.
    pub fn get(&self, addr: &Address) -> u8 {
        let map = self.inner.lock().expect("threat score lock");
        map.get(addr).copied().unwrap_or(0)
    }

    /// Remove score for an address.
    pub fn remove(&self, addr: &Address) {
        let mut map = self.inner.lock().expect("threat score lock");
        map.remove(addr);
    }

    /// Decay all scores by `amount`. Entries that reach 0 are removed.
    pub fn decay(&self, amount: u8) {
        let mut map = self.inner.lock().expect("threat score lock");
        map.retain(|_, score| {
            *score = score.saturating_sub(amount);
            *score > 0
        });
    }

    /// Return a deterministic snapshot (BTreeMap order) of all scores.
    pub fn snapshot(&self) -> BTreeMap<Address, u8> {
        let map = self.inner.lock().expect("threat score lock");
        map.clone()
    }

    /// Number of tracked addresses.
    pub fn len(&self) -> usize {
        let map = self.inner.lock().expect("threat score lock");
        map.len()
    }

    /// True if no addresses have threat scores.
    pub fn is_empty(&self) -> bool {
        let map = self.inner.lock().expect("threat score lock");
        map.is_empty()
    }
}

impl Default for ThreatScoreMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute effective fee after threat score reduction.
///
/// `effective_fee = fee * (100 - min(score, 100)) / 100`
///
/// - Score 0 → full fee
/// - Score 50 → half fee
/// - Score 100 → effective fee = 0 (transaction still included, just lowest priority)
///
/// All arithmetic is integer-only (u64).
pub fn effective_fee(fee: u64, score: u8) -> u64 {
    let s = (score.min(MAX_THREAT_SCORE)) as u64;
    fee * (100 - s) / 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_map_is_empty() {
        let map = ThreatScoreMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn set_and_get() {
        let map = ThreatScoreMap::new();
        let addr = [0x01u8; 32];
        map.set(addr, 42);
        assert_eq!(map.get(&addr), 42);
    }

    #[test]
    fn absent_returns_zero() {
        let map = ThreatScoreMap::new();
        let addr = [0xFFu8; 32];
        assert_eq!(map.get(&addr), 0);
    }

    #[test]
    fn clamped_to_100() {
        let map = ThreatScoreMap::new();
        let addr = [0x02u8; 32];
        map.set(addr, 200);
        assert_eq!(map.get(&addr), 100);
    }

    #[test]
    fn set_zero_removes_entry() {
        let map = ThreatScoreMap::new();
        let addr = [0x03u8; 32];
        map.set(addr, 50);
        assert_eq!(map.len(), 1);
        map.set(addr, 0);
        assert_eq!(map.len(), 0);
        assert_eq!(map.get(&addr), 0);
    }

    #[test]
    fn remove_works() {
        let map = ThreatScoreMap::new();
        let addr = [0x04u8; 32];
        map.set(addr, 80);
        map.remove(&addr);
        assert_eq!(map.get(&addr), 0);
        assert!(map.is_empty());
    }

    #[test]
    fn decay_reduces_scores() {
        let map = ThreatScoreMap::new();
        let a = [0x01u8; 32];
        let b = [0x02u8; 32];
        map.set(a, 20);
        map.set(b, 3);

        map.decay(5);
        assert_eq!(map.get(&a), 15);
        // b was 3, decayed by 5 → 0, removed
        assert_eq!(map.get(&b), 0);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn decay_removes_zeroed_entries() {
        let map = ThreatScoreMap::new();
        let addr = [0x05u8; 32];
        map.set(addr, 5);
        map.decay(5);
        assert!(map.is_empty());
    }

    #[test]
    fn snapshot_is_deterministic() {
        let map = ThreatScoreMap::new();
        let a = [0x01u8; 32];
        let b = [0x02u8; 32];
        let c = [0x03u8; 32];
        map.set(c, 30);
        map.set(a, 10);
        map.set(b, 20);

        let snap = map.snapshot();
        let keys: Vec<_> = snap.keys().collect();
        // BTreeMap guarantees sorted order
        assert_eq!(keys, vec![&a, &b, &c]);
    }

    #[test]
    fn effective_fee_zero_score() {
        assert_eq!(effective_fee(1000, 0), 1000);
    }

    #[test]
    fn effective_fee_50_score() {
        assert_eq!(effective_fee(1000, 50), 500);
    }

    #[test]
    fn effective_fee_100_score() {
        assert_eq!(effective_fee(1000, 100), 0);
    }

    #[test]
    fn effective_fee_score_above_100_clamped() {
        assert_eq!(effective_fee(1000, 200), 0);
    }

    #[test]
    fn effective_fee_zero_fee() {
        assert_eq!(effective_fee(0, 50), 0);
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let map = Arc::new(ThreatScoreMap::new());

        let handles: Vec<_> = (0..4)
            .map(|i| {
                let m = Arc::clone(&map);
                thread::spawn(move || {
                    let addr = [i as u8; 32];
                    m.set(addr, 50);
                    assert!(m.get(&addr) <= 100);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(map.len(), 4);
    }
}
