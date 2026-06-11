# Build Your First AI Entity on NOVAI in 10 Minutes

By the end you'll have a local 4-node devnet running, an AI entity registered on chain, a signal published from that entity, and a memory object owned by it — all queryable via the CLI and JSON-RPC.

NOVAI's distinguishing feature is that AI entities are protocol-level primitives, not smart contracts. An entity is a first-class on-chain identity that holds its own balance, signs its own transactions, publishes signals, and owns memory objects. This tutorial walks through the lifecycle end-to-end.

---

## Prerequisites

- **Rust stable** (the workspace pins the channel via `rust-toolchain.toml` — `rustup` will pick it up automatically)
- **git**, **bash**, and a Unix-like shell (macOS or Linux)
- **~2 GB free disk** for the build + devnet state
- A free TCP port range: `3030`, `8080–8083`, `9000–9003`

Verify your shell can find Rust:

```bash
rustc --version    # any stable build (>= 1.80) is fine; the toolchain file pins what's actually used
cargo --version
```

---

## Step 1 — Clone & build (~2 min cold, ~10 s warm)

```bash
git clone <novai-repo-url>
cd NOVAI-node
cargo build --release -p novai-node -p novai-cli
```

The first build pulls dependencies and compiles ~16 crates. Subsequent builds are incremental. Two binaries land in `target/release/`: `novai-node` and `novai-cli`.

> **Tip.** The rest of the tutorial invokes the binaries directly (`./target/release/novai-cli …`). If you'd rather have `novai-cli` on your `$PATH`, run `cargo install --path tools/novai-cli` once at the end.

---

## Step 2 — Start the local devnet (~10 s)

In a **separate terminal**, leave this running:

```bash
./scripts/devnet.sh
```

It launches four validators on `127.0.0.1:9000–9003`, exposes JSON-RPC on `127.0.0.1:3030`, and writes per-node logs to `/tmp/node{0,1,2,3}.log`. The script waits 5 seconds for the network to stabilize and then prints `✅ All 4 nodes started!`.

Verify the chain is producing blocks:

```bash
curl -s -X POST http://localhost:3030 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getLatestBlock","params":{},"id":1}'
```

Expected (your `height` and hashes will differ):

```json
{"jsonrpc":"2.0","result":{"block_hash":"641c31de…","height":160,"parent_hash":"…","round":0,"state_root":"8b3d34f1…","tx_count":0},"id":1}
```

If `result` is `null`, give it a couple more seconds — the four nodes need three votes per block (BFT 2f+1) and committing only starts once they're peered.

---

## Step 3 — Generate a creator keypair (~5 s)

The "creator" is a normal account that pays to register the AI entity. We'll generate a fresh keypair and capture its address into a shell variable for reuse.

```bash
./target/release/novai-cli keygen --output /tmp/creator.key
```

Expected:

```
Address: 100ecd018efec7aeec47c5734f75b718bee4d7bea687c22ce527c987c4e095bd
Pubkey:  b2663bd8019f151ca4dcc10eaf524efb542889d774c615894a7191fd8816c002
Key saved to: /tmp/creator.key
```

Capture the address:

```bash
CREATOR=$(./target/release/novai-cli key-info --key-file /tmp/creator.key | awk '/^Address/ {print $2}')
echo "$CREATOR"
```

The key file is a **raw 32-byte ed25519 seed** at file mode `0600`. Treat it like any other private key.

---

## Step 4 — Fund the creator from the faucet (~5 s)

The devnet ships a built-in faucet. It dispenses `10_000_000` test tokens per call, with a global 10-second cooldown and a 1-hour per-address cap.

```bash
./target/release/novai-cli faucet --address "$CREATOR"
./target/release/novai-cli balance --address "$CREATOR"
```

Expected:

```
Faucet dispensed 10000000 tokens
TxID: 8aad151badaf3da553acc2c3fdb3628f2724455db7182a879c2a3240c036dd6b
Balance: 10000000
Nonce:   0
```

Note the nonce is `0`: the faucet sent the tokens **to** the creator, so the creator's outgoing nonce hasn't been used yet.

---

## Step 5 — Generate the entity's signing keypair (~5 s)

In NOVAI, an AI entity has its own ed25519 key. The entity uses this key to sign its own transactions (signal publish, memory CRUD) — independently of the creator key. We'll register the entity with this key in the next step.

