# AI_SIGNALS_V1 — AI Signal Specification

## Overview

This document defines the AI signal protocol for NOVAI v1. AI signals are first-class protocol objects that allow AI entities to communicate advisory information, predictions, and recommendations to the network.

## Version

- **Protocol Version:** 1
- **Schema Version:** `SIGNAL_SCHEMA_V1 = 1`
- **Status:** Week 13 implementation

## Normative Language

This specification uses RFC 2119 keywords:
- **MUST** / **SHALL** = absolute requirement
- **MUST NOT** / **SHALL NOT** = absolute prohibition
- **SHOULD** / **SHOULD NOT** = recommended but not required
- **MAY** = optional

Sections marked **(Informative)** are explanatory only. All other sections are normative.

---

## 1. Signal Types

### 1.1 Signal Type Enumeration

AI signals are categorized by type. Each type has a specific semantic meaning.

| Type | Value | Description |
|------|-------|-------------|
| `Anomaly` | 0 | Detected anomalous behavior or state |
| `Optimization` | 1 | Suggested optimization or improvement |
| `Prediction` | 2 | Future state or event prediction |
| `RiskScore` | 3 | Quantified risk assessment |
| `AuditReport` | 4 | Audit or compliance report |
| `SpamRisk` | 5 | Transaction spam risk indicator |
| `CongestionForecast` | 6 | Network congestion prediction |

**Constraints:**
- Type values 0-6 are defined
- Values 7-255 are RESERVED for future use
- Unknown type values MUST be rejected by validators

### 1.2 Signal Type Semantics

#### Anomaly (0)
Indicates the AI has detected behavior or state that deviates significantly from expected patterns.

**Use Cases:**
- Unusual transaction patterns
- State inconsistencies
- Network behavior anomalies

#### Optimization (1)
Suggests improvements to protocol parameters or validator behavior.

**Use Cases:**
- Fee adjustment recommendations
- Block size tuning
- Resource allocation suggestions

#### Prediction (2)
Forecasts future states or events based on historical data.

**Use Cases:**
- Price predictions (for DeFi applications)
- Load forecasting
- User activity predictions

#### RiskScore (3)
Provides quantified risk assessment for transactions, addresses, or protocol states.

**Use Cases:**
- Transaction risk scoring
- Address reputation scoring
- Smart contract risk analysis

#### AuditReport (4)
Formal audit or compliance report.

**Use Cases:**
- Protocol health reports
- Compliance verification
- Security audits

#### SpamRisk (5)
Indicates likelihood that a transaction or address is associated with spam.

**Use Cases:**
- Transaction filtering assistance
- Address reputation
- DOS attack indicators

#### CongestionForecast (6)
Predicts network congestion levels.

**Use Cases:**
- Fee estimation assistance
- Load balancing decisions
- Capacity planning

---

## 2. Signal Structure

### 2.1 AiSignalV1 Fields

```
AiSignalV1 {
    signal_type: AiSignalType,    // 1 byte - Signal category
    height: u64,                  // 8 bytes - Block height when generated
    issuer: [u8; 32],             // 32 bytes - AI entity ID
    confidence: u8,               // 1 byte - 0-255 confidence level
    payload_hash: [u8; 32],       // 32 bytes - Hash of off-chain payload
    zk_proof: Option<Vec<u8>>,    // Variable - Optional ZK proof
    signature: [u8; 64],          // 64 bytes - Ed25519 signature
}
```

### 2.2 Field Specifications

#### signal_type (1 byte)
- MUST be a valid `AiSignalType` discriminant (0-6)
- MUST match the semantic content of the signal

#### height (8 bytes, little-endian)
- MUST be the block height at which the signal was generated
- MUST NOT be in the future (height > current_chain_height)
- MAY be in the past (for delayed signals)

#### issuer (32 bytes)
- MUST be a valid `AiEntityId`
- MUST correspond to a registered AI entity on-chain
- The issuer MUST have `emit_proposals` capability set

#### confidence (1 byte)
- Value range: 0-255
- 0 = no confidence, 255 = maximum confidence
- Interpretation is signal-type-specific

#### payload_hash (32 bytes)
- MUST be `blake3(payload_bytes)`
- Payload bytes are stored off-chain
- Enables on-chain commitment with off-chain data

#### zk_proof (optional, variable length)
- MAY be present for verifiable signals
- If present, MUST be <= 65,536 bytes (64 KB)
- Proof format is application-specific

