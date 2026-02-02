# NOVAI External Audit Scope

**Prepared**: 2026-02-02
**Protocol Version**: 1
**Codebase**: 16 crates, ~48,900 lines Rust
**Reference**: `docs/MAINNET_SPEC.md` for all parameter values and wire formats.

---

## 1. Purpose

This document defines the scope for professional security audit of the NOVAI L1 blockchain protocol. It identifies components ranked by criticality, known limitations, threat vectors, and recommended focus areas for auditors.

---

## 2. Components In Scope

### Critical (consensus safety, state integrity, cryptographic correctness)

| Crate | Lines | Purpose |
|-------|-------|---------|
| `consensus` | ~7,100 | BFT consensus engine: propose/vote/QC/timeout/commit cycle |
| `consensus_types` | ~1,900 | Consensus message types, canonical codecs, leader selection |
| `execution` | ~12,100 | Deterministic state transition: transfers, signals, governance, memory, NNPX |
| `crypto` | ~480 | Ed25519 signing/verification, address derivation, ZK stub |
| `state` | ~1,700 | KV abstraction, key schema, account/fee encoding, column families |
| `smt` | ~710 | Sparse Merkle Tree: domain-separated hashing, node encoding, proof generation |

### High (protocol correctness, data integrity)

| Crate | Lines | Purpose |
|-------|-------|---------|
| `p2p` | ~980 | TCP networking, wire framing, Noise_XX transport encryption |
| `codec` | ~2,000 | Canonical encoding for TxV1, BlockHeaderV1, AI entity, signal, gate codecs |
| `governance` | ~2,400 | Proposal types, lifecycle states, timelock config, audit log |
| `ai_entities` | ~6,800 | AI entity types, signals, tiers, gates, memory, privacy, derived views |
| `genesis` | ~1,100 | Deterministic genesis state generation, golden state root |

### Medium (operational correctness)

| Crate | Lines | Purpose |
|-------|-------|---------|
| `node` | ~3,400 | Node binary, consensus wiring, metrics, P2P connection management |
| `mempool` | ~720 | Transaction ordering and size enforcement |

### Low (advisory, non-consensus)

| Crate | Lines | Purpose |
|-------|-------|---------|
| `copilot` | ~7,400 | Validator co-pilot: anomaly detection, congestion forecasting (advisory only) |
| `types` | ~120 | Shared type aliases and protocol constants |

---

## 3. Known Limitations

### 3.1 TX Signing Has No Domain Separation Tag
Transaction signatures are computed over raw unsigned bytes with no domain tag (`crates/crypto/src/lib.rs:51-55`). All other consensus messages (Vote, Timeout, Proposal) use domain tags (`b"NOVAI_VOTE_V1"`, etc.). This means a valid NOVAI transaction signature could theoretically be valid on another Ed25519 chain if addresses overlap. **Cross-chain replay risk exists.**

### 3.2 Duplicate MAX_TXS_PER_BLOCK Constant
`crates/types/src/lib.rs:25` defines `MAX_TXS_PER_BLOCK = 500` (consensus enforcement). `crates/consensus_types/src/codec.rs:57` defines `MAX_TXS_PER_BLOCK = 10,000` (codec decode bounds). Both serve different purposes at different layers but sharing the same name is confusing and could lead to bugs if a developer uses the wrong one.

### 3.3 Unresolved Clippy Warnings
164 clippy warnings remain as of 2026-02-02. Predominantly `format!` string suggestions (61), cast warnings (35), and deprecated function usage (5) in test files. See `docs/SECURITY_CHECKLIST.md` Item 7 for full breakdown.

### 3.4 NNPX ZK Proofs Are Placeholder
`crates/ai_entities/src/privacy.rs:34` defines `NNPX_ZK_PROOF_DOMAIN` and `crates/crypto/src/zk.rs` provides `StubZkVerifier`, but no real ZK circuit is implemented. The ZK proof field in `PrivatePayloadCommitment` is a 32-byte placeholder.

