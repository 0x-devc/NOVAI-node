# NOVAI Architecture Decisions

> **⚠️ CONSENSUS-CRITICAL SPECIFICATION**
>
> This document contains the canonical specification for consensus-critical components.
>
> **Rules**:
> - If code and docs disagree, **code wins** — but treat the disagreement as a bug that must be fixed immediately.
> - Any change to consensus-critical parameters in this document **requires updating golden tests and test vectors**.
> - These specifications are binding for all node implementations and must produce identical results across all platforms.

## Week 4: Sparse Merkle Tree (SMT) State Root

### Overview
Every block includes a deterministic `state_root` that authenticates the entire state via a Sparse Merkle Tree.

### Consensus-Critical Parameters

#### Height Contract
- **Type**: `u16` (supports 0-65535)
- **Leaf height**: `0`
- **Root height**: `256` (for 256-bit keys)
- **Rationale**: Keys are 32 bytes = 256 bits, requiring 256 levels

#### Domain Separation Tags
Domain separation prevents collision attacks between different node types.
```rust
const TAG_EMPTY: u8 = 0x00;
const TAG_LEAF: u8 = 0x01;
const TAG_INTERNAL: u8 = 0x02;
```

#### Empty Hash Formula
Empty subtrees have deterministic, height-specific hashes:
```
empty_hash(height) = blake3(TAG_EMPTY || height_u16_be)
```

**NOT recursive** — each height has a unique, directly computed hash.

#### Leaf Hash Formula
```
leaf_hash(key, value) = blake3(TAG_LEAF || key32 || blake3(value))
```

Value is hashed first to support arbitrary lengths without changing leaf layout.

#### Internal Node Hash Formula
```
internal_hash(left, right) = blake3(TAG_INTERNAL || left32 || right32)
```

#### Node Encoding Format
Internal nodes are stored as 67 bytes:
```
[node_tag:1][left_child:33][right_child:33]

node_tag = 0xA1 (fixed)

child encoding (33 bytes):
  - Hash pointer:  [0x01][hash:32]
  - Empty subtree: [0x00][height_u16_be:2][padding:30 zeros]
```

**Rationale**: Fixed-size encoding enables deterministic serialization and fast lookups.

#### State Key → SMT Key Mapping
All state keys are mapped to 32-byte SMT keys via:
```rust
smt_key = blake3(state_key_bytes)
```

**Location**: `novai_state::smt_key_for_state_key()`

**Consensus requirement**: This mapping must be used consistently everywhere.

#### Storage Schema
- **SMT root**: `smt/root` → versioned 33-byte encoding (v1 = `0x01 || root32`)
- **SMT nodes**: `smt/node/<hash32>` → 67-byte node encoding

#### Write Ordering
SMT node writes are sorted by key before being added to the atomic batch to ensure deterministic ordering across all nodes.

**Proof level**: Deterministic by construction (explicit sort). Root determinism proven by paranoia test. Full DB byte-for-byte comparison would require key iteration (not currently implemented in MemKv test store). Under collision resistance of Blake3, identical roots strongly imply identical committed content given deterministic construction. This implication is computational: finding distinct states with the same root would require a practical collision/second-preimage attack or violating the deterministic construction rules.

### Implementation Details

#### Files
- `crates/smt/src/hash.rs` — Hashing rules
- `crates/smt/src/node.rs` — Node encoding/decoding
- `crates/smt/src/smt.rs` — Tree logic
- `crates/state/src/lib.rs` — Storage schema + key mapping
- `crates/execution/src/lib.rs` — Atomic integration

#### Tests
- `crates/smt/tests/golden_roots.rs` — Locks hash computation
- `crates/smt/tests/height_contract.rs` — Locks height semantics
- `crates/smt/tests/missing_node_is_error.rs` — Proves error strictness
- `crates/execution/tests/smt_root_recompute_matches.rs` — Nuclear proof (stored = rebuilt)

### References
- Blake3: https://github.com/BLAKE3-team/BLAKE3
- Sparse Merkle Trees: https://eprint.iacr.org/2016/683.pdf
---

## AI-Native Architecture Decisions (Retrofit Week 1)

### Decision A8: AI Entities as Protocol Primitives

AI entities are first-class protocol primitives, NOT smart contracts. They have:

- **Stable Identity**: Computed as `blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator_address)`
- **Persistent Memory**: Separate storage namespace (`ai_memory/`)
- **Economic Agency**: Own assets via `economic_balance`, pay fees
- **Explicit Capabilities**: Permission flags grant/deny specific actions

This design ensures AI behavior is predictable, auditable, and controllable at the protocol level.

### Decision A9: Autonomy Modes

Three autonomy modes define AI behavior progression:

| Mode | Value | Description |
|------|-------|-------------|
| **Advisory** | 0 | Propose only, never execute. All proposals require explicit human/governance approval. |
| **Gated** | 1 | Mode B - Proposals go through approval gates with multisig + timelock. |
| **Autonomous** | 2 | Mode C - Reserved for future. Requires ZK-proof verification for execution. |

Default is **Advisory**. Upgrade to Gated requires governance approval with timelock.

### Decision A10: Capability Manifest

Every AI entity has explicit capabilities encoded as bitflags:

| Bit | Capability | Description |
|-----|------------|-------------|
| 0 | `read_public_chain` | Read blocks, transactions, accounts |
| 1 | `read_memory_objects` | Read L1 on-chain memory |
| 2 | `emit_proposals` | Create proposal objects |
| 3 | `request_execution` | Request Tier 1/2 actions (must pass gates) |
| 4 | `read_nnpx_derived` | Access NNPX derived views (bounded, schema-validated) |

Capabilities are immutable per module version. New capabilities require a new manifest registration.
