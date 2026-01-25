use crate::{is_nnpx_key, Kv, KvBatch, WriteOp};

/// Deterministic in-memory KV store for tests and local execution.
///
/// Invariants:
/// - Single-threaded intended usage.
/// - Key comparisons are bytewise.
/// - Operations are deterministic.
/// - NNPX keys (`b"nnpx/"` prefix) are stored in a separate logical space (Week 22).
///
/// Failure modes:
/// - None; Error type is () to simplify Week 3.
///
/// # Column Family Simulation (Week 22)
///
/// This store simulates RocksDB column families by using two separate entry vectors:
/// - `entries_default`: Public chain data (accounts, consensus, AI, etc.)
/// - `entries_nnpx`: Private data (keys starting with `b"nnpx/"`)
///
/// This ensures that private data is logically isolated from public data,
/// matching the production RocksDB column family behavior.
#[derive(Default, Debug, Clone)]
pub struct MemKv {
    /// Default column family: public chain data.
    entries_default: Vec<(Vec<u8>, Vec<u8>)>,
    /// NNPX column family: private data (Week 22).
    entries_nnpx: Vec<(Vec<u8>, Vec<u8>)>,
}

impl MemKv {
    pub fn new() -> Self {
        Self {
            entries_default: Vec::new(),
            entries_nnpx: Vec::new(),
        }
    }

    /// Get the appropriate entry store for a key based on its prefix.
    #[inline]
    fn entries_for_key(&self, key: &[u8]) -> &Vec<(Vec<u8>, Vec<u8>)> {
        if is_nnpx_key(key) {
            &self.entries_nnpx
        } else {
            &self.entries_default
        }
    }

    /// Get the appropriate mutable entry store for a key based on its prefix.
    #[inline]
    fn entries_for_key_mut(&mut self, key: &[u8]) -> &mut Vec<(Vec<u8>, Vec<u8>)> {
        if is_nnpx_key(key) {
            &mut self.entries_nnpx
        } else {
            &mut self.entries_default
        }
    }

    fn find_index_in(entries: &[(Vec<u8>, Vec<u8>)], key: &[u8]) -> Option<usize> {
        entries.iter().position(|(k, _)| k.as_slice() == key)
    }
}

impl Kv for MemKv {
    type Error = ();

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        let entries = self.entries_for_key(key);
        Ok(entries
            .iter()
            .find(|(k, _)| k.as_slice() == key)
            .map(|(_, v)| v.clone()))
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Self::Error> {
        let entries = self.entries_for_key_mut(key);
        if let Some(i) = Self::find_index_in(entries, key) {
            entries[i].1 = value.to_vec();
            return Ok(());
        }
        entries.push((key.to_vec(), value.to_vec()));
        Ok(())
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), Self::Error> {
        let entries = self.entries_for_key_mut(key);
        if let Some(i) = Self::find_index_in(entries, key) {
            // O(1) deletion; order is not preserved (still deterministic).
            entries.swap_remove(i);
        }
        Ok(())
    }

    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, Self::Error> {
        // Route to appropriate column family based on prefix
        let entries = self.entries_for_key(prefix);
        let mut results: Vec<_> = entries
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .cloned()
            .collect();
        // Sort lexicographically for deterministic ordering (consensus-critical)
        results.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(results)
    }
}

impl KvBatch for MemKv {
    /// Apply multiple operations atomically (all-or-nothing).
    ///
    /// Implementation: Clone entries, apply ops to clone, swap on success.
    /// This guarantees that if any op fails, no changes are visible.
    ///
    /// # Column Family Handling (Week 22)
    ///
    /// Operations are routed to the appropriate column family based on key prefix:
    /// - Keys starting with `b"nnpx/"` go to `entries_nnpx`
    /// - All other keys go to `entries_default`
    fn apply_batch(&mut self, ops: &[WriteOp]) -> Result<(), Self::Error> {
        // Clone current state for both column families
        let mut tmp_default = self.entries_default.clone();
        let mut tmp_nnpx = self.entries_nnpx.clone();

        // Apply all ops to the appropriate clone
        for op in ops {
            match op {
                WriteOp::Put(key, value) => {
                    let tmp = if is_nnpx_key(key) {
                        &mut tmp_nnpx
                    } else {
                        &mut tmp_default
                    };
                    // Find existing entry
                    if let Some(idx) = tmp.iter().position(|(k, _)| k.as_slice() == key.as_slice())
                    {
                        tmp[idx].1 = value.clone();
                    } else {
                        tmp.push((key.clone(), value.clone()));
                    }
                }
                WriteOp::Delete(key) => {
                    let tmp = if is_nnpx_key(key) {
                        &mut tmp_nnpx
                    } else {
                        &mut tmp_default
                    };
                    if let Some(idx) = tmp.iter().position(|(k, _)| k.as_slice() == key.as_slice())
                    {
                        tmp.swap_remove(idx);
                    }
                }
            }
        }

        // Commit: swap the entire state at once (atomic from external view)
        self.entries_default = tmp_default;
        self.entries_nnpx = tmp_nnpx;
        Ok(())
    }
}
