# NOVAI Parameter Lock Policy

**Effective**: 2026-02-02 (Week 28)
**Status**: ACTIVE
**Reference**: `docs/MAINNET_SPEC.md` for all parameter values with source file references.

---

## 1. Lock Statement

As of this date, the protocol components listed in Section 2 are **FROZEN**. No changes are permitted except through the governance process defined in `crates/governance/src/lib.rs`. Unauthorized changes to locked components constitute a protocol violation and must be reverted.

---

## 2. Locked Components (NO changes without governance)

### 2.1 Consensus Validity Rules

- BFT fault tolerance: `f < n/3` where `n = 3f + 1`
- Quorum threshold: `2f + 1` (`crates/consensus_types/src/leader.rs:115-118`)
- 3-chain commit rule (`crates/consensus/src/lib.rs`)
- Leader selection: `validators[(height + round) % n]` (`crates/consensus_types/src/leader.rs:102-108`)
- Validator ordering: sorted by address, lexicographic ascending (`crates/consensus_types/src/leader.rs:54`)
- Validator set constraint: `n = 3f + 1`, minimum `n = 4` (`crates/consensus_types/src/leader.rs:43-51`)

**Affected files**: `crates/consensus/src/lib.rs`, `crates/consensus_types/src/`

### 2.2 State Transition Logic

- Execution dispatch: payload version bytes 1-7 (`crates/execution/src/lib.rs:37,185,259,262,265,272,275`)
- Transfer logic: balance checks, fee deduction, overflow protection via checked arithmetic
- Nonce enforcement: sequential, no gaps (`crates/execution/src/lib.rs` — `NonceMismatch` error)
- Fee pool accounting: all fees collected atomically (`crates/state/src/lib.rs:20` — `KEY_FEE_POOL`)
- SMT root recomputation on every state write

**Affected files**: `crates/execution/src/lib.rs`, `crates/state/src/lib.rs`

### 2.3 Message Wire Formats

- All version bytes: `0x01` for all V1 messages (`crates/consensus_types/src/codec.rs:30-54`)
- Field ordering in all encode/decode functions (consensus-relevant, changing is a hard fork)
- Little-endian byte order for wire codec (`crates/codec/src/lib.rs:69,73`)
- Big-endian byte order for state keys (`crates/state/src/lib.rs:171,179`)
- `TX_V1_OVERHEAD = 149` bytes (`crates/codec/src/lib.rs:221`)
- P2P wire framing: `[len:4 BE][version:1][kind:1][payload]` (`crates/p2p/src/lib.rs:1-5`)
- MessageKind values 1-6 (`crates/p2p/src/lib.rs:24-31`)
- All domain separation tags (see Section 2.4)

**Affected files**: `crates/codec/src/lib.rs`, `crates/consensus_types/src/codec.rs`, `crates/p2p/src/lib.rs`

### 2.4 Domain Separation Tags

Every `b"NOVAI_*"` tag is frozen. Complete list:

| Tag | Source |
|-----|--------|
| `b"NOVAI_VOTE_V1"` | `crates/consensus/src/lib.rs:386` |
| `b"NOVAI_TIMEOUT_V1"` | `crates/consensus/src/lib.rs:591` |
| `b"NOVAI_PROPOSAL_V1"` | Consensus signing (per spec) |
| `b"NOVAI_AI_ENTITY_ID_V1"` | `crates/ai_entities/src/lib.rs:48` |
| `b"NOVAI_MODULE_MANIFEST_V1"` | `crates/ai_entities/src/lib.rs:51` |
| `b"NOVAI_SIGNAL_COMMIT_V1"` | `crates/ai_entities/src/signals.rs:8` |
| `b"NOVAI_APPROVAL_GATE_ID_V1"` | `crates/ai_entities/src/gates.rs:22` |
| `b"NOVAI_MEMORY_OBJECT_ID_V1"` | `crates/ai_entities/src/memory.rs:20` |
| `b"NOVAI_DERIVED_VIEW_ID_V1"` | `crates/ai_entities/src/derived_views.rs:22` |
| `b"NOVAI_ARTIFACT_V1"` | `crates/ai_entities/src/artifacts.rs:24` |
| `b"NOVAI_PROPOSAL_ID_V1"` | `crates/governance/src/lib.rs:270` |
| `b"NOVAI_NNPX_COMMITMENT_V1"` | `crates/ai_entities/src/privacy.rs:25` |
| `b"NOVAI_NNPX_NULLIFIER_V1"` | `crates/ai_entities/src/privacy.rs:28` |
| `b"NOVAI_NNPX_KEY_DERIVE_V1"` | `crates/ai_entities/src/privacy.rs:31` |
| `b"NOVAI_NNPX_ZK_PROOF_V1"` | `crates/ai_entities/src/privacy.rs:34` |

### 2.5 Cryptographic Primitives

- Hashing: blake3 (all contexts)
- Signatures: Ed25519 via `ed25519-dalek` (`crates/crypto/src/lib.rs`)
- Address derivation: `blake3(pubkey)` — no domain tag (`crates/crypto/src/lib.rs:27-31`)
- Transport encryption: Noise_XX_25519_ChaChaPoly_SHA256 (`crates/p2p/src/noise.rs`)
- SMT domain tags: `TAG_EMPTY=0x00`, `TAG_LEAF=0x01`, `TAG_INTERNAL=0x02` (`crates/smt/src/hash.rs:12-14`)