```bash
./target/release/novai-cli keygen --output /tmp/entity.key
```

Expected:

```
Address: ad10a8ecc17f20a164c6f570635778954a4fc21876bd8df7204a4cd56af509da
Pubkey:  01c3d94e5e512f1578f058bd9e992bde38bcea77f3cc19a7de4aeb03e73466c0
Key saved to: /tmp/entity.key
```

The entity's signing pubkey ends up stored in the on-chain entity record. The address derived from this pubkey is what you'll use as `tx.from` for entity-signed transactions.

---

## Step 6 — Register the AI entity (~2 s)

Three pieces of identity end up on chain:

| Field | Source | Purpose |
|---|---|---|
| `entity.id` | `blake3("NOVAI_AI_ENTITY_ID_V1" \|\| code_hash \|\| creator_address)` | Canonical primary key — what `getAiEntity` and the index queries use |
| `entity.address` | `blake3("NOVAI_ADDRESS_V1" \|\| entity_pubkey)` | What appears as `tx.from` when the entity signs |
| `entity.pubkey` | the public key from `/tmp/entity.key` | What verifies the entity's signatures |

The chain maintains a reverse index from address to entity.id, so transactions signed by the entity resolve correctly.

`code_hash` is an opaque 32-byte identifier stored on chain. The chain doesn't enforce that it actually hashes any particular code — in production this would be the hash of your AI agent's code or model weights. For the tutorial we use a recognizable placeholder.

```bash
CODE_HASH=0101010101010101010101010101010101010101010101010101010101010101

./target/release/novai-cli ai register-with-key \
  --key-file        /tmp/creator.key \
  --entity-key-file /tmp/entity.key \
  --code-hash       "$CODE_HASH" \
  --initial-balance 50000 \
  --fee             5000 \
  --json
```

Expected:

```json
{"entity_address":"ad10a8ecc1…","entity_id":"63953c6102…","txid":"0d86aad5…"}
```

Capture the IDs:

```bash
ENTITY_ID=$(./target/release/novai-cli ai register-with-key \
  --key-file /tmp/creator.key --entity-key-file /tmp/entity.key \
  --code-hash "$CODE_HASH" --initial-balance 50000 --fee 5000 --json \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["entity_id"])')
```

> **Skip the second invocation** if you captured the JSON from the first call. Or read it back from `ai info` once you know the entity_id you computed locally — the chain derives it deterministically from your creator address and code hash.

The creator's balance has dropped by `initial_balance + fee = 55_000`:

```bash
./target/release/novai-cli balance --address "$CREATOR"
# Balance: 9945000
# Nonce:   1
```

The creator's nonce ticked from 0 to 1 — that's the register tx. Verify the entity itself:

```bash
./target/release/novai-cli ai info --entity-id "$ENTITY_ID"
```

Expected:

```
ID:              63953c6102d6d2f87a8d545073f190835037a7ecdf65a756f5401ab70cbcf5b1
Code Hash:       0101010101010101010101010101010101010101010101010101010101010101
Creator:         100ecd018efec7aeec47c5734f75b718bee4d7bea687c22ce527c987c4e095bd
Autonomy Mode:   Advisory
Capabilities:    0x07
Balance:         50000
Nonce:           0
Pubkey:          01c3d94e5e512f1578f058bd9e992bde38bcea77f3cc19a7de4aeb03e73466c0
Registered At:   6201
Last Active At:  6201
Active:          true
```

The entity has its own balance (`50000` — funded out of the creator) and its own nonce (starts at `0`). Capabilities `0x07` means the bits for `read_public_chain`, `read_memory_objects`, and `emit_proposals` are set — the defaults for `register-with-key`.

---

## Step 7 — Publish a signal from the entity (~2 s)

A signal is a small on-chain commitment to off-chain content (the full payload lives off chain; the chain stores the hash). Twenty-three categories are defined (signal types 0 through 22). The original seven are: `anomaly`, `optimization`, `prediction`, `risk-score`, `audit-report`, `spam-risk`, `congestion-forecast`. The remainder cover reputation, marketplace, staking, composition, proof submission, subscriptions, payments, SLAs, channels, and oracle anchors; see `crates/ai_entities/src/signals.rs` for the canonical enum.

The signal hash is opaque — for the tutorial we use a placeholder. In a real bot you'd compute `blake3(serialized_payload)` and pin the payload in your artifact store.

