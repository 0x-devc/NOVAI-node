# NOVAI Mainnet Parameter Specification

**Status**: FROZEN
**Version**: 1.0.0
**Date**: 2026-02-02
**Authority**: Code as source of truth. All values extracted from source with file:line references.

> **Rule**: If this document conflicts with code, the code wins.
> See `docs/ARCHITECTURE_DECISIONS.md` section "Code-vs-Spec Precedence".

---

## 1. Protocol Size Limits

All constants defined in `crates/types/src/lib.rs`.

| Parameter | Value | Source |
|-----------|-------|--------|
| `MAX_TX_SIZE` | 131,072 bytes (128 KB) | `crates/types/src/lib.rs:19` |
| `MAX_BLOCK_SIZE` | 2,097,152 bytes (2 MB) | `crates/types/src/lib.rs:22` |
| `MAX_TXS_PER_BLOCK` | 500 | `crates/types/src/lib.rs:25` |
| `MAX_MEMPOOL_BYTES` | 67,108,864 bytes (64 MB) | `crates/types/src/lib.rs:28` |

**Known discrepancy**: `crates/consensus_types/src/codec.rs:57` defines `MAX_TXS_PER_BLOCK = 10,000` for codec-level DoS prevention (decode bounds). The consensus-enforced limit is 500 from `crates/types/src/lib.rs:25`. Both are correct at their respective layers.

---

## 2. Transaction Wire Format (TxV1)

Defined in `crates/codec/src/lib.rs`.

### Unsigned encoding (lines 94-104)

```
[version:1][from:32][pubkey:32][nonce:8][fee:8][payload_len:4][payload:var]
```

- All multi-byte integers are **little-endian**.
- `version` is `TxVersion::V1 = 1` (`crates/types/src/lib.rs:36`).

### Signed encoding (lines 107-111)

```
[unsigned_bytes][sig:64]
```

### Size computation

| Constant | Value | Source |
|----------|-------|--------|
| `TX_V1_OVERHEAD` | 149 bytes | `crates/codec/src/lib.rs:221` |
| `tx_encoded_size(tx)` | `149 + payload.len()` | `crates/codec/src/lib.rs:229-231` |

Breakdown: 1(version) + 32(from) + 32(pubkey) + 8(nonce) + 8(fee) + 4(payload_len) + 64(sig) = 149.

### TxId computation

```
txid = blake3(encode_tx_v1_unsigned(tx))
```

Source: `crates/codec/src/lib.rs:211-217`. No domain separation tag.

---

## 3. Transaction Signing

Source: `crates/crypto/src/lib.rs:51-55`.

```
sig = ed25519_sign(signing_key, encode_tx_v1_unsigned(tx))
```

- **No domain separation tag** for transaction signatures.
- Signature is over raw unsigned bytes.
- Verification: `crates/crypto/src/lib.rs:58-61`.
- Address derivation: `address = blake3(pubkey_bytes)` (`crates/crypto/src/lib.rs:27-31`). No domain tag.

---

## 4. Block Header Wire Format (BlockHeaderV1)

Defined in `crates/codec/src/lib.rs:172-182`.

```
[version:1][height:8][prev_hash:32][state_root:32][tx_root:32][proposer:32][qc_hash:32]
```

- Total: 169 bytes (fixed).
- All multi-byte integers are **little-endian**.
- `version` is `BlockHeaderVersion::V1 = 1` (`crates/types/src/lib.rs:51`).

---

## 5. Consensus Block Wire Format

Defined in `crates/consensus_types/src/codec.rs:73-80`.

```
[version:1][height:8][round:8][parent_hash:32][state_root:32][tx_count:4][txs_bytes]
```

Version constants (all `0x01`):

| Constant | Value | Source |
|----------|-------|--------|
| `BLOCK_V1` | `0x01` | `crates/consensus_types/src/codec.rs:30` |
| `VOTE_UNSIGNED_V1` | `0x01` | `crates/consensus_types/src/codec.rs:33` |
| `VOTE_SIGNED_V1` | `0x01` | `crates/consensus_types/src/codec.rs:36` |
| `QC_V1` | `0x01` | `crates/consensus_types/src/codec.rs:39` |
| `PROPOSAL_V1` | `0x01` | `crates/consensus_types/src/codec.rs:42` |
| `TIMEOUT_UNSIGNED_V1` | `0x01` | `crates/consensus_types/src/codec.rs:45` |
| `TIMEOUT_SIGNED_V1` | `0x01` | `crates/consensus_types/src/codec.rs:48` |
| `BLOCK_REQUEST_V1` | `0x01` | `crates/consensus_types/src/codec.rs:51` |
| `BLOCK_RESPONSE_V1` | `0x01` | `crates/consensus_types/src/codec.rs:54` |

