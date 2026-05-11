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

Submit a `ProofSubmission` signal (type 13). The chain verifies the proof, creates a `VerificationRecord` memory object owned by the issuer, and applies `+3` reputation. Two `proof_type` values are accepted today:

- `PROOF_TYPE_STUB = 0`: development-only path. The stub verifier returns true unconditionally. Use this for plumbing tests and to validate the on-chain side of your pipeline before you have real proofs.
- `PROOF_TYPE_GROTH16 = 1`: real BN254 Groth16. Submit the verifying key and the proof inline in the signal payload; the chain runs `ark_groth16::Groth16::<Bn254>::verify_proof` and rejects on failure.

`PROOF_TYPE_PLONK = 2` is reserved but not wired; submitting it returns `UnsupportedProofType { proof_type: 2 }`.

### What you need
- A registered entity with `emit_proposals` (in the default `0x07` capabilities mask).
- At least `1_000` `economic_balance` for the signal fee.
- For Groth16: a BN254 trusted setup for your circuit and the ability to produce ark-serialize compressed bytes for the resulting `VerifyingKey<Bn254>` and `Proof<Bn254>`.

### Path A: Stub submission (development)

Use this to exercise the on-chain side without producing real proofs.

**Step 1: Build the v1 ProofSubmission payload.** 131 bytes, fixed:

```text
[0x02][signal_hash:32][0x0D][issuer_id:32]
[proof_type:1=0][code_hash:32][computation_hash:32]
```

`signal_hash` is opaque (treat it as a commitment to your off-chain proof artifact if any). `code_hash` should be the hash of the AI module that produced the result; `computation_hash` should bind the computation context (inputs, model version, timestamp). The chain stores both in the resulting `VerificationRecord` but does not interpret them.

**Step 2: Submit.** Sign as a `TxV1` with the entity's key, fee `1_000`, then `client.submit_tx(&signed)`.

**Step 3: Read back the record.** The new memory object has `object_type = 9` and `data` is a 105-byte hex blob:

```text
[proof_type:1][code_hash:32][computation_hash:32][proof_hash:32][height_be:8]
```

For stub submissions `proof_hash = blake3(b"")` (no inline proof bytes); all stub records share that hash, so do not use it as a uniqueness key. Each stub submission still produces a fresh memory object whose `object_id` is unique per `(owner, type, height, data)`.

### Path B: Groth16 submission (production)

This is the real path. The signal payload carries the verifying key and the proof inline; the chain pairing-checks them on the way in.

**Public-input contract.** The verifier computes `public_inputs = code_hash || computation_hash` (64 bytes) and splits the buffer into four BN254 scalar-field elements by big-endian 16-byte halves, lifting each half into `Fr` via `u128::from_be_bytes`:

```
fr[0] = Fr::from(u128::from_be_bytes(code_hash[0..16]))
fr[1] = Fr::from(u128::from_be_bytes(code_hash[16..32]))
fr[2] = Fr::from(u128::from_be_bytes(computation_hash[0..16]))
fr[3] = Fr::from(u128::from_be_bytes(computation_hash[16..32]))
```

Your circuit MUST be set up for exactly four public inputs in this canonical order. The 128-bit values fit comfortably below the BN254 scalar-field modulus (about 2^254), so the mapping is bias-free.

**Step 1: Produce VK and proof off-chain.** Using arkworks 0.5 with `default-features = false`:

```rust
use ark_bn254::{Bn254, Fr};
use ark_groth16::Groth16;
use ark_serialize::CanonicalSerialize;

// 1. trusted setup (your circuit, your RNG)
let pk = Groth16::<Bn254>::generate_random_parameters_with_reduction(
    setup_circuit, &mut rng,
).expect("setup");

// 2. build the same 4 Fr public inputs the chain will compute
let public_inputs_bytes = {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(&code_hash);
    buf[32..].copy_from_slice(&computation_hash);
    buf
};
let fr_inputs: [Fr; 4] = /* hi/lo split as above */;

// 3. prove
let proof = Groth16::<Bn254>::create_random_proof_with_reduction(
    prove_circuit_with(fr_inputs), &pk, &mut rng,
).expect("prove");

// 4. serialize for the wire
let mut vk_bytes = Vec::new();
pk.vk.serialize_compressed(&mut vk_bytes).unwrap();
let mut proof_bytes = Vec::new();
proof.serialize_compressed(&mut proof_bytes).unwrap();
```

The reference circuit and helper used in the NOVAI integration tests live in `crates/execution/tests/verification_system.rs` (search for `gen_valid_groth16_proof`). A canonical BN254 Groth16 VK for four public inputs is roughly 200 to 300 bytes compressed; a Groth16 proof is roughly 128 bytes compressed. Limits: `PROOF_SUBMISSION_MAX_VK_BYTES = 8 KiB`, `PROOF_SUBMISSION_MAX_PROOF_BYTES = 1 KiB`.

