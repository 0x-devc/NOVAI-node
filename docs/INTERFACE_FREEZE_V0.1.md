# NOVAI Interface Freeze Notice - testnet-v0.1

**Effective Date**: January 19, 2026
**Protocol Version**: 1
**Git Tag**: `testnet-v0.1`
**Scope**: Base chain interfaces (pre-AI primitives)

---

## ⚠️ INTERFACE FREEZE IN EFFECT

The interfaces documented below are **FROZEN** as of testnet-v0.1. Any changes to these interfaces constitute a **BREAKING CHANGE** and require:

1. New version constant (e.g., `V2`)
2. Backward compatibility for `V1`
3. Migration documentation
4. Network upgrade announcement
5. Major version bump

---

## 1. Types Interface

**File**: `crates/types/src/lib.rs`
**Purpose**: Core protocol types shared across all components

### 1.1 Type Aliases

```rust
/// Address type: 32-byte ed25519 public key hash
pub type Address = [u8; 32];

/// Transaction ID type: 32-byte hash
pub type TxId = [u8; 32];

/// Generic 32-byte hash type
pub type Hash32 = [u8; 32];

/// Account nonce type
pub type Nonce = u64;

/// Transaction fee type
pub type Fee = u64;

/// Signature bytes type (ed25519 signature)
pub type SignatureBytes = [u8; 64];
```

### 1.2 Version Enums

```rust
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxVersion {
    V1 = 1,
}

impl TxVersion {
    pub fn from_u8(v: u8) -> Option<Self>;
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockHeaderVersion {
    V1 = 1,
}

impl BlockHeaderVersion {
    pub fn from_u8(v: u8) -> Option<Self>;
}
```

### 1.3 Transaction Structure

```rust
/// Canonical V1 transaction.
///
/// Signing rule:
/// - Signature computed over canonical *unsigned* encoding (everything except `sig`)
/// - `from` is Address (blake3 hash of pubkey)
/// - `pubkey` is actual ed25519 public key for verification
/// - Verifiers MUST check: from == blake3(pubkey)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxV1 {
    pub version: TxVersion,
    pub from: Address,
    pub pubkey: [u8; 32],
    pub nonce: Nonce,
    pub fee: Fee,
    pub payload: Vec<u8>,
    pub sig: SignatureBytes,
}
```

### 1.4 Block Header Structure

```rust
/// Canonical V1 block header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockHeaderV1 {
    pub version: BlockHeaderVersion,
    pub height: u64,
    pub prev_hash: Hash32,
    pub state_root: Hash32,
    pub tx_root: Hash32,
    pub proposer: Address,
    pub qc_hash: Hash32,
}
```

---

## 2. Consensus Types Interface

**File**: `crates/consensus_types/src/lib.rs`
**Purpose**: Consensus message types (Block, Vote, QC, Timeout, etc.)

### 2.1 Block Structure

```rust
/// Consensus block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub height: u64,
    pub round: u64,
    pub parent_hash: [u8; 32],
    pub state_root: [u8; 32],
    pub txs: Vec<novai_types::TxV1>,
}
```

### 2.2 Proposal Structures

```rust
/// Proposal message (block + justification).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub block: Block,
    pub justify_qc: QC,
}

/// Signed proposal for network transmission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedProposal {
    pub proposer: Address,
    pub proposal: Proposal,
    pub signature: [u8; 64],
}
```

### 2.3 Vote Structure

```rust
/// Vote message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vote {
    pub height: u64,
    pub round: u64,
    pub block_hash: [u8; 32],
    pub voter: Address,
    pub signature: [u8; 64],
    /// Optional AI signal commitment (hash only, advisory).
    /// Does NOT affect vote validity.
    pub ai_signal_commitment: Option<[u8; 32]>,
}
```

### 2.4 QC Structure