Codec-level DoS bounds:

| Constant | Value | Source |
|----------|-------|--------|
| `MAX_TXS_PER_BLOCK` (codec) | 10,000 | `crates/consensus_types/src/codec.rs:57` |
| `MAX_VOTES_PER_QC` | 11,000 | `crates/consensus_types/src/codec.rs:60` |
| `MIN_TX_BYTES` | 100 | `crates/consensus_types/src/codec.rs:63` |
| `MIN_VOTE_BYTES` | 146 | `crates/consensus_types/src/codec.rs:67` |

---

## 6. P2P Wire Protocol

Defined in `crates/p2p/src/lib.rs`.

### Framing

```
[len:4 BE][version:1][kind:1][payload:len-2 bytes]
```

| Parameter | Value | Source |
|-----------|-------|--------|
| `MAX_WIRE_MSG_BYTES` | 2,097,152 (2 MB) | `crates/p2p/src/lib.rs:19` |
| Wire version | `1` | `crates/p2p/src/lib.rs:117` |

### MessageKind values (`crates/p2p/src/lib.rs:24-31`)

| Kind | Value |
|------|-------|
| `SignedProposal` | `1` |
| `Vote` | `2` |
| `Qc` | `3` |
| `Timeout` | `4` |
| `BlockRequest` | `5` |
| `BlockResponse` | `6` |

### Transport encryption

Noise_XX_25519_ChaChaPoly_SHA256 (`crates/p2p/src/noise.rs`).

---

## 7. Consensus Timeout Configuration

All constants in `crates/consensus/src/lib.rs`.

| Parameter | Value | Source |
|-----------|-------|--------|
| `BASE_TIMEOUT_MS` | 1,000 ms | `crates/consensus/src/lib.rs:20` |
| `TIMEOUT_MULTIPLIER` | 2 | `crates/consensus/src/lib.rs:24` |
| `MAX_TIMEOUT_MS` | 60,000 ms | `crates/consensus/src/lib.rs:28` |
| `CACHE_RETAIN_DEPTH` | 10 blocks | `crates/consensus/src/lib.rs:34` |

Formula: `timeout(round) = min(BASE_TIMEOUT_MS * 2^round, MAX_TIMEOUT_MS)`.

Effective progression: 1s, 2s, 4s, 8s, 16s, 32s, 60s (capped).

**Known discrepancy**: `docs/TUNING_PARAMETERS.md` says `BASE_TIMEOUT_MS = 2000`. Code says 1000. Code wins.

---

## 8. Consensus Message Signing (Domain Tags)

Signatures use domain-separated blake3. Tags are inline string literals, not named constants.

| Message | Domain Tag | Source |
|---------|-----------|--------|
| Vote | `b"NOVAI_VOTE_V1"` | `crates/consensus/src/lib.rs:386` |
| Timeout | `b"NOVAI_TIMEOUT_V1"` | `crates/consensus/src/lib.rs:591` |
| Proposal | `b"NOVAI_PROPOSAL_V1"` | per `docs/CONSENSUS_V1.md:76` |
| Transaction | _(none)_ | `crates/crypto/src/lib.rs:51-55` |

Signing format: `ed25519_sign(key, domain_tag || encode_unsigned(msg))`.

---

## 9. Sparse Merkle Tree (SMT)

### Hash functions (`crates/smt/src/hash.rs`)

| Function | Formula | Source |
|----------|---------|--------|
| Empty hash | `blake3(0x00 \|\| height_u16_be)` | `crates/smt/src/hash.rs:12,28-33` |
| Leaf hash | `blake3(0x01 \|\| key32 \|\| blake3(value))` | `crates/smt/src/hash.rs:13,47-50` |
| Internal hash | `blake3(0x02 \|\| left32 \|\| right32)` | `crates/smt/src/hash.rs:14` |

Domain tags (single bytes, not strings):

