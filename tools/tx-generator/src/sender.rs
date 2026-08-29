//! Account pool management for transaction generation.
//!
//! INVARIANTS:
//! - Keypairs are deterministic from seed index
//! - Nonces monotonically increase per sender
//! - Thread-safe for concurrent access
//!
//! FAILURE MODES:
//! - Nonce exhaustion (u64 overflow) - not realistically reachable

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_crypto::address_from_pubkey;
use novai_types::Address;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Per-sender recovery bookkeeping, shared across every worker.
///
/// Both fields live under one lock deliberately. They are read and written
/// together when deciding whether to resync, and splitting them would let the
/// streak and the cooldown disagree about whether a resync is in order.
#[derive(Debug, Default)]
struct SenderHealth {
    /// Consecutive submissions that told us this sender's nonce is wrong.
    /// Cleared only by an accepted submission.
    nonce_error_streak: u32,
    /// When any worker last began a chain nonce query for this sender.
    last_resync: Option<Instant>,
    /// Corrections applied to this sender without an accepted submission in
    /// between. One is ordinary. A run of them means every correction is
    /// being undone before it can be used, and the sweep is holding a sender
    /// upright rather than fixing it.
    consecutive_corrections: u32,
}

/// Corrections in a row, with nothing accepted between them, that make a
/// sender thrashing rather than recovering. Two could be a slow chain; by the
/// third the sweep has corrected, watched it undone, and corrected again.
pub const THRASH_THRESHOLD: u32 = 3;

/// A sender account with signing capability and nonce tracking.
#[derive(Debug)]
pub struct SenderAccount {
    /// Deterministic index used to derive this account.
    pub index: usize,
    /// Ed25519 signing key.
    pub signing_key: SigningKey,
    /// Ed25519 verifying key (public key).
    pub verifying_key: VerifyingKey,
    /// Derived address: blake3(pubkey).
    pub address: Address,
    /// Current nonce (atomically updated).
    nonce: AtomicU64,
    /// Recovery bookkeeping. Lives on the account rather than in a per-worker
    /// map so that N workers share one view of one sender: otherwise each
    /// worker counts its own streak, needing N times as many rejections to
    /// react, and N workers can each fire their own resync at once.
    health: Mutex<SenderHealth>,
}

impl SenderAccount {
    /// Create account from deterministic seed index.
    ///
    /// Uses a simple deterministic seed: [index as u8; 32] with wraparound.
    /// This ensures reproducible accounts for testing.
    pub fn from_index(index: usize) -> Self {
        // Create deterministic 32-byte seed from index
        let seed_byte = (index % 256) as u8;
        let mut seed = [seed_byte; 32];

        // Add index entropy to avoid collisions when index > 255
        let index_bytes = index.to_le_bytes();
        for (i, &b) in index_bytes.iter().enumerate() {
            seed[i] ^= b;
        }

        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let address = address_from_pubkey(&verifying_key);

        Self {
            index,
            signing_key,
            verifying_key,
            address,
            nonce: AtomicU64::new(0),
            health: Mutex::new(SenderHealth::default()),
        }
    }

    fn health(&self) -> std::sync::MutexGuard<'_, SenderHealth> {
        self.health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// An accepted submission: this sender's nonce is demonstrably right.
    ///
    /// This is also the only proof that a correction stuck. Nothing else
    /// distinguishes a sender that was fixed from one that was reset and
    /// immediately drifted back, which is why the thrash run clears here and
    /// nowhere else.
    pub fn record_accepted(&self) {
        let mut h = self.health();
        h.nonce_error_streak = 0;
        h.consecutive_corrections = 0;
    }

    /// A rejection that says our nonce state is wrong (too low, too high, or
    /// the per-sender slot cap). These are the only rejections that should
    /// push a sender toward a resync.
    pub fn record_nonce_error(&self) {
        let mut h = self.health();
        h.nonce_error_streak = h.nonce_error_streak.saturating_add(1);
    }

    /// A rejection that says nothing about our nonce (fee floor, malformed,
    /// duplicate, transport).
    ///
    /// Deliberately does NOT clear the streak. It used to, and an interleaved
    /// fee-floor or duplicate rejection could then push recovery out
    /// indefinitely by wiping the evidence just before the threshold.
    pub fn record_unrelated_rejection(&self) {}

    /// The sender was corrected from a value read off the chain, so whatever
    /// the streak was counting has been answered.
    ///
    /// The correction is also counted. The sweep used to report only how many
    /// senders it decided to correct, which is the same number whether the
    /// pool converged afterwards or every sender drifted straight back. On
    /// 2026-08-28 it read `corrected=10 senders=10` on consecutive passes
    /// with byte identical per sender values, and nothing in the log could
    /// say that was a failure.
    pub fn record_resynced(&self) {
        let mut h = self.health();
        h.nonce_error_streak = 0;
        h.consecutive_corrections = h.consecutive_corrections.saturating_add(1);
    }

