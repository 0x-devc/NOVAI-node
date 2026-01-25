# NNPX Privacy Contract

> **Week 22 Specification**
>
> This document defines the privacy guarantees, threat model, and enforcement mechanisms
> for the NNPX (Nova Nota Private Exchange) subsystem.

## Overview

NNPX provides privacy-preserving transaction capabilities for NOVAI. The core invariant is:

**AI entities NEVER have direct access to raw private data.**

Private data is stored in a separate column family (`nnpx`) and is cryptographically committed
using hiding commitments. AI entities may only access derived, aggregated, or schema-validated
views of private data through explicit capability grants.

## Terminology

| Term | Definition |
|------|------------|
| **NNPX** | Nova Nota Private Exchange - the privacy subsystem |
| **Private Store** | RocksDB column family `nnpx` containing encrypted payloads |
| **Commitment** | `blake3(DOMAIN \|\| encrypted_payload)` - hides content |
| **Nullifier** | `blake3(DOMAIN \|\| secret \|\| counter)` - prevents double-spend |
| **Derived View** | Aggregated/transformed data that reveals no individual records |

## Privacy Guarantees

### G1: Storage Isolation

Private data is stored in a physically separate RocksDB column family (`nnpx`).
All keys in the private store use the prefix `b"nnpx/"`.

```
Public Store (default CF):     Private Store (nnpx CF):
├── accounts/                  ├── nnpx/commitments/
├── ai/entities/               ├── nnpx/nullifiers/
├── ai/memory_objects/         └── nnpx/encrypted/
└── consensus/
```

### G2: AI Access Prohibition

AI entities are **prohibited** from reading raw private data. This is enforced at multiple levels:

1. **Capability Level**: AI entities are created with `read_nnpx_derived: false` by default
2. **Execution Level**: All `nnpx/` key access is blocked for AI-initiated operations
3. **Storage Level**: Column family access is routed separately

The `read_nnpx_derived` capability, when granted, allows access ONLY to:
- Aggregate statistics (e.g., "total private transaction count")
- Schema-validated views (e.g., "is address X in the privacy set?")
- Never raw encrypted payloads or decryption keys

### G3: Commitment Hiding

Commitments hide payload content using domain-separated hashing:

```
commitment_hash = blake3(b"NOVAI_NNPX_COMMITMENT_V1" || encrypted_payload)
```

Properties:
- Same payload + same randomness = same commitment (deterministic)
- Same payload + different randomness = different commitment (hiding)
- Different payloads = different commitments (binding, under collision resistance)

### G4: Nullifier Uniqueness

Nullifiers prevent double-spend of private assets:

```
nullifier = blake3(b"NOVAI_NNPX_NULLIFIER_V1" || spending_secret || counter)
```

Properties:
- Each spending secret + counter pair produces a unique nullifier
- Nullifiers are stored in a set; duplicates are rejected
- Nullifiers reveal nothing about the underlying commitment

## Threat Model

### In Scope

| Threat | Mitigation |
|--------|------------|
| Malicious AI entity attempts to read private data | Capability check + storage isolation |
| AI entity registration with elevated capabilities | Registration rejects `read_nnpx_derived: true` for AI |
| Double-spend of private assets | Nullifier uniqueness enforcement |
| Commitment manipulation | Domain separation prevents cross-context attacks |
| Consensus nodes see encrypted data | Expected - they validate commitments, not decrypt |

### Out of Scope (Future Work)

| Threat | Status |
|--------|--------|
| Traffic analysis / timing attacks | Not addressed in Week 22 |
| Malicious validator collusion for metadata | Requires additional mixnet/relay |
| ZK proof soundness | Proof stub only; real proofs in future week |
| Key management / recovery | User responsibility in Week 22 |

## Storage Schema

### Key Prefixes

```rust
/// Private store prefix - routes to nnpx column family
pub const KEY_PREFIX_NNPX: &[u8] = b"nnpx/";

/// Commitment records: nnpx/commitments/{commitment_hash}
pub const KEY_PREFIX_NNPX_COMMITMENTS: &[u8] = b"nnpx/commitments/";

/// Nullifier set: nnpx/nullifiers/{nullifier}
pub const KEY_PREFIX_NNPX_NULLIFIERS: &[u8] = b"nnpx/nullifiers/";

/// Encrypted payloads: nnpx/encrypted/{commitment_hash}
pub const KEY_PREFIX_NNPX_ENCRYPTED: &[u8] = b"nnpx/encrypted/";
```

### Column Family Routing