| Tag | Value | Source |
|-----|-------|--------|
| `TAG_EMPTY` | `0x00` | `crates/smt/src/hash.rs:12` |
| `TAG_LEAF` | `0x01` | `crates/smt/src/hash.rs:13` |
| `TAG_INTERNAL` | `0x02` | `crates/smt/src/hash.rs:14` |

### Node encoding (`crates/smt/src/node.rs:32-33`)

| Parameter | Value |
|-----------|-------|
| `Node::ENCODED_LEN` | 67 bytes |
| Node tag byte | `0xA1` |

Format: `[0xA1][left_child:33][right_child:33]`.

Child encoding:
- Hash: `[0x01][hash:32]`
- Empty: `[0x00][height:u16 be][padding:29 zeros]`

Tree height: 256 (keys are 256-bit blake3 hashes).

---

## 10. State Key Schema

All prefixes defined in `crates/state/src/lib.rs`.

### Core state keys

| Key/Prefix | Value | Source |
|------------|-------|--------|
| `KEY_PREFIX_ACCOUNTS` | `b"accounts/"` | `crates/state/src/lib.rs:17` |
| `KEY_FEE_POOL` | `b"fee_pool"` | `crates/state/src/lib.rs:20` |
| `KEY_SMT_ROOT` | `b"smt/root"` | `crates/state/src/lib.rs:23` |
| `KEY_PREFIX_SMT_NODE` | `b"smt/node/"` | `crates/state/src/lib.rs:26` |

### Consensus persistence keys

| Key/Prefix | Value | Source |
|------------|-------|--------|
| `KEY_COMMITTED_HEIGHT` | `b"consensus/committed_height"` | `crates/state/src/lib.rs:29` |
| `KEY_PREFIX_BLOCKS` | `b"consensus/blocks/"` | `crates/state/src/lib.rs:32` |
| `KEY_PREFIX_QCS` | `b"consensus/qcs/"` | `crates/state/src/lib.rs:35` |
| `KEY_HIGHEST_QC` | `b"consensus/highest_qc"` | `crates/state/src/lib.rs:38` |

### AI storage keys

| Key/Prefix | Value | Source |
|------------|-------|--------|
| `KEY_PREFIX_AI_ENTITIES` | `b"ai/entities/"` | `crates/state/src/lib.rs:45` |
| `KEY_PREFIX_AI_MEMORY` | `b"ai/memory/"` | `crates/state/src/lib.rs:48` |
| `KEY_PREFIX_AI_PARAMS` | `b"ai/params/"` | `crates/state/src/lib.rs:51` |
| `KEY_PREFIX_AI_SIGNALS` | `b"ai/signals/"` | `crates/state/src/lib.rs:54` |
| `KEY_PREFIX_AI_SIGNALS_BY_TYPE` | `b"ai/signals/by_type/"` | `crates/state/src/lib.rs:57` |
| `KEY_PREFIX_AI_SIGNALS_BY_ISSUER` | `b"ai/signals/by_issuer/"` | `crates/state/src/lib.rs:60` |
| `KEY_PREFIX_AI_MEMORY_OBJECTS` | `b"ai/memory_objects/"` | `crates/state/src/lib.rs:68` |
| `KEY_PREFIX_AI_MEMORY_COUNT` | `b"ai/memory_count/"` | `crates/state/src/lib.rs:72` |
| `KEY_PREFIX_AI_MEMORY_BY_TYPE` | `b"ai/memory_by_type/"` | `crates/state/src/lib.rs:76` |

### NNPX privacy keys

| Key/Prefix | Value | Source |
|------------|-------|--------|
| `KEY_PREFIX_NNPX` | `b"nnpx/"` | `crates/state/src/lib.rs:84` |
| `KEY_PREFIX_NNPX_COMMITMENTS` | `b"nnpx/commitments/"` | `crates/state/src/lib.rs:88` |
| `KEY_PREFIX_NNPX_NULLIFIERS` | `b"nnpx/nullifiers/"` | `crates/state/src/lib.rs:92` |
| `KEY_PREFIX_NNPX_ENCRYPTED` | `b"nnpx/encrypted/"` | `crates/state/src/lib.rs:96` |
| `CF_NNPX` | `"nnpx"` | `crates/state/src/lib.rs:99` |
| `CF_DEFAULT` | `"default"` | `crates/state/src/lib.rs:102` |

### Derived views keys

