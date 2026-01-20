//! RocksDB-backed KV implementation (feature-gated).
//!
//! Enable with: `--features rocksdb`

use crate::{Kv, KvBatch, WriteOp};

#[cfg(feature = "rocksdb")]
use rocksdb::{Options, WriteBatch, DB};

#[cfg(feature = "rocksdb")]
use std::path::Path;

/// RocksDB KV store.
///
/// Notes:
/// - Intended for local development / persistence.
/// - Determinism concerns are handled at the execution layer; this is just byte I/O.
#[cfg(feature = "rocksdb")]
#[derive(Debug)]
pub struct RocksKv {
    db: DB,
}

#[cfg(feature = "rocksdb")]
impl RocksKv {
    /// Open (or create) a RocksDB instance at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rocksdb::Error> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let db = DB::open(&opts, path)?;
        Ok(Self { db })
    }
}

#[cfg(feature = "rocksdb")]
impl Kv for RocksKv {
    type Error = rocksdb::Error;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        Ok(self.db.get(key)?.map(|v| v.to_vec()))
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Self::Error> {
        self.db.put(key, value)
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), Self::Error> {
        self.db.delete(key)
    }

    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, Self::Error> {
        use rocksdb::IteratorMode;

        let mut results = Vec::new();
        let iter = self
            .db
            .iterator(IteratorMode::From(prefix, rocksdb::Direction::Forward));

        for item in iter {
            let (key, value) = item?;
            // Stop when we move past the prefix
            if !key.starts_with(prefix) {
                break;
            }
            results.push((key.to_vec(), value.to_vec()));
        }
        // RocksDB iterator is already sorted lexicographically
        Ok(results)
    }
}

#[cfg(feature = "rocksdb")]
impl KvBatch for RocksKv {
    /// Apply multiple operations atomically using RocksDB's WriteBatch.
    ///
    /// RocksDB guarantees that either all operations in a WriteBatch succeed
    /// or none take effect (atomic commit at the DB level).
    fn apply_batch(&mut self, ops: &[WriteOp]) -> Result<(), Self::Error> {
        let mut batch = WriteBatch::default();

        for op in ops {
            match op {
                WriteOp::Put(key, value) => {
                    batch.put(key, value);
                }
                WriteOp::Delete(key) => {
                    batch.delete(key);
                }
            }
        }

        // Atomic commit: all operations succeed or none take effect
        self.db.write(batch)
    }
}

#[cfg(all(test, feature = "rocksdb"))]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn rocks_kv_roundtrip_put_get_delete() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("db");

        let mut db = RocksKv::open(&path).unwrap();

        let k = b"hello";
        let v = b"world";

        assert_eq!(db.get(k).unwrap(), None);

        db.put(k, v).unwrap();
        assert_eq!(db.get(k).unwrap(), Some(v.to_vec()));

        db.delete(k).unwrap();
        assert_eq!(db.get(k).unwrap(), None);
    }

    #[test]
    fn rocks_kv_batch_atomic_commit() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("db_batch");

        let mut db = RocksKv::open(&path).unwrap();

        // Apply batch with multiple operations
        let ops = vec![
            WriteOp::Put(b"key1".to_vec(), b"value1".to_vec()),
            WriteOp::Put(b"key2".to_vec(), b"value2".to_vec()),
            WriteOp::Put(b"key3".to_vec(), b"value3".to_vec()),
        ];

        db.apply_batch(&ops).unwrap();

        // All keys should exist
        assert_eq!(db.get(b"key1").unwrap(), Some(b"value1".to_vec()));
        assert_eq!(db.get(b"key2").unwrap(), Some(b"value2".to_vec()));
        assert_eq!(db.get(b"key3").unwrap(), Some(b"value3".to_vec()));

        // Delete via batch
        let ops = vec![
            WriteOp::Delete(b"key1".to_vec()),
            WriteOp::Delete(b"key2".to_vec()),
        ];

        db.apply_batch(&ops).unwrap();

        // key1 and key2 should be gone, key3 remains
        assert_eq!(db.get(b"key1").unwrap(), None);
        assert_eq!(db.get(b"key2").unwrap(), None);
        assert_eq!(db.get(b"key3").unwrap(), Some(b"value3".to_vec()));
    }
}