### 3.5 Mixed Endianness Across Layers
Wire format codec (`crates/codec/src/lib.rs`) uses **little-endian** for all multi-byte integers. State key encoding (`crates/state/src/lib.rs:171,179,304,571-572`) uses **big-endian** for height keys, balance, nonce, and count fields to ensure correct RocksDB lexicographic ordering. This is intentional but requires auditors to verify consistency at each layer.

### 3.6 Autonomous AI Mode Not Implemented
`AutonomyMode::Autonomous = 2` (`crates/ai_entities/src/lib.rs:92`) is defined but reserved. No execution path processes autonomous AI actions. The value can be stored on-chain but has no effect beyond `Gated` mode.

### 3.7 HashMap in Consensus State
`crates/consensus/src/lib.rs:111-127` uses `HashMap` for `pending_votes`, `block_cache`, `qc_cache`, `block_by_hash`, and `pending_timeouts`. Current usage is order-independent (lookups by key, max-finding iteration) but future changes could introduce nondeterminism.

### 3.8 Validator Set Size Constraint
`crates/consensus_types/src/leader.rs:47-51` requires `n = 3f + 1` (not merely `n >= 3f + 1`). Valid sizes: 4, 7, 10, 13, ... This is stricter than typical BFT implementations and limits validator set flexibility.

---

## 4. Threat Model

### 4.1 Byzantine Validators (f < n/3)
The protocol tolerates up to `f` Byzantine validators where `n = 3f + 1`. Byzantine behavior includes equivocation (signing conflicting votes), withholding votes, and proposing invalid blocks. Safety depends on the 3-chain commit rule implemented in `crates/consensus/src/lib.rs`. Liveness depends on timeout-based view change (`BASE_TIMEOUT_MS = 1000`, exponential backoff).

### 4.2 Network Partition / Eclipse Attacks
Under partial synchrony, messages may be delayed but eventually delivered. Network partitions are handled by the timeout mechanism: nodes advance rounds via timeout quorum (2f+1 timeouts). Eclipse attacks isolating a single validator are mitigated by the quorum requirement. Catch-up sync (`crates/consensus/src/lib.rs` recovery logic) allows nodes to rejoin after partition healing.

### 4.3 State Corruption / SMT Proof Forgery
The Sparse Merkle Tree uses domain-separated blake3 hashing (`crates/smt/src/hash.rs:12-14`) with three distinct tags (empty=0x00, leaf=0x01, internal=0x02). Forging a proof requires finding a blake3 preimage. State root is committed in every block and verified during sync. Corruption of local RocksDB is not defended against (trusted local storage assumption).

### 4.4 AI Signal Manipulation / False Signals
AI signals are advisory only — they do not affect consensus validity. A malicious AI entity can emit false signals, but signals are non-binding per `docs/AI_SIGNALS_V1.md`. Signal commitments are domain-separated (`b"NOVAI_SIGNAL_COMMIT_V1"`). The risk is validators acting on false advisory information, which is an operational concern, not a consensus safety issue.

### 4.5 Governance Attacks
Potential vectors: (a) timelock bypass — execute proposal before timelock expires; (b) proposal stuffing — flood governance with proposals; (c) expired proposal execution — execute after expiry. Timelock enforcement is height-based and deterministic (`crates/governance/src/lib.rs:78-86`). Tested in `crates/execution/tests/adversarial_timelock.rs` and `adversarial_proposal_spam.rs`. Two critical vulnerabilities were found and patched during adversarial testing (`docs/ADVERSARIAL_GOVERNANCE_REPORT.md`).

### 4.6 NNPX Privacy Attacks
Potential vectors: (a) nullifier correlation — linking nullifiers to transactions; (b) commitment forgery — creating valid-looking fake commitments; (c) AI entity accessing raw private data. Column family isolation (`CF_NNPX`) enforces storage-level separation. AI access is blocked at the execution layer (`crates/execution/src/lib.rs:1844-1929`). Tested in 35 adversarial tests (`docs/ADVERSARIAL_PRIVACY_REPORT.md` — 0 vulnerabilities found). ZK proofs are placeholder, so privacy currently relies on encryption rather than zero-knowledge.