| Key/Prefix | Value | Source |
|------------|-------|--------|
| `KEY_PREFIX_DERIVED_VIEWS` | `b"derived_views/"` | `crates/state/src/lib.rs:113` |
| `KEY_PREFIX_DERIVED_VIEWS_AUDIT` | `b"derived_views/audit/"` | `crates/state/src/lib.rs:119` |
| `KEY_PREFIX_DERIVED_VIEWS_BY_SCHEMA` | `b"derived_views/by_schema/"` | `crates/state/src/lib.rs:123` |
| `KEY_PREFIX_DERIVED_VIEWS_BY_CREATOR` | `b"derived_views/by_creator/"` | `crates/state/src/lib.rs:127` |

### Governance keys

| Key/Prefix | Value | Source |
|------------|-------|--------|
| `KEY_PREFIX_GOVERNANCE_PROPOSALS` | `b"governance/proposals/"` | `crates/state/src/lib.rs:135` |
| `KEY_PREFIX_GOVERNANCE_LOG` | `b"governance/log/"` | `crates/state/src/lib.rs:139` |
| `KEY_PREFIX_GOVERNANCE_PROPOSALS_BY_STATE` | `b"governance/proposals_by_state/"` | `crates/state/src/lib.rs:143` |
| `KEY_PREFIX_APPROVAL_GATES` | `b"ai/gates/"` | `crates/state/src/lib.rs:147` |

---

## 11. Execution Payload Types

Defined in `crates/execution/src/lib.rs`.

| Constant | Value | Source |
|----------|-------|--------|
| `EXECUTION_VERSION` | `1` | `crates/execution/src/lib.rs:34` |
| `TRANSFER_PAYLOAD_V1` | `1` | `crates/execution/src/lib.rs:37` |

Transfer payload format: `[version:1][to:32][amount_be:8]` = 41 bytes.

Payload version byte routing (from execution dispatch):

| Version Byte | Constant | Payload Type | Source |
|-------------|----------|-------------|--------|
| `1` | `TRANSFER_PAYLOAD_V1` | Transfer | `crates/execution/src/lib.rs:37` |
| `2` | `SIGNAL_COMMITMENT_PAYLOAD_V1` | AI Signal Commitment | `crates/execution/src/lib.rs:185` |
| `3` | `CREATE_MEMORY_OBJECT_PAYLOAD_V1` | Create Memory Object | `crates/execution/src/lib.rs:259` |
| `4` | `UPDATE_MEMORY_OBJECT_PAYLOAD_V1` | Update Memory Object | `crates/execution/src/lib.rs:262` |
| `5` | `DELETE_MEMORY_OBJECT_PAYLOAD_V1` | Delete Memory Object | `crates/execution/src/lib.rs:265` |
| `6` | `SUBMIT_PROPOSAL_PAYLOAD_V1` | Submit Governance Proposal | `crates/execution/src/lib.rs:272` |
| `7` | `EXECUTE_PROPOSAL_PAYLOAD_V1` | Execute Governance Proposal | `crates/execution/src/lib.rs:275` |

---

## 12. AI Entity System

### Entity types (`crates/ai_entities/src/lib.rs`)

Identity computation:
```
entity_id = blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator)
```
Source: `crates/ai_entities/src/lib.rs:48,238-243`.

### Autonomy modes (`crates/ai_entities/src/lib.rs:83-93`)

| Mode | Value | Description |
|------|-------|-------------|
| `Advisory` | `0` | Emit proposals only, never execute |
| `Gated` | `1` | Proposals go through approval gates |
| `Autonomous` | `2` | Reserved (requires ZK proofs) |

### Capability flags (`crates/ai_entities/src/lib.rs:114-148`)

| Bit | Capability | Description |
|-----|-----------|-------------|
| 0 | `read_public_chain` | Read blocks, txs, accounts |
| 1 | `read_memory_objects` | Read L1 memory objects |
| 2 | `emit_proposals` | Emit proposal objects |
| 3 | `request_execution` | Request Tier 1/2 action execution |
| 4 | `read_nnpx_derived` | Read NNPX derived views |
| 5-7 | `_reserved` | Reserved for future use |

### Well-known identifiers (`crates/ai_entities/src/lib.rs:65-80`)

