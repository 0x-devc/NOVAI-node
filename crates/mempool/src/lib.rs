use std::collections::{BTreeMap, HashMap, VecDeque};
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// H-08: Maximum pending transactions from a single sender during insertion.
/// Prevents one address from monopolizing mempool capacity.
pub const MAX_PENDING_PER_SENDER: usize = 16;

/// Errors returned by [`Mempool`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MempoolError {
    Duplicate,
}

/// A simple FIFO mempool keyed by a transaction id.
///
/// Notes:
/// - Ordering is FIFO by insertion time.
/// - `remove()` is supported; drained items skip anything already removed.
/// - This is intentionally minimal for Week 2 wiring.
pub struct Mempool<Id, Tx>
where
    Id: Eq + Hash + Copy,
{
    id_of: Arc<dyn Fn(&Tx) -> Id + Send + Sync>,
    by_id: HashMap<Id, Tx>,
    order: VecDeque<Id>,
}

impl<Id, Tx> Mempool<Id, Tx>
where
    Id: Eq + Hash + Copy,
{
    /// Create a new mempool with a function that extracts the tx id from a transaction.
    pub fn new(id_of: impl Fn(&Tx) -> Id + Send + Sync + 'static) -> Self {
        Self {
            id_of: Arc::new(id_of),
            by_id: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Insert a transaction. Rejects duplicates by tx id.
    pub fn insert(&mut self, tx: Tx) -> Result<(), MempoolError> {
        let id = (self.id_of)(&tx);
        if self.by_id.contains_key(&id) {
            return Err(MempoolError::Duplicate);
        }

        self.by_id.insert(id, tx);
        self.order.push_back(id);
        Ok(())
    }

    /// Remove a transaction by id.
    pub fn remove(&mut self, id: Id) -> Option<Tx> {
        self.by_id.remove(&id)
    }

    /// Returns true if the mempool currently contains this id.
    pub fn contains(&self, id: Id) -> bool {
        self.by_id.contains_key(&id)
    }

    /// Get a reference to a tx by id.
    pub fn get(&self, id: Id) -> Option<&Tx> {
        self.by_id.get(&id)
    }

    /// Number of currently-stored transactions.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// True if empty.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Drain up to `max` transactions in FIFO order.
    ///
    /// This skips ids that were previously removed.
    pub fn drain_ready(&mut self, max: usize) -> Vec<Tx> {
        // Avoid Vec capacity overflow if `max` is huge.
        let cap = max.min(self.by_id.len());
        let mut out = Vec::with_capacity(cap);

        while out.len() < max {
            let Some(id) = self.order.pop_front() else {
                break;
            };

            if let Some(tx) = self.by_id.remove(&id) {
                out.push(tx);
            }
        }

        out
    }
}

// -----------------------------------------------------------------------------
// Week 2 "real" mempool: TxV1 policy enforcement + deterministic fee-priority.
// -----------------------------------------------------------------------------

use novai_codec::txid_v1;
use novai_crypto::{address_from_pubkey, pubkey_from_bytes, verify_tx_v1};
use novai_types::{Address, TxId, TxV1};

/// Provides the current expected nonce for a sender address (state snapshot).
///
/// Week 2: this can be a stub backed by a HashMap in node/tests.
/// Later: it will be backed by actual state.
pub trait NonceProvider {
    fn expected_nonce(&self, from: &Address) -> u64;
}

/// Errors for the V1 tx mempool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxMempoolError {
    Duplicate,
    FeeTooLow {
        min_fee: u64,
        got: u64,
    },
    NonceTooLow {
        expected: u64,
        got: u64,
    },
    InvalidSignature,
    InvalidPublicKey,
    AddressMismatch,
    CodecError,
    TxTooLarge {
        size: usize,
        max: usize,
    },
    MempoolFull {
        current: usize,
        max: usize,
    },
    /// H-08: Too many pending transactions from a single sender.
    SenderLimitExceeded {
        address: Address,
        count: usize,
        max: usize,
    },
}

/// A mempool specifically for canonical TxV1.
///
/// Policy (Week 2):
/// - Reject invalid signatures.
/// - Reject fee < min_fee.
/// - Reject nonce < expected_nonce(from).
/// - Drain policy:
///   - Ready if nonce == expected_nonce(from)
///   - Sort by fee DESC, then txid ASC (deterministic)
///   - Fairness cap: at most K txs per sender per drain batch
pub struct TxMempool {
    min_fee: u64,
    fairness_cap_per_sender: usize,
    by_id: HashMap<TxId, TxV1>,
    /// Total encoded bytes of all transactions currently in the mempool.
    total_bytes: usize,
    /// Dynamic fee floor set by the congestion responder.
    /// `effective_min_fee = max(min_fee, dynamic_fee_floor)`.
    dynamic_fee_floor: Arc<AtomicU64>,
    /// Per-address threat scores (0-100) set by the spam detector.
    /// Used in `drain_ready()` to deprioritize (never reject) suspicious senders.
    threat_scores: Arc<Mutex<BTreeMap<Address, u8>>>,
    /// Fast-path flag: true when threat_scores map is known to be empty.
    /// Avoids Mutex lock in `drain_ready()` when no spam has been detected.
    threat_scores_empty: Arc<AtomicBool>,
    /// Insertion timestamps for stale transaction eviction.
    /// Txs older than a configurable max age are purged to free capacity
    /// (prevents future-nonce txs from permanently filling the mempool).
    insertion_times: HashMap<TxId, Instant>,
    /// H-08: Per-sender pending transaction count for admission control.
    by_sender_count: HashMap<Address, usize>,
}

impl TxMempool {
    pub fn new(min_fee: u64, fairness_cap_per_sender: usize) -> Self {
        Self {
            min_fee,
            fairness_cap_per_sender: fairness_cap_per_sender.max(1),
            by_id: HashMap::new(),
            total_bytes: 0,
            dynamic_fee_floor: Arc::new(AtomicU64::new(0)),
            threat_scores: Arc::new(Mutex::new(BTreeMap::new())),
            threat_scores_empty: Arc::new(AtomicBool::new(true)),
            insertion_times: HashMap::new(),
            by_sender_count: HashMap::new(),
        }
    }

    /// Returns the shared dynamic fee floor atomic.
    ///
    /// The congestion responder writes to this; the mempool reads it during `insert()`.
    pub fn dynamic_fee_floor(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.dynamic_fee_floor)
    }

    /// Effective minimum fee: `max(base_min_fee, dynamic_fee_floor)`.
    pub fn effective_min_fee(&self) -> u64 {
        let dynamic = self.dynamic_fee_floor.load(Ordering::Relaxed);
        self.min_fee.max(dynamic)
    }

    /// Returns the shared threat scores map and empty flag.
    ///
    /// The copilot thread writes updated scores; the mempool reads them during `drain_ready()`.
    /// Callers MUST set the empty flag to `false` when adding scores and `true` when clearing.
    pub fn threat_scores(&self) -> (Arc<Mutex<BTreeMap<Address, u8>>>, Arc<AtomicBool>) {
        (
            Arc::clone(&self.threat_scores),
            Arc::clone(&self.threat_scores_empty),
        )
    }

    /// Total encoded bytes of all transactions currently in the mempool.
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn contains(&self, id: &TxId) -> bool {
        self.by_id.contains_key(id)
    }

    pub fn get(&self, id: &TxId) -> Option<&TxV1> {
        self.by_id.get(id)
    }

    pub fn remove(&mut self, id: &TxId) -> Option<TxV1> {
        let tx = self.by_id.remove(id)?;
        self.total_bytes -= novai_codec::tx_encoded_size(&tx);
        self.insertion_times.remove(id);
        Some(tx)
    }

    /// Insert a TxV1 after enforcing Week 2 policy rules.
    ///
    /// Returns the computed TxId (blake3(unsigned_bytes)).
    pub fn insert(
        &mut self,
        tx: TxV1,
        nonce_provider: &impl NonceProvider,
    ) -> Result<TxId, TxMempoolError> {
        // min fee (uses effective = max(base, dynamic_floor))
        let effective = self.effective_min_fee();
        if tx.fee < effective {
            return Err(TxMempoolError::FeeTooLow {
                min_fee: effective,
                got: tx.fee,
            });
        }

        // nonce sanity vs snapshot
        let expected = nonce_provider.expected_nonce(&tx.from);
        if tx.nonce < expected {
            return Err(TxMempoolError::NonceTooLow {
                expected,
                got: tx.nonce,
            });
        }

        // H-08: Per-sender admission control (before expensive sig verification)
        let sender_count = self.by_sender_count.get(&tx.from).copied().unwrap_or(0);
        if sender_count >= MAX_PENDING_PER_SENDER {
            return Err(TxMempoolError::SenderLimitExceeded {
                address: tx.from,
                count: sender_count,
                max: MAX_PENDING_PER_SENDER,
            });
        }

        // M-07: Size limits BEFORE expensive signature verification.
        // No point verifying a signature if the tx is too large or mempool is full.
        let size = novai_codec::tx_encoded_size(&tx);
        if size > novai_types::MAX_TX_SIZE {
            return Err(TxMempoolError::TxTooLarge {
                size,
                max: novai_types::MAX_TX_SIZE,
            });
        }
        if self.total_bytes + size > novai_types::MAX_MEMPOOL_BYTES {
            return Err(TxMempoolError::MempoolFull {
                current: self.total_bytes,
                max: novai_types::MAX_MEMPOOL_BYTES,
            });
        }

        // Verify address matches pubkey
        let vk = pubkey_from_bytes(&tx.pubkey).map_err(|_| TxMempoolError::InvalidPublicKey)?;
        let expected_addr = address_from_pubkey(&vk);
        if tx.from != expected_addr {
            return Err(TxMempoolError::AddressMismatch);
        }

        // verify domain-tagged signature (NOVAI_TX_V1 || unsigned_bytes)
        let sig_ok = verify_tx_v1(&vk, &tx).map_err(|_| TxMempoolError::CodecError)?;
        if !sig_ok {
            return Err(TxMempoolError::InvalidSignature);
        }

        // compute txid (hash of canonical unsigned bytes)
        let id = txid_v1(&tx).map_err(|_| TxMempoolError::CodecError)?;

        // dedupe
        if self.by_id.contains_key(&id) {
            return Err(TxMempoolError::Duplicate);
        }

        self.total_bytes += size;
        self.insertion_times.insert(id, Instant::now());
        *self.by_sender_count.entry(tx.from).or_insert(0) += 1;
        self.by_id.insert(id, tx);
        Ok(id)
    }

    /// Drain up to `max` ready transactions under fee-priority + fairness.
    ///
    /// Sort uses `effective_fee = fee * (100 - threat_score) / 100` so that
    /// high-threat senders are deprioritized but NEVER dropped.
    pub fn drain_ready(&mut self, max: usize, nonce_provider: &impl NonceProvider) -> Vec<TxV1> {
        if max == 0 || self.by_id.is_empty() {
            return Vec::new();
        }

        // Fast-path: skip Mutex lock when no threat scores exist.
        let scores = if self.threat_scores_empty.load(Ordering::Relaxed) {
            BTreeMap::new()
        } else {
            self.threat_scores
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default()
        };

        // Single pass: evict stale txs AND gather ready candidates.
        // Stale = nonce < expected (chain committed past their slot, provably dead).
        // Ready = nonce == expected (eligible for next block).
        // Combined to avoid O(2n) double-scan with Mutex lock per tx.
        let mut candidates: Vec<(u64, u64, TxId, Address)> = Vec::with_capacity(self.by_id.len());
        let mut stale_ids: Vec<TxId> = Vec::new();

        for (id, tx) in &self.by_id {
            let expected = nonce_provider.expected_nonce(&tx.from);
            if tx.nonce < expected {
                stale_ids.push(*id);
            } else if tx.nonce == expected {
                let score = scores.get(&tx.from).copied().unwrap_or(0);
                let s = (score.min(100)) as u64;
                let eff = tx.fee * (100 - s) / 100;
                candidates.push((eff, tx.fee, *id, tx.from));
            }
        }

        // Remove stale txs (frees per-sender slots for fresh txs)
        for id in &stale_ids {
            if let Some(tx) = self.by_id.remove(id) {
                self.total_bytes -= novai_codec::tx_encoded_size(&tx);
                self.insertion_times.remove(id);
                if let Some(c) = self.by_sender_count.get_mut(&tx.from) {
                    *c = c.saturating_sub(1);
                    if *c == 0 {
                        self.by_sender_count.remove(&tx.from);
                    }
                }
            }
        }

        // Sort: effective_fee DESC, then raw_fee DESC, then txid ASC (deterministic).
        candidates.sort_by(|(eff_a, fee_a, id_a, _), (eff_b, fee_b, id_b, _)| {
            eff_b
                .cmp(eff_a)
                .then_with(|| fee_b.cmp(fee_a))
                .then_with(|| id_a.cmp(id_b))
        });

        let cap = max.min(candidates.len());
        let mut out: Vec<TxV1> = Vec::with_capacity(cap);
        let mut per_sender: HashMap<Address, usize> = HashMap::new();
        let mut selected_ids: Vec<TxId> = Vec::with_capacity(cap);

        for (_eff, _fee, id, from) in candidates {
            if selected_ids.len() >= max {
                break;
            }

            let c = per_sender.entry(from).or_insert(0);
            if *c >= self.fairness_cap_per_sender {
                continue;
            }

            *c += 1;
            selected_ids.push(id);
        }

        for id in selected_ids {
            if let Some(tx) = self.by_id.remove(&id) {
                self.total_bytes -= novai_codec::tx_encoded_size(&tx);
                self.insertion_times.remove(&id);
                // H-08: Decrement per-sender count
                if let Some(c) = self.by_sender_count.get_mut(&tx.from) {
                    *c = c.saturating_sub(1);
                    if *c == 0 {
                        self.by_sender_count.remove(&tx.from);
                    }
                }
                out.push(tx);
            }
        }

        out
    }

    /// Re-insert a previously validated transaction without re-checking
    /// signature, nonce, or fee.
    ///
    /// Safe because these txs were already validated before being drained
    /// from the mempool. Used by the block proposal layer to return overflow
    /// txs that didn't fit in the current block.
    pub fn reinsert_unchecked(&mut self, tx: TxV1) -> Result<TxId, TxMempoolError> {
        let id = txid_v1(&tx).map_err(|_| TxMempoolError::CodecError)?;

        if self.by_id.contains_key(&id) {
            return Err(TxMempoolError::Duplicate);
        }

        let size = novai_codec::tx_encoded_size(&tx);
        self.total_bytes += size;
        self.insertion_times.insert(id, Instant::now());
        *self.by_sender_count.entry(tx.from).or_insert(0) += 1;
        self.by_id.insert(id, tx);
        Ok(id)
    }

    /// Purge transactions that have been in the mempool longer than `max_age`.
    ///
    /// This prevents future-nonce transactions from permanently filling the
    /// mempool. When the tx-generator's nonce desyncs from the chain, it can
    /// leave orphan transactions (nonce > expected) that `drain_ready()` will
    /// never select. Without eviction these consume capacity forever.
    ///
    /// Returns the number of transactions purged.
    pub fn purge_stale(&mut self, max_age: std::time::Duration) -> usize {
        let now = Instant::now();
        let stale_ids: Vec<TxId> = self
            .insertion_times
            .iter()
            .filter(|(_, &inserted_at)| now.duration_since(inserted_at) >= max_age)
            .map(|(&id, _)| id)
            .collect();

        let count = stale_ids.len();
        for id in stale_ids {
            if let Some(tx) = self.by_id.remove(&id) {
                self.total_bytes -= novai_codec::tx_encoded_size(&tx);
                // H-08: Decrement per-sender count
                if let Some(c) = self.by_sender_count.get_mut(&tx.from) {
                    *c = c.saturating_sub(1);
                    if *c == 0 {
                        self.by_sender_count.remove(&tx.from);
                    }
                }
            }
            self.insertion_times.remove(&id);
        }

        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Tx {
        id: u64,
        payload: &'static str,
    }

    #[test]
    fn insert_and_get_and_contains() {
        let mut mp = Mempool::<u64, Tx>::new(|tx| tx.id);

        mp.insert(Tx {
            id: 1,
            payload: "a",
        })
        .unwrap();
        assert!(mp.contains(1));
        assert_eq!(mp.len(), 1);

        let tx = mp.get(1).unwrap();
        assert_eq!(tx.payload, "a");
    }

    #[test]
    fn duplicate_rejected() {
        let mut mp = Mempool::<u64, Tx>::new(|tx| tx.id);

        mp.insert(Tx {
            id: 7,
            payload: "x",
        })
        .unwrap();
        let err = mp
            .insert(Tx {
                id: 7,
                payload: "y",
            })
            .unwrap_err();
        assert_eq!(err, MempoolError::Duplicate);

        // original remains
        assert_eq!(mp.get(7).unwrap().payload, "x");
        assert_eq!(mp.len(), 1);
    }

    #[test]
    fn remove_works() {
        let mut mp = Mempool::<u64, Tx>::new(|tx| tx.id);

        mp.insert(Tx {
            id: 2,
            payload: "b",
        })
        .unwrap();
        let removed = mp.remove(2).unwrap();
        assert_eq!(removed.payload, "b");
        assert!(!mp.contains(2));
        assert_eq!(mp.len(), 0);
    }

    #[test]
    fn drain_ready_fifo_and_skips_removed() {
        let mut mp = Mempool::<u64, Tx>::new(|tx| tx.id);

        mp.insert(Tx {
            id: 1,
            payload: "a",
        })
        .unwrap();
        mp.insert(Tx {
            id: 2,
            payload: "b",
        })
        .unwrap();
        mp.insert(Tx {
            id: 3,
            payload: "c",
        })
        .unwrap();

        // remove one in the middle before draining
        mp.remove(2);

        let drained = mp.drain_ready(10);
        let payloads: Vec<_> = drained.into_iter().map(|t| t.payload).collect();
        assert_eq!(payloads, vec!["a", "c"]);
        assert_eq!(mp.len(), 0);
        assert!(mp.is_empty());
    }

    #[test]
    fn drain_respects_max() {
        let mut mp = Mempool::<u64, Tx>::new(|tx| tx.id);

        mp.insert(Tx {
            id: 1,
            payload: "a",
        })
        .unwrap();
        mp.insert(Tx {
            id: 2,
            payload: "b",
        })
        .unwrap();
        mp.insert(Tx {
            id: 3,
            payload: "c",
        })
        .unwrap();

        let drained1 = mp.drain_ready(2);
        assert_eq!(drained1.len(), 2);
        assert_eq!(mp.len(), 1);

        let drained2 = mp.drain_ready(2);
        assert_eq!(drained2.len(), 1);
        assert_eq!(mp.len(), 0);
    }

    // -----------------------------
    // TxMempool (Week 2 policy) tests
    // -----------------------------

    use ed25519_dalek::{SigningKey, VerifyingKey};
    use novai_crypto::{address_from_pubkey, sign_tx_v1};
    use novai_types::TxVersion;

    fn test_keypair(seed: u8) -> (SigningKey, VerifyingKey) {
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let vk: VerifyingKey = sk.verifying_key();
        (sk, vk)
    }

    #[derive(Default)]
    struct TestNonceProvider {
        map: HashMap<Address, u64>,
    }

    impl TestNonceProvider {
        fn set(&mut self, from: Address, nonce: u64) {
            self.map.insert(from, nonce);
        }
    }

    impl NonceProvider for TestNonceProvider {
        fn expected_nonce(&self, from: &Address) -> u64 {
            *self.map.get(from).unwrap_or(&0)
        }
    }

    fn make_signed_tx(
        from_sk: &SigningKey,
        from_vk: &VerifyingKey,
        nonce: u64,
        fee: u64,
        payload: &[u8],
    ) -> TxV1 {
        let from_addr = address_from_pubkey(from_vk);

        let mut tx = TxV1 {
            version: TxVersion::V1,
            from: from_addr,
            pubkey: from_vk.to_bytes(),
            nonce,
            fee,
            payload: payload.to_vec(),
            sig: [0u8; 64],
        };

        sign_tx_v1(from_sk, &mut tx).expect("sign_tx_v1");
        tx
    }

    #[test]
    fn rejects_below_min_fee() {
        let (sk, vk) = test_keypair(7);
        let from: Address = address_from_pubkey(&vk);

        let mut np = TestNonceProvider::default();
        np.set(from, 0);

        let mut mp = TxMempool::new(10, 2);
        let tx = make_signed_tx(&sk, &vk, 0, 9, b"p");
        let err = mp.insert(tx, &np).unwrap_err();
        assert!(matches!(err, TxMempoolError::FeeTooLow { .. }));
    }

    #[test]
    fn rejects_nonce_too_low() {
        let (sk, vk) = test_keypair(9);
        let from: Address = address_from_pubkey(&vk);

        let mut np = TestNonceProvider::default();
        np.set(from, 5);

        let mut mp = TxMempool::new(1, 2);
        let tx = make_signed_tx(&sk, &vk, 4, 1, b"p");
        let err = mp.insert(tx, &np).unwrap_err();
        assert!(matches!(err, TxMempoolError::NonceTooLow { .. }));
    }

    #[test]
    fn rejects_invalid_signature() {
        let (_sk1, vk1) = test_keypair(1);
        let from1: Address = address_from_pubkey(&vk1);

        let (sk2, _vk2) = test_keypair(2);

        let mut np = TestNonceProvider::default();
        np.set(from1, 0);

        let mut mp = TxMempool::new(1, 2);

        // Build a tx "from1" but sign it with sk2 (wrong key) => should fail.
        let mut tx = TxV1 {
            version: TxVersion::V1,
            from: from1,
            pubkey: vk1.to_bytes(),
            nonce: 0,
            fee: 1,
            payload: b"x".to_vec(),
            sig: [0u8; 64],
        };

        sign_tx_v1(&sk2, &mut tx).expect("sign_tx_v1");

        let err = mp.insert(tx, &np).unwrap_err();
        assert_eq!(err, TxMempoolError::InvalidSignature);
    }

    #[test]
    fn drain_is_fee_priority_and_nonce_ready() {
        let (sk, vk) = test_keypair(3);
        let from: Address = address_from_pubkey(&vk);

        let mut np = TestNonceProvider::default();
        np.set(from, 0);

        let mut mp = TxMempool::new(1, 10);

        // nonce 0 ready, fee 5
        let tx_a = make_signed_tx(&sk, &vk, 0, 5, b"a");
        // nonce 1 NOT ready initially, fee 999 (should not drain yet)
        let tx_b = make_signed_tx(&sk, &vk, 1, 999, b"b");
        // nonce 0 ready, fee 10 (should drain first)
        let tx_c = make_signed_tx(&sk, &vk, 0, 10, b"c");

        mp.insert(tx_a, &np).unwrap();
        mp.insert(tx_b, &np).unwrap();
        mp.insert(tx_c, &np).unwrap();

        let drained = mp.drain_ready(10, &np);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].payload, b"c");
        assert_eq!(drained[1].payload, b"a");

        // Now advance expected nonce to 1, tx_b becomes ready.
        np.set(from, 1);
        let drained2 = mp.drain_ready(10, &np);
        assert_eq!(drained2.len(), 1);
        assert_eq!(drained2[0].payload, b"b");
    }

    #[test]
    fn fairness_cap_limits_per_sender() {
        let (sk1, vk1) = test_keypair(5);
        let (sk2, vk2) = test_keypair(6);
        let from1: Address = address_from_pubkey(&vk1);
        let from2: Address = address_from_pubkey(&vk2);

        let mut np = TestNonceProvider::default();
        np.set(from1, 0);
        np.set(from2, 0);

        let mut mp = TxMempool::new(1, 1); // cap = 1 per sender per drain

        // Two ready txs from sender1 (both nonce 0) and one from sender2.
        let s1_hi = make_signed_tx(&sk1, &vk1, 0, 100, b"s1_hi");
        let s1_lo = make_signed_tx(&sk1, &vk1, 0, 1, b"s1_lo");
        let s2_mid = make_signed_tx(&sk2, &vk2, 0, 50, b"s2_mid");

        mp.insert(s1_hi, &np).unwrap();
        mp.insert(s1_lo, &np).unwrap();
        mp.insert(s2_mid, &np).unwrap();

        let drained = mp.drain_ready(10, &np);

        // Should pick: sender1 highest fee and sender2 tx (cap blocks second sender1 tx).
        assert_eq!(drained.len(), 2);
        let payloads: Vec<Vec<u8>> = drained.into_iter().map(|t| t.payload).collect();
        assert!(payloads.contains(&b"s1_hi".to_vec()));
        assert!(payloads.contains(&b"s2_mid".to_vec()));
        assert!(!payloads.contains(&b"s1_lo".to_vec()));
    }

    // -----------------------------------------
    // Size limit enforcement tests
    // -----------------------------------------

    #[test]
    fn rejects_tx_too_large() {
        let (sk, vk) = test_keypair(10);
        let from: Address = address_from_pubkey(&vk);

        let mut np = TestNonceProvider::default();
        np.set(from, 0);

        let mut mp = TxMempool::new(1, 10);

        // Create a tx with payload that pushes encoded size over MAX_TX_SIZE
        let oversized_payload = vec![0xAA; novai_types::MAX_TX_SIZE]; // 128KB payload + 149 overhead > 128KB
        let tx = make_signed_tx(&sk, &vk, 0, 1, &oversized_payload);
        let err = mp.insert(tx, &np).unwrap_err();
        assert!(
            matches!(err, TxMempoolError::TxTooLarge { .. }),
            "expected TxTooLarge, got {err:?}"
        );
    }

    #[test]
    fn rejects_mempool_full() {
        let mut mp = TxMempool::new(1, 1000);
        let mut np = TestNonceProvider::default();

        // Fill the mempool close to MAX_MEMPOOL_BYTES using multiple senders
        // (each sender limited to MAX_PENDING_PER_SENDER by H-08).
        let payload_size = 64 * 1024; // 64KB payload per tx
        let tx_size = novai_codec::TX_V1_OVERHEAD + payload_size;
        let count_to_fill = novai_types::MAX_MEMPOOL_BYTES / tx_size; // ~1023
        let per_sender = super::MAX_PENDING_PER_SENDER;
        let num_senders = (count_to_fill / per_sender) + 1;

        let mut inserted = 0;
        for s in 0..num_senders {
            let (sk, vk) = test_keypair(100 + s as u8);
            let from: Address = address_from_pubkey(&vk);
            np.set(from, 0);
            for i in 0..per_sender {
                if inserted >= count_to_fill {
                    break;
                }
                let payload = vec![0xBB; payload_size];
                let tx = make_signed_tx(&sk, &vk, i as u64, 1, &payload);
                mp.insert(tx, &np).unwrap();
                np.set(from, (i + 1) as u64);
                inserted += 1;
            }
        }

        // Now the mempool should be nearly full. One more from a new sender should fail.
        let (sk_extra, vk_extra) = test_keypair(200);
        let from_extra: Address = address_from_pubkey(&vk_extra);
        np.set(from_extra, 0);
        let payload = vec![0xCC; payload_size];
        let tx = make_signed_tx(&sk_extra, &vk_extra, 0, 1, &payload);
        let err = mp.insert(tx, &np).unwrap_err();
        assert!(
            matches!(err, TxMempoolError::MempoolFull { .. }),
            "expected MempoolFull, got {err:?}"
        );
    }

    #[test]
    fn total_bytes_tracks_through_insert_drain_remove() {
        let (sk, vk) = test_keypair(12);
        let from: Address = address_from_pubkey(&vk);

        let mut np = TestNonceProvider::default();
        np.set(from, 0);

        let mut mp = TxMempool::new(1, 10);
        assert_eq!(mp.total_bytes(), 0);

        // Insert two txs
        let tx_a = make_signed_tx(&sk, &vk, 0, 10, b"aaa");
        let tx_b = make_signed_tx(&sk, &vk, 1, 5, b"bbb");
        let size_a = novai_codec::tx_encoded_size(&tx_a);
        let size_b = novai_codec::tx_encoded_size(&tx_b);

        let _id_a = mp.insert(tx_a, &np).unwrap();
        assert_eq!(mp.total_bytes(), size_a);

        mp.insert(tx_b, &np).unwrap();
        assert_eq!(mp.total_bytes(), size_a + size_b);

        // Drain one (nonce 0 is ready)
        let drained = mp.drain_ready(1, &np);
        assert_eq!(drained.len(), 1);
        assert_eq!(mp.total_bytes(), size_b);

        // Remove the other
        np.set(from, 1);
        let id_b = novai_codec::txid_v1(&make_signed_tx(&sk, &vk, 1, 5, b"bbb")).unwrap();
        mp.remove(&id_b);
        assert_eq!(mp.total_bytes(), 0);
    }

    // -----------------------------------------
    // Dynamic fee floor tests
    // -----------------------------------------

    #[test]
    fn dynamic_fee_floor_overrides_base_when_higher() {
        let (sk, vk) = test_keypair(20);
        let from: Address = address_from_pubkey(&vk);

        let mut np = TestNonceProvider::default();
        np.set(from, 0);

        let mut mp = TxMempool::new(5, 10); // base min_fee = 5
        assert_eq!(mp.effective_min_fee(), 5);

        // Set dynamic floor higher than base
        mp.dynamic_fee_floor().store(50, Ordering::Relaxed);
        assert_eq!(mp.effective_min_fee(), 50);

        // Tx with fee=10 should be rejected (below dynamic floor of 50)
        let tx = make_signed_tx(&sk, &vk, 0, 10, b"low");
        let err = mp.insert(tx, &np).unwrap_err();
        assert!(matches!(err, TxMempoolError::FeeTooLow { min_fee: 50, .. }));

        // Tx with fee=50 should be accepted
        let tx = make_signed_tx(&sk, &vk, 0, 50, b"ok");
        mp.insert(tx, &np).unwrap();
    }

    #[test]
    fn base_min_fee_used_when_dynamic_is_lower() {
        let (sk, vk) = test_keypair(21);
        let from: Address = address_from_pubkey(&vk);

        let mut np = TestNonceProvider::default();
        np.set(from, 0);

        let mut mp = TxMempool::new(100, 10); // base min_fee = 100
        mp.dynamic_fee_floor().store(10, Ordering::Relaxed); // dynamic = 10 (lower)
        assert_eq!(mp.effective_min_fee(), 100); // base wins

        // Tx with fee=50 rejected (below base of 100)
        let tx = make_signed_tx(&sk, &vk, 0, 50, b"low");
        let err = mp.insert(tx, &np).unwrap_err();
        assert!(matches!(
            err,
            TxMempoolError::FeeTooLow { min_fee: 100, .. }
        ));
    }

    #[test]
    fn dynamic_fee_floor_zero_is_effectively_disabled() {
        let (sk, vk) = test_keypair(22);
        let from: Address = address_from_pubkey(&vk);

        let mut np = TestNonceProvider::default();
        np.set(from, 0);

        let mut mp = TxMempool::new(1, 10);
        // Default dynamic floor is 0 — base min_fee of 1 wins
        assert_eq!(mp.effective_min_fee(), 1);

        let tx = make_signed_tx(&sk, &vk, 0, 1, b"min");
        mp.insert(tx, &np).unwrap();
    }

    use std::sync::atomic::Ordering;

    #[test]
    fn dynamic_fee_floor_arc_is_shared() {
        let mp = TxMempool::new(1, 10);
        let floor = mp.dynamic_fee_floor();
        floor.store(42, Ordering::Relaxed);
        assert_eq!(mp.effective_min_fee(), 42);
    }

    // -----------------------------------------
    // Threat score deprioritization tests
    // -----------------------------------------

    #[test]
    fn high_threat_tx_sorted_behind_unthreatened() {
        // Two senders: sender1 has high fee but high threat score,
        // sender2 has lower fee but no threat.
        let (sk1, vk1) = test_keypair(30);
        let (sk2, vk2) = test_keypair(31);
        let from1: Address = address_from_pubkey(&vk1);
        let from2: Address = address_from_pubkey(&vk2);

        let mut np = TestNonceProvider::default();
        np.set(from1, 0);
        np.set(from2, 0);

        let mut mp = TxMempool::new(1, 10);

        // sender1: fee=100, threat_score=80 → effective=100*(100-80)/100 = 20
        // sender2: fee=50,  threat_score=0  → effective=50
        let tx1 = make_signed_tx(&sk1, &vk1, 0, 100, b"threat");
        let tx2 = make_signed_tx(&sk2, &vk2, 0, 50, b"clean");

        mp.insert(tx1, &np).unwrap();
        mp.insert(tx2, &np).unwrap();

        // Set threat score for sender1
        {
            let (scores, empty_flag) = mp.threat_scores();
            let mut map = scores.lock().unwrap();
            map.insert(from1, 80);
            empty_flag.store(false, Ordering::Relaxed);
        }

        let drained = mp.drain_ready(10, &np);
        assert_eq!(drained.len(), 2);
        // sender2 (effective=50) should come before sender1 (effective=20)
        assert_eq!(drained[0].payload, b"clean");
        assert_eq!(drained[1].payload, b"threat");
    }

    #[test]
    fn zero_threat_score_unaffected() {
        let (sk1, vk1) = test_keypair(32);
        let (sk2, vk2) = test_keypair(33);
        let from1: Address = address_from_pubkey(&vk1);
        let from2: Address = address_from_pubkey(&vk2);

        let mut np = TestNonceProvider::default();
        np.set(from1, 0);
        np.set(from2, 0);

        let mut mp = TxMempool::new(1, 10);

        // Both have zero threat → normal fee priority
        let tx_hi = make_signed_tx(&sk1, &vk1, 0, 100, b"hi");
        let tx_lo = make_signed_tx(&sk2, &vk2, 0, 10, b"lo");

        mp.insert(tx_hi, &np).unwrap();
        mp.insert(tx_lo, &np).unwrap();

        let drained = mp.drain_ready(10, &np);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].payload, b"hi");
        assert_eq!(drained[1].payload, b"lo");
    }

    #[test]
    fn max_threat_still_drainable() {
        // Score=100 → effective_fee=0, but transaction MUST still be returned
        let (sk, vk) = test_keypair(34);
        let from: Address = address_from_pubkey(&vk);

        let mut np = TestNonceProvider::default();
        np.set(from, 0);

        let mut mp = TxMempool::new(1, 10);
        let tx = make_signed_tx(&sk, &vk, 0, 1000, b"maxed");
        mp.insert(tx, &np).unwrap();

        {
            let (scores, empty_flag) = mp.threat_scores();
            let mut map = scores.lock().unwrap();
            map.insert(from, 100);
            empty_flag.store(false, Ordering::Relaxed);
        }

        let drained = mp.drain_ready(10, &np);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].payload, b"maxed");
    }

    #[test]
    fn threat_scores_arc_is_shared() {
        let mp = TxMempool::new(1, 10);
        let (scores, empty_flag) = mp.threat_scores();
        let addr = [0x42u8; 32];
        scores.lock().unwrap().insert(addr, 75);
        empty_flag.store(false, Ordering::Relaxed);

        // Mempool reads the same map
        let (mp_scores, _) = mp.threat_scores();
        assert_eq!(*mp_scores.lock().unwrap().get(&addr).unwrap(), 75);
    }
}
