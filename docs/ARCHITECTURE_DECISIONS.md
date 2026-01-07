# NOVAI Architecture Decisions

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