**Step 2: Build the v2 ProofSubmission payload.** Variable length:

```text
[0x02][signal_hash:32][0x0D][issuer_id:32]
[proof_type:1=1][code_hash:32][computation_hash:32]
[vk_len_be:4][vk_bytes...][proof_len_be:4][proof_bytes...]
```

The 131-byte v1 prefix is preserved bit-for-bit. After it, big-endian `u32` length prefixes precede `vk_bytes` and `proof_bytes`. With a 280-byte VK and a 128-byte proof, total payload is `131 + 4 + 280 + 4 + 128 = 547` bytes.

**Step 3: Submit.** Sign as a `TxV1`, fee `1_000`, then `client.submit_tx(&signed)`. The chain decodes, dispatches to `Groth16Verifier`, and either accepts (write record, bump rep) or rejects with `ProofVerificationFailed`.

**Step 4: Read back the record.** Same memory object layout as Path A. For Groth16 submissions `proof_hash = blake3(proof_bytes)` and IS the stable per-proof identifier you can use as a uniqueness key.

### Confirming the reputation effect

The `+3` reputation bump applies to the issuer on success regardless of path:

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

If the submission appears and `reputation_score` bumped by `3`, the proof verified.

### VK-to-code binding (current limitation)

The chain does NOT enforce a binding between the supplied `vk_bytes` and the entity's `code_hash`. Off-chain observers should recompute `vk_hash = blake3(vk_bytes)` and compare against the entity's published expected VK. A future feature will add either a `vk_commitment` field on `AiEntity` or a per-entity `VkRegistry` memory object; both options are additive to the current wire format.

