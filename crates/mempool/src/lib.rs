use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::Arc;

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

use novai_codec::{encode_tx_v1_unsigned, txid_v1};
use novai_crypto::{address_from_pubkey, pubkey_from_bytes, verify_bytes};
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
    FeeTooLow { min_fee: u64, got: u64 },
    NonceTooLow { expected: u64, got: u64 },
    InvalidSignature,
    InvalidPublicKey,
    AddressMismatch,
    CodecError,
    TxTooLarge { size: usize, max: usize },
    MempoolFull { current: usize, max: usize },
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
}

impl TxMempool {
    pub fn new(min_fee: u64, fairness_cap_per_sender: usize) -> Self {
        Self {
            min_fee,
            fairness_cap_per_sender: fairness_cap_per_sender.max(1),
            by_id: HashMap::new(),
            total_bytes: 0,
        }
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
        // min fee
        if tx.fee < self.min_fee {
            return Err(TxMempoolError::FeeTooLow {
                min_fee: self.min_fee,
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

        // Verify address matches pubkey
        let vk = pubkey_from_bytes(&tx.pubkey).map_err(|_| TxMempoolError::InvalidPublicKey)?;
        let expected_addr = address_from_pubkey(&vk);
        if tx.from != expected_addr {
            return Err(TxMempoolError::AddressMismatch);
        }

        // canonical unsigned bytes
        let unsigned = encode_tx_v1_unsigned(&tx).map_err(|_| TxMempoolError::CodecError)?;

        // verify signature
        if !verify_bytes(&vk, &unsigned, &tx.sig) {
            return Err(TxMempoolError::InvalidSignature);
        }

        // compute txid (hash of canonical unsigned bytes)
        let id = txid_v1(&tx).map_err(|_| TxMempoolError::CodecError)?;

        // dedupe
        if self.by_id.contains_key(&id) {
            return Err(TxMempoolError::Duplicate);
        }

        // size limits (defense-in-depth: also checked at RPC layer)
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

        self.total_bytes += size;
        self.by_id.insert(id, tx);
        Ok(id)
    }

    /// Drain up to `max` ready transactions under fee-priority + fairness.
    pub fn drain_ready(&mut self, max: usize, nonce_provider: &impl NonceProvider) -> Vec<TxV1> {
        if max == 0 || self.by_id.is_empty() {
            return Vec::new();
        }

        // Gather ready candidates.
        let mut candidates: Vec<(u64, TxId, Address)> = Vec::with_capacity(self.by_id.len());

        for (id, tx) in &self.by_id {
            let expected = nonce_provider.expected_nonce(&tx.from);
            if tx.nonce == expected {
                candidates.push((tx.fee, *id, tx.from));
            }
        }

        // Sort: fee DESC, txid ASC.
        candidates.sort_by(|(fee_a, id_a, _), (fee_b, id_b, _)| {
            fee_b.cmp(fee_a).then_with(|| id_a.cmp(id_b))
        });

        let cap = max.min(candidates.len());
        let mut out: Vec<TxV1> = Vec::with_capacity(cap);
        let mut per_sender: HashMap<Address, usize> = HashMap::new();
        let mut selected_ids: Vec<TxId> = Vec::with_capacity(cap);

        for (_fee, id, from) in candidates {
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
        self.by_id.insert(id, tx);
        Ok(id)
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
    use novai_codec::encode_tx_v1_unsigned;
    use novai_crypto::{address_from_pubkey, sign_bytes};
    use novai_types::{SignatureBytes, TxVersion};

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

        let unsigned = encode_tx_v1_unsigned(&tx).expect("unsigned encode");
        let sig: SignatureBytes = sign_bytes(from_sk, &unsigned);
        tx.sig = sig;
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

        let unsigned = encode_tx_v1_unsigned(&tx).expect("unsigned encode");
        tx.sig = sign_bytes(&sk2, &unsigned);

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
            "expected TxTooLarge, got {:?}",
            err
        );
    }

    #[test]
    fn rejects_mempool_full() {
        let (sk, vk) = test_keypair(11);
        let from: Address = address_from_pubkey(&vk);

        let mut np = TestNonceProvider::default();
        np.set(from, 0);

        let mut mp = TxMempool::new(1, 1000);

        // Fill the mempool close to MAX_MEMPOOL_BYTES.
        // Each tx with 1-byte payload is 150 bytes encoded (TX_V1_OVERHEAD + 1).
        // Insert enough txs to nearly fill 64MB, then verify the next one fails.
        // For speed, we use a large payload per tx to fill faster.
        let payload_size = 64 * 1024; // 64KB payload per tx
        let tx_size = novai_codec::TX_V1_OVERHEAD + payload_size;
        let count_to_fill = novai_types::MAX_MEMPOOL_BYTES / tx_size; // ~1023

        for i in 0..count_to_fill {
            let payload = vec![0xBB; payload_size];
            let tx = make_signed_tx(&sk, &vk, i as u64, 1, &payload);
            mp.insert(tx, &np).unwrap();
            np.set(from, (i + 1) as u64);
        }

        // Now the mempool should be nearly full. One more should fail.
        let payload = vec![0xCC; payload_size];
        let tx = make_signed_tx(&sk, &vk, count_to_fill as u64, 1, &payload);
        let err = mp.insert(tx, &np).unwrap_err();
        assert!(
            matches!(err, TxMempoolError::MempoolFull { .. }),
            "expected MempoolFull, got {:?}",
            err
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
}
