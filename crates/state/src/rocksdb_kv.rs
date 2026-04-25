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
use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamily, ColumnFamilyDescriptor, Options, WriteBatch, DB,
};

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
        // H-10: paranoid_checks REMOVED — caused 75→7 blocks/sec degradation
        // over 2.5M blocks. RocksDB already verifies block checksums on read
        // by default; paranoid_checks added redundant re-verification on every
        // operation plus compaction checks that compound with DB growth.
        // Cap open files to prevent FD exhaustion when multiple instances
        // share a single machine (e.g., 4-node testnet on one server).
        opts.set_max_open_files(256);

        // Explicit LRU block cache shared across all CFs.
        // Without this, RocksDB allocates an unbounded default block cache
        // that grows with the working set. 8MB is sufficient for our access
        // pattern (sequential block reads, small QC lookups).
        let block_cache = Cache::new_lru_cache(8 * 1024 * 1024);
        let mut table_opts = BlockBasedOptions::default();
        table_opts.set_block_cache(&block_cache);

        // Write-heavy blockchain workload tuning (memory-bounded):
        // - 16MB write buffer (down from 64MB) → bounds memory at
        //   16MB × 2 memtables × 2 CFs = 64MB max for write buffers.
        //   At our ~100KB/sec write rate, each buffer fills in ~160s —
        //   still 100x over-provisioned vs flush latency.
        // - 2 memtables (down from 3) → caps write buffer memory
        // - 32MB L0 target file size → smaller SST files, faster compaction
        // - Dynamic level sizing → optimizes LSM tree shape automatically
        // - 4 background jobs → parallel flush + compaction
        // - LZ4 compression → ~50% size reduction, fast enough for hot path
        // - Zstd for bottommost level → best compression ratio for cold data
        // - Relaxed L0 write stall thresholds → prevent stalls at scale
        opts.set_write_buffer_size(16 * 1024 * 1024);
        opts.set_max_write_buffer_number(2);
        opts.set_target_file_size_base(32 * 1024 * 1024);
        opts.set_level_compaction_dynamic_level_bytes(true);
        opts.set_max_background_jobs(4);
        opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
        opts.set_bottommost_compression_type(rocksdb::DBCompressionType::Zstd);
        opts.set_block_based_table_factory(&table_opts);
        // Prevent write stalls under compaction pressure at scale.
        // Defaults (slowdown=20, stop=36) can trigger on long-running
        // chains with continuous insert/delete cycles. Raising these
        // thresholds keeps the consensus hot path unblocked.
        opts.set_level_zero_slowdown_writes_trigger(36);
        opts.set_level_zero_stop_writes_trigger(64);

        // Build column family descriptors with full write-heavy tuning.
        // CF-level options override DB-level options for per-CF settings
        // (write buffers, compression, compaction triggers), so we must
        // replicate the tuning on each CF descriptor.
        let cf_opts = || {
            let mut o = Options::default();
            o.set_write_buffer_size(16 * 1024 * 1024);
            o.set_max_write_buffer_number(2);
            o.set_target_file_size_base(32 * 1024 * 1024);
            o.set_level_compaction_dynamic_level_bytes(true);
            o.set_compression_type(rocksdb::DBCompressionType::Lz4);
            o.set_bottommost_compression_type(rocksdb::DBCompressionType::Zstd);
            o.set_level_zero_slowdown_writes_trigger(36);
            o.set_level_zero_stop_writes_trigger(64);
            // Share the block cache across CFs to bound total memory
            let mut cf_table_opts = BlockBasedOptions::default();
            cf_table_opts.set_block_cache(&block_cache);
            o.set_block_based_table_factory(&cf_table_opts);
            o
        };
        let cf_descriptors = vec![
            ColumnFamilyDescriptor::new(CF_DEFAULT, cf_opts()),
            // Always include nnpx CF — create_missing_column_families handles
            // the case where the DB exists without it (migration support).
            ColumnFamilyDescriptor::new(CF_NNPX, cf_opts()),
        ];

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

    /// Force a compaction over the half-open range `[start, end)` on the
    /// default column family.
    ///
    /// `persist_commit_atomic` writes `WriteOp::Delete` tombstones for blocks
    /// and QCs older than `PRUNE_RETAIN_BLOCKS`, but RocksDB only frees the
    /// underlying SST bytes when a compaction visits the range. On a
    /// long-running chain (millions of heights), background compaction may
    /// not get there for a long time. Periodically forcing a compaction over
    /// the pruned range bounds disk usage near the retention window.
    ///
    /// `start = None` means "from the start of the keyspace"; same for `end`.
    /// This is a blocking call but is fast over already-tombstoned data.
    pub fn compact_range_default(&self, start: Option<&[u8]>, end: Option<&[u8]>) {
        let cf = self
            .db
            .cf_handle(CF_DEFAULT)
            .expect("default column family must exist");
        self.db.compact_range_cf(cf, start, end);
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
