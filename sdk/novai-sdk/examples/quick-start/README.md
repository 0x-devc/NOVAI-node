# Build a NOVAI Bot in Rust

A working example that drives a local NOVAI node end-to-end through `novai-sdk`: connect, fund an account, transfer tokens, register an AI entity, and verify it on chain. ~140 lines of commented Rust, runs in under a minute.

If this is your first time on NOVAI, walk through [`docs/tutorials/FIRST_AI_ENTITY.md`](../../../../docs/tutorials/FIRST_AI_ENTITY.md) first — it sets up the local devnet this example talks to.

---

## Prerequisites

- **A running local devnet** on `http://localhost:3030`. From the repo root:

  ```bash
  ./scripts/devnet.sh
  ```

  Leave that running in another terminal.

- **Rust stable** — the workspace's `rust-toolchain.toml` pins the channel; `rustup` will pick it up automatically.

That's it. The SDK is a workspace member and the example is a Cargo `--example` target, so you don't need to install or publish anything.

---

## Run it

From the repo root, with a devnet running:

```bash
cargo run --release --example quick-start -p novai-sdk
```

Cargo auto-discovers `examples/quick-start/main.rs` and links it against the SDK. The first compile pulls in `reqwest` + `tokio` + the SDK's workspace deps (~10s on a warm cache); subsequent runs are instant.

---

## Tour `main.rs`

The example is a single async file. Each section below maps to a labelled block.

### 1. Connect

```rust
let client = Client::new("http://localhost:3030");

let latest = client.call("novai_getLatestBlock", json!({})).await?;
if latest.is_null() { /* devnet not running */ }
```

`Client` wraps every JSON-RPC method the node exposes. `call()` is the escape hatch for raw RPC — useful for endpoints the SDK hasn't yet wrapped (like `getLatestBlock`). It returns a `serde_json::Value` you can index into.

### 2. Generate keypairs

```rust
let (sender_sk, sender_pk) = keys::generate();
let sender_addr = keys::address(&sender_pk);
```

`keys::generate()` returns a `(SigningKey, VerifyingKey)` from `ed25519_dalek`. `keys::address()` derives the 32-byte NOVAI address: `blake3("NOVAI_ADDRESS_V1" || pubkey)`. The same scheme used by `novai-cli`, so a key file written by either tool can be loaded by the other.

### 3. Fund the sender

```rust
let (faucet_txid, amount) = client.faucet(&sender_addr).await?;
tokio::time::sleep(Duration::from_millis(1500)).await;
let (balance, nonce) = client.get_balance(&sender_addr).await?;
```

The faucet only works on a node started with `--dev-keys --allow-insecure-dev-keys` (`scripts/devnet.sh` provides this). Dispenses 10,000,000 tokens per call, with a 1-hour cooldown per address and a 10-second global cooldown.

`client.get_balance()` returns `(String, u64)` — balance is a string because it's a `u128` that exceeds JSON number precision; nonce fits in `u64`.

### 4. Transfer

```rust
let tx = tx::transfer(&sender_sk, nonce, 1_000 /* fee */, &recipient_addr, 100_000 /* amount */)?;
let txid = client.submit_tx(&tx).await?;
```

Tx builders return a fully signed `TxV1`. The signature shape is consistent across builders: **`(&signing_key, nonce, fee, …type-specific args)`**. Amounts are `u64` for transfers, `u128` for entity balances. `?` propagates any signing or RPC errors as `novai_sdk::Error`.

### 5. Register an AI entity

```rust
let (_entity_sk, entity_pk) = keys::generate();
let code_hash = [0x01u8; 32]; // opaque placeholder

let reg_tx = tx::register_ai_entity_with_key(
    &sender_sk,
    sender_nonce,
    5_000,                          // fee — must meet MIN_FEE_REGISTER_AI_ENTITY_WITH_KEY
    &code_hash,
    &entity_pk,                     // the entity's own signing key
    AutonomyMode::Gated,
    Capabilities::advisory(),       // read_public_chain + read_memory_objects + emit_proposals
    50_000,                         // initial entity balance
)?;
client.submit_tx(&reg_tx).await?;
```

