# NOVAI JSON-RPC Reference

Every endpoint a NOVAI node exposes, with request/response shapes, error codes, and curl examples captured from a live devnet.

For the runnable end-to-end flow, see [`docs/tutorials/FIRST_AI_ENTITY.md`](tutorials/FIRST_AI_ENTITY.md). For SDK wrappers in TypeScript or Rust, see [`sdk/novai-sdk-ts/examples/quick-start/`](../sdk/novai-sdk-ts/examples/quick-start/) and [`sdk/novai-sdk/examples/quick-start/`](../sdk/novai-sdk/examples/quick-start/).

---

## Transport

- **Protocol**: JSON-RPC 2.0 over HTTP POST.
- **Content-Type**: `application/json`.
- **Default endpoint**: `http://127.0.0.1:3030`.
- **Override**: `novai-node run --rpc-port <N> --rpc-bind <host>`. The default binding is loopback only; expose externally only via a reverse proxy (auth, TLS).

A request is a JSON object with `{ jsonrpc: "2.0", method, params, id }`. A successful response is `{ jsonrpc: "2.0", result, id }`. An error response is `{ jsonrpc: "2.0", error: { code, message }, id }`.

## Conventions

- **Hex encoding**: all 32-byte fields (addresses, hashes, txids, entity IDs) are lowercase hex with no `0x` prefix, exactly 64 characters. The node accepts upper/mixed case on input but normalises to lowercase on output.
- **Numerics**: `u64` fields (heights, nonces, fees) are JSON numbers. **`u128` fields (balances) are decimal strings** to avoid JSON number-precision loss.
- **Booleans**: standard `true` / `false`.
- **Optional results**: queries that may not find a record return `null` in the relevant field (`entity: null`, the entire `result` for blocks/txs); they do **not** raise errors.

## Limits

| Limit | Value | Source |
|---|---|---|
| Request rate (per source IP) | 100 req/s | `MAX_RPC_REQUESTS_PER_SEC` |
| Concurrent in-flight RPCs | 64 | `MAX_CONCURRENT_RPC` |
| Request body size | 512 KiB | `MAX_RPC_BODY_SIZE` |
| Response body size | 10 MiB | `MAX_RPC_RESPONSE_SIZE` |
| Tx hex (submit) | 256 KiB (= 128 KiB binary) | `MAX_TX_SIZE × 2` |
| Signal query height range | 10 000 blocks | `MAX_SIGNAL_QUERY_RANGE` |

Rate-limited requests get HTTP `429 Too Many Requests` (no JSON-RPC envelope).

---

## Method index