### 4.7 Mempool Flooding / Transaction Spam
Mempool has a 64 MB size limit (`MAX_MEMPOOL_BYTES`). Individual transactions are capped at 128 KB (`MAX_TX_SIZE`). Defense-in-depth size enforcement at 4 layers: RPC admission, mempool insertion, block proposal, and consensus validation. Fee-based prioritization in `crates/mempool/src/lib.rs`.

### 4.8 P2P Transport Attacks
P2P uses Noise_XX_25519_ChaChaPoly_SHA256 (`crates/p2p/src/noise.rs`) for transport encryption. Wire messages are length-prefixed with a 2 MB cap (`MAX_WIRE_MSG_BYTES`). Potential vectors: (a) handshake manipulation; (b) message amplification via broadcast; (c) connection exhaustion. Peer management in `crates/p2p/src/lib.rs` removes disconnected peers but has no rate limiting or connection caps.

### 4.9 Codec Attacks
Malformed messages could cause parsing panics or logic errors. All decode functions validate version bytes, check length bounds, and reject trailing bytes. Codec-level DoS bounds (`MAX_TXS_PER_BLOCK = 10,000`, `MAX_VOTES_PER_QC = 11,000`) prevent allocation bombs. Golden vector tests lock encoding formats across versions.

---

## 5. Security Assumptions

| Assumption | Details |
|-----------|---------|
| BFT safety | Requires >2n/3 honest validators (`n = 3f + 1`) |
| Network model | Partially synchronous (messages eventually delivered) |
| Cryptographic hardness | Ed25519 (RFC 8032), blake3, X25519 unbroken |
| Local storage | RocksDB is trusted (no disk tampering detection) |
| Time | Wall clock not used in consensus; round-based progression only |
| Determinism | No floats, no HashMap-order-dependent consensus logic, canonical encoding |

---

## 6. Out of Scope

- Client-side wallets and key management software
- Off-chain AI inference engines and ML model execution
- Bridge / interoperability protocols with other chains
- Smart contract VM / general-purpose programmability (not implemented)
- Frontend applications and RPC API security
- Deployment infrastructure (cloud providers, Docker, Kubernetes)
- Operating system and hardware security

---

## 7. Recommended Audit Focus Areas

Prioritized by risk and impact:

1. **Consensus state machine** — Vote/QC validation, 3-chain commit rule, view change correctness. Files: `crates/consensus/src/lib.rs`.
2. **State transition determinism** — All validators must produce identical state roots from identical inputs. Files: `crates/execution/src/lib.rs`.
3. **Codec canonical encoding** — Round-trip correctness, no trailing bytes, version validation. Files: `crates/codec/src/lib.rs`, `crates/consensus_types/src/codec.rs`.
4. **SMT proof generation and verification** — Domain separation, leaf/internal distinction, empty tree handling. Files: `crates/smt/src/`.
5. **Transfer execution** — Balance overflow/underflow, fee deduction, nonce enforcement. Files: `crates/execution/src/lib.rs`.
6. **Governance timelock enforcement** — Cannot execute before timelock, cannot execute expired proposals, state machine transitions. Files: `crates/governance/src/lib.rs`, `crates/execution/src/lib.rs`.
7. **AI tier enforcement** — Tier0Never must unconditionally block. Exhaustive match in `tier_for_action()`. Files: `crates/ai_entities/src/tiers.rs`.
8. **NNPX nullifier uniqueness** — Double-spend prevention via nullifier set. Files: `crates/execution/src/lib.rs` (NNPX section), `crates/ai_entities/src/privacy.rs`.
9. **P2P message authentication** — Noise handshake correctness, message framing validation. Files: `crates/p2p/src/noise.rs`, `crates/p2p/src/lib.rs`.
10. **Mixed endianness correctness** — Verify LE in wire codec vs BE in state keys is consistently applied at all boundaries. Files: `crates/codec/src/lib.rs`, `crates/state/src/lib.rs`.