**Affected files**: `crates/crypto/src/lib.rs`, `crates/p2p/src/noise.rs`, `crates/smt/src/hash.rs`

### 2.6 State Key Schema

All key prefixes defined in `crates/state/src/lib.rs:17-147` are frozen. See `docs/MAINNET_SPEC.md` Section 10 for the complete list.

- RocksDB column families: `default`, `nnpx` (`crates/state/src/lib.rs:99-102`)

**Affected files**: `crates/state/src/lib.rs`

---

## 3. Governance-Adjustable Parameters

These parameters MAY be changed via governance proposals:

| Parameter | Current Value | Source | Proposal Type | Timelock |
|-----------|--------------|--------|---------------|----------|
| `MAX_TX_SIZE` | 128 KB | `crates/types/src/lib.rs:19` | ParamChange | 1,000 blocks |
| `MAX_BLOCK_SIZE` | 2 MB | `crates/types/src/lib.rs:22` | ParamChange | 1,000 blocks |
| `MAX_TXS_PER_BLOCK` | 500 | `crates/types/src/lib.rs:25` | ParamChange | 1,000 blocks |
| `MAX_MEMPOOL_BYTES` | 64 MB | `crates/types/src/lib.rs:28` | ParamChange | 1,000 blocks |
| `BASE_TIMEOUT_MS` | 1,000 | `crates/consensus/src/lib.rs:20` | ParamChange | 1,000 blocks |
| `TIMEOUT_MULTIPLIER` | 2 | `crates/consensus/src/lib.rs:24` | ParamChange | 1,000 blocks |
| `MAX_TIMEOUT_MS` | 60,000 | `crates/consensus/src/lib.rs:28` | ParamChange | 1,000 blocks |
| `default_timelock_blocks` | 1,000 | `crates/governance/src/lib.rs:98` | PolicyChange | 5,000 blocks |
| `high_risk_timelock_blocks` | 5,000 | `crates/governance/src/lib.rs:99` | PolicyChange | 5,000 blocks |
| `emergency_timelock_blocks` | 100 | `crates/governance/src/lib.rs:100` | PolicyChange | 5,000 blocks |
| `default_expiry_blocks` | 50,000 | `crates/governance/src/lib.rs:101` | ParamChange | 1,000 blocks |
| `MAX_MEMORY_OBJECTS_PER_ENTITY` | 100 | `crates/ai_entities/src/memory.rs:29` | ParamChange | 1,000 blocks |
| `MAX_MEMORY_OBJECT_SIZE` | 64 KB | `crates/ai_entities/src/memory.rs:26` | ParamChange | 1,000 blocks |
| `MAX_DERIVED_VIEW_SIZE` | 16 KB | `crates/ai_entities/src/derived_views.rs:29` | ParamChange | 1,000 blocks |
| `MAX_ARTIFACT_SIZE` | 50 MB | `crates/ai_entities/src/artifacts.rs:27` | ParamChange | 1,000 blocks |
| `MAX_APPROVERS` | 256 | `crates/ai_entities/src/gates.rs:25` | ParamChange | 1,000 blocks |
| AI action tier mappings | see `tiers.rs` | `crates/ai_entities/src/tiers.rs:237-253` | PolicyChange | 5,000 blocks |
| `CACHE_RETAIN_DEPTH` | 10 | `crates/consensus/src/lib.rs:34` | ParamChange | 1,000 blocks |

---

## 4. Change Process

1. **Submit**: Send transaction with `SUBMIT_PROPOSAL_PAYLOAD_V1` (payload version byte `6`, defined at `crates/execution/src/lib.rs:272`). Proposal must reference a valid approval gate.
2. **Approve**: Collect required approvals per gate type (Multisig, Threshold, or TimelockOnly). Gate validation rules in `crates/ai_entities/src/gates.rs:415-468`.
3. **Wait**: Timelock period enforced based on proposal type:
   - Standard (`ParamChange`, `ModuleRollback`): 1,000 blocks
   - High-risk (`ModuleActivation`, `PolicyChange`): 5,000 blocks
   - Emergency (`EmergencyFreeze`): 100 blocks
4. **Execute**: Send transaction with `EXECUTE_PROPOSAL_PAYLOAD_V1` (payload version byte `7`, defined at `crates/execution/src/lib.rs:275`). Execution fails if timelock has not elapsed or proposal has expired.
5. **Audit**: Audit log entry created automatically for every state transition (`crates/governance/src/lib.rs:168-187` — `AuditLogEntry`). Actions logged: Submitted, Approved, Executed, Rejected, Expired.

---

## 5. Violation Policy

Any code change that modifies a locked component (Section 2) without a corresponding on-chain governance proposal is a **protocol violation**. Such changes:

- Must be reverted immediately
- Constitute a hard fork if deployed to validators
- Invalidate the parameter freeze declared in this document

This policy is enforced by golden vector tests (which fail if encoding changes) and the governance audit log (which records all parameter modifications on-chain).