| Category | Method | Brief |
|---|---|---|
| Blocks | [`novai_getLatestBlock`](#novai_getlatestblock) | Latest committed block header |
| | [`novai_getBlockByHeight`](#novai_getblockbyheight) | Block header at a given height |
| | [`novai_getBlockByHash`](#novai_getblockbyhash) | Block header by its hash |
| Accounts | [`novai_getBalance`](#novai_getbalance) | Account balance and nonce |
| | [`novai_getNonce`](#novai_getnonce) | Account expected nonce |
| Transactions | [`novai_submitTransaction`](#novai_submittransaction) | Submit a signed tx |
| | [`novai_getTransaction`](#novai_gettransaction) | Tx receipt by txid |
| AI entities | [`novai_getAiEntity`](#novai_getaientity) | AI entity record by id |
| Memory objects | [`novai_getMemoryObjects`](#novai_getmemoryobjects) | All memory objects owned by entity |
| Signals | [`novai_getSignalsByHeight`](#novai_getsignalsbyheight) | Signals at a height |
| | [`novai_getSignalsByIssuer`](#novai_getsignalsbyissuer) | Signals from an entity (range) |
| | [`novai_getSignalsByType`](#novai_getsignalsbytype) | Signals by type (range) |
| Dev | [`novai_faucet`](#novai_faucet) | Mint test tokens (dev mode only) |

All examples assume `URL=http://localhost:3030` and a `./scripts/devnet.sh` running.

---

## Block methods

### `novai_getLatestBlock`

Returns the header of the most recently committed block.

**Params**: none — pass `{}`.

**Result** (`null` if no blocks have committed yet):

```jsonc
{
  "block_hash":  "<hex32>",   // canonical hash of this block
  "parent_hash": "<hex32>",   // canonical hash of the parent block
  "state_root":  "<hex32>",   // SMT root after applying this block
  "height":      <u64>,
  "round":       <u64>,       // consensus round in which the block was committed
  "tx_count":    <u64>        // number of transactions in this block
}
```

**Errors**: only the global ones (`-32600` malformed envelope, `-32601` unknown method).

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getLatestBlock","params":{},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "block_hash":  "6358124aa37e167d9b0f1ba8c43b2b5b76595862694c36f413a9acd173512689",
    "parent_hash": "f4224ed3b546ee33fc9006c406816bfc60bda5f33c650c30ff2f7f321f29120f",
    "state_root":  "be66f7485fc259ae01b6bd139b9fa26636660291d227cdb90aa9f7db6d3e97ee",
    "height": 2574,
    "round": 0,
    "tx_count": 0
  },
  "id": 1
}
```

---

### `novai_getBlockByHeight`

Returns the header for a given height.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `height` | `u64` | must be ≤ committed height |

**Result**: same shape as `getLatestBlock`, or `null` if no such height (this should be unreachable given the validation).

**Errors**:

| Code | When |
|---|---|
| `-32602` | missing/non-numeric `height` |
| `-32602` | `height` exceeds the committed height (response includes the actual cap) |
| `-32002` | DB read failure |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getBlockByHeight","params":{"height":377},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "block_hash":  "f13b0fe415672bdbae53432bfffb7558ffa314db5824b321517230e2aea84084",
    "parent_hash": "0a29f578d57abd845403b3cce8fae281da364dd015bb440192e41fcac2a8fec9",
    "state_root":  "be66f7485fc259ae01b6bd139b9fa26636660291d227cdb90aa9f7db6d3e97ee",
    "height": 377,
    "round": 0,
    "tx_count": 1
  },
  "id": 1
}
```

---

### `novai_getBlockByHash`

Returns the header for a given block hash. Useful for following parent chains backwards.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `hash` | `hex32` | exactly 64 hex chars |

**Result**: same shape as `getLatestBlock`, or `null` if the hash isn't known.

**Errors**:

| Code | When |
|---|---|
| `-32602` | hash isn't 32 bytes |
| `-32002` | DB read failure |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getBlockByHash","params":{"hash":"f13b0fe415672bdbae53432bfffb7558ffa314db5824b321517230e2aea84084"},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "block_hash":  "f13b0fe415672bdbae53432bfffb7558ffa314db5824b321517230e2aea84084",
    "parent_hash": "0a29f578d57abd845403b3cce8fae281da364dd015bb440192e41fcac2a8fec9",
    "state_root":  "be66f7485fc259ae01b6bd139b9fa26636660291d227cdb90aa9f7db6d3e97ee",
    "height": 377,
    "round": 0,
    "tx_count": 1
  },
  "id": 1
}
```

---

## Account methods

### `novai_getBalance`

Returns an account's current balance (u128) and expected nonce.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `address` | `hex32` | 32-byte account address |

**Result**:

```jsonc
{
  "balance": "<decimal-string>",  // u128, returned as a string
  "nonce":   <u64>                // next nonce a tx from this account must use
}
```

**Errors**:

| Code | When |
|---|---|
| `-32602` | address isn't 32 bytes |
| `-32002` | DB read failure |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getBalance","params":{"address":"4069c3445cdef6af7cfa421b9d0ebefa7b4f039ad2d74ad9c6af0e5e05cc71ee"},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": { "balance": "9945000", "nonce": 1 },
  "id": 1
}
```

---

### `novai_getNonce`

Returns just the expected nonce for an account. Cheaper than `getBalance` if you don't need the balance.

**Params**: same as `getBalance`.

**Result**:

```jsonc
{ "nonce": <u64> }
```

**Errors**: same as `getBalance`.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getNonce","params":{"address":"4069c3445cdef6af7cfa421b9d0ebefa7b4f039ad2d74ad9c6af0e5e05cc71ee"},"id":1}'
```

```json
{ "jsonrpc": "2.0", "result": { "nonce": 1 }, "id": 1 }
```

---

## Transaction methods

### `novai_submitTransaction`

Submits a hex-encoded, signed `TxV1` to the mempool. The node validates the signature, address derivation, fee minimum, and size, then broadcasts it via P2P. It does **not** wait for inclusion.

**Building the hex.** Use one of the SDKs — see [`sdk/novai-sdk-ts/`](../sdk/novai-sdk-ts/) (TypeScript) or [`sdk/novai-sdk/`](../sdk/novai-sdk/) (Rust). Hand-rolling the 149-byte canonical encoding plus ed25519 signature is possible but error-prone; the `txid` returned by the node only confirms the bytes parsed and the signature verified — not that the tx will succeed at execution.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `tx` | `string` | hex-encoded signed tx; max 262 144 chars (= 128 KiB binary) |

**Result**:

```jsonc
{ "txid": "<hex32>" }   // blake3 of the unsigned canonical encoding
```

**Errors**:

| Code | When |
|---|---|
| `-32602` | params malformed |
| `-32000` | tx hex too large; tx decode failed; tx exceeds binary size cap; signature invalid; mempool rejected (duplicate, nonce out of order, fee below minimum) |
| `-32002` | DB read failure during validation |

**Example** (request shown abridged — the `tx` value is a real ~300-byte hex blob built via the Rust SDK):

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_submitTransaction","params":{"tx":"01<224 hex chars>"},"id":1}'
```

```json
{ "jsonrpc": "2.0", "result": { "txid": "f5796d3963b94b59017fa9f2fa7b015956cbd082c32c04f69b1d88ae007f140e" }, "id": 1 }
```

A returned `txid` means **accepted into the mempool**. To confirm execution, poll [`novai_getTransaction`](#novai_gettransaction) until the txid resolves to a `(block_height, tx_index)`.

---

### `novai_getTransaction`

Returns receipt metadata for a committed transaction. Returns `null` if the txid isn't known (either invalid, still in mempool, or evicted).

**Params**:

| Field | Type | Notes |
|---|---|---|
| `txid` | `hex32` | as returned by `submitTransaction` |

**Result**:

```jsonc
{
  "block_height": <u64>,    // height of the block the tx landed in
  "tx_index":     <u64>,    // 0-based index within that block's tx list
  "from":         "<hex32>",
  "nonce":        <u64>,
  "fee":          <u64>,
  "payload_len":  <u64>     // length of the inner payload in bytes
}
```

The receipt does **not** carry the payload itself or the success/failure status — execution outcome is reflected in subsequent state (e.g. balance debit, entity record). To distinguish "committed but execution rejected" from "still pending", combine with the relevant state read.

**Errors**:

| Code | When |
|---|---|
| `-32602` | txid isn't 32 bytes |
| `-32002` | DB read failure |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getTransaction","params":{"txid":"f5796d3963b94b59017fa9f2fa7b015956cbd082c32c04f69b1d88ae007f140e"},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "block_height": 377,
    "tx_index": 0,
    "from": "4069c3445cdef6af7cfa421b9d0ebefa7b4f039ad2d74ad9c6af0e5e05cc71ee",
    "nonce": 0,
    "fee": 5000,
    "payload_len": 83
  },
  "id": 1
}
```

---

## AI entity methods

### `novai_getAiEntity`

Returns the full on-chain record for an AI entity. Returns `{ "entity": null }` (note: not `null` at the top level) if the entity doesn't exist.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `entity_id` | `hex32` | the canonical entity id, `blake3("NOVAI_AI_ENTITY_ID_V1" \|\| code_hash \|\| creator)` |

**Result**:

```jsonc
{
  "entity": {
    "id":               "<hex32>",          // canonical entity id (matches the param)
    "code_hash":        "<hex32>",          // identifier supplied at registration
    "creator":          "<hex32>",          // creator account address
    "autonomy_mode":    <u8>,               // 0 = Advisory, 1 = Gated, 2 reserved
    "capabilities":     <u8>,               // bitfield (see Capabilities)
    "economic_balance": "<decimal-string>", // u128
    "nonce":            <u64>,              // next nonce for entity-signed txs
    "pubkey":           "<hex32>",          // ed25519 verifying key (zeros if registered without a key)
    "memory_root":      "<hex32>",          // (reserved; currently zero)
    "params_root":      "<hex32>",          // (reserved; currently zero)
    "registered_at":    <u64>,              // block height of the register tx
    "last_active_at":   <u64>,              // most recent height at which this entity sent a tx
    "is_active":        true                // false after a successful deactivation proposal
  }
}
```

**Capabilities bits** (combine to form the byte):

| Bit | Flag | Meaning |
|---|---|---|
| 0 (`0x01`) | `read_public_chain` | can read blocks, txs, accounts |
| 1 (`0x02`) | `read_memory_objects` | can create/update/delete memory objects |
| 2 (`0x04`) | `emit_proposals` | can publish signal commitments and submit proposals |
| 3 (`0x08`) | `request_execution` | can request gated Tier-1/2 execution |
| 4 (`0x10`) | `read_nnpx_derived` | can read schema-validated NNPX derived views |

**Errors**:

| Code | When |
|---|---|
| `-32602` | `entity_id` isn't 32 bytes |
| `-32002` | DB read failure |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getAiEntity","params":{"entity_id":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1"},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "entity": {
      "id":               "44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1",
      "code_hash":        "0101010101010101010101010101010101010101010101010101010101010101",
      "creator":          "4069c3445cdef6af7cfa421b9d0ebefa7b4f039ad2d74ad9c6af0e5e05cc71ee",
      "autonomy_mode":    0,
      "capabilities":     7,
      "economic_balance": "48500",
      "nonce":            2,
      "pubkey":           "60356527146eacd35696afd3a8fd71d42312c2aa4262ede7afd2f879bf6dfa36",
      "memory_root":      "0000000000000000000000000000000000000000000000000000000000000000",
      "params_root":      "0000000000000000000000000000000000000000000000000000000000000000",
      "registered_at":    377,
      "last_active_at":   497,
      "is_active":        true
    }
  },
  "id": 1
}
```

---

## Memory object methods

### `novai_getMemoryObjects`

Returns every memory object owned by an entity. Order is by `object_id`.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `entity_id` | `hex32` | canonical entity id (same value used in `getAiEntity`) |

**Result**:

```jsonc
{
  "objects": [
    {
      "object_id":    "<hex32>",
      "object_type":  <u8>,            // 0..=4 (see Memory types below)
      "owner_entity": "<hex32>",       // always equal to the param
      "created_at":   <u64>,           // block height
      "updated_at":   <u64>,           // block height
      "data":         "<hex>",         // hex-encoded payload
      "data_size":    <u64>            // payload length in bytes
    }
  ]
}
```

**Memory object types**:

| Byte | Variant |
|---|---|
| 0 | `chain-summary` |
| 1 | `label-index` |
| 2 | `embedding-commitment` |
| 3 | `anomaly-log` |
| 4 | `statistics-snapshot` |

**Limits**: max 100 objects per entity, 64 KiB per object.

**Errors**:

| Code | When |
|---|---|
| `-32602` | `entity_id` isn't 32 bytes |
| `-32002` | DB read failure |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getMemoryObjects","params":{"entity_id":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1"},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "objects": [
      {
        "object_id":    "ba6c5de5a537dd07f44389d7ea794969b96305fd7cea845d05be23e256561103",
        "object_type":  0,
        "owner_entity": "44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1",
        "created_at":   497,
        "updated_at":   497,
        "data":         "64656d6f206461746120666f7220525043207265666572656e6365",
        "data_size":    27
      }
    ]
  },
  "id": 1
}
```

The `data` field decodes to the original bytes (`echo 64656d6f206461746120666f7220525043207265666572656e6365 | xxd -r -p` → `demo data for RPC reference`).

---

## Signal methods

All three signal queries return the same shape. Differences are how the index is keyed: by height alone, by issuer entity within a height range, or by signal type within a height range.

```jsonc
{
  "signals": [
    {
      "commitment_hash": "<hex32>",   // signed commitment to off-chain content
      "signal_type":     <u8>,        // 0..=6 (see Signal types below)
      "height":          <u64>,
      "issuer":          "<hex32>"    // canonical entity id
    }
  ]
}
```

**Signal types**:

| Byte | Variant |
|---|---|
| 0 | `anomaly` |
| 1 | `optimization` |
| 2 | `prediction` |
| 3 | `risk-score` |
| 4 | `audit-report` |
| 5 | `spam-risk` |
| 6 | `congestion-forecast` |

**Common errors**:

| Code | When |
|---|---|
| `-32602` | param decode failure or invalid hex length |
| `-32602` | `end_height − start_height > 10000` (range queries) |
| `-32002` | DB read failure |

---

### `novai_getSignalsByHeight`

Returns every signal published at a specific block height.

**Params**:

| Field | Type |
|---|---|
| `height` | `u64` |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getSignalsByHeight","params":{"height":453},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "signals": [
      {
        "commitment_hash": "0202020202020202020202020202020202020202020202020202020202020202",
        "signal_type": 0,
        "height": 453,
        "issuer": "44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1"
      }
    ]
  },
  "id": 1
}
```

---

### `novai_getSignalsByIssuer`

Returns every signal published by a given entity within a height window.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `issuer` | `hex32` | canonical entity id |
| `start_height` | `u64` | inclusive |
| `end_height` | `u64` | inclusive; must satisfy `end - start ≤ 10000` |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getSignalsByIssuer","params":{"issuer":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1","start_height":0,"end_height":1000},"id":1}'
```

Response shape identical to `getSignalsByHeight`.

---

### `novai_getSignalsByType`

Returns every signal of a given type within a height window.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `signal_type` | `u8` | one of `0..=6` |
| `start_height` | `u64` | inclusive |
| `end_height` | `u64` | inclusive; must satisfy `end - start ≤ 10000` |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getSignalsByType","params":{"signal_type":0,"start_height":0,"end_height":1000},"id":1}'
```

Response shape identical to `getSignalsByHeight`.

---

## Dev-only methods

### `novai_faucet`

Mints `10_000_000` test tokens to the supplied address. Available **only** when the node was launched with `--dev-keys --allow-insecure-dev-keys` (which `scripts/devnet.sh` provides).

**Cooldowns**:

| Limit | Window |
|---|---|
| Per-address | 3600 s (1 h) |
| Global (any address) | 10 s |

**Params**:

| Field | Type | Notes |
|---|---|---|
| `address` | `hex32` | recipient |

**Result**:

```jsonc
{
  "txid":   "<hex32>",        // the faucet's signed transfer
  "amount": "10000000"        // u64 as string for symmetry with balances
}
```

**Errors**:

| Code | When |
|---|---|
| `-32602` | address isn't 32 bytes; node not in dev-mode |
| `-32000` | per-address cooldown still active (message includes `try again in N seconds`) |
| `-32000` | global cooldown still active |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_faucet","params":{"address":"4069c3445cdef6af7cfa421b9d0ebefa7b4f039ad2d74ad9c6af0e5e05cc71ee"},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": { "txid": "3e96a7b7de4bb62c90e0244732e3a238646a86c74f7725e41efd1ca9987ec557", "amount": "10000000" },
  "id": 1
}
```

---

## Error codes

The node uses a small set of codes consistently. Standard JSON-RPC 2.0 codes:

| Code | Meaning | Where it comes from |
|---|---|---|
| `-32600` | Invalid Request | malformed JSON-RPC envelope (missing `jsonrpc`/`method`) |
| `-32601` | Method not found | unknown method name |
| `-32602` | Invalid params | wrong type, missing field, hex length wrong, range too large, height in the future |

Server-defined codes:

| Code | Meaning | Common triggers |
|---|---|---|
| `-32000` | Application error | tx too large, tx decode/sig failure, mempool reject, faucet cooldown |
| `-32002` | Internal storage error | RocksDB read or codec failure during query |

**Examples** (real responses captured from the devnet):

Hex length wrong:

```json
{ "jsonrpc": "2.0", "error": { "code": -32602, "message": "address must be 32 bytes, got 4" }, "id": 1 }
```

Signal range too large:

```json
{ "jsonrpc": "2.0", "error": { "code": -32602, "message": "Height range too large: max 10000 heights per query" }, "id": 1 }
```

Block height beyond commit:

```json
{ "jsonrpc": "2.0", "error": { "code": -32602, "message": "Height 1000000 exceeds committed height 2837" }, "id": 1 }
```

Unknown method:

```json
{ "jsonrpc": "2.0", "error": { "code": -32601, "message": "Method not found: novai_pancake" }, "id": 1 }
```

---

## Field reference

### Numeric ranges

| Field | Type | Range |
|---|---|---|
| `height`, `round`, `nonce`, `tx_index`, `block_height` | `u64` | `0 .. 2^64-1` |
| `fee` | `u64` | `0 .. 2^64-1`; minimum varies per tx type |
| `balance`, `economic_balance` | `u128` (string) | `0 .. 2^128-1` |
| `signal_type` | `u8` | `0..=6` |
| `object_type`, `autonomy_mode`, `capabilities` | `u8` | `0..=255` |

### Length-tagged fields

| Field | Length | Notes |
|---|---|---|
| `address`, `entity_id`, `block_hash`, `parent_hash`, `state_root`, `code_hash`, `pubkey`, `txid`, `commitment_hash`, `object_id`, `issuer`, `from`, `creator`, `memory_root`, `params_root` | 32 bytes / 64 hex chars | lowercase, no `0x` prefix |
| `signature` (inside encoded tx) | 64 bytes / 128 hex chars | ed25519 detached |
| `tx` (param to `submitTransaction`) | ≤ 256 KiB hex (= 128 KiB binary) | full canonical encoding |

---

## See also

- [`docs/tutorials/FIRST_AI_ENTITY.md`](tutorials/FIRST_AI_ENTITY.md) — end-to-end CLI walkthrough.
- [`sdk/novai-sdk-ts/examples/quick-start/`](../sdk/novai-sdk-ts/examples/quick-start/) — TypeScript SDK example.
- [`sdk/novai-sdk/examples/quick-start/`](../sdk/novai-sdk/examples/quick-start/) — Rust SDK example.
- [`crates/node/src/rpc.rs`](../crates/node/src/rpc.rs) — RPC server implementation.
