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

**HTTP route (not JSON-RPC)**: `GET /faucet/<address>` shares the bind port and is available only when the node is launched in Dev-mode. Path matching happens before JSON-RPC parsing, so the route is not visible to JSON-RPC tooling and does not appear in the JSON-RPC rate-limit counters. See [`novai_faucet`](#novai_faucet) for the JSON-RPC equivalent and per-IP cooldowns.

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
| Payments (Week 28) | [`novai_getPaymentsByEntity`](#novai_getpaymentsbyentity) | Payments where the entity is payer or payee (range) |
| Service discovery (Week 29) | [`novai_getServiceDescriptorsByCategory`](#novai_getservicedescriptorsbycategory) | Service descriptors filtered by category |
| VK registry (Week 30) | [`novai_getVkRegistration`](#novai_getvkregistration) | A registered Groth16 verifying key by handle |
| | [`novai_listVkRegistrations`](#novai_listvkregistrations) | All VK registrations owned by an entity |
| SLAs (Week 31) | [`novai_getSlaAgreement`](#novai_getslaagreement) | An SLA memory object by `(owner, object_id)` |
| | [`novai_getActiveSla`](#novai_getactivesla) | The currently active SLA between a buyer and seller |
| | [`novai_listSlasByBuyer`](#novai_listslasbybuyer) | SLAs where the entity is the buyer (range) |
| | [`novai_listSlasBySeller`](#novai_listslasbyseller) | SLAs where the entity is the seller (range) |
| Payment channels (Week 32) | [`novai_getPaymentChannel`](#novai_getpaymentchannel) | A payment channel by `(owner, object_id)` |
| | [`novai_listChannelsByPartyA`](#novai_listchannelsbypartya) | Channels where the entity is party A (range) |
| | [`novai_listChannelsByPartyB`](#novai_listchannelsbypartyb) | Channels where the entity is party B (range) |
| | [`novai_getChannelDisputeStatus`](#novai_getchanneldisputestatus) | Dispute window status with derived `finalize_ready` |
| Entity upgrades (Week 34) | [`novai_getUpgradeHistory`](#novai_getupgradehistory) | EntityUpgrade history for an entity (range) |
| Oracle anchors (Week 35) | [`novai_getOracleAnchor`](#novai_getoracleanchor) | An oracle anchor by `signal_hash` |
| | [`novai_getOracleAnchorsByEntity`](#novai_getoracleanchorsbyentity) | Oracle anchors posted by an entity (range) |
| | [`novai_getOracleAnchorsByTag`](#novai_getoracleanchorsbytag) | Oracle anchors matching a `data_tag` (range) |
| Dev | [`novai_faucet`](#novai_faucet) | Mint test tokens (dev mode only) |

All examples assume `URL=http://localhost:3030` and a `./scripts/devnet.sh` running.

---

## Block methods

### `novai_getLatestBlock`

Returns the header of the most recently committed block.

**Params**: none. Pass `{}`.

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

**Building the hex.** Use one of the SDKs. See [`sdk/novai-sdk-ts/`](../sdk/novai-sdk-ts/) (TypeScript) or [`sdk/novai-sdk/`](../sdk/novai-sdk/) (Rust). Hand-rolling the 149-byte canonical encoding plus ed25519 signature is possible but error-prone; the `txid` returned by the node only confirms the bytes parsed and the signature verified, not that the tx will succeed at execution.

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
| `-32000` | tx hex too large; tx decode failed; tx exceeds binary size cap; signature invalid |
| `-32001` | mempool at capacity |
| `-32010` | nonce too low (message: `NonceTooLow: expected N, got M`) |
| `-32011` | fee too low (message: `FeeTooLow: minimum N, got M`) |
| `-32012` | per-sender pending limit exceeded (message: `SenderLimitExceeded: max N pending per sender`) |
| `-32013` | other validation error |
| `-32002` | DB read failure during validation |

**Example** (request shown abridged. The `tx` value is a real ~300-byte hex blob built via the Rust SDK):

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

The receipt does **not** carry the payload itself or the success/failure status. Execution outcome is reflected in subsequent state (e.g. balance debit, entity record). To distinguish "committed but execution rejected" from "still pending", combine with the relevant state read.

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
    "id":                       "<hex32>",          // canonical entity id (matches the param)
    "code_hash":                "<hex32>",          // identifier supplied at registration
    "creator":                  "<hex32>",          // creator account address
    "autonomy_mode":            <u8>,               // 0 = Advisory, 1 = Gated, 2 reserved
    "capabilities":             <u8>,               // bitfield (see Capabilities)
    "economic_balance":         "<decimal-string>", // u128
    "nonce":                    <u64>,              // next nonce for entity-signed txs
    "pubkey":                   "<hex32>",          // ed25519 verifying key (zeros if registered without a key)
    "memory_root":              "<hex32>",          // (reserved; currently zero)
    "params_root":              "<hex32>",          // (reserved; currently zero)
    "registered_at":            <u64>,              // block height of the register tx
    "last_active_at":           <u64>,              // most recent height at which this entity sent a tx
    "is_active":                true,               // false after a deactivation proposal OR auto-pause from a failed required composition dependency
    "reputation_score":         <u16>,              // current reputation in [0, 100]; defaults to 50 for new entities
    "total_transactions":       <u32>,              // count of transactions counted toward reputation (e.g., job completions)
    "reputation_events_count":  <u32>,              // number of reputation events ever applied to this entity
    "stake_balance":            "<decimal-string>", // u128 stake locked as collateral
    "stake_locked_until":       <u64>,              // block height until which stake cannot be withdrawn (0 = unlocked)
    "upgrade_count":            <u32>,              // number of EntityUpgrade transactions applied to this entity (Week 34)
    "last_upgrade_height":      <u64>               // block height of the most recent upgrade; 0 if never upgraded
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
| 5 (`0x20`) | `submit_reputation_updates` | can issue ReputationUpdate, StakeSlash, and CompositionCheck signals (oracle-only) |
| 6 (`0x40`) | `post_oracle_anchors` | can post OracleAnchor signals (Week 35) |

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
      "object_type":  <u8>,            // 0..=15 (see Memory types below)
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

| Byte | Variant | Notes |
|---|---|---|
| 0 | `chain-summary` | |
| 1 | `label-index` | |
| 2 | `embedding-commitment` | |
| 3 | `anomaly-log` | |
| 4 | `statistics-snapshot` | |
| 5 | `reputation-event` | audit record of a reputation change |
| 6 | `rating` | counterparty rating record |
| 7 | `signal-catalog` | marketplace pricing for priced signals (max 10 entries, 101 B max) |
| 8 | `composition-graph` | cross-entity dependency declarations (max 10 deps, 441 B max) |
| 9 | `verification-record` | ZK proof attestation, 105 B fixed |
| 10 | `delegation-grant` | capability delegation grant (42 B fixed; max 20 per delegator) |
| 11 | `subscription` | recurring payment subscription (114 B fixed; max 10 per subscriber) |
| 12 | `service-descriptor` | Week 29 Agent Discovery Registry entry (144 B fixed; immutable `category`) |
| 13 | `vk-registration` | Week 30 Groth16 verifying-key registration (variable; max 8 KiB VK) |
| 14 | `sla-agreement` | Week 31 SLA between buyer (owner) and seller; auto-slashes on threshold breach |
| 15 | `payment-channel` | Week 32 bidirectional payment channel state (222 B fixed) |

**Limits**: max 100 objects per entity, 64 KiB per object. `SignalCatalog`, `CompositionGraph`, `VerificationRecord`, `DelegationGrant`, `Subscription`, `ServiceDescriptor`, `SlaAgreement`, and `PaymentChannel` are smaller by construction; `VkRegistration` is variable but capped at 8 KiB by the protocol's VK-size limit.

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
      "signal_type":     <u8>,        // 0..=22 (see Signal types below)
      "height":          <u64>,
      "issuer":          "<hex32>"    // canonical entity id
    }
  ]
}
```

**Signal types**:

| Byte | Variant | Notes |
|---|---|---|
| 0 | `anomaly` | base 66 B payload |
| 1 | `optimization` | base 66 B payload |
| 2 | `prediction` | base 66 B payload |
| 3 | `risk-score` | base 66 B payload |
| 4 | `audit-report` | base 66 B payload |
| 5 | `spam-risk` | base 66 B payload |
| 6 | `congestion-forecast` | base 66 B payload |
| 7 | `reputation-update` | base + 35 B tail; issuer needs bit 5 |
| 8 | `signal-purchase` | base + 41 B tail |
| 9 | `stake-deposit` | base + 16 B tail |
| 10 | `stake-withdraw` | base + 16 B tail |
| 11 | `stake-slash` | base + 51 B tail; issuer needs bit 5 |
| 12 | `composition-check` | base + 34 B tail; issuer needs bit 5 |
| 13 | `proof-submission` | base + 65 B tail |
| 14 | `subscription-create` | base + 49 B tail; locks `rate_per_block * duration_blocks` from the subscriber |
| 15 | `subscription-cancel` | base + 32 B tail; settles accrued payment and refunds the remainder |
| 16 | `payment-request` | base + 112 B tail; Week 28 native x402. Optional Week 33 splits and Week 36 condition trailers |
| 17 | `service-attestation` | base + 65 B tail; Week 28 delivery attestation by the original payer |
| 18 | `sla-accept` | base + 64 B tail; Week 31 SLA acceptance by the seller |
| 19 | `channel-accept` | base + 64 B tail; Week 32 channel acceptance by party B |
| 20 | `channel-close` | base + 233 B tail with both parties' signatures; Week 32 cooperative settle or unilateral close |
| 21 | `channel-finalize` | base + 64 B tail; Week 32 permissionless finalize after the dispute window |
| 22 | `oracle-anchor` | base + variable 82..=113 B tail; issuer needs bit 6 `post_oracle_anchors` (Week 35) |

The signal index returns only the base header (`commitment_hash`, `signal_type`, `height`, `issuer`). Tx payload tails are not surfaced by these queries; reconstruct them from `getTransaction.payload_len` plus a direct read of the inner tx if you need them. Where the chain stores extra structured aux rows (Week 28 `PaymentRecord`, Week 33 `PaymentSplitsRecord`, Week 36 `PaymentConditionRecord`, Week 35 `OracleAnchorRecord`), use the dedicated query methods below to read them.

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
| `signal_type` | `u8` | one of `0..=22` |
| `start_height` | `u64` | inclusive |
| `end_height` | `u64` | inclusive; must satisfy `end - start ≤ 10000` |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getSignalsByType","params":{"signal_type":0,"start_height":0,"end_height":1000},"id":1}'
```

Response shape identical to `getSignalsByHeight`.

---

## Payment methods (Week 28)

Payments are native x402-style per-request transfers from a payer entity to a payee. On a successful `PaymentRequest` signal the chain debits `amount + fee` from the payer, credits `amount` to the payee (or to the configured split recipients), routes the fee to `treasury/marketplace`, and writes a canonical `PaymentRecord` aux row keyed by the wrapping signal's `signal_hash`. Week 33 added an optional splits trailer (2..=8 recipients summing to 10 000 basis points) and Week 36 added an optional condition trailer that gates the payment on an oracle anchor; the payment query surfaces both when present.

All payment records share this shape:

```jsonc
{
  "payer":                   "<hex32>",          // payer entity id
  "payee":                   "<hex32>",          // payee entity id (= splits[0].recipient_entity_id for split payments)
  "amount":                  "<decimal-string>", // u128, base units
  "service_descriptor_hash": "<hex32>",          // carried verbatim from the PaymentRequest tail
  "request_hash":            "<hex32>",          // per-request commitment
  "payment_height":          <u64>,              // block height the payment settled at
  "max_block_height":        <u64>,              // expiry height bound at submission
  "attested_status":         "delivered" | "failed" | null,
  "attested_height":         <u64> | null,
  "splits":                  null | [           // null for legacy single-recipient payments
    {
      "recipient_entity_id": "<hex32>",
      "basis_points":        <u16>,
      "credited_amount":     "<decimal-string>"
    }
  ],
  "condition":               null | {           // null for unconditional payments
    "kind":               "anchor_exists" | "anchor_data_hash_equals" | "anchor_tag_equals" | "anchor_not_expired",
    "anchor_signal_hash": "<hex32>",
    "expected_data_hash": "<hex32>" | null,    // populated for kind == anchor_data_hash_equals
    "expected_tag":       <string> | null,     // populated for kind == anchor_tag_equals (lossy UTF-8)
    "expected_tag_hex":   "<hex>"   | null     // populated for kind == anchor_tag_equals (exact bytes)
  }
}
```

### `novai_getPaymentsByEntity`

Returns every `PaymentRecord` where the queried entity is the `payer` or the `payee` within a height window, joined with the optional Week 33 splits aux row and the optional Week 36 condition aux row.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `entity_id` | `hex32` | canonical entity id |
| `role` | `string` | `"payer"` (outgoing) or `"payee"` (incoming) |
| `start_height` | `u64` | inclusive |
| `end_height` | `u64` | inclusive; must satisfy `end - start ≤ 10000` |

**Result**: `{ "payments": [PaymentRecord, ...] }` using the shape above.

**Errors**:

| Code | When |
|---|---|
| `-32602` | `entity_id` isn't 32 bytes; `role` not `"payer"` or `"payee"`; range too large |
| `-32002` | DB read failure |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getPaymentsByEntity","params":{"entity_id":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1","role":"payee","start_height":0,"end_height":10000},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "payments": [
      {
        "payer":                   "f3b91eaa5e5f3a8b8b4c0c9f59f1c4d72c2f4ddf3a2a7f9c8a2c9b7d6e5f4a3b",
        "payee":                   "44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1",
        "amount":                  "100000",
        "service_descriptor_hash": "1b1c1d1e1f202122232425262728292a2b2c2d2e2f3031323334353637383940",
        "request_hash":            "abababababababababababababababababababababababababababababababab",
        "payment_height":          612,
        "max_block_height":        700,
        "attested_status":         "delivered",
        "attested_height":         615,
        "splits":                  null,
        "condition":               null
      }
    ]
  },
  "id": 1
}
```

---

## Service discovery methods (Week 29)

Service descriptors are the on-chain Agent Discovery Registry: a publisher entity describes a callable service (price, stake floor, capability tags) so other agents can find it without an off-chain directory. The descriptor's `category` byte is set at create time and is immutable; the rest of the fields can be updated by the owner.

All service-descriptor records share this shape:

```jsonc
{
  "object_id":                   "<hex32>",
  "owner_entity":                "<hex32>",
  "created_at":                  <u64>,
  "updated_at":                  <u64>,
  "version":                     <u8>,
  "service_name_hash":           "<hex32>",          // off-chain canonical service name commitment
  "service_url_hash":            "<hex32>",          // off-chain endpoint URL commitment
  "description_hash":            "<hex32>",          // off-chain long-description commitment
  "category":                    <u8>,
  "category_label":              "data-oracle" | "inference" | ... | "reserved" | "governance" | "unknown",
  "price_per_call":              "<decimal-string>", // u128 base units; "0" if free
  "subscription_rate_per_block": "<decimal-string>", // u128; "0" if no subscription pricing
  "min_reputation_score":        <u16>,
  "min_stake":                   "<decimal-string>", // u128
  "capability_tags":             <u32>,              // off-chain-defined bitfield
  "status":                      <u8>,
  "status_label":                "active" | "paused" | "deprecated" | "unknown"
}
```

### `novai_getServiceDescriptorsByCategory`

Returns every `ServiceDescriptor` memory object whose `category` byte equals the queried value. Category 0..=15 is the well-known range; 16..=255 is reserved for governance allocation. No height windowing applies.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `category` | `u8` | service category discriminant |

**Result**: `{ "descriptors": [ServiceDescriptor, ...] }` using the shape above.

**Errors**:

| Code | When |
|---|---|
| `-32602` | `category` not in `0..=255` (i.e. malformed JSON number) |
| `-32002` | DB read failure |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getServiceDescriptorsByCategory","params":{"category":1},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "descriptors": [
      {
        "object_id":                   "5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e",
        "owner_entity":                "44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1",
        "created_at":                  720,
        "updated_at":                  720,
        "version":                     1,
        "service_name_hash":           "1111111111111111111111111111111111111111111111111111111111111111",
        "service_url_hash":            "2222222222222222222222222222222222222222222222222222222222222222",
        "description_hash":            "3333333333333333333333333333333333333333333333333333333333333333",
        "category":                    1,
        "category_label":              "inference",
        "price_per_call":              "2500",
        "subscription_rate_per_block": "0",
        "min_reputation_score":        50,
        "min_stake":                   "10000",
        "capability_tags":             0,
        "status":                      1,
        "status_label":                "active"
      }
    ]
  },
  "id": 1
}
```

---

## VK registry methods (Week 30)

A `VkRegistration` publishes a Groth16 verifying key on chain so subsequent `ProofSubmission` signals can reference it by its 32-byte handle instead of inlining the VK every time. The `code_hash`, `proof_type`, and `vk_bytes` are immutable once written; only the free-form `label` can be updated. Submissions referencing the handle use `proof_type = PROOF_TYPE_GROTH16_REGISTERED = 3`.

All VK-registration records share this shape:

```jsonc
{
  "object_id":        "<hex32>",          // canonical registry handle
  "owner_entity":     "<hex32>",
  "created_at":       <u64>,
  "updated_at":       <u64>,
  "version":          <u8>,
  "proof_type":       <u8>,
  "proof_type_label": "stub" | "groth16" | "plonk" | "groth16-registered" | "plonk-registered" | "unknown",
  "code_hash":        "<hex32>",          // canonical code_hash this VK verifies
  "label":            "<string>",         // lossy-UTF-8 free-form label
  "vk_len":           <usize>,            // == len(hex::decode(vk_bytes_hex))
  "vk_bytes_hex":     "<hex>"             // full compressed VK
}
```

### `novai_getVkRegistration`

Point-resolves a single `VkRegistration` by its memory-object id (the canonical handle a `ProofSubmission` carries).

**Params**:

| Field | Type | Notes |
|---|---|---|
| `id` | `hex32` | the VK registry handle |

**Result**:

```jsonc
{ "registration": VkRegistration | null }
```

**Errors**:

| Code | When |
|---|---|
| `-32602` | `id` isn't 32 bytes |
| `-32002` | DB read failure |

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getVkRegistration","params":{"id":"9090909090909090909090909090909090909090909090909090909090909090"},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "registration": {
      "object_id":        "9090909090909090909090909090909090909090909090909090909090909090",
      "owner_entity":     "44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1",
      "created_at":       820,
      "updated_at":       820,
      "version":          1,
      "proof_type":       1,
      "proof_type_label": "groth16",
      "code_hash":        "0101010101010101010101010101010101010101010101010101010101010101",
      "label":            "inference-vk-v1",
      "vk_len":           280,
      "vk_bytes_hex":     "<560 hex chars>"
    }
  },
  "id": 1
}
```

---

### `novai_listVkRegistrations`

Returns every `VkRegistration` owned by the queried entity. Bounded by `MAX_VK_REGISTRATIONS_PER_ENTITY` (= 8 in v1).

**Params**:

| Field | Type | Notes |
|---|---|---|
| `entity_id` | `hex32` | owner entity id |

**Result**:

```jsonc
{ "registrations": [VkRegistration, ...] }
```

**Errors**: same as `getVkRegistration`.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_listVkRegistrations","params":{"entity_id":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1"},"id":1}'
```

Response shape: `{ "registrations": [VkRegistration, ...] }` using the same per-record shape as `getVkRegistration`.

---

## SLA methods (Week 31)

`SlaAgreement` memory objects encode a service-level agreement between a buyer (the memory-object owner) and a seller. The chain auto-slashes the seller's stake when `violation_count` crosses `violation_threshold` inside the active window. SLA acceptance is a separate signal (`sla-accept`, type 18) emitted by the seller.

All SLA records share this shape:

```jsonc
{
  "object_id":                "<hex32>",
  "owner_entity":             "<hex32>",          // == buyer_entity_id for active SLAs
  "created_at":               <u64>,
  "updated_at":               <u64>,
  "version":                  <u8>,
  "buyer_entity_id":          "<hex32>",
  "seller_entity_id":         "<hex32>",
  "service_descriptor_hash":  "<hex32>",
  "status":                   <u8>,
  "status_label":             "proposed" | "active" | "completed" | "violated" | "cancelled" | "unknown",
  "created_at_height":        <u64>,
  "accepted_at_height":       <u64>,
  "start_height":             <u64>,
  "end_height":               <u64>,
  "violation_count":          <u32>,
  "violation_threshold":      <u32>,
  "max_response_time_blocks": <u32>,
  "min_uptime_bps":           <u16>,
  "min_delivery_success_bps": <u16>,
  "price_per_call":           "<decimal-string>", // u128
  "slash_amount":             "<decimal-string>", // u128; penalty on breach
  "terminated_at_height":     <u64>,
  "slashed_amount":           "<decimal-string>"  // u128; actual debit applied
}
```

### `novai_getSlaAgreement`

Point-resolves a single SLA memory object by the `(owner, object_id)` pair.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `owner` | `hex32` | buyer entity id (the SLA's memory-object owner) |
| `object_id` | `hex32` | SLA memory object id |

**Result**:

```jsonc
{ "agreement": SlaAgreement | null }
```

**Errors**: `-32602` for malformed hex; `-32002` for DB read failure.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getSlaAgreement","params":{"owner":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1","object_id":"7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a"},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "agreement": {
      "object_id":                "7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a",
      "owner_entity":             "44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1",
      "created_at":               900,
      "updated_at":               905,
      "version":                  1,
      "buyer_entity_id":          "44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1",
      "seller_entity_id":         "5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b",
      "service_descriptor_hash":  "1b1c1d1e1f202122232425262728292a2b2c2d2e2f3031323334353637383940",
      "status":                   1,
      "status_label":             "active",
      "created_at_height":        900,
      "accepted_at_height":       905,
      "start_height":             910,
      "end_height":               2000,
      "violation_count":          0,
      "violation_threshold":      3,
      "max_response_time_blocks": 50,
      "min_uptime_bps":           9500,
      "min_delivery_success_bps": 9700,
      "price_per_call":           "1000",
      "slash_amount":             "50000",
      "terminated_at_height":     0,
      "slashed_amount":           "0"
    }
  },
  "id": 1
}
```

---

### `novai_getActiveSla`

Returns the SLA that is currently open between a given `(buyer, seller)` pair via the active-pair singleton index (max one active SLA per pair).

**Params**:

| Field | Type | Notes |
|---|---|---|
| `buyer` | `hex32` | buyer entity id |
| `seller` | `hex32` | seller entity id |

**Result**: identical to `getSlaAgreement`. `{ "agreement": null }` when no active SLA exists for the pair.

**Errors**: same as `getSlaAgreement`.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getActiveSla","params":{"buyer":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1","seller":"5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b"},"id":1}'
```

Response shape identical to `getSlaAgreement`.

---

### `novai_listSlasByBuyer`

Returns every SLA where the queried entity is the buyer (memory-object owner) and the SLA's `created_at` falls within the height window.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `entity_id` | `hex32` | buyer entity id |
| `start_height` | `u64` | inclusive |
| `end_height` | `u64` | inclusive; must satisfy `end - start ≤ 10000` |

**Result**:

```jsonc
{ "agreements": [SlaAgreement, ...] }
```

**Errors**: `-32602` for malformed hex or range too large; `-32002` for DB read failure.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_listSlasByBuyer","params":{"entity_id":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1","start_height":0,"end_height":10000},"id":1}'
```

Response shape: `{ "agreements": [SlaAgreement, ...] }` using the same per-record shape as `getSlaAgreement`.

---

### `novai_listSlasBySeller`

Returns every SLA where the queried entity is the seller and the SLA's `created_at` falls within the height window. Bounded internally by the per-buyer cap (= 8 in v1).

**Params**: same shape as `listSlasByBuyer`, with `entity_id` interpreted as the seller.

**Result**: `{ "agreements": [SlaAgreement, ...] }`.

**Errors**: same as `listSlasByBuyer`.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_listSlasBySeller","params":{"entity_id":"5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b","start_height":0,"end_height":10000},"id":1}'
```

Response shape identical to `listSlasByBuyer`.

---

## Payment channel methods (Week 32)

`PaymentChannel` memory objects encode a bidirectional off-chain payment channel between two entities (party A as the memory-object owner, party B as the named counterparty). Off-chain state updates are doubly signed and applied via a `ChannelClose` signal; cooperative settles distribute instantly, while unilateral closes open a dispute window that any caller can finalize after the deadline.

All payment-channel records share this shape:

```jsonc
{
  "object_id":               "<hex32>",
  "owner_entity":            "<hex32>",          // == party_a_entity_id
  "created_at":              <u64>,
  "updated_at":              <u64>,
  "version":                 <u8>,
  "party_a_entity_id":       "<hex32>",
  "party_b_entity_id":       "<hex32>",
  "sla_object_id":           "<hex32>",          // optional binding to an SlaAgreement (zero if unbound)
  "status":                  <u8>,
  "status_label":            "proposed" | "open" | "closing" | "unknown",
  "deposit_a":               "<decimal-string>", // u128
  "deposit_b":               "<decimal-string>", // u128
  "balance_a":               "<decimal-string>", // u128, current recorded
  "balance_b":               "<decimal-string>", // u128, current recorded
  "nonce":                   <u64>,              // highest applied off-chain state nonce
  "proposed_at_height":      <u64>,
  "accepted_at_height":      <u64>,
  "closing_at_height":       <u64>,
  "dispute_deadline_height": <u64>,
  "dispute_window_blocks":   <u32>
}
```

### `novai_getPaymentChannel`

Point-resolves a payment channel by `(owner, object_id)`. Owner is party A.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `owner` | `hex32` | party A entity id (memory-object owner) |
| `object_id` | `hex32` | channel memory object id |

**Result**:

```jsonc
{ "channel": PaymentChannel | null }
```

**Errors**: `-32602` for malformed hex; `-32002` for DB read failure.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getPaymentChannel","params":{"owner":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1","object_id":"c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1"},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "channel": {
      "object_id":               "c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1",
      "owner_entity":            "44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1",
      "created_at":              1020,
      "updated_at":              1025,
      "version":                 1,
      "party_a_entity_id":       "44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1",
      "party_b_entity_id":       "5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b",
      "sla_object_id":           "0000000000000000000000000000000000000000000000000000000000000000",
      "status":                  1,
      "status_label":            "open",
      "deposit_a":               "200000",
      "deposit_b":               "200000",
      "balance_a":               "180000",
      "balance_b":               "220000",
      "nonce":                   7,
      "proposed_at_height":      1020,
      "accepted_at_height":      1025,
      "closing_at_height":       0,
      "dispute_deadline_height": 0,
      "dispute_window_blocks":   100
    }
  },
  "id": 1
}
```

---

### `novai_listChannelsByPartyA`

Returns every `PaymentChannel` whose memory-object owner (party A) is the queried entity and whose `created_at` falls within the height window.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `entity_id` | `hex32` | party A entity id |
| `start_height` | `u64` | inclusive |
| `end_height` | `u64` | inclusive; must satisfy `end - start ≤ 10000` |

**Result**:

```jsonc
{ "channels": [PaymentChannel, ...] }
```

**Errors**: `-32602` for malformed hex or range too large; `-32002` for DB read failure.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_listChannelsByPartyA","params":{"entity_id":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1","start_height":0,"end_height":10000},"id":1}'
```

Response shape: `{ "channels": [PaymentChannel, ...] }` using the same per-record shape as `getPaymentChannel`.

---

### `novai_listChannelsByPartyB`

Returns every `PaymentChannel` where the queried entity is the embedded counterparty (party B). The by-party-B secondary index embeds party A so primary-record resolution is O(1) per match.

**Params**: same shape as `listChannelsByPartyA`, with `entity_id` interpreted as party B.

**Result**: `{ "channels": [PaymentChannel, ...] }`.

**Errors**: same as `listChannelsByPartyA`.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_listChannelsByPartyB","params":{"entity_id":"5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b","start_height":0,"end_height":10000},"id":1}'
```

Response shape identical to `listChannelsByPartyA`.

---

### `novai_getChannelDisputeStatus`

Returns the dispute-window fields for a channel plus the derived `blocks_remaining` and `finalize_ready`, so a client does not need to combine a separate latest-block read with the channel record. When the channel is not in the `CLOSING` state, `blocks_remaining` is `0` and `finalize_ready` is `false`. When the channel does not resolve (wrong type or missing), `found` is `false` and the other fields are placeholder zeros.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `owner` | `hex32` | party A entity id |
| `object_id` | `hex32` | channel memory object id |

**Result**:

```jsonc
{
  "found":                   <bool>,
  "status":                  <u8>,
  "status_label":            "proposed" | "open" | "closing" | "unknown",
  "closing_at_height":       <u64>,
  "dispute_deadline_height": <u64>,
  "current_height":          <u64>,
  "blocks_remaining":        <u64>,   // dispute_deadline_height.saturating_sub(current_height) when CLOSING, else 0
  "finalize_ready":          <bool>   // true iff status == CLOSING && current_height > dispute_deadline_height
}
```

**Errors**: `-32602` for malformed hex; `-32002` for DB read failure.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getChannelDisputeStatus","params":{"owner":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1","object_id":"c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1"},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "found":                   true,
    "status":                  2,
    "status_label":            "closing",
    "closing_at_height":       1100,
    "dispute_deadline_height": 1200,
    "current_height":          1180,
    "blocks_remaining":        20,
    "finalize_ready":          false
  },
  "id": 1
}
```

---

## Entity upgrade methods (Week 34)

`EntityUpgrade` is a top-level transaction type (byte 11) that swaps an entity's `code_hash` while preserving `entity_id` and all id-keyed state (reputation, stake, balance, open SLAs, payment channels, memory objects). Each upgrade writes an `UpgradeRecord` row to a per-entity history index; the entity record itself records `upgrade_count` and `last_upgrade_height` (surfaced by `getAiEntity`).

### `novai_getUpgradeHistory`

Returns every `UpgradeRecord` for the entity whose `upgrade_height` falls within the height window, ordered ascending by height.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `entity_id` | `hex32` | the entity whose history to read |
| `start_height` | `u64` | inclusive |
| `end_height` | `u64` | inclusive; must satisfy `end - start ≤ 10000` |

**Result**:

```jsonc
{
  "upgrades": [
    {
      "old_code_hash":  "<hex32>",  // pre-upgrade code hash
      "new_code_hash":  "<hex32>",  // post-upgrade code hash
      "upgrade_height": <u64>,
      "upgrade_count":  <u32>,      // monotonic counter, 1 for the first upgrade
      "reason_hash":    "<hex32>"   // optional off-chain reason commitment; zero if unused
    }
  ]
}
```

**Errors**: `-32602` for malformed `entity_id` or range too large; `-32002` for DB read failure.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getUpgradeHistory","params":{"entity_id":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1","start_height":0,"end_height":10000},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "upgrades": [
      {
        "old_code_hash":  "0101010101010101010101010101010101010101010101010101010101010101",
        "new_code_hash":  "0202020202020202020202020202020202020202020202020202020202020202",
        "upgrade_height": 1500,
        "upgrade_count":  1,
        "reason_hash":    "0000000000000000000000000000000000000000000000000000000000000000"
      }
    ]
  },
  "id": 1
}
```

---

## Oracle anchor methods (Week 35)

`OracleAnchor` signals are commitments to external off-chain data, posted by entities that hold the `post_oracle_anchors` capability (bit 6). The chain stores each anchor as an `OracleAnchorRecord` KV aux row at `ai/oracle_anchors/by_hash/<signal_hash>` plus height-ordered by-entity and by-tag scan indexes. Anchors are reputation-neutral on post (`total_transactions++` only); the challenge mechanism is deferred. `expiry_height` is advisory and is NOT enforced by the chain.

All anchor records share this shape:

```jsonc
{
  "issuer_entity_id":   "<hex32>",
  "data_hash":          "<hex32>",          // blake3 commitment to the off-chain data
  "external_timestamp": <u64>,              // opaque; no on-chain wall-clock binds it
  "source_hash":        "<hex32>",          // optional source commitment; zero if unused
  "expiry_height":      <u64>,              // advisory; 0 if unset
  "anchor_height":      <u64>,              // block height the anchor was committed at
  "data_tag":           "<string>",         // lossy UTF-8 view of the tag bytes
  "data_tag_hex":       "<hex>"             // exact tag bytes (1..=32) hex-encoded
}
```

### `novai_getOracleAnchor`

Point-resolves a single anchor by the wrapping signal's canonical `signal_hash`.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `signal_hash` | `hex32` | the 32-byte signal hash of the wrapping `OracleAnchor` signal |

**Result**:

```jsonc
{ "anchor": OracleAnchor | null }
```

**Errors**: `-32602` for malformed hex; `-32002` for DB read failure.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getOracleAnchor","params":{"signal_hash":"e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0"},"id":1}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "anchor": {
      "issuer_entity_id":   "44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1",
      "data_hash":          "1111111111111111111111111111111111111111111111111111111111111111",
      "external_timestamp": 1717000000,
      "source_hash":        "0000000000000000000000000000000000000000000000000000000000000000",
      "expiry_height":      0,
      "anchor_height":      1800,
      "data_tag":           "price/ETH-USD",
      "data_tag_hex":       "70726963652f4554482d555344"
    }
  },
  "id": 1
}
```

