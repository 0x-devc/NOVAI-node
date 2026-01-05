# macOS: building `novai-state` with RocksDB

`novai-state` supports an optional RocksDB backend:

```bash
cargo test -p novai-state --features rocksdb
cargo clippy -p novai-state --features rocksdb -- -D warnings