    /// Corrections applied since this sender last had anything accepted.
    pub fn consecutive_corrections(&self) -> u32 {
        self.health().consecutive_corrections
    }

    /// Is this sender being corrected repeatedly without ever recovering?
    ///
    /// A true here means the sweep is not the thing that will fix it. The
    /// cause is downstream: nothing this sender submits is being accepted, so
    /// the chain nonce never advances and the local one climbs back to the
    /// same offset before the next pass.
    pub fn is_thrashing(&self) -> bool {
        self.consecutive_corrections() >= THRASH_THRESHOLD
    }

    pub fn nonce_error_streak(&self) -> u32 {
        self.health().nonce_error_streak
    }

    /// Claim the right to resync this sender, at most once per
    /// `min_interval` across all workers. Returns true to exactly one caller
    /// per window.
    ///
    /// The test and the set happen in one critical section, which is the
    /// whole point: several workers routinely notice the same sick sender in
    /// the same instant, and the resync fires precisely when the endpoint is
    /// already struggling, so letting each of them fire its own query is the
    /// worst available response.
    pub fn try_begin_resync(&self, min_interval: Duration) -> bool {
        let mut h = self.health();
        let now = Instant::now();
        match h.last_resync {
            Some(previous) if now.duration_since(previous) < min_interval => false,
            _ => {
                h.last_resync = Some(now);
                true
            }
        }
    }

    /// Get current nonce without incrementing.
    #[allow(dead_code)]
    pub fn current_nonce(&self) -> u64 {
        self.nonce.load(Ordering::SeqCst)
    }

    /// Claim the next nonce (atomic increment, returns previous value).
    ///
    /// This is the nonce that should be used for the next transaction.
    pub fn claim_nonce(&self) -> u64 {
        self.nonce.fetch_add(1, Ordering::SeqCst)
    }

    /// Rollback nonce (for retry scenarios).
    ///
    /// SAFETY: Only safe if the transaction was not submitted to the node.
    /// Using this after submission can cause nonce gaps.
    #[allow(dead_code)]
    pub fn rollback_nonce(&self) {
        self.nonce.fetch_sub(1, Ordering::SeqCst);
    }

    /// Reset nonce to a specific value.
    ///
    /// Used to recover from nonce desync (e.g., after node restart with fresh
    /// state). In-flight transactions with higher nonces will be rejected, but
    /// subsequent transactions will use the reset value.
    pub fn reset_nonce(&self, value: u64) {
        self.nonce.store(value, Ordering::SeqCst);
    }
}

/// Pool of sender accounts for transaction generation.
pub struct SenderPool {
    accounts: Vec<Arc<SenderAccount>>,
    next_index: AtomicU64,
    /// Per-sender shares. `None` is strict round robin, which gives every
    /// sender exactly the same rate and is what this pool did before.
    selector: Option<crate::rates::WeightedSelector>,
}

impl SenderPool {
    /// Create pool with specified number of sender accounts.
    ///
    /// Accounts are deterministically generated from indices 0..count.
    pub fn new(count: usize) -> Self {
        let accounts = (0..count)
            .map(|i| Arc::new(SenderAccount::from_index(i)))
            .collect();

        Self {
            accounts,
            next_index: AtomicU64::new(0),
            selector: None,
        }
    }

    /// Give each sender its own share of the offered rate.
    ///
    /// This changes WHICH sender gets a tick, never HOW MANY ticks there are,
    /// so the aggregate stays exactly `--tps`. Drawn from `seed`, so the same
    /// seed places the same load twice.
    #[must_use]
    pub fn with_rate_distribution(
        mut self,
        dist: crate::rates::RateDistribution,
        seed: u64,
    ) -> Self {
        let weights = crate::rates::draw_weights(self.accounts.len(), dist, seed);
        self.selector = crate::rates::WeightedSelector::new(&weights, seed);
        self
    }

    /// The share of the offered rate sender `index` is expected to receive,
    /// or an equal share when the pool is homogeneous.
    pub fn share_of(&self, index: usize) -> f64 {
        match &self.selector {
            Some(sel) => sel.share(index),
            None => 1.0 / self.accounts.len() as f64,
        }
    }

    /// Get the next sender for a tick.
    ///
    /// Round robin by default, which spreads the offered rate equally. With a
    /// rate distribution attached the sender is drawn in proportion to its
    /// share instead, so the pool behaves like a population of independent
    /// agents rather than N copies of one client.
    pub fn next_sender(&self) -> Arc<SenderAccount> {
        if let Some(sel) = &self.selector {
            return Arc::clone(&self.accounts[sel.next_index()]);
        }
        let idx = self.next_index.fetch_add(1, Ordering::SeqCst);
        let account_idx = (idx as usize) % self.accounts.len();
        Arc::clone(&self.accounts[account_idx])
    }

