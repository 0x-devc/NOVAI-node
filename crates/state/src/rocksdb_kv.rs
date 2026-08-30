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
    BlockBasedOptions, Cache, ColumnFamily, ColumnFamilyDescriptor, Options, WriteBatch,
    WriteOptions, DB,
};

#[cfg(feature = "rocksdb")]
use std::path::Path;

/// On-disk byte attribution of a database's live SST files against one
/// half-open key range (gate G0, the `novai_db_bytes` gauges).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DbSizeByRange {
    /// Every live SST file in every column family.
    pub total: u64,
    /// Files whose whole key span lies inside the queried range.
    pub in_range: u64,
    /// Files that cross a range boundary, or whose key span RocksDB did not
    /// report, and so cannot be attributed to either side.
    pub straddling: u64,
}

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
        // that grows with the working set. 4MB is sufficient for our access
        // pattern (sequential block reads, small QC lookups).
        let block_cache = Cache::new_lru_cache(4 * 1024 * 1024);
        let mut table_opts = BlockBasedOptions::default();
        table_opts.set_block_cache(&block_cache);

        // Write-heavy blockchain workload tuning (memory-bounded):
        // - 8MB write buffer × 2 memtables × 2 CFs = 32MB max write buffer mem.
        // - 32MB L0 target file size keeps SST files small for fast compaction.
        // - Dynamic level sizing optimizes LSM tree shape automatically.
        // - 2 background jobs balances flush + compaction parallelism with
        //   CPU contention on resource-shared hosts.
        // - LZ4 compression on hot levels (fast), Zstd on bottommost (best
        //   ratio for cold data).
        // - Relaxed L0 write stall thresholds prevent stalls at scale.
        // - bytes_per_sync = 1MB smooths SST and WAL fsyncs into chunks
        //   instead of bursty per-write fsyncs, reducing IO spike latency.
        opts.set_write_buffer_size(8 * 1024 * 1024);
        opts.set_max_write_buffer_number(2);
        opts.set_target_file_size_base(32 * 1024 * 1024);
        opts.set_level_compaction_dynamic_level_bytes(true);
        opts.set_max_background_jobs(2);
        opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
        opts.set_bottommost_compression_type(rocksdb::DBCompressionType::Zstd);
        opts.set_block_based_table_factory(&table_opts);
        opts.set_bytes_per_sync(1024 * 1024);
        opts.set_wal_bytes_per_sync(1024 * 1024);
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
            o.set_write_buffer_size(8 * 1024 * 1024);
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

    /// Synchronously flush the default column family's memtable to L0.
    ///
    /// Bug 1 latent concern B (see `docs/gate3-bug1-diagnosis.md` Risk 2):
    /// the forced compaction at `crates/node/src/main.rs:303-304` previously
    /// ran without a preceding flush. RocksDB's WAL is bandwidth-fsynced
    /// (`set_bytes_per_sync(1MB)` / `set_wal_bytes_per_sync(1MB)` in
    /// `RocksKv::open`), so recently-written default-CF ops could live in
    /// the memtable and an unfsynced WAL segment when compaction starts. A
    /// crash in that window would lose those ops. Calling this method
    /// before `compact_range_default` guarantees the memtable is persisted
    /// to L0 SST files before compaction runs.
    ///
    /// # Errors
    /// Returns the underlying RocksDB error if the flush fails.
    pub fn flush_default(&self) -> Result<(), rocksdb::Error> {
        let cf = self
            .db
            .cf_handle(CF_DEFAULT)
            .expect("default column family must exist");
        self.db.flush_cf(cf)
    }

    /// Create a consistent, independently openable copy of this database at
    /// `path`, across BOTH column families (gate F5 Stage 2).
    ///
    /// This is the only way to capture a point-in-time image of a RUNNING
    /// node. There is no single atomic filesystem boundary otherwise: a commit
    /// is two separate write batches (the consensus batch and the execution
    /// batch), so a plain directory copy of a live node smears across the copy
    /// duration and can land mid-commit.
    ///
    /// COMMIT-PATH COST. The caller holds the database lock for the duration
    /// of this call, so what it does matters: RocksDB flushes the memtables and
    /// then HARD LINKS the SST files. It does not read, rewrite or copy the
    /// data. Cost is therefore a flush plus a directory of links, independent
    /// of database size, and emphatically NOT the full-scan and tree-rebuild
    /// work that turns a checkpoint into a servable snapshot. That work runs
    /// off this lock, against the created checkpoint, and cannot run here
    /// because it needs no handle to this database at all.
    ///
    /// `path` must not already exist; RocksDB creates it and fails otherwise,
    /// which is the behaviour the caller wants (never silently reuse a stale
    /// checkpoint).
    ///
    /// # Errors
    /// Returns the underlying RocksDB error if the checkpoint cannot be
    /// created.
    pub fn create_checkpoint(&self, path: impl AsRef<Path>) -> Result<(), rocksdb::Error> {
        rocksdb::checkpoint::Checkpoint::new(&self.db)?.create_checkpoint(path)
    }

    /// Attribute the live SST bytes against the half-open key range
    /// `[lo, hi)` (gate G0, the `novai_db_bytes` gauges).
    ///
    /// Answers "how much of this database is family X", which for
    /// `smt/node/` is the question of how much of a node's disk is dead SMT
    /// versions. The SMT writes 256 nodes per state key updated, is content
    /// addressed so a changed subtree never overwrites its predecessor, and
    /// the 50k prune deletes only `consensus/blocks/` and `consensus/qcs/`.
    /// Nothing collects the remainder, and until now nothing measured it
    /// either: there is no disk metric in the node at all.
    ///
    /// Attribution is by SST file, not by key. RocksDB reports each live
    /// file's exact size and its exact first and last key, so a file whose
    /// whole key span lies inside the range is attributed exactly, with no
    /// estimation anywhere. Only files that CROSS a boundary are ambiguous,
    /// and those are reported separately as `straddling` rather than being
    /// silently pushed to one side. At the configured 32 MiB target file
    /// size a real database has hundreds of files and at most a couple can
    /// straddle any one boundary, so the ambiguity is small and, more
    /// importantly, visible.
    ///
    /// Sizes are compressed on-disk bytes, so they answer a `du`, not a
    /// logical key-and-value sum. That is the number the disk question wants.
    ///
    /// LIVE SST FILES ONLY. Data still in a memtable or an unflushed WAL is
    /// not counted, so a freshly started node reports 0 until its first
    /// flush even though its directory is not empty. On a 25 GB node the SST
    /// files are essentially all of it and the distinction does not matter,
    /// but 0 here means "nothing flushed yet", not "no database".
    ///
    /// Cost is metadata only: no key-range estimation, no iteration, no data
    /// block reads. It is nonetheless a whole-database call and belongs on a
    /// timer, never on the commit path.
    ///
    /// # Errors
    /// Returns the underlying RocksDB error if the file listing fails.
    pub fn live_bytes_in_range(
        &self,
        lo: &[u8],
        hi: &[u8],
    ) -> Result<DbSizeByRange, rocksdb::Error> {
        let mut sizes = DbSizeByRange::default();
        for file in self.db.live_files()? {
            let size = file.size as u64;
            sizes.total += size;
            // `end_key` is the file's LARGEST key and is inclusive, so a file
            // sits wholly inside the half-open [lo, hi) when its first key is
            // at or after lo and its last key is strictly before hi.
            match (file.start_key.as_deref(), file.end_key.as_deref()) {
                (Some(start), Some(end)) => {
                    if end < lo || start >= hi {
                        // Wholly outside: counted in the total and nowhere else.
                    } else if start >= lo && end < hi {
                        sizes.in_range += size;
                    } else {
                        sizes.straddling += size;
                    }
                }
                // A file whose key span RocksDB did not report cannot be
                // attributed. Call it ambiguous rather than guessing a side:
                // guessing would put the error inside the number the gauge
                // exists to produce, where nothing could see it.
                _ => sizes.straddling += size,
            }
        }
        Ok(sizes)
    }

    /// Visit every `(key, value)` under `prefix` WITHOUT materialising them.
    ///
    /// [`Kv::scan_prefix`] collects into a `Vec`, which is fine for the small
    /// families the RPC reads but catastrophic for a whole-database scan: the
    /// SMT node store has roughly 256 records per authenticated leaf and is
    /// never garbage collected, so a real node's key set is tens of millions of
    /// rows. Measured on this tree, collecting them costs about 277 bytes per
    /// row, which is 1.8 GB for a live tree of 27,308 leaves and several times
    /// that once the dead nodes are counted. A caller that only wants the
    /// authenticated leaves would pay all of it and then discard 99 percent.
    ///
    /// This streams instead, so peak memory is the caller's retained subset
    /// rather than the whole database.
    ///
    /// # Errors
    /// Returns the underlying RocksDB error if iteration fails.
    pub fn for_each_prefix<F>(&self, prefix: &[u8], mut f: F) -> Result<(), rocksdb::Error>
    where
        F: FnMut(&[u8], &[u8]),
    {
        use rocksdb::IteratorMode;

        let cf = self.cf_for_key(prefix);
        let iter = self
            .db
            .iterator_cf(cf, IteratorMode::From(prefix, rocksdb::Direction::Forward));
        for item in iter {
            let (key, value) = item?;
            if !key.starts_with(prefix) {
                break;
            }
            f(&key, &value);
        }
        Ok(())
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

    /// Durable put: fsync the WAL before returning (gate 9).
    ///
    /// `put`/`apply_batch` use default write options, which only bandwidth-fsync
    /// the WAL (`set_wal_bytes_per_sync` in `open`), so a recent write can sit in
    /// an unfsynced WAL segment and be lost on a crash. This forces a per-write
    /// fsync via `WriteOptions::set_sync`, so the value is on stable storage once
    /// this returns. Used only for the once-per-block vote high-water mark, so the
    /// fsync cost is constant in block size and never scales with transactions.
    fn put_synced(&mut self, key: &[u8], value: &[u8]) -> Result<(), Self::Error> {
        let cf = self.cf_for_key(key);
        let mut opts = WriteOptions::default();
        opts.set_sync(true);
        self.db.put_cf_opt(cf, key, value, &opts)
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

    /// Fill a family with enough rows to make a flush produce a real SST.
    fn fill(db: &mut RocksKv, prefix: &[u8], rows: usize) {
        let mut ops = Vec::with_capacity(rows);
        for i in 0..rows {
            let mut key = prefix.to_vec();
            key.extend_from_slice(&(i as u64).to_be_bytes());
            // Incompressible-ish values, so LZ4 cannot collapse the two
            // families to indistinguishable sizes.
            let value: Vec<u8> = (0..96u32)
                .map(|b| (b.wrapping_mul(2_654_435_761).wrapping_add(i as u32) % 251) as u8)
                .collect();
            ops.push(WriteOp::Put(key, value));
        }
        db.apply_batch(&ops).unwrap();
        db.flush_default().unwrap();
    }

    // ==========================================================================
    // Gate G0: the db-bytes split
    //
    // The question this gauge exists to answer is how much of a node's ~25 GB
    // is dead SMT versions. `smt/node/` is content-addressed, so a changed
    // subtree yields a NEW key and never overwrites the old one, and the 50k
    // prune deletes only `consensus/blocks/` and `consensus/qcs/`. Nothing
    // collects the rest.
    //
    // The failure mode these pins exist to catch is a gauge that reports the
    // database total under a prefix label. That number looks entirely
    // plausible on a dashboard, is wrong by exactly the amount anyone would
    // want to know, and nothing else in the system would contradict it.
    // ==========================================================================

    #[test]
    fn live_bytes_in_range_separates_smt_nodes_from_the_rest() {
        let dir = tempdir().unwrap();
        let mut db = RocksKv::open(dir.path().join("db_split")).unwrap();

        // Each family is flushed on its own, so each lands in its own SST with
        // a key span that does not cross the boundary. Ten times as many SMT
        // rows as account rows, mirroring the real shape: 256 SMT nodes are
        // written per state key updated.
        fill(&mut db, b"accounts/", 400);
        fill(&mut db, b"smt/node/", 4_000);

        let sizes = db
            .live_bytes_in_range(b"smt/node/", b"smt/node0")
            .unwrap();
        let other = sizes.total - sizes.in_range - sizes.straddling;

        assert!(sizes.total > 0, "the database must have live SST files");
        assert!(
            sizes.in_range > 0,
            "the smt/node/ family must be attributed, got 0 of {}",
            sizes.total
        );
        // THE anti-relabelling assertion. A gauge that returns the total under
        // the smt label leaves nothing outside it and fails right here.
        assert!(
            other > 0,
            "attributing the whole database to smt/node/ is a relabelled total, \
             not a split: total {} in_range {} straddling {}",
            sizes.total,
            sizes.in_range,
            sizes.straddling
        );
        assert!(
            sizes.in_range < sizes.total,
            "the smt share must be a proper part of the total"
        );
        // The split must account for every byte, with nothing invented and
        // nothing dropped.
        assert_eq!(
            sizes.in_range + sizes.straddling + other,
            sizes.total,
            "the three buckets must partition the total exactly"
        );
        // Ten times the rows must show up as the larger share, which is the
        // directional claim the 25 GB question actually rests on.
        assert!(
            sizes.in_range > other,
            "4,000 smt rows must outweigh 400 account rows: in_range {} other {}",
            sizes.in_range,
            other
        );
    }

    #[test]
    fn live_bytes_in_range_reports_zero_for_a_family_that_is_absent() {
        let dir = tempdir().unwrap();
        let mut db = RocksKv::open(dir.path().join("db_absent")).unwrap();

        // Only accounts. Querying the SMT range must find nothing, rather than
        // falling back to the total.
        fill(&mut db, b"accounts/", 400);

        let sizes = db
            .live_bytes_in_range(b"smt/node/", b"smt/node0")
            .unwrap();
        assert!(sizes.total > 0, "the accounts family must be on disk");
        assert_eq!(
            sizes.in_range, 0,
            "no smt/node/ rows exist, so no bytes may be attributed to it"
        );
        assert_eq!(sizes.straddling, 0);
    }

    #[test]
    fn live_bytes_in_range_is_symmetric_under_the_complementary_query() {
        let dir = tempdir().unwrap();
        let mut db = RocksKv::open(dir.path().join("db_symmetric")).unwrap();

        fill(&mut db, b"accounts/", 800);
        fill(&mut db, b"smt/node/", 800);

        let smt = db.live_bytes_in_range(b"smt/node/", b"smt/node0").unwrap();
        let acct = db.live_bytes_in_range(b"accounts/", b"accounts0").unwrap();

        assert_eq!(smt.total, acct.total, "the total cannot depend on the query");
        // Querying the other family must move the attributed bytes to the other
        // family. A stub keyed to the total reports the same in_range for both
        // and fails here even if it somehow survived the partition assertions.
        assert!(smt.in_range > 0 && acct.in_range > 0);
        assert!(
            smt.in_range + acct.in_range <= smt.total,
            "two disjoint families cannot together exceed the total"
        );
    }

    #[test]
    fn live_bytes_in_range_reports_a_boundary_crossing_file_as_ambiguous() {
        let dir = tempdir().unwrap();
        let mut db = RocksKv::open(dir.path().join("db_straddle")).unwrap();

        // ONE flush carrying both families, so the resulting SST spans from
        // `accounts/` to `smt/node/` and crosses the queried boundary. This is
        // not a contrived case: compaction merges families on a real node, so
        // boundary-crossing files are the normal steady state and the honest
        // answer for them is "cannot attribute", not a coin flip.
        let mut ops = Vec::new();
        for i in 0..400u64 {
            let mut k = b"accounts/".to_vec();
            k.extend_from_slice(&i.to_be_bytes());
            ops.push(WriteOp::Put(k, vec![0xa5; 96]));
            let mut k = b"smt/node/".to_vec();
            k.extend_from_slice(&i.to_be_bytes());
            ops.push(WriteOp::Put(k, vec![0x5a; 96]));
        }
        db.apply_batch(&ops).unwrap();
        db.flush_default().unwrap();

        let sizes = db.live_bytes_in_range(b"smt/node/", b"smt/node0").unwrap();
        assert!(sizes.total > 0);
        assert_eq!(
            sizes.straddling, sizes.total,
            "a file spanning the boundary must be reported as ambiguous"
        );
        assert_eq!(
            sizes.in_range, 0,
            "a boundary-crossing file must not be claimed as in-range: \
             folding ambiguity into the answer hides the error inside the \
             number the gauge exists to produce"
        );
    }

    #[test]
    fn live_bytes_total_matches_the_sst_files_on_disk() {
        // The plan asks for the gauge to be checked against an independent du
        // of the database directory. This is that check, read through the
        // filesystem rather than through RocksDB's own metadata, so the two
        // instruments are genuinely independent. SST files only: the total is
        // defined as live SST bytes and deliberately excludes the WAL,
        // MANIFEST, OPTIONS and LOG, which are neither state nor growing.
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("db_du");
        let mut db = RocksKv::open(&db_path).unwrap();
        fill(&mut db, b"accounts/", 400);
        fill(&mut db, b"smt/node/", 4_000);

        let reported = db.live_bytes_in_range(b"smt/node/", b"smt/node0").unwrap();

        let mut on_disk = 0u64;
        for entry in std::fs::read_dir(&db_path).unwrap() {
            let entry = entry.unwrap();
            if entry.path().extension().is_some_and(|e| e == "sst") {
                on_disk += entry.metadata().unwrap().len();
            }
        }

        assert!(on_disk > 0, "the fixture must have produced SST files");
        assert_eq!(
            reported.total, on_disk,
            "the reported total must equal the SST bytes an independent \
             directory walk finds"
        );
    }

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