| Identifier | Value | Source |
|-----------|-------|--------|
| `CORE_OBSERVER_CODE_HASH` | `blake3("NOVAI_CORE_OBSERVER_V1")` | `crates/ai_entities/src/lib.rs:65-68` |
| `PROTOCOL_CREATOR` | `blake3("NOVAI_PROTOCOL_GENESIS_V1")` | `crates/ai_entities/src/lib.rs:77-80` |

---

## 13. AI Signal System

### Signal types (`crates/ai_entities/src/signals.rs:13-21`)

| Type | Value |
|------|-------|
| `Anomaly` | `0` |
| `Optimization` | `1` |
| `Prediction` | `2` |
| `RiskScore` | `3` |
| `AuditReport` | `4` |
| `SpamRisk` | `5` |
| `CongestionForecast` | `6` |

### Signal commitment hash

```
commitment = blake3("NOVAI_SIGNAL_COMMIT_V1" || signal_type || height_le8 || issuer || confidence || payload_hash || proof_binding)
```

Source: `crates/ai_entities/src/signals.rs:8,76-99`.

### Action tiers (`crates/ai_entities/src/tiers.rs:33-58`)

| Tier | Value | AI Executable | Description |
|------|-------|---------------|-------------|
| `Tier0Never` | `0` | No | Consensus-critical, never AI-allowed |
| `Tier1High` | `1` | Yes (with gates) | Core protocol parameters |
| `Tier2Medium` | `2` | Yes (with gates) | Operational parameters |
| `Tier3Low` | `3` | Yes (with gates) | Operational tuning |

### Action type to tier mapping (`crates/ai_entities/src/tiers.rs:237-253`)

| Action | Value | Tier |
|--------|-------|------|
| `ModifyConsensusRule` | `0` | Tier0Never |
| `ModifyStateTransition` | `1` | Tier0Never |
| `UpdateBaseFee` | `10` | Tier1High |
| `UpdateBlockLimit` | `11` | Tier1High |
| `ActivateModule` | `12` | Tier1High |
| `UpdatePeerScoring` | `20` | Tier2Medium |
| `UpdateSpamThreshold` | `21` | Tier2Medium |
| `EmitAuditReport` | `22` | Tier2Medium |

### Approval gates (`crates/ai_entities/src/gates.rs`)

| Parameter | Value | Source |
|-----------|-------|--------|
| `MAX_APPROVERS` | 256 | `crates/ai_entities/src/gates.rs:25` |

Gate types (`crates/ai_entities/src/gates.rs:41-60`):

| Type | Value | Description |
|------|-------|-------------|
| `Multisig` | `0` | N-of-M signatures required |
| `Threshold` | `1` | At least N signatures |
| `TimelockOnly` | `2` | Auto-approved after timelock |

Gate ID computation:
```
gate_id = blake3("NOVAI_APPROVAL_GATE_ID_V1" || gate_type || threshold_be4 || approver_count_be4 || sorted_approvers || timelock_be8 || expiry_be8 || flags)
```
Source: `crates/ai_entities/src/gates.rs:22,351-387`.

---

## 14. Memory Objects

Defined in `crates/ai_entities/src/memory.rs`.

| Parameter | Value | Source |
|-----------|-------|--------|
| `MAX_MEMORY_OBJECT_SIZE` | 65,536 bytes (64 KB) | `crates/ai_entities/src/memory.rs:26` |
| `MAX_MEMORY_OBJECTS_PER_ENTITY` | 100 | `crates/ai_entities/src/memory.rs:29` |
| `MEMORY_OBJECT_CODEC_V1` | `1` | `crates/ai_entities/src/memory.rs:23` |

Object ID: `blake3("NOVAI_MEMORY_OBJECT_ID_V1" || ...)` (`crates/ai_entities/src/memory.rs:20`).

Memory object types (`crates/ai_entities/src/memory.rs:45-57`):

| Type | Value |
|------|-------|
| `ChainSummary` | `0` |
| `LabelIndex` | `1` |
| `EmbeddingCommitment` | `2` |
| `AnomalyLog` | `3` |
| `StatisticsSnapshot` | `4` |

---

## 15. NNPX Privacy System

### Domain separation constants (`crates/ai_entities/src/privacy.rs:25-34`)

