//! RocksDB-backed KV implementation (feature-gated).
//!
//! Enable with: `--features rocksdb`
//!
//! # Column Family Support (Week 22)
//!
//! This implementation uses two column families:
//! - `default`: Public chain data (accounts, consensus, AI entities, etc.)
//! - `nnpx`: Private data (keys starting with `b"nnpx/"`)
//!
//! Keys are automatically routed to the appropriate column family based on
//! their prefix. This provides physical storage isolation for private data.

use crate::{is_nnpx_key, Kv, KvBatch, WriteOp, CF_DEFAULT, CF_NNPX};

#[cfg(feature = "rocksdb")]
use rocksdb::{ColumnFamily, ColumnFamilyDescriptor, Options, WriteBatch, DB};

#[cfg(feature = "rocksdb")]
use std::path::Path;

/// RocksDB KV store with column family support.
///
/// Notes:
/// - Intended for local development / persistence.
/// - Determinism concerns are handled at the execution layer; this is just byte I/O.
/// - Uses two column families: `default` (public) and `nnpx` (private).
#[cfg(feature = "rocksdb")]
#[derive(Debug)]
pub struct RocksKv {
    db: DB,
}

#[cfg(feature = "rocksdb")]
impl RocksKv {
    /// Open (or create) a RocksDB instance at `path` with column family support.
    ///
    /// # Column Families (Week 22)
    ///
    /// Creates two column families:
    /// - `default`: Public chain data
    /// - `nnpx`: Private data (physically isolated)
    ///
    /// If the database already exists without the `nnpx` column family,
    /// it will be created automatically (migration support).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rocksdb::Error> {
        let path = path.as_ref();
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        // Cap open files to prevent FD exhaustion when multiple instances
        // share a single machine (e.g., 4-node testnet on one server).
        opts.set_max_open_files(256);

        // Try to list existing column families
        let existing_cfs = match DB::list_cf(&opts, path) {
            Ok(cfs) => cfs,
            Err(_) => {
                // Database doesn't exist yet, will be created with both CFs
                vec![CF_DEFAULT.to_string()]
            }
        };

        // Build column family descriptors
        let mut cf_descriptors = Vec::new();

        // Default CF always exists
        cf_descriptors.push(ColumnFamilyDescriptor::new(CF_DEFAULT, Options::default()));

        // Add nnpx CF if it exists or create it
        if existing_cfs.contains(&CF_NNPX.to_string())
            || !existing_cfs.contains(&CF_NNPX.to_string())
        {
            cf_descriptors.push(ColumnFamilyDescriptor::new(CF_NNPX, Options::default()));
        }

        let db = DB::open_cf_descriptors(&opts, path, cf_descriptors)?;
        Ok(Self { db })
    }

    /// Get the column family handle for a key based on its prefix.
    #[inline]
    fn cf_for_key(&self, key: &[u8]) -> &ColumnFamily {
        if is_nnpx_key(key) {
            self.db
                .cf_handle(CF_NNPX)
                .expect("nnpx column family must exist")
        } else {
            self.db
                .cf_handle(CF_DEFAULT)
                .expect("default column family must exist")
        }
    }
}

#[cfg(feature = "rocksdb")]
impl Kv for RocksKv {
    type Error = rocksdb::Error;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        let cf = self.cf_for_key(key);
        Ok(self.db.get_cf(cf, key)?.map(|v| v.to_vec()))
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Self::Error> {
        let cf = self.cf_for_key(key);
        self.db.put_cf(cf, key, value)
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), Self::Error> {
        let cf = self.cf_for_key(key);
        self.db.delete_cf(cf, key)
    }

    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, Self::Error> {
        use rocksdb::IteratorMode;

        let cf = self.cf_for_key(prefix);
        let mut results = Vec::new();
        let iter = self
            .db
            .iterator_cf(cf, IteratorMode::From(prefix, rocksdb::Direction::Forward));

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
    ///
    /// # Column Family Routing (Week 22)
    ///
    /// Operations are automatically routed to the appropriate column family:
    /// - Keys starting with `b"nnpx/"` go to the `nnpx` CF
    /// - All other keys go to the `default` CF
    fn apply_batch(&mut self, ops: &[WriteOp]) -> Result<(), Self::Error> {
        let mut batch = WriteBatch::default();

        for op in ops {
            match op {
                WriteOp::Put(key, value) => {
                    let cf = self.cf_for_key(key);
                    batch.put_cf(cf, key, value);
                }
                WriteOp::Delete(key) => {
                    let cf = self.cf_for_key(key);
                    batch.delete_cf(cf, key);
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
