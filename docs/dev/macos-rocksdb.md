# macOS: building `novai-state` with RocksDB

`novai-state --features rocksdb` depends on `librocksdb-sys`, which uses `bindgen` and requires `libclang` to be available at build time.

If you see an error like:

- `dyld: Library not loaded: @rpath/libclang.dylib`

…it means your build can’t find `libclang`.

## Install LLVM (Homebrew)

```bash
brew install llvm