```bash
SIGNAL_HASH=0202020202020202020202020202020202020202020202020202020202020202

./target/release/novai-cli signal publish \
  --key-file          /tmp/entity.key \
  --signal-hash       "$SIGNAL_HASH" \
  --signal-type       anomaly \
  --issuer-entity-id  "$ENTITY_ID" \
  --fee               1000
```

Expected:

```
Signal commitment submitted
Type:   anomaly
Issuer: 63953c6102…
TxID:   11e128e67be747abe712d166eebf9a26351e23a11e4b30a02bb4b53ab72202f5
```

Note: the signal tx is signed by `/tmp/entity.key` (the **entity** key), not the creator key. The fee is paid out of the entity's own balance (`50_000` → `49_000`).

---

## Step 8 — Create a memory object (~2 s)

Memory objects are entity-owned content-addressed key/value records. Sixteen types are defined (memory object types 0 through 15). The original five are: `chain-summary`, `label-index`, `embedding-commitment`, `anomaly-log`, `statistics-snapshot`. The remainder cover reputation events, ratings, signal catalogs, composition graphs, verification records, delegation grants, subscriptions, service descriptors, VK registrations, SLA agreements, and payment channels; see `crates/ai_entities/src/memory.rs` for the canonical enum. Each entity can own up to 100 objects, capped at 64 KiB each.

```bash
./target/release/novai-cli memory create \
  --key-file    /tmp/entity.key \
  --object-type chain-summary \
  --data        "Tutorial demo: chain summary at registration time" \
  --fee         500
```

Expected:

```
Memory object creation submitted
Type: chain-summary
Size: 49 bytes
TxID: 0c51d5e0fe0409bb79927ee0e50ae97a62209f4e9cfa640ccd309a1d82e2e230
```

Again, the entity key signs and the entity pays the fee.

---

## Step 9 — Read state back (~1 min)

Three queries, then the equivalent JSON-RPC call so you can see what's actually on the wire.

**Entity state** (notice `Nonce: 2` and `Balance: 48500` — both txs landed):

```bash
./target/release/novai-cli ai info --entity-id "$ENTITY_ID"
```

```
…
Balance:         48500
Nonce:           2
Last Active At:  6689
…
```

**Memory objects owned by the entity:**

```bash
./target/release/novai-cli memory list --entity-id "$ENTITY_ID"
```

```
OBJECT ID                                                           TYPE  SIZE  CREATED  UPDATED
bfb962b13c6cf62ccaaa56f82e9efd24f259b684dce2598090f366c255f677b2     0     49    6689     6689
```

**Signals published by the entity** — query a 9 000-block window ending at the current height. The chain caps each query at 10 000 blocks, so a fixed range like `0–10000` will miss your signal as soon as the chain runs past block 10 000.

```bash
LATEST=$(curl -s -X POST http://localhost:3030 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getLatestBlock","params":{},"id":1}' \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["height"])')
START=$(( LATEST > 9000 ? LATEST - 9000 : 0 ))

./target/release/novai-cli signal by-issuer \
  --issuer "$ENTITY_ID" \
  --start  "$START" \
  --end    "$LATEST"
```

**Same query as raw JSON-RPC:**

```bash
curl -s -X POST http://localhost:3030 \
  -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"method\":\"novai_getAiEntity\",\"params\":{\"entity_id\":\"$ENTITY_ID\"},\"id\":1}"
```

Returns the same entity record the CLI shows, but as JSON — the shape every SDK speaks against.

---

## Cleanup

```bash
pkill -f 'novai-node run'
```

This stops all four validators. The state directory (`~/.novai/data/validator-{0..3}`) persists; remove it with `rm -rf ~/.novai/data` for a clean reset before the next run.

---

## Where next

You now have a working AI entity on a local NOVAI chain. From here:

- **TypeScript SDK** — same flow from `@novai/sdk`. See `sdk/novai-sdk-ts/examples/` (in progress).
- **Rust SDK** — same flow from the `novai-sdk` crate. See `sdk/novai-sdk/examples/` (in progress).
- **Full RPC reference** — every method with request/response shapes: `docs/RPC_REFERENCE.md`.
- **Architecture deep dive** — how consensus, execution, and state commitment fit together: `docs/ARCHITECTURE.md` (in progress).

> **One-liner install** if you want `novai-cli` on your `$PATH`:
> ```bash
> cargo install --path tools/novai-cli
> ```
