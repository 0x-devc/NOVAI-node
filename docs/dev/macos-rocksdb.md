# macOS RocksDB Build Setup

Building `novai-state` with the `rocksdb` feature on macOS requires `libclang` for the `bindgen` dependency.

## Install LLVM (Homebrew)
```bash
brew install llvm
```

## Configure Environment (Current Shell)

Run these commands in your current terminal session:
```bash
eval "$(/opt/homebrew/bin/brew shellenv)"
export LIBCLANG_PATH="$(brew --prefix llvm)/lib"
export DYLD_FALLBACK_LIBRARY_PATH="$LIBCLANG_PATH"
```

Verify the installation:
```bash
ls -la "$LIBCLANG_PATH/libclang.dylib"
```

You should see the `libclang.dylib` file exists.

## Make Permanent (Optional)

To avoid setting these variables every time, add them to your shell profile:

**For `~/.zshrc` (if using zsh):**
```bash
cat >> ~/.zshrc << 'EOF'
# novai-state rocksdb (bindgen needs libclang)
eval "$(/opt/homebrew/bin/brew shellenv)"
export LIBCLANG_PATH="$(brew --prefix llvm)/lib"
export DYLD_FALLBACK_LIBRARY_PATH="$LIBCLANG_PATH"
EOF
```

**For `~/.zprofile` (alternative):**
```bash
cat >> ~/.zprofile << 'EOF'
# novai-state rocksdb (bindgen needs libclang)
eval "$(/opt/homebrew/bin/brew shellenv)"
export LIBCLANG_PATH="$(brew --prefix llvm)/lib"
export DYLD_FALLBACK_LIBRARY_PATH="$LIBCLANG_PATH"
EOF
```

Then reload your profile:
```bash
source ~/.zshrc
# or
source ~/.zprofile
```

## Build and Test

Once configured, you can build with RocksDB support:
```bash
cargo test -p novai-state --features rocksdb
cargo clippy -p novai-state --features rocksdb -- -D warnings
```

## Troubleshooting

If you see errors like `Unable to find libclang`, verify:
```bash
echo "$LIBCLANG_PATH"
echo "$DYLD_FALLBACK_LIBRARY_PATH"
ls -la "$LIBCLANG_PATH/libclang.dylib"
```

All three commands should produce valid output.
