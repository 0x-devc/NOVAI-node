//! novai-node
//!
//! Purpose: Consensus node binary and library.

use std::sync::{Mutex, MutexGuard};

pub mod consensus_node;
pub mod exec_apply;
pub mod faucet_rate_limit;
pub mod metrics;
pub mod rpc;
pub mod snapshot;

/// Extension trait for poison-safe mutex locking (H-04).
///
/// Recovers from poisoned mutexes instead of panicking, preventing
/// one thread's panic from cascading to all threads via mutex poisoning.
///
/// A poisoned mutex indicates a thread panicked while holding the lock.
/// The inner data is still accessible and the lock can be re-acquired.
pub trait MutexExt<T> {
    /// Lock the mutex, recovering if it was poisoned by a panicked thread.
    fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> MutexExt<T> for Mutex<T> {
    fn lock_or_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| {
            tracing::error!("Mutex was poisoned (thread panic), recovering");
            poisoned.into_inner()
        })
    }
}