#### signature (64 bytes)
- Ed25519 signature over the commitment hash
- Domain-separated signing (see Section 3)

---

## 3. Cryptographic Operations

### 3.1 Domain Separation

All cryptographic operations use domain separation to prevent cross-context attacks.

| Operation | Domain Tag |
|-----------|------------|
| Signal Commitment | `NOVAI_SIGNAL_COMMIT_V1` |
| Signal Signing | `NOVAI_SIGNAL_SIGN_V1` |

### 3.2 Commitment Computation

The commitment hash binds to all fields except the signature:

```
commitment_hash = blake3(
    "NOVAI_SIGNAL_COMMIT_V1" ||
    signal_type ||
    height (LE) ||
    issuer ||
    confidence ||
    payload_hash ||
    zk_proof_encoding
)
```

**ZK Proof Encoding:**
- If `zk_proof` is None: `0x00000000` (4 zero bytes)
- If `zk_proof` is Some(p): `len(p) as u32 LE || p`

### 3.3 Signature Computation

Signals MUST be signed by the issuing AI entity:

```
signed_bytes = "NOVAI_SIGNAL_SIGN_V1" || commitment_hash
signature = ed25519_sign(issuer_private_key, signed_bytes)
```

### 3.4 Signature Verification

```
verify(issuer_public_key, "NOVAI_SIGNAL_SIGN_V1" || commitment_hash, signature)
```

Where `issuer_public_key` is derived from the AI entity's registration.

---

## 4. Wire Format

### 4.1 AiSignalV1 Encoding

```
| Offset | Size    | Field        | Notes                    |
|--------|---------|--------------|--------------------------|
| 0      | 1       | version      | 0x01                     |
| 1      | 1       | signal_type  | AiSignalType discriminant|
| 2      | 8       | height       | u64 LE                   |
| 10     | 32      | issuer       | AiEntityId               |
| 42     | 1       | confidence   | u8                       |
| 43     | 32      | payload_hash | blake3 hash              |
| 75     | 1       | proof_flag   | 0 = None, 1 = Some       |
| 76     | var     | proof_data   | If flag=1: len(4) + data |
| var    | 64      | signature    | Ed25519 signature        |
```

**Size without proof:** 140 bytes
**Size with proof:** 140 + 4 + proof_len bytes

### 4.2 SignalCommitment Encoding

Compact commitment for indexing and vote inclusion:

```
| Offset | Size | Field           | Notes                    |
|--------|------|-----------------|--------------------------|
| 0      | 1    | version         | 0x01                     |
| 1      | 32   | commitment_hash | blake3 commitment        |
| 33     | 1    | signal_type     | AiSignalType discriminant|
| 34     | 8    | height          | u64 LE                   |
| 42     | 32   | issuer          | AiEntityId               |
```

**Fixed size:** 74 bytes

---

## 5. Publisher Rules

### 5.1 Eligibility Requirements

An AI entity MAY emit signals if:

1. The entity is registered on-chain with valid `AiEntityId`
2. The entity has `emit_proposals` capability set
3. The entity is not in a frozen or suspended state
4. The entity has sufficient economic balance for signal fees (if applicable)

### 5.2 Rate Limiting

Validators SHOULD implement rate limiting:

- **Per-entity limit:** Maximum signals per entity per block
- **Global limit:** Maximum total signals per block
- **Burst allowance:** Short-term burst capacity

**Recommended defaults:**
- Per-entity: 10 signals/block
- Global: 1000 signals/block
- Burst: 2x per-entity limit for 10 blocks

### 5.3 Signal Fees

**(Future - Not Implemented in V1)**

Signals MAY require fees to prevent spam:
- Fee amount depends on signal type and size
- Fees are deducted from entity's `economic_balance`
- Fee pricing may be dynamic based on network load

---

## 6. Approval Gate Framework

### 6.1 Gate Types

AI-triggered actions (not signals) require approval gates:

| Gate Type | Value | Description |
|-----------|-------|-------------|
| `Multisig` | 0 | Requires exactly N-of-M signatures |
| `Threshold` | 1 | Requires at least N signatures |
| `TimelockOnly` | 2 | Auto-approved after delay |

### 6.2 Gate Structure