```rust
/// Quorum Certificate (aggregated votes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QC {
    pub height: u64,
    pub round: u64,
    pub block_hash: [u8; 32],
    pub votes: Vec<Vote>,
}
```

### 2.5 Timeout Structure

```rust
/// Timeout message for view-change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeout {
    pub height: u64,
    pub round: u64,
    pub voter: Address,
    pub highest_qc: Option<QC>,
    pub signature: [u8; 64],
}
```

### 2.6 Block Sync Structures

```rust
/// Block request for peer-to-peer sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRequest {
    pub requester: Address,
    pub start_height: u64,
    pub end_height: u64,
}

/// Block response for peer-to-peer sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockResponse {
    pub responder: Address,
    pub request_start: u64,
    pub request_end: u64,
    pub blocks: Vec<Block>,
}
```

### 2.7 Hash Function

```rust
/// Compute the hash of a block using canonical encoding.
///
/// # Panics
/// Panics if block encoding fails (should never happen for valid blocks).
#[must_use]
pub fn block_hash(block: &Block) -> [u8; 32];
```

---

## 3. State Interface

**File**: `crates/state/src/lib.rs`
**Purpose**: State storage abstraction and key schema

### 3.1 Key Prefixes (Consensus-Critical)

```rust
/// Canonical prefix for account records.
pub const KEY_PREFIX_ACCOUNTS: &[u8] = b"accounts/";

/// Canonical key for the fee pool balance record.
pub const KEY_FEE_POOL: &[u8] = b"fee_pool";

/// Canonical key for the current SMT root.
pub const KEY_SMT_ROOT: &[u8] = b"smt/root";

/// Canonical prefix for SMT node records.
pub const KEY_PREFIX_SMT_NODE: &[u8] = b"smt/node/";

/// Canonical key for committed height.
pub const KEY_COMMITTED_HEIGHT: &[u8] = b"consensus/committed_height";

/// Canonical prefix for block records by height.
pub const KEY_PREFIX_BLOCKS: &[u8] = b"consensus/blocks/";

/// Canonical prefix for QC records by height.
pub const KEY_PREFIX_QCS: &[u8] = b"consensus/qcs/";

/// Canonical key for the highest QC seen.
pub const KEY_HIGHEST_QC: &[u8] = b"consensus/highest_qc";

/// Canonical prefix for AI entity records.
pub const KEY_PREFIX_AI_ENTITIES: &[u8] = b"ai/entities/";

/// Canonical prefix for AI memory records.
pub const KEY_PREFIX_AI_MEMORY: &[u8] = b"ai/memory/";

/// Canonical prefix for AI parameter records.
pub const KEY_PREFIX_AI_PARAMS: &[u8] = b"ai/params/";

/// Canonical prefix for AI signal records.
pub const KEY_PREFIX_AI_SIGNALS: &[u8] = b"ai/signals/";
```

### 3.2 Storage Traits

```rust
/// Minimal KV interface for state storage.
pub trait Kv {
    type Error;
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error>;
    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Self::Error>;
    fn delete(&mut self, key: &[u8]) -> Result<(), Self::Error>;
}

/// Extended KV trait with atomic batch support.
///
/// Implementations MUST guarantee all-or-nothing semantics.
pub trait KvBatch: Kv {
    /// Apply multiple operations atomically (all-or-nothing).
    fn apply_batch(&mut self, ops: &[WriteOp]) -> Result<(), Self::Error>;
}

/// Write operation for atomic batching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOp {
    Put(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
}
```

### 3.3 State Records

```rust
/// State encoding version.
pub const STATE_CODEC_V1: u8 = 1;

/// SMT root encoding version.
pub const SMT_ROOT_CODEC_V1: u8 = 1;

/// Account state record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStateV1 {
    pub balance: u128,
    pub nonce: u64,
}

/// Fee pool state record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeePoolV1 {
    pub balance: u128,
}
```

### 3.4 Key Builder Functions