Keys starting with `b"nnpx/"` are automatically routed to the `nnpx` column family.
All other keys use the `default` column family.

```rust
fn route_to_cf(key: &[u8]) -> &str {
    if key.starts_with(KEY_PREFIX_NNPX) {
        CF_NNPX  // "nnpx"
    } else {
        CF_DEFAULT  // "default"
    }
}
```

## PrivatePayloadCommitment Structure

```rust
/// A commitment to an encrypted private payload.
///
/// This structure is stored on-chain and reveals nothing about
/// the underlying payload content.
pub struct PrivatePayloadCommitment {
    /// blake3(DOMAIN || encrypted_payload) - hides the content
    pub commitment_hash: [u8; 32],

    /// blake3(DOMAIN || secret || counter) - prevents double-spend
    pub nullifier: [u8; 32],

    /// X25519 public key for payload encryption
    pub encryption_pubkey: [u8; 32],

    /// ZK proof stub (placeholder for future validity proofs)
    pub zk_proof: [u8; 32],
}
```

### Encoding Format

```
[version:1][commitment_hash:32][nullifier:32][encryption_pubkey:32][zk_proof:32]

Total: 129 bytes
Version: 0x01 (PRIVATE_PAYLOAD_COMMITMENT_V1)
```

## Enforcement Points

### 1. Entity Registration

When an AI entity is registered, the system enforces:
- `read_nnpx_derived` MUST be `false` for all AI entities
- Only human-controlled accounts may have derived view access

### 2. Transaction Execution

Before any storage operation:
```rust
fn validate_nnpx_access(key: &[u8], caller: &Caller) -> Result<(), ExecError> {
    if key.starts_with(KEY_PREFIX_NNPX) {
        if let Caller::AiEntity(_) = caller {
            return Err(ExecError::NnpxAccessDenied);
        }
    }
    Ok(())
}
```

### 3. Nullifier Validation

Before accepting a private transaction:
```rust
fn validate_nullifier(db: &impl Kv, nullifier: &[u8; 32]) -> Result<(), ExecError> {
    let key = nnpx_nullifier_key(nullifier);
    if db.get(&key)?.is_some() {
        return Err(ExecError::NullifierAlreadySpent);
    }
    Ok(())
}
```

## Domain Separation Constants

All NNPX cryptographic operations use domain-separated hashing to prevent
cross-context attacks:

```rust
/// Domain separator for commitment hash computation.
pub const NNPX_COMMITMENT_DOMAIN: &[u8] = b"NOVAI_NNPX_COMMITMENT_V1";

/// Domain separator for nullifier computation.
pub const NNPX_NULLIFIER_DOMAIN: &[u8] = b"NOVAI_NNPX_NULLIFIER_V1";

/// Domain separator for encryption key derivation.
pub const NNPX_KEY_DERIVE_DOMAIN: &[u8] = b"NOVAI_NNPX_KEY_DERIVE_V1";
```

## Security Properties

### Property 1: AI Isolation

```
FORALL ai_entity:
  ai_entity.capabilities.read_nnpx_derived == false
  => ai_entity CANNOT read any key starting with b"nnpx/"
```

### Property 2: Commitment Binding

```
FORALL c1, c2:
  c1.commitment_hash == c2.commitment_hash
  => c1.encrypted_payload == c2.encrypted_payload
  (under collision resistance of blake3)
```

### Property 3: Nullifier Uniqueness

```
FORALL tx1, tx2:
  tx1.nullifier == tx2.nullifier AND tx1 committed
  => tx2 REJECTED with NullifierAlreadySpent
```

## Testing Requirements

### Required Unit Tests

1. **`ai_cannot_read_nnpx`**: Verify AI entity operations are rejected for `nnpx/` keys
2. **`commitment_hides_payload`**: Verify same payload with different randomness produces different commitments
3. **`nullifier_prevents_reuse`**: Verify duplicate nullifiers are rejected

### Integration Tests

1. **Column family isolation**: Verify `nnpx` CF is physically separate
2. **Capability enforcement**: Verify registration rejects AI with `read_nnpx_derived: true`
3. **End-to-end flow**: Create commitment, spend with nullifier, verify double-spend blocked

## Future Extensions

| Week | Feature |
|------|---------|
| 23+ | ZK validity proofs (replace stub) |
| 24+ | Derived view computation engine |
| 25+ | Private-to-public bridge |
| TBD | Mixnet integration for metadata privacy |

---

**Document Status**: Week 22 Specification
**Last Updated**: Week 22
**Author**: NOVAI Protocol Team