    /// Get sender by index.
    #[allow(dead_code)]
    pub fn get_sender(&self, index: usize) -> Option<Arc<SenderAccount>> {
        self.accounts.get(index).cloned()
    }

    /// Total number of accounts in pool.
    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    /// Check if pool is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    /// Find a sender account by its address.
    pub fn find_by_address(&self, address: &Address) -> Option<Arc<SenderAccount>> {
        self.accounts
            .iter()
            .find(|a| a.address == *address)
            .cloned()
    }

    /// Get all accounts (startup nonce resync, reporting).
    pub fn all_accounts(&self) -> &[Arc<SenderAccount>] {
        &self.accounts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_keypair_is_deterministic() {
        let acc1 = SenderAccount::from_index(5);
        let acc2 = SenderAccount::from_index(5);

        assert_eq!(acc1.signing_key.to_bytes(), acc2.signing_key.to_bytes());
        assert_eq!(acc1.verifying_key.to_bytes(), acc2.verifying_key.to_bytes());
        assert_eq!(acc1.address, acc2.address);
    }

    #[test]
    fn sender_address_matches_pubkey() {
        let acc = SenderAccount::from_index(10);
        let expected_address = address_from_pubkey(&acc.verifying_key);
        assert_eq!(acc.address, expected_address);
    }

    #[test]
    fn nonce_increments_atomically() {
        let acc = SenderAccount::from_index(0);

        assert_eq!(acc.current_nonce(), 0);
        assert_eq!(acc.claim_nonce(), 0);
        assert_eq!(acc.current_nonce(), 1);
        assert_eq!(acc.claim_nonce(), 1);
        assert_eq!(acc.current_nonce(), 2);
    }

    #[test]
    fn nonce_rollback_works() {
        let acc = SenderAccount::from_index(0);

        acc.claim_nonce(); // 0 -> 1
        acc.claim_nonce(); // 1 -> 2
        assert_eq!(acc.current_nonce(), 2);

        acc.rollback_nonce(); // 2 -> 1
        assert_eq!(acc.current_nonce(), 1);
    }

    #[test]
    fn reset_nonce_then_claim_starts_at_value() {
        let acc = SenderAccount::from_index(0);
        assert_eq!(acc.current_nonce(), 0);

        acc.reset_nonce(272);
        assert_eq!(acc.claim_nonce(), 272);
        assert_eq!(acc.claim_nonce(), 273);
    }

    #[test]
    fn pool_round_robin_cycles() {
        let pool = SenderPool::new(3);

        let s0 = pool.next_sender();
        let s1 = pool.next_sender();
        let s2 = pool.next_sender();
        let s3 = pool.next_sender();

        assert_eq!(s0.index, 0);
        assert_eq!(s1.index, 1);
        assert_eq!(s2.index, 2);
        assert_eq!(s3.index, 0); // wraps around
    }

    #[test]
    fn pool_get_sender_returns_correct_account() {
        let pool = SenderPool::new(5);

        let acc = pool.get_sender(3).unwrap();
        assert_eq!(acc.index, 3);

        assert!(pool.get_sender(10).is_none());
    }

    #[test]
    fn pool_len_and_is_empty() {
        let pool = SenderPool::new(10);
        assert_eq!(pool.len(), 10);
        assert!(!pool.is_empty());

        let empty_pool = SenderPool::new(0);
        assert_eq!(empty_pool.len(), 0);
        assert!(empty_pool.is_empty());
    }

    #[test]
    fn concurrent_nonce_claims_no_duplicates() {
        use std::collections::HashSet;
        use std::sync::Arc;
        use std::thread;

        let acc = Arc::new(SenderAccount::from_index(0));
        let mut handles = vec![];

        // Spawn 10 threads, each claiming 100 nonces
        for _ in 0..10 {
            let acc = Arc::clone(&acc);
            let handle = thread::spawn(move || {
                let mut nonces = vec![];
                for _ in 0..100 {
                    nonces.push(acc.claim_nonce());
                }
                nonces
            });
            handles.push(handle);
        }

        // Collect all claimed nonces
        let mut all_nonces = HashSet::new();
        for handle in handles {
            let nonces = handle.join().unwrap();
            for n in nonces {
                assert!(all_nonces.insert(n), "Duplicate nonce: {n}");
            }
        }

        // Should have exactly 1000 unique nonces (0..999)
        assert_eq!(all_nonces.len(), 1000);
        assert_eq!(acc.current_nonce(), 1000);
    }
}