```rust
/// Build canonical key for account: `b"accounts/" ++ addr32`.
pub fn account_key(addr: &[u8; 32]) -> Vec<u8>;

/// Build canonical SMT node key: `b"smt/node/" ++ node_hash32`.
pub fn smt_node_key(node_hash: &[u8; 32]) -> Vec<u8>;

/// Build canonical block key: `b"consensus/blocks/" ++ height_be8`.
pub fn block_key(height: u64) -> Vec<u8>;

/// Build canonical QC key: `b"consensus/qcs/" ++ height_be8`.
pub fn qc_key(height: u64) -> Vec<u8>;

/// Build canonical AI entity key: `b"ai/entities/" ++ entity_id32`.
pub fn ai_entity_key(entity_id: &[u8; 32]) -> Vec<u8>;

/// Build canonical AI memory key: `b"ai/memory/" ++ entity_id32 ++ "/" ++ slot`.
pub fn ai_memory_key(entity_id: &[u8; 32], slot: &[u8]) -> Vec<u8>;

/// Build canonical AI params key: `b"ai/params/" ++ entity_id32 ++ "/" ++ param_name`.
pub fn ai_params_key(entity_id: &[u8; 32], param_name: &[u8]) -> Vec<u8>;

/// Build canonical AI signal key: `b"ai/signals/" ++ height_be8 ++ "/" ++ issuer32`.
pub fn ai_signal_key(height: u64, issuer: &[u8; 32]) -> Vec<u8>;

/// Map variable-length state keys to 32-byte SMT keys.
///
/// Rule: `smt_key = blake3(state_key_bytes)`
///
/// CONSENSUS-CRITICAL: Do not change without network upgrade.
pub fn smt_key_for_state_key(key: &[u8]) -> [u8; 32];
```

### 3.5 Encoding Functions

```rust
/// Encode AccountStateV1: [version:1][balance_be:16][nonce_be:8] = 25 bytes
pub fn encode_account_v1(a: &AccountStateV1) -> [u8; 1 + 16 + 8];

/// Decode AccountStateV1 from canonical bytes.
pub fn decode_account_v1(bytes: &[u8]) -> Result<AccountStateV1, StateDecodeError>;

/// Encode FeePoolV1: [version:1][balance_be:16] = 17 bytes
pub fn encode_fee_pool_v1(p: &FeePoolV1) -> [u8; 1 + 16];

/// Decode FeePoolV1 from canonical bytes.
pub fn decode_fee_pool_v1(bytes: &[u8]) -> Result<FeePoolV1, StateDecodeError>;

/// Encode SMT root: [version:1][root32] = 33 bytes
pub fn encode_smt_root_v1(root: &[u8; 32]) -> [u8; 1 + 32];

/// Decode SMT root from canonical bytes.
pub fn decode_smt_root_v1(bytes: &[u8]) -> Result<[u8; 32], StateDecodeError>;
```

---

## 4. Execution Interface

**File**: `crates/execution/src/lib.rs`
**Purpose**: Deterministic state transition logic

### 4.1 Version Constants

```rust
/// Execution version.
pub const EXECUTION_VERSION: u8 = 1;

/// Transfer payload version.
pub const TRANSFER_PAYLOAD_V1: u8 = 1;

/// AI entity encoding version.
pub const AI_ENTITY_CODEC_V1: u8 = 1;
```

### 4.2 Transfer Payload

```rust
/// Transfer payload structure: [version:1][to:32][amount_be:8] = 41 bytes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPayloadV1 {
    pub to: Address,
    pub amount: u64,
}
```

### 4.3 Error Type

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError<E> {
    Db(E),
    Decode(StateDecodeError),
    BadPayloadLength { expected: usize, got: usize },
    BadPayloadVersion { expected: u8, got: u8 },
    NonceMismatch { expected: Nonce, got: Nonce },
    InsufficientFunds { balance: u128, needed: u128 },
    Overflow,
    NonceOverflow,
}
```

### 4.4 Transfer Functions

```rust
/// Decode transfer payload from tx.payload.
///
/// # Errors
/// Returns error if payload length or version is invalid.
pub fn decode_transfer_payload_v1(payload: &[u8]) -> Result<TransferPayloadV1, ExecError<()>>;

