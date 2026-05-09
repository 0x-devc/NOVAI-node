# Get a Local NOVAI Node Running in 5 Minutes

By the end of this guide you will have a 4-node devnet running locally, a funded account, and a verified transfer. To register an AI entity, publish signals, or create memory objects, follow [tutorials/FIRST_AI_ENTITY.md](tutorials/FIRST_AI_ENTITY.md) next. For patterns covering reputation, marketplace, staking, composition, and ZK proofs, see [AI_ENTITY_COOKBOOK.md](AI_ENTITY_COOKBOOK.md).

---

## Prerequisites

- Rust stable. The workspace pins a channel via `rust-toolchain.toml`; `rustup` picks it up automatically.
- `git`, `bash`, macOS or Linux.
- About 2 GB of free disk for build artifacts and devnet state.
- Free TCP ports: `3030`, `8080-8083`, `9000-9003`.

Confirm Rust is installed:

```bash
rustc --version
cargo --version
```

If `cargo build` fails on macOS due to RocksDB, see [dev/macos-rocksdb.md](dev/macos-rocksdb.md).

---

## Build the binaries

```bash
git clone <repo-url>
cd NOVAI-node
cargo build --release -p novai-node -p novai-cli
```

The first build takes about 2 minutes cold. Subsequent builds are incremental. Two binaries land in `target/release/`: `novai-node` and `novai-cli`.

---

## Start the local devnet

In a separate terminal, leave this running:

```bash
./scripts/devnet.sh
```

The script kills any old `novai-node run` processes, then launches four validators on `127.0.0.1:9000-9003`. Only node 0 binds the JSON-RPC port at `127.0.0.1:3030`. Per-node logs land at `/tmp/node{0,1,2,3}.log`. The script waits 5 seconds for peering and prints `All 4 nodes started!`.

Confirm the chain is producing blocks:

```bash
curl -s -X POST http://localhost:3030 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getLatestBlock","params":{},"id":1}'
```

Expected (your `height` and hashes will differ):

```json
{"jsonrpc":"2.0","result":{"block_hash":"641c31de...","height":160,"parent_hash":"...","round":0,"state_root":"8b3d34f1...","tx_count":0},"id":1}
```

If `result` is `null`, wait a few more seconds. The chain needs three of four validators to agree (BFT 2f+1) before the first block commits.

---

## Generate a keypair

In your original terminal:

```bash
./target/release/novai-cli keygen --output /tmp/alice.key
```

Output:

```
Address: <64 hex chars>
Pubkey:  <64 hex chars>
Key saved to: /tmp/alice.key
```

The key file is a raw 32-byte Ed25519 seed at file mode `0600`. Treat it like any other private key.

Capture the address into a shell variable:

```bash
ALICE=$(./target/release/novai-cli key-info --key-file /tmp/alice.key | awk '/^Address/ {print $2}')
echo "$ALICE"
```

---

## Fund the account from the faucet

The devnet runs with `--dev-keys --allow-insecure-dev-keys`, which exposes a development faucet endpoint. Each call dispenses 10,000,000 base units. Cooldowns: 1 hour per address, 10 seconds global.

```bash
./target/release/novai-cli faucet --address "$ALICE"
./target/release/novai-cli balance --address "$ALICE"
```

Expected:

```
Faucet dispensed 10000000 tokens
TxID: <hex>
Balance: 10000000
Nonce:   0
```

The nonce is `0` because the faucet sent tokens *to* Alice. Alice has not signed anything yet.

If `Balance: 0`, your faucet tx has not been included yet. Wait a few seconds and re-run `balance`.

---

## Send a transfer

Generate a recipient key, then transfer:

```bash
./target/release/novai-cli keygen --output /tmp/bob.key
BOB=$(./target/release/novai-cli key-info --key-file /tmp/bob.key | awk '/^Address/ {print $2}')

./target/release/novai-cli transfer \
  --key-file /tmp/alice.key \
  --to       "$BOB" \
  --amount   1000 \
  --fee      100
```

Expected:

```
Transfer submitted
To:     <bob hex>
Amount: 1000
TxID:   <hex>
```

Verify:

```bash
./target/release/novai-cli balance --address "$BOB"
# Balance: 1000
./target/release/novai-cli balance --address "$ALICE"
# Balance: 9998900
# Nonce:   1
```

Alice paid `amount + fee = 1100`. Her nonce ticked to 1.

The minimum transfer fee is 100 base units. The minimum recipient credit is `MIN_ACCOUNT_BALANCE = 1_000` to prevent dust spam on new accounts.

---

## Stop the devnet

```bash
pkill -f 'novai-node run'
```

State persists in `~/.novai/data/validator-{0,1,2,3}`. For a clean reset:

```bash
rm -rf ~/.novai/data
```

---

## Next steps

You have a running chain, a funded account, and a confirmed transfer. From here:

- **Register an AI entity, publish a signal, create a memory object:** [tutorials/FIRST_AI_ENTITY.md](tutorials/FIRST_AI_ENTITY.md). Eight detailed steps.
- **Recipes for reputation, marketplace, staking, composition, and ZK proofs:** [AI_ENTITY_COOKBOOK.md](AI_ENTITY_COOKBOOK.md).
- **Every JSON-RPC method with request and response shapes:** [RPC_REFERENCE.md](RPC_REFERENCE.md).
- **How NOVAI fits together at the protocol level:** [BUILDER_OVERVIEW.md](BUILDER_OVERVIEW.md).

If you would rather have `novai-cli` on your `$PATH`:

```bash
cargo install --path tools/novai-cli
```

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `curl` returns `"result":null` | Chain has not committed its first block | Wait 5-10 seconds and retry |
| Faucet returns `cooldown active` | Per-address (1 hour) or global (10 second) cooldown | Wait, or use a different address |
| `Balance: 0` after faucet | Faucet tx not yet included | Retry `balance` after a few seconds |
| `Connection refused` on port 3030 | Node 0 has not bound RPC yet | Wait, or check `tail /tmp/node0.log` |
| `Address already in use` from devnet.sh | A prior `novai-node run` is still alive | `pkill -f 'novai-node run'` and rerun |
| Build fails on RocksDB (macOS) | Missing system dependency | See [dev/macos-rocksdb.md](dev/macos-rocksdb.md) |
| Transfer rejected with `FeeTooLow` | Fee below the minimum for the tx type | Use `--fee 100` for transfers, `--fee 1000` for signals, `--fee 5000` for entity registration |
| Transfer rejected with `NonceTooLow` | The CLI fetched a stale nonce | Re-run the command. The CLI auto-fetches from the node before signing |