### Common errors
- `UnsupportedProofType { proof_type }`: `proof_type > PROOF_TYPE_MAX` (which is `1` today). Submitting `2` (PLONK) is the canonical example.
- `ProofVerificationFailed`: Groth16 path only. Causes: tampered proof bytes, wrong VK, mismatched public inputs (the chain's reconstructed `code_hash || computation_hash` differs from what the proof was bound to), malformed ark-serialize bytes.
- `VerifyingKeyTooLarge { actual, max }`: v2 `vk_bytes` length above 8 KiB.
- `ProofBytesTooLarge { actual, max }`: v2 `proof_bytes` length above 1 KiB.
- `IssuerMissingCapability`: rare; `emit_proposals` is part of the default `0x07`.

---

## Recipe 6: Subscribe to a Producer's Signal Stream

This recipe shows how a consumer entity (the SUBSCRIBER) sets up a recurring payment to a producer entity in exchange for a fixed signal type, then how it cancels early to reclaim unused funds.

### What you need
- Two registered entities. The subscriber needs `economic_balance >= rate_per_block * duration_blocks`. Both need the `emit_proposals` capability (default for `Capabilities::gated()`).
- The producer's 32-byte entity id.
- A target `signal_type` byte for the producer signal the subscription pays for (informational; the runtime does not enforce that the producer actually publishes that type).

### Step 1: Issue SubscriptionCreate (signal type 14)

Wire layout for the 49-byte tail: `producer_entity_id:32 | covered_signal_type:1 | rate_per_block_be:8 | duration_blocks_be:8`. Total payload is 115 bytes.

```bash
novai-cli signal publish \
  --key-file subscriber.key \
  --signal-hash 00000000000000000000000000000000000000000000000000000000000000c1 \
  --signal-type subscription-create \
  --issuer-entity-id <subscriber_id> \
  --producer-entity-id <producer_id> \
  --covered-signal-type 2 \
  --rate-per-block 10 \
  --duration-blocks 10000 \
  --fee 1000
```

This debits `rate_per_block * duration_blocks` (in this example, 100,000 base units) from the subscriber's `economic_balance` and creates a `MemoryObjectType::Subscription` (variant 11) memory object owned by the subscriber. The record fixes `start_height = current_height`, `end_height = start_height + duration_blocks`, and `last_settled_height = start_height`.

`duration_blocks` must satisfy `>= MIN_SUBSCRIPTION_DURATION = 100`. Each subscriber may hold at most `MAX_SUBSCRIPTIONS_PER_ENTITY = 10` `Subscription` memory objects (active or cancelled); cancelled records still occupy a slot and must be reclaimed via `DELETE_MEMORY_OBJECT` to make room for new subscriptions. Subscriptions also count against the global `MAX_MEMORY_OBJECTS_PER_ENTITY = 100` cap.

### Step 2: Confirm the lock

```bash
curl -s -X POST http://localhost:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getMemoryObjects","params":{"entity_id":"<subscriber_id>"},"id":1}'
```

Look for a `Subscription` (object_type = 11) entry. The 114-byte data field decodes as `subscriber_entity_id:32 | producer_entity_id:32 | covered_signal_type:1 | rate_per_block_be:8 | start_height_be:8 | end_height_be:8 | last_settled_height_be:8 | total_locked_be:16 | is_active:1`. Capture its `object_id` for the cancel step.

### Step 3: Cancel early (signal type 15) and settle

The producer is paid lazily: no funds move at create time, and no funds move while the subscription runs. The first time settlement happens is when the subscriber issues `SubscriptionCancel`. Cancelling computes `accrued_blocks = min(current_height, end_height) - last_settled_height` and routes the funds in three pieces:

1. Producer receives `accrued_blocks * rate_per_block`, less the standard 2% marketplace fee. The fee accrues to `KEY_MARKETPLACE_TREASURY` exactly as for `SignalPurchase`.
2. On the unaccrued remainder (`total_locked - accrued_blocks * rate_per_block`), the producer is also paid a 5% cancel fee (`SUBSCRIPTION_CANCEL_FEE_BPS = 500`). This compensates the producer for early termination and is paid 100% to the producer with no marketplace cut.
3. The subscriber is refunded the rest of the unaccrued remainder.

```bash
novai-cli signal publish \
  --key-file subscriber.key \
  --signal-hash 00000000000000000000000000000000000000000000000000000000000000c2 \
  --signal-type subscription-cancel \
  --issuer-entity-id <subscriber_id> \
  --subscription-id <object_id_from_step_2> \
  --fee 1000
```

The `Subscription` memory object is rewritten in place with `is_active = false` and `last_settled_height` advanced to `min(current_height, end_height)`. The `object_id` is stable across the rewrite (it was hashed at create time over the original data), so the same id keeps addressing the now-cancelled record.

### Settlement worked example

Take `rate_per_block = 10`, `duration_blocks = 10_000`, so `total_locked = 100_000`. Cancel after 1,000 of the 10,000 blocks have elapsed:

| Quantity | Math | Result |
|---|---|---|
| `accrued_gross` | `1_000 * 10` | `10_000` |
| `accrued_fee` (2% to treasury) | `10_000 * 200 / 10_000` | `200` |
| `accrued_net` (to producer) | `10_000 - 200` | `9_800` |
| `remaining` | `100_000 - 10_000` | `90_000` |
| `cancel_fee` (5% to producer) | `90_000 * 500 / 10_000` | `4_500` |
| `refund` (to subscriber) | `90_000 - 4_500` | `85_500` |
| Producer credit | `9_800 + 4_500` | `14_300` |
| Treasury credit | `200` | `200` |

Cancelling at or after `end_height` settles the full duration with no refund and no cancel fee; the producer receives the full `accrued_net`.

### Common errors
- `SubscriptionInsufficientBalance`: subscriber's `economic_balance` does not cover `rate_per_block * duration_blocks`.
- `SubscriptionDurationTooShort { required, given }`: `duration_blocks < MIN_SUBSCRIPTION_DURATION = 100`.
- `SubscriptionLimitExceeded { current, max }`: subscriber holds 10 `Subscription` records already (active or cancelled). Reclaim slots with `DELETE_MEMORY_OBJECT`.
- `SubscriptionProducerNotFound` / `SubscriptionProducerNotActive`: bad `--producer-entity-id`.
- `SubscriptionSelfReferential`: subscriber id equals producer id.
- `SubscriptionNotFound`: cancel issued by an entity other than the original subscriber, or `--subscription-id` does not match any record under the issuer.
- `SubscriptionNotActive`: the referenced subscription has already been cancelled.
- `SubscriptionRateOverflow`: forward-compat path; with v1's u64-bounded `rate_per_block` and `duration_blocks` the product cannot overflow `u128`. Surfaces only if either operand is widened.

### Known v1 limitations
- Only the original subscriber may cancel. If the subscriber abandons the subscription (never cancels), the producer cannot trigger settlement and forfeits the last unsettled blocks of payment.
- Settlement is not triggered automatically by other signals from either party in v1; it happens exclusively at `SubscriptionCancel` time. A future revision may plumb settlement into a per-block tick or into every signal handler.

---

## Recipe 7: Delegate a Capability (Master + Sub-Entity)

This recipe shows how a master entity grants a single capability (here, `emit_proposals`) to a sub-entity for a bounded duration, so the sub-entity can act on the master's behalf without holding the capability statically. Revocation is a one-tx `DELETE_MEMORY_OBJECT` and takes effect immediately.

### What you need
- A master entity (the DELEGATOR) registered with the capability you intend to grant. `Capabilities::advisory()` gives you `read_public_chain | read_memory_objects | emit_proposals = 0x07`, which is enough for this recipe.
- A sub-entity (the DELEGATE) registered with `Capabilities::read_only() = 0x03` (no `emit_proposals` of its own).
- Both entities' 32-byte ids; the delegate's id is what gets embedded in the grant payload.

### Step 1: Master issues a DelegationGrant (memory object type 10)

Wire layout for the 42-byte payload: `version:1 | delegate_entity_id:32 | granted_capabilities:1 | expires_at_be:8`. `version` must equal 1. `granted_capabilities` is the same byte format as `Capabilities::to_byte`: bit 2 (`0x04`) is `emit_proposals`. `expires_at` is a block height; pass `0` for no expiry.

```bash
# Build the 42-byte payload by hand (no CLI sugar yet) and base64/hex-encode it
# for `novai-cli memory create --data-hex`. The payload below grants 0x04
# (emit_proposals) to the delegate, no expiry.
PAYLOAD_HEX=$(python3 -c "
import sys
delegate_id = bytes.fromhex('<delegate_id>')
version = b'\\x01'
granted = b'\\x04'
expires = (0).to_bytes(8, 'big')
sys.stdout.write((version + delegate_id + granted + expires).hex())
")

novai-cli memory create \
  --key-file master.key \
  --object-type delegation-grant \
  --data-hex $PAYLOAD_HEX \
  --fee 500
```

The master entity's `economic_balance` is debited by the fee. The runtime decodes the payload, verifies the delegate is not the master itself (`InvalidDelegationSelf` otherwise), verifies every bit set in `granted_capabilities` is also set in the master's static caps (`DelegationCapabilityNotHeld` otherwise), and verifies the master holds fewer than `MAX_DELEGATION_GRANTS = 20` open grants (`DelegationCountExceeded` otherwise). On success it writes the primary memory object record AND a secondary index entry `ai/delegations_by_delegate/<delegate_id>/<grant_id>` whose value is the master's id.

### Step 2: The sub-entity uses the delegated capability

The sub-entity emits a signal it could not have emitted before the grant. `dispatch_tx` calls `requires_capability(db, &sub_entity, current_height, |c| c.emit_proposals)`. The fast path sees `sub_entity.capabilities.emit_proposals == false` and falls through to the slow path. The resolver scans `ai/delegations_by_delegate/<sub_entity_id>/`, finds the master's grant, confirms the master is still `is_active` and the grant has not expired, and ORs `0x04` into the effective set. The signal passes admission.

```bash
novai-cli signal publish \
  --key-file sub_entity.key \
  --signal-hash 0000000000000000000000000000000000000000000000000000000000000071 \
  --signal-type anomaly \
  --issuer-entity-id <sub_entity_id> \
  --fee 1000
```

### Step 3: Confirm the grant

Query the master's memory objects and look for `object_type = 10`:

```bash
curl -s -X POST http://localhost:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getMemoryObjects","params":{"entity_id":"<master_id>"},"id":1}'
```

The `data` field of the grant decodes as `version:1 | delegate_entity_id:32 | granted_capabilities:1 | expires_at_be:8`.

### Step 4: Revoke

Delete the grant memory object:

```bash
novai-cli memory delete \
  --key-file master.key \
  --object-id <grant_id> \
  --fee 500
```

The atomic batch tears down both the primary record and the by-delegate index entry. Any subsequent signal from the sub-entity that depends on the delegated capability is rejected immediately with `IssuerMissingCapability`; there is no propagation delay.

### Common errors
- `InvalidDelegationSelf`: master's id equals the embedded `delegate_entity_id`.
- `DelegationCapabilityNotHeld`: `granted_capabilities` includes a bit the master does not hold statically. An entity cannot grant authority it lacks.
- `DelegationCountExceeded { current, max }`: master already holds 20 open grants. Delete one before issuing another. Expired-but-undeleted grants still count toward the cap; reclaim slots with `DELETE_MEMORY_OBJECT`.
- `InvalidDelegationGrant`: payload bytes do not decode as `DelegationGrantData` (wrong length, wrong version byte).
- `DelegationGrantNotUpdatable`: `UPDATE_MEMORY_OBJECT` targeting a `DelegationGrant` is rejected outright. Grants are immutable; delete and recreate to change scope or duration.
- `IssuerMissingCapability` on the sub-entity's signal: no active grant covers the requested capability. Check that the master is still `is_active`, that `expires_at == 0` or `current_height < expires_at`, and that the master has not deleted the grant.

### Known v1 limitations
- Delegation is not transitive: a grant from A to B does not let B re-delegate A's capability to a third entity C. The resolver only consults grants directly naming the calling entity.
- There is no separate `manage_delegations` capability bit. Any entity that can issue `CREATE_MEMORY_OBJECT` (`read_memory_objects = bit 1`) can create a `DelegationGrant`, with the subset check ensuring it cannot grant authority it does not itself hold.
- Expired grants are NOT swept automatically. They remain in state until the delegator issues `DELETE_MEMORY_OBJECT`, and they count against `MAX_DELEGATION_GRANTS` until then.

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
