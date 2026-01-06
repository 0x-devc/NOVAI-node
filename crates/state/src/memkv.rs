use crate::{Kv, KvBatch, WriteOp};

/// Deterministic in-memory KV store for tests and local execution.
///
/// Invariants:
/// - Single-threaded intended usage.
/// - Key comparisons are bytewise.
/// - Operations are deterministic.
///
/// Failure modes:
/// - None; Error type is () to simplify Week 3.
#[derive(Default, Debug, Clone)]
pub struct MemKv {
    entries: Vec<(Vec<u8>, Vec<u8>)>,
}

impl MemKv {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn find_index(&self, key: &[u8]) -> Option<usize> {
        self.entries.iter().position(|(k, _)| k.as_slice() == key)
    }
}

impl Kv for MemKv {
    type Error = ();

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        Ok(self
            .entries
            .iter()
            .find(|(k, _)| k.as_slice() == key)
            .map(|(_, v)| v.clone()))
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Self::Error> {
        if let Some(i) = self.find_index(key) {
            self.entries[i].1 = value.to_vec();
            return Ok(());
        }
        self.entries.push((key.to_vec(), value.to_vec()));
        Ok(())
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), Self::Error> {
        if let Some(i) = self.find_index(key) {
            // O(1) deletion; order is not preserved (still deterministic).
            self.entries.swap_remove(i);
        }
        Ok(())
    }
}

impl KvBatch for MemKv {
    /// Apply multiple operations atomically (all-or-nothing).
    ///
    /// Implementation: Clone entries, apply ops to clone, swap on success.
    /// This guarantees that if any op fails, no changes are visible.
    fn apply_batch(&mut self, ops: &[WriteOp]) -> Result<(), Self::Error> {
        // Clone current state
        let mut tmp = self.entries.clone();

        // Apply all ops to the clone
        for op in ops {
            match op {
                WriteOp::Put(key, value) => {
                    // Find existing entry
                    if let Some(idx) = tmp.iter().position(|(k, _)| k.as_slice() == key.as_slice())
                    {
                        tmp[idx].1 = value.clone();
                    } else {
                        tmp.push((key.clone(), value.clone()));
                    }
                }
                WriteOp::Delete(key) => {
                    if let Some(idx) = tmp.iter().position(|(k, _)| k.as_slice() == key.as_slice())
                    {
                        tmp.swap_remove(idx);
                    }
                }
            }
        }

        // Commit: swap the entire state at once (atomic from external view)
        self.entries = tmp;
        Ok(())
    }
}