/// Encode transfer payload to canonical bytes.
#[must_use]
pub fn encode_transfer_payload_v1(p: &TransferPayloadV1) -> [u8; 1 + 32 + 8];

/// Apply a single TxV1 as TransferPayloadV1 against account state machine.
///
/// Rules:
/// - Nonce exact match
/// - Sender balance >= amount + fee
/// - Checked arithmetic only
/// - Debit sender by (amount + fee), credit receiver by amount
/// - Increment sender nonce by 1
/// - Add fee to fee_pool
///
/// ATOMIC: All state changes applied in single batch (all-or-nothing).
///
/// # Errors
/// Returns error if nonce mismatch, insufficient funds, payload decode fails, or DB error.
pub fn apply_tx_v1_transfer<K: KvBatch>(db: &mut K, tx: &TxV1) -> Result<(), ExecError<K::Error>>;
```

### 4.5 AI Entity Functions

```rust
/// Encode AI entity: 171 bytes total
/// [version:1][code_hash:32][creator:32][autonomy_mode:1][capabilities:1]
/// [economic_balance_be:16][nonce_be:8][memory_root:32][params_root:32]
/// [registered_at_be:8][last_active_at_be:8]
#[must_use]
pub fn encode_ai_entity_v1(entity: &AiEntity) -> [u8; 171];

/// Decode AI entity from canonical bytes.
///
/// # Errors
/// Returns error if payload length or version is invalid.
pub fn decode_ai_entity_v1(bytes: &[u8]) -> Result<AiEntity, ExecError<()>>;

/// Read AI entity from storage.
///
/// # Errors
/// Returns error if DB read fails or stored bytes are malformed.
pub fn read_ai_entity<K: Kv>(
    db: &K,
    entity_id: &[u8; 32],
) -> Result<Option<AiEntity>, ExecError<K::Error>>;

/// Write AI entity to storage (returns WriteOp for batching).
#[must_use]
pub fn write_ai_entity_op(entity: &AiEntity) -> WriteOp;

/// Read AI memory slot value.
///
/// # Errors
/// Returns error if DB read fails.
pub fn read_ai_memory<K: Kv>(
    db: &K,
    entity_id: &[u8; 32],
    slot: &[u8],
) -> Result<Option<Vec<u8>>, ExecError<K::Error>>;

/// Create WriteOp to write AI memory slot.
#[must_use]
pub fn write_ai_memory_op(entity_id: &[u8; 32], slot: &[u8], value: Vec<u8>) -> WriteOp;

/// Create WriteOp to delete AI memory slot.
#[must_use]
pub fn delete_ai_memory_op(entity_id: &[u8; 32], slot: &[u8]) -> WriteOp;
```

---

## 5. Consensus Interface

**File**: `crates/consensus/src/lib.rs`
**Purpose**: Consensus state machine and commit pipeline

### 5.1 Timeout Constants

```rust
/// Base timeout duration in milliseconds (round 0).
pub const BASE_TIMEOUT_MS: u64 = 2000;

/// Timeout multiplier for exponential backoff.
pub const TIMEOUT_MULTIPLIER: u64 = 2;

/// Maximum timeout duration in milliseconds.
pub const MAX_TIMEOUT_MS: u64 = 60_000;

/// Calculate timeout duration for given round.
///
/// Uses exponential backoff: `min(BASE_TIMEOUT_MS * 2^round, MAX_TIMEOUT_MS)`
#[must_use]
pub fn timeout_for_round(round: u64) -> u64;
```

### 5.2 AI Commit Hook Trait

```rust
/// Trait for processing AI state updates during block commit.
pub trait AiCommitHook {
    /// Called when blocks are committed.
    /// Returns AI-related WriteOps for atomic persistence.
    fn on_commit(&self, blocks: &[Block]) -> Vec<novai_state::WriteOp>;
}