---

### `novai_getOracleAnchorsByEntity`

Returns every `OracleAnchorRecord` posted by the queried entity whose `anchor_height` falls within the chain-height window. The optional inclusive `[ts_min, ts_max]` filters by the anchor's external timestamp (post-filter, applied in memory).

**Params**:

| Field | Type | Notes |
|---|---|---|
| `entity_id` | `hex32` | issuer entity id |
| `start_height` | `u64` | inclusive |
| `end_height` | `u64` | inclusive; must satisfy `end - start ≤ 10000` |
| `ts_min` | `u64` (optional) | lower bound on `external_timestamp` |
| `ts_max` | `u64` (optional) | upper bound on `external_timestamp` |

**Result**:

```jsonc
{ "anchors": [OracleAnchor, ...] }
```

**Errors**: `-32602` for malformed hex or range too large; `-32002` for DB read failure.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getOracleAnchorsByEntity","params":{"entity_id":"44a2cb6444921083ca5c483c0ba809db0bc83e12c3a264d52400d7576aca2ac1","start_height":0,"end_height":10000},"id":1}'
```

Response shape: `{ "anchors": [OracleAnchor, ...] }` using the same per-record shape as `getOracleAnchor`.

---

### `novai_getOracleAnchorsByTag`

Returns every `OracleAnchorRecord` whose `data_tag` matches the queried tag (e.g. `"price/ETH-USD"`) within the chain-height window. Matching is by the domain-separated blake3 hash of the tag bytes, so callers pass the raw tag string. The optional `[ts_min, ts_max]` filter behaves as in `getOracleAnchorsByEntity`.

**Params**:

| Field | Type | Notes |
|---|---|---|
| `data_tag` | `string` | raw tag string, 1..=32 bytes |
| `start_height` | `u64` | inclusive |
| `end_height` | `u64` | inclusive; must satisfy `end - start ≤ 10000` |
| `ts_min` | `u64` (optional) | lower bound on `external_timestamp` |
| `ts_max` | `u64` (optional) | upper bound on `external_timestamp` |

**Result**: `{ "anchors": [OracleAnchor, ...] }`.

**Errors**: `-32602` for empty / oversized `data_tag` or range too large; `-32002` for DB read failure.

**Example**:

```bash
curl -s -X POST $URL -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getOracleAnchorsByTag","params":{"data_tag":"price/ETH-USD","start_height":0,"end_height":10000},"id":1}'
```

Response shape identical to `getOracleAnchorsByEntity`.

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

The node uses these codes. Standard JSON-RPC 2.0 codes:

| Code | Meaning | Where it comes from |
|---|---|---|
| `-32700` | Parse error | request body is not valid JSON |
| `-32600` | Invalid Request | malformed JSON-RPC envelope (missing `jsonrpc`/`method`) |
| `-32601` | Method not found | unknown method name |
| `-32602` | Invalid params | wrong type, missing field, hex length wrong, range too large, height in the future |

Server-defined codes:

| Code | Meaning | Common triggers |
|---|---|---|
| `-32000` | Application error | tx too large, tx decode/sig failure, faucet cooldown, faucet disabled |
| `-32001` | Mempool full | mempool at capacity; submitter must back off |
| `-32002` | Internal storage error | RocksDB read or codec failure during query |
| `-32003` | Response too large | assembled response exceeds 10 MiB |
| `-32010` | Nonce too low | tx nonce is below the sender's expected nonce |
| `-32011` | Fee too low | tx fee is below the per-tx-type minimum |
| `-32012` | Sender limit exceeded | too many pending txs from this sender in the mempool |
| `-32013` | Other validation error | catch-all for additional mempool validation failures |

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
| `signal_type` | `u8` | `0..=22` |
| `object_type` | `u8` | `0..=15` |
| `autonomy_mode`, `capabilities` | `u8` | `0..=255` (only specific bits or values are valid; see relevant section) |

### Length-tagged fields

| Field | Length | Notes |
|---|---|---|
| `address`, `entity_id`, `block_hash`, `parent_hash`, `state_root`, `code_hash`, `pubkey`, `txid`, `commitment_hash`, `object_id`, `issuer`, `from`, `creator`, `memory_root`, `params_root` | 32 bytes / 64 hex chars | lowercase, no `0x` prefix |
| `signature` (inside encoded tx) | 64 bytes / 128 hex chars | ed25519 detached |
| `tx` (param to `submitTransaction`) | ≤ 256 KiB hex (= 128 KiB binary) | full canonical encoding |

---

## Observed gaps

Surface that has shipped at the protocol layer but is not yet exposed by the RPC. Recipes in [AI_ENTITY_COOKBOOK.md](AI_ENTITY_COOKBOOK.md) work around these where they can.

| Gap | Impact | Workaround today |
|---|---|---|
| No treasury balance method | `treasury/ai`, `treasury/marketplace`, `treasury/slash` accumulate value but are not addressable accounts | Direct KV inspection (string keys: `treasury/ai`, `treasury/marketplace`, `treasury/slash`) |
| No mempool query | Cannot enumerate pending txs | Poll `getTransaction(txid)` for inclusion |
| No event subscription | RPC is HTTP request/response only; no WebSocket or push | Poll `getLatestBlock` and the signal index |
| No transaction logs / traces | Receipts carry only `(block_height, tx_index, from, nonce, fee, payload_len)` | For execution outcome, cross-check the relevant state read after `getTransaction` returns non-null |
| Memory object filter by type | `getMemoryObjects` returns all objects for an entity; no per-type query | Filter client-side on `object_type` |
| Signal payload tail not surfaced | Index returns only base header (commitment_hash, signal_type, height, issuer); the tail bytes for signals 7..=22 are not in the response | Read the wrapping tx via `getTransaction`, or use the dedicated aux-record methods (`getPaymentsByEntity`, `getOracleAnchor`, etc.) where the chain persists structured aux rows |

---

## See also

- [`docs/tutorials/FIRST_AI_ENTITY.md`](tutorials/FIRST_AI_ENTITY.md): end-to-end CLI walkthrough.
- [`sdk/novai-sdk-ts/examples/quick-start/`](../sdk/novai-sdk-ts/examples/quick-start/): TypeScript SDK example.
- [`sdk/novai-sdk/examples/quick-start/`](../sdk/novai-sdk/examples/quick-start/): Rust SDK example.
- [`crates/node/src/rpc.rs`](../crates/node/src/rpc.rs): RPC server implementation.