```
ApprovalGate {
    gate_id: [u8; 32],              // Deterministic identifier
    gate_type: GateType,            // Approval mechanism
    required_approvers: Vec<Address>, // Who can approve
    threshold: u32,                 // Required approval count
    timelock_blocks: u64,           // Delay after approval
    expiry_blocks: u64,             // Proposal expiration
    veto_enabled: bool,             // Validator veto rights
    freeze_enabled: bool,           // Gate can be frozen
}
```

### 6.3 Gate ID Computation

Gate IDs are content-addressable:

```
gate_id = blake3(
    "NOVAI_APPROVAL_GATE_ID_V1" ||
    gate_type ||
    threshold (BE) ||
    approvers_count (BE) ||
    sorted_approvers ||
    timelock_blocks (BE) ||
    expiry_blocks (BE) ||
    flags
)
```

### 6.4 Validation Rules

A gate is valid if:
1. `threshold <= required_approvers.len()` (for Multisig/Threshold)
2. `expiry_blocks > timelock_blocks`
3. No duplicate addresses in `required_approvers`
4. `threshold > 0` for Multisig/Threshold types
5. `required_approvers.len() <= 256`
6. `required_approvers` is empty for TimelockOnly type

---

## 7. Action Tiering

### 7.1 Security Tiers

AI actions are classified by security level:

| Tier | Value | Security Level | Description |
|------|-------|----------------|-------------|
| `Tier0Never` | 0 | Forbidden | Never allowed via AI |
| `Tier1High` | 1 | High | Requires strongest gates |
| `Tier2Medium` | 2 | Medium | Moderate gate requirements |
| `Tier3Low` | 3 | Low | Minimal approval needed |

### 7.2 Action Type Mapping

| Action | Tier | Rationale |
|--------|------|-----------|
| `ModifyConsensusRule` | 0 | Consensus-critical |
| `ModifyStateTransition` | 0 | Consensus-critical |
| `UpdateBaseFee` | 1 | Core protocol |
| `UpdateBlockLimit` | 1 | Core protocol |
| `ActivateModule` | 1 | Core protocol |
| `UpdatePeerScoring` | 2 | Operational |
| `UpdateSpamThreshold` | 2 | Operational |
| `EmitAuditReport` | 2 | Operational |

### 7.3 Tier Enforcement

```
tier = tier_for_action(action_type)
if tier == Tier0Never:
    REJECT (action never allowed)
elif tier == Tier1High:
    REQUIRE high-security gate (high threshold, long timelock)
elif tier == Tier2Medium:
    REQUIRE medium-security gate
elif tier == Tier3Low:
    REQUIRE minimal gate (may be single-signer)
```

---

## 8. Attack Model

### 8.1 Threat Overview

AI signals introduce new attack vectors that must be mitigated at the protocol level.

### 8.2 Poisoning Attack

**Description:** Malicious AI entity emits misleading signals to manipulate network behavior.

**Example:** An AI issues false `Anomaly` signals about a legitimate validator, causing other nodes to lower their peer score.

**Attack Vector:**
1. Attacker registers AI entity
2. Entity gains reputation through legitimate signals
3. Entity emits poisoned signals during critical moments
4. Network acts on false information

**Mitigations:**
- **Publisher Restriction:** Only registered entities can emit signals (prevents anonymous attacks)
- **Capability Gates:** Entity must have `emit_proposals` capability
- **Confidence Weighting:** Signals with low confidence are weighted less
- **Cross-Validation:** Multiple AI entities must agree for high-impact decisions
- **(Future) Reputation Slashing:** Detected poisoning results in economic penalty

### 8.3 Sybil Attack

**Description:** Attacker spawns many AI entities to flood signals or artificially create consensus.

**Example:** Attacker registers 100 AI entities, each emitting `RiskScore` signals about the same address to manipulate its reputation.

**Attack Vector:**
1. Attacker creates many AI entities with minimal stake
2. Entities collude to emit coordinated false signals
3. Volume of signals creates false appearance of consensus
4. Network acts on artificially inflated signal count

**Mitigations:**
- **Economic Stake:** Registration requires minimum stake
- **Rate Limiting:** Per-entity signal limits prevent flooding
- **Global Limits:** Total signals per block capped
- **Stake-Weighted Signals:** Higher-stake entities' signals weighted more
- **(Future) Signal Fees:** Economic cost per signal emission

### 8.4 False Positive Attack

