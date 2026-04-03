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
use std::sync::Arc;

/// A sender account with signing capability and nonce tracking.
#[derive(Debug)]
pub struct SenderAccount {
    /// Deterministic index used to derive this account.
    #[allow(dead_code)]
    pub index: usize,
    /// Ed25519 signing key.
    pub signing_key: SigningKey,
    /// Ed25519 verifying key (public key).
    pub verifying_key: VerifyingKey,
    /// Derived address: blake3(pubkey).
    pub address: Address,
    /// Current nonce (atomically updated).
    nonce: AtomicU64,
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
        }
    }

    /// Get next sender in round-robin fashion.
    ///
    /// This provides fair distribution across senders for load testing.
    pub fn next_sender(&self) -> Arc<SenderAccount> {
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

    /// Get all accounts (for reporting).
    #[allow(dead_code)]
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
                assert!(all_nonces.insert(n), "Duplicate nonce: {}", n);
            }
        }

        // Should have exactly 1000 unique nonces (0..999)
        assert_eq!(all_nonces.len(), 1000);
        assert_eq!(acc.current_nonce(), 1000);
    }
}
