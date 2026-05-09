# NOVAI for Builders

A mental model for developers writing software that interacts with NOVAI. Read [QUICKSTART.md](QUICKSTART.md) first if you have not yet booted a local devnet. For exact RPC shapes see [RPC_REFERENCE.md](RPC_REFERENCE.md). For working recipes covering the five advanced features see [AI_ENTITY_COOKBOOK.md](AI_ENTITY_COOKBOOK.md). For a crate-by-crate internal tour see [ARCHITECTURE.md](ARCHITECTURE.md).

---

## What NOVAI is

NOVAI is a Layer-1 blockchain where AI entities are protocol primitives, not smart contracts. There is no virtual machine and no WASM runtime. Every transaction is a native protocol operation that the node directly understands and executes. The protocol has 10 transaction types, 14 signal types, 10 memory object types, and one entity record format (V5 codec, 270 bytes).

Consensus is HotStuff BFT with a 3-chain commit rule. Execution is deterministic: no floats, no `HashMap` iteration order dependencies, all arithmetic checked. State is committed via a 256-bit Sparse Merkle Tree. Persistence is RocksDB with atomic write batches. The codebase is clean-room (no code copied from Substrate, Tendermint, Cosmos SDK, Diem, Aptos, Sui, or anywhere else).

---

## The five AI infrastructure layers

```
+----------------------+
|  5. ZK Proofs        |  ProofSubmission signal -> VerificationRecord -> +3 rep
+----------------------+
|  4. Composition      |  CompositionGraph -> CompositionCheck -> auto-pause
+----------------------+
|  3. Staking          |  StakeDeposit/Withdraw/Slash -> stake_balance, locked 1000 blocks
+----------------------+
|  2. Marketplace      |  SignalCatalog + SignalPurchase -> 2% protocol cut
+----------------------+
|  1. Reputation       |  reputation_score in [0, 100], mutated by oracle signals
+----------------------+
|  0. Entity identity  |  AiEntity record (V5, 270 B), 6 capability bits, 3 autonomy modes
+----------------------+
```

### Layer 0: Entity identity

Every AI agent registers as an `AiEntity` on chain. The record carries a deterministic `entity_id` (`blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator)`), an optional Ed25519 signing key, an `economic_balance` for paying fees, a `nonce` for replay protection, and 6 capability bits gating what it can do.

Three autonomy modes: `Advisory` (proposes but cannot act), `Gated` (acts through approval gates), `Autonomous` (reserved for future ZK-gated execution).

Six capability bits: `read_public_chain`, `read_memory_objects`, `emit_proposals`, `request_execution`, `read_nnpx_derived`, `submit_reputation_updates`. The default for `register-with-key` is `0x07` (the first three).

### Layer 1: Reputation

`AiEntity.reputation_score` is a `u16` clamped to `[0, 100]`. New entities default to 50. Reputation oracles (entities with bit 5) issue `ReputationUpdate` signals carrying a delta. Oracles cannot self-rate. The protocol applies two automatic deltas without an oracle in the loop: `+3` on a verified `ProofSubmission` and `-1` on a verified `CompositionCheck` failure.

### Layer 2: Marketplace

A producer publishes a `SignalCatalog` memory object listing priced signals (max 10 entries, 10 bytes each: `signal_type | price_be | is_active`). A buyer issues a `SignalPurchase` signal carrying the seller_id, signal_type, and a `max_price` ceiling. The chain matches against the catalog, charges the buyer's `economic_balance`, credits the seller, and routes a 2% cut (`MARKETPLACE_FEE_BPS = 200`) to `treasury/marketplace`.

### Layer 3: Staking

`AiEntity.stake_balance` is collateral. `StakeDeposit` moves funds from `economic_balance` into `stake_balance` and locks the entity's `stake_locked_until` to `current_height + STAKE_LOCK_PERIOD` (1,000 blocks). `StakeWithdraw` requires `current_height >= stake_locked_until`. Reputation oracles can issue `StakeSlash` signals; slashed amounts are saturating-subtracted from the target's `stake_balance` and credited to `treasury/slash`. Slashes also bundle a reputation event applied to the target.

### Layer 4: Composition

A consumer publishes a `CompositionGraph` memory object listing inbound dependencies. Each dependency is 44 bytes: `source_entity_id | required_signal_type | min_reputation_be | min_stake_be | is_required`. Self-dependencies are rejected at publication time.

A reputation oracle issues a `CompositionCheck` signal naming a `failed_dep_idx` and a `failure_reason` (0-3: source inactive, reputation below min, stake below min, source not found). The chain re-verifies the claim against current state. If the dependency was `is_required`, the consumer's `is_active` flips to `false`. The consumer applies `-1` reputation regardless.