/// No-op implementation (current phase, no AI processing).
pub struct NoopAiHook;
```

### 5.3 Error Type

```rust
#[derive(Debug)]
pub enum ConsensusError {
    InvalidBlock(String),
    InvalidVote(String),
    QcFormationFailed(String),
    StateError(String),
    CodecError(String),
    CryptoError(String),
    NotLeader,
}
```

### 5.4 Consensus State Structure

```rust
/// Consensus state for a single node.
pub struct ConsensusState {
    pub height: u64,
    pub round: u64,
    pub highest_qc: Option<QC>,
    pub pending_votes: HashMap<[u8; 32], Vec<Vote>>,
    pub our_address: Address,
    pub last_proposed: Option<(u64, u64)>,
    pub voted_in_round: HashSet<Address>,
    pub committed_height: u64,
    pub block_cache: HashMap<u64, Block>,
    pub qc_cache: HashMap<u64, QC>,
    pub block_by_hash: HashMap<[u8; 32], Block>,
    pub pending_timeouts: HashMap<(u64, u64), Vec<Timeout>>,
    pub timed_out_in_round: HashSet<Address>,
    pub view_changes_total: u64,
}
```

### 5.5 Core Methods (Summary)

```rust
impl ConsensusState {
    pub fn new(our_address: Address) -> Self;

    // Block proposal
    pub fn propose_block<K>(...) -> Result<Block, ConsensusError>
    where K: novai_state::Kv, K::Error: std::fmt::Debug;

    pub fn verify_block<K>(&self, block: &Block, state_db: &K) -> Result<(), ConsensusError>
    where K: novai_state::Kv, K::Error: std::fmt::Debug;

    // Voting
    pub fn create_vote(&self, block: &Block, signing_key: &SigningKey) -> Result<Vote, ConsensusError>;
    pub fn add_vote(&mut self, vote: Vote, validator_pubkeys: &[(Address, VerifyingKey)]) -> Result<(), ConsensusError>;
    pub fn try_form_qc(&mut self, block_hash: &[u8; 32], validator_set: &[Address]) -> Result<Option<QC>, ConsensusError>;

    // Leader selection
    pub fn compute_leader_for_view(view_height: u64, round: u64, validator_set: &[Address]) -> Result<Address, ConsensusError>;

    // Timeouts and view change
    pub fn create_timeout(&self, signing_key: &SigningKey) -> Result<Timeout, ConsensusError>;
    pub fn add_timeout(&mut self, timeout: Timeout, validator_pubkeys: &[(Address, VerifyingKey)]) -> Result<(), ConsensusError>;
    pub fn try_advance_round(&mut self, validator_set: &[Address]) -> bool;

    // Commit pipeline
    pub fn cache_block(&mut self, block: Block);
    pub fn cache_qc_and_check_commit(&mut self, qc: QC) -> Result<Vec<Block>, ConsensusError>;
    pub fn apply_commits(&mut self, blocks: &[Block]);
    pub fn apply_commits_with_ai_hook(&mut self, blocks: &[Block], ai_hook: &dyn AiCommitHook) -> Vec<novai_state::WriteOp>;
    pub fn check_no_fork(&self, block: &Block);

    // Persistence
    pub fn persist_commit_atomic<K>(&self, db: &mut K, blocks: &[Block], qc: &QC, new_committed_height: u64, ai_ops: Option<&[novai_state::WriteOp]>) -> Result<(), ConsensusError>
    where K: novai_state::KvBatch, K::Error: std::fmt::Debug;

    // Recovery
    pub fn recover<K>(our_address: Address, db: &K) -> Result<Self, ConsensusError>
    where K: novai_state::Kv, K::Error: std::fmt::Debug;