| Constant | Value |
|----------|-------|
| `NNPX_COMMITMENT_DOMAIN` | `b"NOVAI_NNPX_COMMITMENT_V1"` |
| `NNPX_NULLIFIER_DOMAIN` | `b"NOVAI_NNPX_NULLIFIER_V1"` |
| `NNPX_KEY_DERIVE_DOMAIN` | `b"NOVAI_NNPX_KEY_DERIVE_V1"` |
| `NNPX_ZK_PROOF_DOMAIN` | `b"NOVAI_NNPX_ZK_PROOF_V1"` |

### Encoding constants (`crates/ai_entities/src/privacy.rs:41-45`)

| Constant | Value |
|----------|-------|
| `PRIVATE_PAYLOAD_COMMITMENT_V1` | `1` |
| `PRIVATE_PAYLOAD_COMMITMENT_LEN` | 129 bytes |

Commitment layout: version(1) + commitment_hash(32) + nullifier(32) + encryption_pubkey(32) + zk_proof(32) = 129.

### Derived views (`crates/ai_entities/src/derived_views.rs`)

| Parameter | Value | Source |
|-----------|-------|--------|
| `MAX_DERIVED_VIEW_SIZE` | 16,384 bytes (16 KB) | `crates/ai_entities/src/derived_views.rs:29` |
| `DERIVED_VIEW_CODEC_V1` | `1` | `crates/ai_entities/src/derived_views.rs:25` |

View ID: `blake3("NOVAI_DERIVED_VIEW_ID_V1" || ...)` (`crates/ai_entities/src/derived_views.rs:22`).

### Artifact storage (`crates/ai_entities/src/artifacts.rs`)

| Parameter | Value | Source |
|-----------|-------|--------|
| `MAX_ARTIFACT_SIZE` | 52,428,800 bytes (50 MB) | `crates/ai_entities/src/artifacts.rs:27` |

Artifact hash: `blake3("NOVAI_ARTIFACT_V1" || content)` (`crates/ai_entities/src/artifacts.rs:24`).

---

## 16. Governance System

Defined in `crates/governance/src/lib.rs`.

### Proposal types (`crates/governance/src/lib.rs:283-300`)

| Type | Value | Risk Level |
|------|-------|------------|
| `ParamChange` | `0` | Standard |
| `ModuleActivation` | `1` | High |
| `ModuleRollback` | `2` | Standard |
| `PolicyChange` | `3` | High |
| `EmergencyFreeze` | `4` | Emergency |

### Proposal states (`crates/governance/src/lib.rs:346-365`)

| State | Value |
|-------|-------|
| `Submitted` | `0` |
| `Approved` | `1` |
| `Executable` | `2` |
| `Executed` | `3` |
| `Expired` | `4` |
| `Rejected` | `5` |

### Default governance configuration (`crates/governance/src/lib.rs:96-103`)

| Parameter | Value | Description |
|-----------|-------|-------------|
| `default_timelock_blocks` | 1,000 | ~2.7 hours at 10s blocks |
| `high_risk_timelock_blocks` | 5,000 | ~13.9 hours at 10s blocks |
| `emergency_timelock_blocks` | 100 | ~16.7 minutes at 10s blocks |
| `default_expiry_blocks` | 50,000 | ~5.8 days at 10s blocks |

Proposal ID: `blake3("NOVAI_PROPOSAL_ID_V1" || ...)` (`crates/governance/src/lib.rs:270`).

### Audit actions (`crates/governance/src/lib.rs:126-141`)

| Action | Value |
|--------|-------|
| `Submitted` | `0` |
| `Approved` | `1` |
| `Executed` | `2` |
| `Rejected` | `3` |
| `Expired` | `4` |

---

## 17. Genesis Configuration

Defined in `crates/genesis/src/lib.rs`.

| Constraint | Value | Source |
|-----------|-------|--------|
| Min validators | 1 | `crates/genesis/src/lib.rs:210-213` |
| Max validators | 100 | `crates/genesis/src/lib.rs:215-218` |
| Min protocol_version | 1 | `crates/genesis/src/lib.rs:199-203` |
| Timestamp format | RFC3339 | `crates/genesis/src/lib.rs:206-207` |
| Pubkey format | 64 hex chars (32 bytes) | `crates/genesis/src/lib.rs:224` |
| Address format | 64 hex chars (32 bytes) | `crates/genesis/src/lib.rs:508` |

Genesis block: height=0, round=0, parent_hash=`[0u8; 32]`, empty txs (`crates/genesis/src/lib.rs:460-466`).