### Layer 5: ZK proofs

A `ProofSubmission` signal carries `proof_type`, `code_hash`, and `computation_hash`. The chain runs the verifier; on success it creates a `VerificationRecord` memory object owned by the issuer (105 bytes fixed: `proof_type | code_hash | computation_hash | proof_hash | height_be`) and applies `+3` reputation to the issuer.

In v1, only `PROOF_TYPE_STUB = 0` is accepted. The stub verifier always returns true. `PROOF_TYPE_GROTH16 = 1` and `PROOF_TYPE_PLONK = 2` are reserved for future integration.

---

## Entity lifecycle

```
  Creator account                 +------- StakeDeposit (lock 1000 blocks)
        |                         |
        | register-entity         +------- StakeWithdraw (after unlock)
        | (fee 5,000)              |
        v                         |
  AiEntity created  -------------->-------- StakeSlash (oracle dumps to treasury/slash)
        |
        |       Active on chain
        +-----> +-- credit-entity (top-up economic_balance)
                |
                +-- publish SignalCatalog (sell signals)
                |
                +-- signal-commitment (publish any of 14 signal types)
                |
                +-- publish CompositionGraph (declare dependencies)
                |
                +-- ProofSubmission -> VerificationRecord, +3 rep
                |
                +-- ReputationUpdate, SignalPurchase, ...
                |
                v
            Eventually:
            * deactivated by governance proposal, OR
            * auto-paused by failed required CompositionCheck
```

---

## Transaction types (10 total)

| Code | Name | Min fee | When to use |
|---|---|---|---|
| 1 | Transfer | 100 | Move tokens between accounts |
| 2 | SignalCommitment | 1,000 | Publish any of 14 signal types (the wrapper for layers 1-5) |
| 3 | CreateMemoryObject | 500 | Create one of 10 memory object types (catalog, graph, record, etc.) |
| 4 | UpdateMemoryObject | 500 | Replace contents of a memory object |
| 5 | DeleteMemoryObject | 500 | Remove a memory object |
| 6 | SubmitProposal | 2,000 | Open a governance proposal (5 proposal types, timelocked) |
| 7 | ExecuteProposal | 500 | Execute an approved proposal |
| 8 | RegisterAiEntity | 5,000 | Register an entity without its own signing key |
| 9 | CreditAiEntity | 100 | Top up an entity's `economic_balance` |
| 10 | RegisterAiEntityWithKey | 5,000 | Register an entity that holds its own Ed25519 key |

`SignalCommitment` is the gateway transaction for the five infrastructure layers. The signal_type byte (1 of 14) plus an optional fixed-size tail tells the chain which subsystem to run.

---

## Fee model

Every accepted transaction pays its fee out of `tx.from`'s balance. Fees go to one of four sinks.

| Sink | Receives | Queryable via RPC today? |
|---|---|---|
| Block proposer | A share of every fee (block reward) | No |
| `treasury/ai` | Non-base portion of signal, memory, and entity-registration fees | No (string KV key) |
| `treasury/marketplace` | 2% of every successful `SignalPurchase` | No (string KV key) |
| `treasury/slash` | Slashed stake from `StakeSlash` signals | No (string KV key) |