    pub fn recover_with_cache<K>(our_address: Address, db: &K, cache_depth: u64) -> Result<Self, ConsensusError>
    where K: novai_state::Kv, K::Error: std::fmt::Debug;

    pub fn catch_up_to<K>(&mut self, db: &K, target_height: u64) -> Result<usize, ConsensusError>
    where K: novai_state::Kv, K::Error: std::fmt::Debug;

    // Utilities
    pub fn load_committed_height<K>(db: &K) -> Result<u64, ConsensusError>
    where K: novai_state::Kv, K::Error: std::fmt::Debug;

    pub fn load_block<K>(db: &K, height: u64) -> Result<Option<Block>, ConsensusError>
    where K: novai_state::Kv, K::Error: std::fmt::Debug;

    pub fn verify_block_chain(blocks: &[Block], expected_first_parent: [u8; 32]) -> Result<(), ConsensusError>;
}
```

---

## 6. Wire Format Encodings

**Files**: `crates/consensus_types/src/codec.rs`, `crates/codec/src/lib.rs`

### 6.1 Encoding Version Constants

```rust
// From consensus_types/src/codec.rs
pub const BLOCK_V1: u8 = 1;
pub const PROPOSAL_V1: u8 = 1;
pub const VOTE_V1: u8 = 1;
pub const QC_V1: u8 = 1;
pub const TIMEOUT_V1: u8 = 1;
pub const BLOCK_REQUEST_V1: u8 = 1;
pub const BLOCK_RESPONSE_V1: u8 = 1;
```

### 6.2 Canonical Encoding Rules

All wire formats follow these rules:

1. **Big-endian** byte order for multi-byte integers
2. **Version prefix** (1 byte) for forward compatibility
3. **Deterministic** field ordering (no HashMap iteration)
4. **Fixed-size arrays** where possible for constant-time parsing
5. **Domain-separated hashing** for signatures (e.g., `b"NOVAI_VOTE_V1"`)

### 6.3 Example Encoding Lengths

| Type | Encoding Length | Notes |
|------|----------------|-------|
| `AccountStateV1` | 25 bytes | `[v:1][balance:16][nonce:8]` |
| `FeePoolV1` | 17 bytes | `[v:1][balance:16]` |
| `SMT Root` | 33 bytes | `[v:1][root:32]` |
| `TransferPayloadV1` | 41 bytes | `[v:1][to:32][amount:8]` |
| `AiEntity` | 171 bytes | (see Execution Interface) |
| `Vote` | Variable | Includes optional AI signal |
| `Block` | Variable | Includes tx list |

---

## 7. Golden Vectors

The following golden vector files are **LOCKED** and MUST NOT change:

### 7.1 Codec Golden Vectors

**Location**: `crates/codec/tests/vectors/*.bin`

- `tx_v1_unsigned.bin` - Unsigned transaction encoding
- `tx_v1_signed.bin` - Signed transaction encoding
- `header_v1.bin` - Block header encoding
- `ai_signal_v1.bin` - AI signal encoding
- `commitment_v1.bin` - Commitment encoding

### 7.2 Consensus Types Golden Vectors

**Location**: `crates/consensus_types/tests/vectors/*.bin`

- `vote_unsigned.bin` - Unsigned vote encoding
- `vote_signed.bin` - Signed vote encoding
- `vote_with_signal.bin` - Vote with AI signal commitment
- `qc_empty.bin` - QC with no votes (genesis)
- `qc_with_votes.bin` - QC with votes
- `block_empty.bin` - Block with no transactions
- `block_with_tx.bin` - Block with transactions
- `proposal_v1.bin` - Proposal encoding
- `proposal_signed.bin` - Signed proposal encoding
- `timeout_no_qc.bin` - Timeout without QC
- `timeout_with_qc.bin` - Timeout with QC

### 7.3 AI Entities Golden Vectors

**Location**: `crates/ai_entities/tests/vectors/*.bin`

- `ai_entity_v1.bin` - AI entity encoding

### 7.4 State Golden Roots

**Location**: Test code in `crates/state/tests/`

- `test_golden_ai_inclusive_root` - SMT root with AI entities included

---

## 8. Change Policy

### 8.1 Prohibited Changes

The following changes are **PROHIBITED** without major version bump:

❌ Changing field order in any struct
❌ Changing encoding format (byte layout)
❌ Changing golden vectors
❌ Changing key prefixes (e.g., `b"accounts/"`)
❌ Changing hash function (currently `blake3`)
❌ Changing signature domain tags (e.g., `b"NOVAI_VOTE_V1"`)
❌ Removing public functions
❌ Changing function signatures (parameters, return types)
❌ Changing version constants without backward compatibility
❌ Changing `smt_key_for_state_key()` mapping rule

### 8.2 Allowed Changes

The following changes are **ALLOWED**:

✅ Bug fixes that preserve interface behavior
✅ Performance optimizations (internal only)
✅ Documentation updates
✅ New functions (additive only)
✅ New version variants (V2) with V1 backward compatibility
✅ Internal refactoring (no public API changes)

### 8.3 Breaking Change Procedure

If a breaking change is absolutely necessary:

1. **Create V2 version** with new version constant
2. **Maintain V1 support** for backward compatibility period
3. **Write migration guide** in `docs/`
4. **Update golden vectors** with both V1 and V2 examples
5. **Announce network upgrade** with timeline
6. **Bump major version** (e.g., `0.1.0` → `1.0.0`)
7. **Update this document** with frozen V2 interfaces

---

## 9. Verification Commands

Run these commands to verify interfaces haven't changed:

```bash
# Verify all tests pass (including golden vectors)
cargo test --workspace

# Verify golden vector tests specifically
cargo test --workspace golden

# Verify no regressions
cargo test --workspace

# Verify clippy clean
cargo clippy --workspace

# Verify license compliance
cargo deny check licenses

# Verify deterministic builds (optional, requires nightly)
# cargo build --release
# sha256sum target/release/novai_node
```

### Expected Results

All commands must produce:
- ✅ Tests: All pass (330+)
- ✅ Clippy: Zero warnings
- ✅ Licenses: OK (no GPL/AGPL)

---

## 10. Enforcement

### 10.1 CI Checks

The following CI checks enforce this freeze:

1. **Golden vector tests** - Detect encoding changes
2. **Clippy lints** - Detect API changes
3. **Test suite** - Detect behavior changes
4. **License check** - Detect dependency changes

### 10.2 Code Review

All PRs must be reviewed for:

- Interface stability (no breaking changes)
- Golden vector preservation
- Documentation updates
- Backward compatibility

### 10.3 Git Tags

This freeze is tagged as `testnet-v0.1`. Any breaking change requires a new tag.

---

## 11. Contact

For questions about interface changes or freeze exceptions:

- Open an issue with label `interface-freeze`
- Provide detailed justification for proposed change
- Include migration plan and backward compatibility strategy

---

## Signatures

**Freeze Date**: January 19, 2026
**Frozen By**: Claude Sonnet 4.5 (NOVAI Protocol Engineer)
**Git Tag**: `testnet-v0.1`
**Review Status**: Week 12 Review Gate Complete
**Next Review**: Before Week 20 (if needed)

---

## Appendix: Complete Interface File Listing

```
crates/types/src/lib.rs                          (103 lines)
crates/consensus_types/src/lib.rs                (100 lines)
crates/consensus_types/src/codec.rs              (800+ lines)
crates/state/src/lib.rs                          (437 lines)
crates/execution/src/lib.rs                      (701 lines)
crates/consensus/src/lib.rs                      (2039 lines)
```

**Total Interface Code**: ~4200 lines frozen
