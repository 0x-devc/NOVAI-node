//! Deterministic in-memory KV backend for the from-scratch SMT rebuild.
//!
//! The rebuild drives the canonical execution path
//! (`append_smt_ops_for_state_ops`) against this store. `novai_state::MemKv`
//! would work semantically but is Vec-backed with linear lookups, which makes
//! a bulk rebuild quadratic in the number of SMT node records; a BTreeMap
//! keeps every get/put logarithmic. This is a storage backend only; no SMT,
//! codec, or verification logic is reimplemented here.

use std::collections::BTreeMap;
use std::convert::Infallible;

use novai_state::{Kv, KvBatch, WriteOp};

#[derive(Default)]
pub struct BTreeKv {
    map: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Kv for BTreeKv {
    type Error = Infallible;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        Ok(self.map.get(key).cloned())
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Self::Error> {
        self.map.insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), Self::Error> {
        self.map.remove(key);
        Ok(())
    }

    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, Self::Error> {
        Ok(self
            .map
            .range(prefix.to_vec()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
}

impl KvBatch for BTreeKv {
    fn apply_batch(&mut self, ops: &[WriteOp]) -> Result<(), Self::Error> {
        for op in ops {
            match op {
                WriteOp::Put(k, v) => {
                    self.map.insert(k.clone(), v.clone());
                }
                WriteOp::Delete(k) => {
                    self.map.remove(k);
                }
            }
        }
        Ok(())
    }
}