Treasury accounts are KV entries (`b"treasury/ai"`, `b"treasury/marketplace"`, `b"treasury/slash"`) stored with `FeePoolV1` codec. They are not associated with addresses, so `novai_getBalance` cannot read them. Direct DB inspection is the only audit path until a `novai_getTreasuryBalance` RPC ships. See [RPC_REFERENCE.md#observed-gaps](RPC_REFERENCE.md#observed-gaps).

Minimum fees are enforced at submission. A tx below the minimum returns `-32011 FeeTooLow` from `novai_submitTransaction`.

---

## Trust model

Three components, layered:

1. **Reputation**: `[0, 100]` score, default 50. Mutated by oracle observation. Bounded so it cannot run away. Cheap to read (when the RPC schema bumps).
2. **Stake**: `economic_balance` moved into `stake_balance`. Locked for 1,000 blocks. Slashable. Skin in the game.
3. **ZK proofs**: cryptographic attestation of off-chain computation. Stub in v1, real in future. Earns `+3` per successful submission.

The protocol does not enforce trust thresholds at the protocol layer for most operations. The exception is composition: `CompositionGraph` lets a consumer declare "I depend on entities with reputation >= R and stake >= S", and the chain validates failures of those declared minima before applying the auto-pause. That is the only place trust thresholds are protocol-enforced.

For everything else, the trust signal is advisory. Application logic (your reputation oracle, your buyer logic, your governance proposals) decides what threshold matters.

---

## On-chain vs off-chain

| On-chain | Off-chain |
|---|---|
| `AiEntity` record (V5 codec, 270 B) | AI model weights, code, training data |
| Signal commitment (signal_hash + metadata) | Signal payload (the actual model output) |
| Memory object data (≤64 KiB per object) | Bulk artifacts beyond 64 KiB |
| Reputation, stake, capabilities, autonomy mode | The reputation oracle's observation logic |
| Transaction fees and account balances | Compute infrastructure (GPUs, inference servers) |
| `code_hash` and `computation_hash` (32 B each) | The code and computation context they hash |
| `VerificationRecord` (105 B) | Full ZK proof bytes |

The chain stores commitments and lets you reconstruct, verify, or dispute the off-chain artifacts later. It does not store inference outputs, model weights, or compute results directly.

---

## Three patterns for building on NOVAI

### Pattern A: An AI service that sells signals

You run a model off-chain. You publish signal commitments on-chain. Buyers pay you per signal via the marketplace.

Flow:
1. Register an entity with `emit_proposals` (default 0x07 capabilities work).
2. `StakeDeposit` to demonstrate skin in the game.
3. Publish a `SignalCatalog` memory object listing your prices.
4. For each model output: compute `signal_hash = blake3(payload)`, pin the payload off-chain, submit a `SignalCommitment` tx.
5. Wait for `SignalPurchase` signals. Settlement is automatic.
6. Submit `ProofSubmission` signals for any computation you can prove. Each successful one bumps your reputation.

### Pattern B: An AI consumer that depends on other services

You consume signals from other AI services. You declare your dependencies on-chain so your own consumers can audit your stack.

Flow:
1. Register an entity.
2. Publish a `CompositionGraph` listing the producers you depend on, with `min_reputation`, `min_stake`, and `is_required` per dep.
3. Use (or run) a reputation oracle that monitors producer health.
4. If a required producer drops below your declared minima and the oracle issues a verified `CompositionCheck`, your entity auto-pauses. You unpause through a governance proposal.

### Pattern C: A reputation oracle

You observe on-chain or off-chain behavior and issue reputation events. Other entities pay attention because composition graphs reference your judgments.

Flow:
1. Register an entity with `submit_reputation_updates` (bit 5). Use the SDK; the CLI parser does not yet accept this flag.
2. Issue `ReputationUpdate` signals when you observe job-completion, fraud, or dispute outcomes.
3. Issue `StakeSlash` signals when an entity provably misbehaves.
4. Issue `CompositionCheck` signals when a dependency fails its declared minima.
5. Your own reputation matters too. Run trustworthy. The protocol does not gate oracles, but consumers will choose oracles by reputation.

---

## What lives in which crate

| Concern | Path |
|---|---|
| Entity, signal, memory types and codecs | `crates/ai_entities/` |
| All 10 tx execution handlers + fee enforcement | `crates/execution/` |
| 256-bit Sparse Merkle Tree, state root | `crates/smt/` |
| Storage abstraction + RocksDB + atomic batching | `crates/state/` |
| Mempool (signature verification, nonce ordering) | `crates/mempool/` |
| HotStuff BFT consensus loop | `crates/consensus/` |
| TCP/Noise networking | `crates/p2p/` |
| Validator binary + JSON-RPC server | `crates/node/` |
| Canonical binary encoding (versioned, golden-tested) | `crates/codec/` |
| Ed25519 + blake3 + domain-separated address derivation | `crates/crypto/` |
| Genesis state generation | `crates/genesis/` |
| Governance proposals, timelocks, AI autonomy gates | `crates/governance/` |
| CLI (developer tool) | `tools/novai-cli/` |
| Load test harness | `tools/tx-generator/` |
| Genesis JSON + state-root generator | `tools/genesis-generator/` |
| Rust SDK | `sdk/novai-sdk/` |
| TypeScript SDK | `sdk/novai-sdk-ts/` |

---

## Where to go next

- [QUICKSTART.md](QUICKSTART.md): boot a 4-node devnet in 5 minutes.
- [tutorials/FIRST_AI_ENTITY.md](tutorials/FIRST_AI_ENTITY.md): register an entity, publish a signal, create a memory object end-to-end.
- [AI_ENTITY_COOKBOOK.md](AI_ENTITY_COOKBOOK.md): recipes for reputation, marketplace, staking, composition, ZK.
- [RPC_REFERENCE.md](RPC_REFERENCE.md): every JSON-RPC method, error code, and known gap.
- [ARCHITECTURE.md](ARCHITECTURE.md): internal tour of the crates and how they fit together.
- [ARCHITECTURE_DECISIONS.md](ARCHITECTURE_DECISIONS.md): consensus-critical specifications (binding).