Two keys are involved: the **creator's** key (which pays the registration fee out of its account) and the **entity's** key (which the entity will use to sign its own future transactions — signal publishes, memory writes, etc.).

`Capabilities::advisory()` packs three bits: `read_public_chain`, `read_memory_objects`, `emit_proposals`. The byte representation is `0x07`. Use `Capabilities::gated()` if the entity also needs `request_execution` (`0x0F`).

### 6. Verify on chain

```rust
let entity_id = tx::compute_entity_id(&code_hash, &sender_addr);
let entity = client.get_ai_entity(&entity_id).await?.ok_or("not found")?;
```

`tx::compute_entity_id()` mirrors the chain's deterministic derivation: `blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator)`. You can compute this client-side without ever talking to a node — useful for predicting the entity id before you submit the registration tx.

`client.get_ai_entity()` returns `Option<AiEntityInfo>` (`None` if the entity doesn't exist).

---

## Real captured output

From a single run on a fresh 4-node devnet:

```
Connected. Chain at height 146.

Sender    address: 8e42929bf9717e94e4a0ec996c3145ec8307f20a1791e154e6666a993187e1a5
Recipient address: a6f998217178125a2b7d7eff21ab6d31e2b0eeca67408fe33d17cdb099e16df5

Faucet dispensed 10000000 tokens (tx a79b00057af78bc1…).
Sender balance: 10000000, nonce: 0

Transfer 100000 tokens → recipient submitted (tx bbea6b1a5f7c3014…).
Sender    balance: 9899000 (was 10000000)
Recipient balance: 100000

Entity registration submitted (tx 6f788bc8c7457596…).

Entity 4bbf3e093aab9c9b… on chain:
  creator:        8e42929bf9717e94e4a0ec996c3145ec8307f20a1791e154e6666a993187e1a5
  pubkey:         3c49bb1c3feb1338bc3e2da146fd152e9f11ade620afe5b95726097339d26e4a
  balance:        50000
  autonomy_mode:  1 (Gated)
  capabilities:   0x07
  registered_at:  block 189
  is_active:      true
```

Walking the numbers:

| Account | Before | After | Δ |
|---|---|---|---|
| Sender | `10_000_000` | `9_899_000` | `−101_000` (= 100,000 transfer + 1,000 fee) |
| Recipient | `0` | `100_000` | `+100_000` |

After registration the sender's balance would also drop by `50_000 + 5_000 = 55_000` (initial entity balance + fee); the example doesn't print it but a follow-up `client.get_balance()` would confirm.

`capabilities: 0x07` matches the bits we set (read_public_chain | read_memory_objects | emit_proposals).

---

## Troubleshooting

- **`Chain has not committed any blocks yet`** — the devnet hasn't reached its first commit. With four validators it usually takes 1–2 seconds; if it lingers, check `/tmp/node{0,1,2,3}.log`.
- **`error: failed to run custom build command for openssl-sys`** — you need OpenSSL development headers. On macOS: `brew install openssl@3`; on Debian/Ubuntu: `sudo apt-get install libssl-dev pkg-config`. (`reqwest` pulls in `native-tls` by default.)
- **`RPC error -32601: Method not found`** — the running node is older than the SDK and doesn't speak the method. Update both to the same revision.
- **Faucet returns "dev mode disabled"** — the node was launched without `--dev-keys --allow-insecure-dev-keys`. Use `scripts/devnet.sh` (which sets these), not a hand-rolled launch.

---

## What's next

- **TypeScript SDK tutorial** — same flow in TypeScript: [`sdk/novai-sdk-ts/examples/quick-start/`](../../../novai-sdk-ts/examples/quick-start/).
- **Build a bot** — extend this example to publish signals and own memory objects (Phase 3.2 of the roadmap).
- **Full RPC reference** — every method the node exposes, with request/response shapes (Phase 1.4, in progress).
