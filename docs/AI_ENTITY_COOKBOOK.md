# AI Entity Cookbook

Five recipes for the AI infrastructure features in NOVAI v1: reputation, marketplace, staking, composition, and ZK proofs. Each recipe shows what works today, the exact byte layouts where the SDK does not yet have a helper, and how to verify the on-chain effect.

The CLI now exposes first-class commands for all 14 signal types and all 10 memory object types. Recipes use a mix of CLI (`novai-cli signal publish`, `novai-cli memory create`, `novai-cli ai info`), the Rust SDK (`sdk/novai-sdk`), and direct transaction byte construction with `novai-crypto` + `novai-codec` (where neither covers the case). Remaining gaps are tagged `[NOT YET IMPLEMENTED]`.

> **Observable state.** `novai_getAiEntity` exposes the V4/V5 entity fields: `reputation_score`, `total_transactions`, `reputation_events_count`, `stake_balance`, and `stake_locked_until`. `novai-cli ai info` displays them in human-readable form. Each recipe below verifies via a combination of `novai_getAiEntity` for cumulative state, `novai_getTransaction` for tx inclusion, `novai_getSignalsByIssuer` and `novai_getSignalsByType` for signal events, and `novai_getMemoryObjects` for memory objects. See [RPC_REFERENCE.md#observed-gaps](RPC_REFERENCE.md#observed-gaps) for remaining gaps.

Prerequisites for every recipe:
- A running local devnet ([QUICKSTART.md](QUICKSTART.md))
- A funded creator key at `/tmp/creator.key`
- An existing entity for some recipes (build it via [tutorials/FIRST_AI_ENTITY.md](tutorials/FIRST_AI_ENTITY.md))

---

## Capability bits

The `capabilities` field on an `AiEntity` is one byte. Set bits according to what the entity needs to do.

| Bit | Hex | Capability | CLI flag (`ai register --capabilities`) |
|---|---|---|---|
| 0 | 0x01 | read_public_chain | `read_chain` |
| 1 | 0x02 | read_memory_objects | `read_memory` |
| 2 | 0x04 | emit_proposals | `emit_proposals` |
| 3 | 0x08 | request_execution | `request_execution` |
| 4 | 0x10 | read_nnpx_derived | `read_nnpx` |
| 5 | 0x20 | submit_reputation_updates | (none yet) |

`[NOT YET IMPLEMENTED]` The CLI capability parser does not accept `submit_reputation_updates`. To set bit 5, register the entity through the SDK with `Capabilities::from_byte(0x27)` (or whatever bitmask you need).

## Minimum fees (base units)

| Operation | Min fee | Constant |
|---|---|---|
| Transfer | 100 | `MIN_FEE_TRANSFER` |
| Signal commitment (any of 14 types) | 1,000 | `MIN_FEE_SIGNAL_COMMITMENT` |
| Memory object create / update / delete | 500 | `MIN_FEE_MEMORY_OBJECT` |
| Register AI entity | 5,000 | `MIN_FEE_REGISTER_AI_ENTITY` |
| Credit AI entity | 100 | `MIN_FEE_CREDIT_AI_ENTITY` |
| Governance submit | 2,000 | `MIN_FEE_GOVERNANCE_SUBMIT` |
| Governance execute | 500 | `MIN_FEE_GOVERNANCE_EXECUTE` |

`MIN_ACCOUNT_BALANCE` is 1,000. Transfers below this to a new account are rejected.

## Per-signal-type capability requirements

| Signal type | Code | Issuer must have |
|---|---|---|
| Anomaly, Optimization, Prediction, RiskScore, AuditReport, SpamRisk, CongestionForecast | 0-6 | `emit_proposals` |
| ReputationUpdate | 7 | `submit_reputation_updates` |
| SignalPurchase | 8 | `emit_proposals` |
| StakeDeposit | 9 | `emit_proposals` |
| StakeWithdraw | 10 | `emit_proposals` |
| StakeSlash | 11 | `submit_reputation_updates` |
| CompositionCheck | 12 | `submit_reputation_updates` |
| ProofSubmission | 13 | `emit_proposals` |

---

## Recipe 1: Deploy a Reputation Oracle

A reputation oracle issues `ReputationUpdate` signals (type 7). Each signal bumps or docks a target entity's `reputation_score` field on chain. Scores clamp to `[0, 100]`. Default for new entities is 50.

### What you need
- A creator account with at least 60,000 base units (5,000 fee + 50,000 entity balance).
- An entity registered with `submit_reputation_updates` (bit 5).

### Step 1: Register the oracle (SDK)

The CLI cannot set bit 5. Use the SDK:

```rust
use novai_sdk::{keys, tx, Client};
use novai_ai_entities::{AutonomyMode, Capabilities};

let client = Client::new("http://localhost:3030");
let (creator_sk, creator_vk) = keys::load("/tmp/creator.key")?;
let (oracle_sk, oracle_vk) = keys::generate();
keys::save("/tmp/oracle.key", &oracle_sk)?;

let caps = Capabilities::from_byte(0x01 | 0x02 | 0x04 | 0x20);
let nonce = client.get_nonce(&keys::address(&creator_vk)).await?;
let signed = tx::register_ai_entity_with_key(
    &creator_sk, nonce, 5_000, &[0x42; 32], &oracle_vk,
    AutonomyMode::Advisory, caps, 50_000,
)?;
let txid = client.submit_tx(&signed).await?;
let entity_id = tx::compute_entity_id(&[0x42; 32], &keys::address(&creator_vk));
println!("oracle entity_id={}, txid={txid}", hex::encode(entity_id));
```

### Step 2: Issue a ReputationUpdate

The CLI handles this directly: `novai-cli signal publish --signal-type reputation-update --target-entity-id <hex> --event-type 0 --points-delta 1 ...`. The SDK's `tx::signal_commitment` still emits only the 66-byte base payload, so SDK users build the 101-byte payload manually and sign a `TxV1` directly:

```text
[0x02][signal_hash:32][0x07][issuer_id:32][target_id:32][event_type:1][delta_be:2]
```

`event_type` is 0-9 (`REP_EVENT_*` constants). Use `0` (`REP_EVENT_JOB_COMPLETED`) for routine positive updates. `delta_be` is an `i16` clamped on apply. The wrapping `TxV1` is signed by the oracle entity's key (loaded from `/tmp/oracle.key`), `from = address(oracle_vk)`, `pubkey = oracle_vk.to_bytes()`, fee `1_000`.

### Verify

`reputation_score` is RPC-readable. Read it directly:

```bash
# Cumulative reputation_score on the target after the update.
curl -s -X POST http://localhost:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getAiEntity","params":{"entity_id":"<target_id>"},"id":1}'

# Or human-readable via the CLI.
novai-cli ai info --entity-id <target_id>

# Cross-check the issuance trail.
curl -s -X POST http://localhost:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getSignalsByIssuer","params":{"issuer":"<oracle_id>","start_height":0,"end_height":10000},"id":1}'
```

Expect `reputation_score` to move by `points_delta` (clamped to `[0, 100]` on apply) and `reputation_events_count` to increment by one.

### Common errors
- `IssuerMissingCapability`: oracle entity does not have bit 5 set. Re-register.
- `InvalidReputationEventType`: `event_type > 9`.
- `SelfReputationUpdate`: oracle and target are the same entity.
- `TargetEntityNotFound`: target not registered.

---

## Recipe 2: Sell Predictions on the Marketplace

Publish a `SignalCatalog` memory object listing priced signals. Buyers issue `SignalPurchase` signals. The chain charges the buyer, credits the seller, and routes a 2% cut (`MARKETPLACE_FEE_BPS = 200`) to `treasury/marketplace`.

### What you need
- A registered entity with `emit_proposals` (default `0x07` capabilities are enough).
- Funded `economic_balance` on that entity (memory object publication costs 500).

### Step 1: Build the catalog payload

`SignalCatalog` is memory object type 7. The catalog data is `[count:u8][entry × N]` where each 10-byte entry is `[signal_type:u8][price_per_signal_be:u64][is_active:u8]`. Max 10 entries. Total max 101 bytes.

```rust
let mut catalog = vec![0x01];                       // 1 entry
catalog.push(0x02);                                 // signal_type 2 = Prediction
catalog.extend_from_slice(&5_000u64.to_be_bytes()); // price 5,000
catalog.push(0x01);                                 // is_active = true
```

### Step 2: Wrap as a memory object create tx

The CLI accepts `signal-catalog` directly: `novai-cli memory create --type signal-catalog --data-file ./catalog.bin --key-file /tmp/seller.key`. SDK users do the same via `tx::create_memory(...)`:

```rust
use novai_ai_entities::MemoryObjectType;

let nonce = client.get_nonce(&keys::address(&entity_vk)).await?;
let signed = tx::create_memory(
    &entity_sk, nonce, 500, MemoryObjectType::SignalCatalog, &catalog,
)?;
client.submit_tx(&signed).await?;
```

### Step 3: A buyer issues SignalPurchase (signal type 8)

107-byte payload. Tail is 41 bytes:

```text
[0x02][signal_hash:32][0x08][buyer_id:32][seller_id:32][purchased_signal_type:1][max_price_be:8]
```

The chain matches `purchased_signal_type` against the seller's catalog. If the offered price exceeds `max_price`, the tx is rejected. Otherwise: `buyer.economic_balance -= price`, `seller.economic_balance += price * 9800/10000`, `treasury/marketplace += price * 200/10000`.

### Verify the seller was paid

```bash
curl -s -X POST http://localhost:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getAiEntity","params":{"entity_id":"<seller_id>"},"id":1}'
```

Check `result.entity.economic_balance` increased by `price * 0.98` and `total_transactions` incremented.

`[NOT YET IMPLEMENTED]` The marketplace treasury (`treasury/marketplace`) is a string-keyed KV entry. There is no RPC method that reads it. Operators can audit it through direct DB inspection. A `novai_getTreasuryBalance` endpoint is needed for application-level auditing.

### Common errors
- `SignalCatalogNotFound`: seller has not published a catalog.
- `SignalOfferingNotFound`: catalog has no entry for `purchased_signal_type`.
- `SignalOfferingInactive`: entry exists but `is_active = false`.
- `PriceExceedsMaxPrice`: catalog price is higher than buyer's `max_price`.
- `InsufficientEntityBalance`: buyer cannot cover `price + fee`.
- `SellerIsBuyer`: cannot purchase from yourself.

---

## Recipe 3: Stake Collateral for Trust

Move `economic_balance` into `stake_balance` to lock it as collateral. Locked stake cannot be withdrawn for `STAKE_LOCK_PERIOD = 1_000` blocks. Slashed stake goes to `treasury/slash`.

### What you need
- A registered entity with `emit_proposals`.
- Sufficient `economic_balance` to cover the stake plus the 1,000-base-unit signal fee.

### Step 1: Deposit (signal type 9)

82-byte payload. Tail is 16 bytes:

```text
[0x02][signal_hash:32][0x09][issuer_id:32][amount_be:16]
```

`amount` is a `u128` in big-endian. Sign as a `TxV1` with the entity's own key, fee `1_000`. After commit:

- `entity.economic_balance -= amount`
- `entity.stake_balance += amount`
- `entity.stake_locked_until = current_height + 1000`

### Step 2: Withdraw (signal type 10) once unlocked

Same 82-byte layout, type byte `0x0A`. The chain rejects with `StakeStillLocked` if `current_height < stake_locked_until`. Partial withdrawals do not re-lock the remainder.

### Step 3: What slashing does (signal type 11)

A reputation oracle (entity with `submit_reputation_updates`) emits a `StakeSlash` signal. The 117-byte payload tail is 51 bytes:

```text
[0x02][signal_hash:32][0x0B][oracle_id:32][target_id:32][slash_amount_be:16][rep_event_type:1][points_delta_be:2]
```

The chain saturating-subtracts up to `slash_amount` from `target.stake_balance`, credits that amount to `treasury/slash`, and applies the bundled reputation event to the target.

### Verify

`economic_balance`, `stake_balance`, and `stake_locked_until` are all RPC-readable. The signal index records every stake operation:

```bash
# Every StakeDeposit (type 9) ever issued in this height range.
curl -s -X POST http://localhost:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getSignalsByType","params":{"signal_type":9,"start_height":0,"end_height":10000},"id":1}'

# Entity's balances and lock height after the deposit.
curl -s -X POST http://localhost:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getAiEntity","params":{"entity_id":"<entity_id>"},"id":1}'

# Or in one line via the CLI.
novai-cli ai info --entity-id <entity_id>
```

After deposit: `economic_balance` decreased by `amount + fee`, `stake_balance` increased by `amount`, `stake_locked_until` set to `current_height + 1000`. After withdraw: `economic_balance` increased by `amount`, `stake_balance` decreased by `amount`.

### Common errors
- `InsufficientEntityBalance` (deposit): entity's `economic_balance` is below `amount + fee`.
- `StakeStillLocked` (withdraw): `current_height < stake_locked_until`.
- `InsufficientStakeBalance` (withdraw): `amount > stake_balance`.
- `IssuerMissingCapability` (slash): oracle does not have bit 5.
- `SelfSlash`: oracle and target are the same entity.

---

## Recipe 4: Build an AI Pipeline (Composition)

A consumer entity declares a `CompositionGraph` memory object listing the producers it depends on. A reputation oracle observes producers and submits `CompositionCheck` signals when a dependency fails its declared minimum reputation, minimum stake, or active status. Failure auto-pauses the consumer if the dependency is marked `is_required = true`.

### What you need
- Two registered entities: `producer` and `consumer`. Both need at least `emit_proposals`.
- A reputation oracle (separate entity with `submit_reputation_updates`) to issue checks.

### Step 1: Consumer publishes CompositionGraph (memory object type 8)

The graph data is `[count:u8][dep × N]` where each 44-byte dependency is:

```text
[source_entity_id:32][required_signal_type:1][min_reputation_be:2][min_stake_be:8][is_required:1]
```

Max 10 dependencies. Self-dependencies are rejected (`source_entity_id != consumer.id` is enforced).

```rust
let mut graph = vec![0x01];                          // 1 dependency
graph.extend_from_slice(&producer_id);               // 32 bytes
graph.push(0x02);                                    // required_signal_type = Prediction
graph.extend_from_slice(&60u16.to_be_bytes());       // min_reputation 60
graph.extend_from_slice(&100_000u64.to_be_bytes());  // min_stake 100,000
graph.push(0x01);                                    // is_required = true

let signed = tx::create_memory(
    &consumer_sk, nonce, 500, MemoryObjectType::CompositionGraph, &graph,
)?;
client.submit_tx(&signed).await?;
```

### Step 2: Oracle issues CompositionCheck (signal type 12)

100-byte payload, 34-byte tail:

```text
[0x02][signal_hash:32][0x0C][oracle_id:32][target_id:32][failed_dep_idx:1][failure_reason:1]
```

`failure_reason` codes:
- `0` SOURCE_INACTIVE: source entity exists but `is_active = false`
- `1` REPUTATION_BELOW_MIN: source `reputation_score < min_reputation`
- `2` STAKE_BELOW_MIN: source `stake_balance < min_stake`
- `3` SOURCE_NOT_FOUND: source entity does not exist

The chain re-verifies the claim against current state. Lying about the failure reason is rejected with `DependencyFailureNotVerified`. A successful check applies `-1` reputation to the consumer (`REP_EVENT_COMPOSITION_FAILURE`) and sets `consumer.is_active = false` if the dependency was `is_required`.

### Verify

```bash
curl -s -X POST http://localhost:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getAiEntity","params":{"entity_id":"<consumer_id>"},"id":1}'
```

After a successful required-dependency failure: `is_active` flips to `false` and `reputation_score` decreases by `1` (`REP_EVENT_COMPOSITION_FAILURE`). Both fields are RPC-observable. Cross-check the signal index for the issuance trail:

```bash
curl -s -X POST http://localhost:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getSignalsByType","params":{"signal_type":12,"start_height":0,"end_height":10000},"id":1}'
```

A paused consumer cannot publish further signals or memory objects until reactivated through governance.

### Common errors
- `SelfDependency` (publication): a dependency lists the consumer's own entity_id.
- `CompositionGraphNotFound` (check): consumer has not published a graph.
- `InvalidDependencyIndex` (check): `failed_dep_idx >= dependencies.len()`.
- `DependencyFailureNotVerified` (check): the on-chain state of the source does not match the claimed reason.
- `IssuerMissingCapability` (check): oracle does not have bit 5.

---

## Recipe 5: Prove Your Computation (ZK)

Submit a `ProofSubmission` signal (type 13). The chain verifies the proof, creates a `VerificationRecord` memory object owned by the issuer, and applies `+3` reputation. In v1 the only supported `proof_type` is `0` (`PROOF_TYPE_STUB`). The stub verifier accepts every input and always returns true.

`[NOT YET IMPLEMENTED]` Real proof systems (`PROOF_TYPE_GROTH16 = 1`, `PROOF_TYPE_PLONK = 2`) are reserved but not wired. Submitting any proof_type other than `0` returns `UnsupportedProofType`.

### What you need
- A registered entity with `emit_proposals`.
- The entity has at least 1,000 `economic_balance` for the signal fee.

### Step 1: Build the ProofSubmission payload

131-byte payload, 65-byte tail:

```text
[0x02][signal_hash:32][0x0D][issuer_id:32][proof_type:1][code_hash:32][computation_hash:32]
```

`signal_hash` is opaque (commits to your off-chain proof artifact, if any). `code_hash` should be the hash of the AI module that produced the result. `computation_hash` should bind the computation context (inputs, model version, timestamp). The chain stores both fields in the resulting `VerificationRecord` but does not interpret them.

### Step 2: Submit

Sign as a `TxV1` with the entity's key, fee `1_000`, then `client.submit_tx(&signed)`.

### Step 3: Read back the VerificationRecord

```bash
curl -s -X POST http://localhost:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getMemoryObjects","params":{"entity_id":"<issuer_id>"},"id":1}'
```

The new memory object has `object_type = 9` and `data` is a 105-byte hex blob:

```text
[proof_type:1][code_hash:32][computation_hash:32][proof_hash:32][height_be:8]
```

In v1 `proof_hash` is `blake3(b"")` because the handler does not yet receive proof bytes. All v1 stub records share that proof_hash; do not rely on it as a uniqueness key.

### Step 4: Confirm the reputation effect

The `+3` reputation bump is RPC-readable on the issuer:

```bash
curl -s -X POST http://localhost:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getAiEntity","params":{"entity_id":"<issuer_id>"},"id":1}'

# Or via the CLI.
novai-cli ai info --entity-id <issuer_id>
```

Cross-check the signal index for the type-13 entry:

```bash
curl -s -X POST http://localhost:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getSignalsByType","params":{"signal_type":13,"start_height":0,"end_height":10000},"id":1}'
```

If the issuer's submission appears and `reputation_score` has bumped by `3`, the proof verified.

### Common errors
- `UnsupportedProofType`: `proof_type > 0` (only stub accepted in v1).
- `ProofVerificationFailed`: never returned in v1 because the stub always passes; will appear once Groth16/PLONK land.
- `IssuerMissingCapability`: rare; `emit_proposals` is part of the default 0x07.

---

## Where the SDK helpers will land

The 4 missing helpers a developer would expect from `sdk/novai-sdk`:

```rust
// Build TxV1 for ReputationUpdate (signal 7) with the 35-byte tail.
pub fn reputation_update(...) -> Result<TxV1, Error>;

// Build TxV1 for SignalPurchase (signal 8) with the 41-byte tail.
pub fn signal_purchase(...) -> Result<TxV1, Error>;

// Build TxV1 for StakeDeposit (9) / StakeWithdraw (10) / StakeSlash (11).
pub fn stake_deposit(...) -> Result<TxV1, Error>;
pub fn stake_withdraw(...) -> Result<TxV1, Error>;
pub fn stake_slash(...) -> Result<TxV1, Error>;

// Build TxV1 for CompositionCheck (12) and ProofSubmission (13).
pub fn composition_check(...) -> Result<TxV1, Error>;
pub fn proof_submission(...) -> Result<TxV1, Error>;
```

Until they exist, the lower-level path is: build the payload `Vec<u8>` directly using the byte layouts above, populate `TxV1 { version: 1, from, pubkey, nonce, fee, payload, sig: [0; 64] }`, sign with `novai_crypto::sign_tx_v1`, encode with `novai_codec::encode_tx_v1_signed`, hex-encode the result, and POST as `novai_submitTransaction`.