Validator ordering: sorted by address (ascending) (`crates/genesis/src/lib.rs:501`).

Golden state root test: `crates/genesis/src/lib.rs:943-983`.

---

## 18. Cryptographic Primitives

| Primitive | Algorithm | Library |
|-----------|-----------|---------|
| Hashing | blake3 | `blake3` crate |
| Signatures | Ed25519 (RFC 8032) | `ed25519-dalek` |
| Address derivation | `blake3(pubkey)` | `crates/crypto/src/lib.rs:27-31` |
| Key exchange (P2P) | X25519 (Noise_XX) | `crates/p2p/src/noise.rs` |
| Symmetric encryption (P2P) | ChaCha20-Poly1305 | Via Noise protocol |

---

## 19. Complete Domain Separation Tag Registry

All `b"NOVAI_*"` tags found in source code:

### Consensus signing tags (inline literals)

| Tag | Purpose | Source |
|-----|---------|--------|
| `b"NOVAI_VOTE_V1"` | Vote signature domain | `crates/consensus/src/lib.rs:386` |
| `b"NOVAI_TIMEOUT_V1"` | Timeout signature domain | `crates/consensus/src/lib.rs:591` |
| `b"NOVAI_PROPOSAL_V1"` | Proposal signature domain | per spec |

### Identity/commitment tags (named constants)

| Tag | Purpose | Source |
|-----|---------|--------|
| `b"NOVAI_AI_ENTITY_ID_V1"` | AI entity ID derivation | `crates/ai_entities/src/lib.rs:48` |
| `b"NOVAI_MODULE_MANIFEST_V1"` | Module manifest ID | `crates/ai_entities/src/lib.rs:51` |
| `b"NOVAI_SIGNAL_COMMIT_V1"` | Signal commitment hash | `crates/ai_entities/src/signals.rs:8` |
| `b"NOVAI_APPROVAL_GATE_ID_V1"` | Gate ID derivation | `crates/ai_entities/src/gates.rs:22` |
| `b"NOVAI_MEMORY_OBJECT_ID_V1"` | Memory object ID | `crates/ai_entities/src/memory.rs:20` |
| `b"NOVAI_DERIVED_VIEW_ID_V1"` | Derived view ID | `crates/ai_entities/src/derived_views.rs:22` |
| `b"NOVAI_ARTIFACT_V1"` | Artifact content hash | `crates/ai_entities/src/artifacts.rs:24` |
| `b"NOVAI_PROPOSAL_ID_V1"` | Governance proposal ID | `crates/governance/src/lib.rs:270` |

### NNPX privacy tags (named constants)

| Tag | Purpose | Source |
|-----|---------|--------|
| `b"NOVAI_NNPX_COMMITMENT_V1"` | Private payload commitment | `crates/ai_entities/src/privacy.rs:25` |
| `b"NOVAI_NNPX_NULLIFIER_V1"` | Nullifier derivation | `crates/ai_entities/src/privacy.rs:28` |
| `b"NOVAI_NNPX_KEY_DERIVE_V1"` | Encryption key derivation | `crates/ai_entities/src/privacy.rs:31` |
| `b"NOVAI_NNPX_ZK_PROOF_V1"` | ZK proof binding | `crates/ai_entities/src/privacy.rs:34` |

---

## 20. License Policy

Enforced via `deny.toml`.

Allowed licenses: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0.

Denied: GPL, AGPL, LGPL (any version).

---

## Appendix A: Known Spec-vs-Code Discrepancies

These discrepancies exist between older documentation and current code. **Code is authoritative.**

| Document | Claim | Code Reality | Winner |
|----------|-------|-------------|--------|
| `docs/CONSENSUS_V1.md` | `MAX_TXS_PER_BLOCK = 10,000` | `500` (`crates/types/src/lib.rs:25`) | Code |
| `docs/CONSENSUS_V1.md` | Individual tx size = 10 KB | `128 KB` (`crates/types/src/lib.rs:19`) | Code |
| `docs/TUNING_PARAMETERS.md` | `BASE_TIMEOUT_MS = 2000` | `1000` (`crates/consensus/src/lib.rs:20`) | Code |
| `consensus_types/codec.rs` | `MAX_TXS_PER_BLOCK = 10,000` | Codec decode bounds only; consensus enforces 500 | Both correct at their layers |