**Description:** Deliberately triggering false alarms to cause unnecessary defensive responses.

**Example:** AI emits continuous `SpamRisk` signals about legitimate transactions, causing them to be deprioritized.

**Attack Vector:**
1. Entity emits high-confidence false positive signals
2. Network defense mechanisms activate
3. Legitimate activity is impacted
4. Network throughput or user experience degrades

**Mitigations:**
- **Confidence Thresholds:** Actions only triggered above confidence threshold
- **Signal Correlation:** Single signal doesn't trigger major actions
- **Historical Analysis:** Signal accuracy tracked over time
- **Graduated Response:** Severity of response proportional to signal consensus
- **(Future) Accuracy Tracking:** Entities with poor accuracy lose influence

### 8.5 Timing Attack

**Description:** Timing signal emission to cause maximum disruption during critical moments.

**Example:** Emit false `CongestionForecast` signal just before a high-value transaction to manipulate fee estimation.

**Attack Vector:**
1. Monitor mempool for target transactions
2. Emit manipulative signal timed to affect target
3. Target transaction uses incorrect parameters
4. Attacker profits from manipulation

**Mitigations:**
- **Signal Latency:** Signals have minimum block delay before effect
- **Fee Estimation Windows:** Fee estimates use historical data, not instant signals
- **Anomaly Detection:** Rapid signal changes flagged as suspicious
- **(Future) Commit-Reveal:** Important signals use commit-reveal scheme

### 8.6 Replay Attack

**Description:** Re-broadcasting old signals as if they were new.

**Example:** Replay an old `RiskScore` signal after conditions have changed.

**Attack Vector:**
1. Record legitimate signal from honest entity
2. Wait for conditions to change
3. Rebroadcast old signal
4. Network acts on stale information

**Mitigations:**
- **Height Binding:** Signals bind to specific block height
- **Expiration:** Signals older than N blocks are rejected
- **Nonce Tracking:** (If applicable) Monotonic signal counter
- **Commitment Uniqueness:** Commitment hash includes height

---

## 9. Future Extensions

### 9.1 Planned Enhancements

1. **Signal Fees:** Economic cost for signal emission
2. **Reputation System:** Track signal accuracy over time
3. **Stake-Weighted Signals:** Higher stake = more signal weight
4. **ZK Signal Verification:** Verify signals without revealing inputs
5. **Cross-Entity Validation:** Require multiple AI agreement
6. **Signal Aggregation:** Combine multiple signals into summaries

### 9.2 Backward Compatibility

- Version byte allows future format changes
- Unknown signal types are rejected (not ignored)
- New optional fields use explicit flags
- Schema version changes require coordinated upgrade

---

## 10. Implementation Notes

### 10.1 Reference Implementation

- **Signal Types:** `crates/ai_entities/src/signals.rs`
- **Gate Types:** `crates/ai_entities/src/gates.rs`
- **Tiering Engine:** `crates/ai_entities/src/tiers.rs`
- **Signal Codec:** `crates/codec/src/ai_signal_codec.rs`
- **Gate Codec:** `crates/codec/src/gate_codec.rs`

### 10.2 Test Coverage

- Unit tests for all type methods
- Golden vector tests for encoding stability
- Roundtrip tests for encode/decode
- Validation tests for edge cases

### 10.3 Constants

```rust
// Signal constants
pub const SIGNAL_SCHEMA_V1: u8 = 1;
pub const MAX_ZK_PROOF_SIZE: usize = 65_536; // 64 KB

// Gate constants
pub const MAX_APPROVERS: usize = 256;
pub const APPROVAL_GATE_V1_MIN_SIZE: usize = 59;

// Domain separators
pub const SIGNAL_COMMIT_DOMAIN_V1: &[u8] = b"NOVAI_SIGNAL_COMMIT_V1";
pub const GATE_ID_DOMAIN: &[u8] = b"NOVAI_APPROVAL_GATE_ID_V1";
```

---

## References

- NOVAI Architecture Decisions (`docs/ARCHITECTURE_DECISIONS.md`)
- NOVAI Consensus V1 (`docs/CONSENSUS_V1.md`)
- Blake3 specification: https://github.com/BLAKE3-team/BLAKE3
- Ed25519 specification: RFC 8032

---

**Document Status:** Living document, updated as protocol evolves.
**Last Updated:** Week 13 - AI Signal Spec v1 + Approval Gate Framework
