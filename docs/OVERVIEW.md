# What is NOVAI

NOVAI is a clean-room implemented Layer-1 blockchain designed from the ground up to support first-class AI entities as native protocol primitives. Unlike traditional blockchains where AI capabilities are bolted on via smart contracts, NOVAI integrates AI entities into its consensus layer with explicit capabilities, economic agency, and governance-controlled autonomy modes. The protocol combines Byzantine Fault Tolerant consensus, deterministic state transitions, and structured privacy (NNPX) to create a foundation where AI systems can operate transparently and safely alongside human validators.

NOVAI implements a HotStuff-inspired consensus protocol with a 3-chain commit rule for safety and view-change mechanisms for liveness. Every transaction execution is deterministic and verifiable; all encoding is canonical and golden-vector tested. The network achieves finality through a deterministic leader schedule and quorum-based approval, with crash-safe persistence enabling node restart recovery.

The protocol is currently in active development, having completed core consensus, execution, persistence, AI primitives, anomaly detection, and governance foundations. The codebase is Apache-2.0 licensed and strictly enforces clean-room development principles: no copied consensus code, no GPL dependencies, and mandatory license gates on all external dependencies.

## Key Features

- **First-Class AI Entities**: AI systems are protocol primitives with stable identities, persistent memory, economic balances, and explicit capability manifests. Not smart contracts.
- **Clean-Room Consensus**: Originally implemented HotStuff-like BFT with 3-chain commit rule, exponential backoff timeouts, and deterministic leader rotation.
- **Deterministic Execution**: No floats, no non-deterministic iteration, all arithmetic checked. Identical transaction execution across all nodes.
- **Canonical Encoding**: All protocol types use versioned, canonical encodings with golden vector tests locked for stability.
- **NNPX Privacy Layer**: Separate storage for encrypted private data with cryptographic commitments and AI access prohibition -- raw private data never visible to AI entities.
- **Governance with AI Autonomy Control**: Five proposal types (ParamChange, ModuleActivation, ModuleRollback, PolicyChange, EmergencyFreeze) with timelocked execution and explicit AI autonomy upgrade gating.
- **Validator Co-Pilot**: Statistics-based anomaly detection for chain health, mempool congestion forecasting, and transaction spam detection -- advisory only, never affects consensus.
- **Crash-Safe Persistence**: RocksDB-backed state with atomic write batching, committed_height tracking, and restart recovery mechanisms.
- **Sparse Merkle Tree State Root**: Deterministic state authentication via 256-bit SMT with domain-separated hashing and fast lookup proofs.

## Architecture

### Crate Dependency Graph

```
+-------------------------------------------------------------+
|                      novai-node (binary)                     |
|  +-> consensus_node: Orchestrates consensus rounds           |
|  +-> metrics: Prometheus/observability                       |
|  +-> rpc: JSON-RPC interface                                 |
+----------------------------+--------------------------------+
                             |
          +------------------+------------------+
          |                  |                  |
    +-----v------+    +-----v------+    +------v-------+
    | consensus   |    | execution  |    | mempool      |
    | (BFT)       |    | (state)    |    | (pending tx) |
    +-----+------+    +-----+------+    +--------------+
          |                  |
          +------------------+--------------------+
                             |                    |
          +------------------v--------+    +------v--------+
          |  consensus_types          |    | smt / state   |
          |  (msgs, codec)            |    | (SMT root)    |
          +---------------------------+    +---------------+
                             |
          +------------------v----------------------------+
          |  types, codec, crypto, p2p                    |
          |  (Foundation layer)                           |
          +-----------------------------------------------+

AI & Governance Extensions:
+-> ai_entities (first-class AI: signals, gates, tiers, memory)
+-> ai_service (Anthropic Claude API integration)
+-> copilot (validator co-pilot: anomaly detection)
+-> governance (proposal lifecycle, timelocks)
```

### Core Crates

| Crate | Purpose |
|-------|---------|
| `consensus` | HotStuff BFT engine: proposal/vote/QC cycle, timeout view-change, 3-chain commit detection |
| `consensus_types` | Wire protocol: SignedProposal, Vote, QC, Timeout, canonical codecs |
| `node` | Validator node orchestration: consensus loop, block storage, state machine |
| `execution` | Deterministic state transition: transfers, nonce validation, SMT root updates |
| `smt` | Sparse Merkle Tree: domain-separated hashing, node encoding/decoding |
| `state` | Storage abstraction: account state, SMT nodes, committed_height persistence |
| `p2p` | TCP-based networking: Noise XX encryption, message broadcast, peer management |
| `mempool` | Transaction validation and pending pool management |
| `codec` | Type serialization: variable-length encoding, version prefixes, golden vectors |
| `crypto` | Ed25519 signatures, Blake3 hashing, address derivation |
| `types` | Protocol primitives: TxV1, Block, Address, Nonce |
| `ai_entities` | AI primitives: AiSignalV1, ApprovalGate, autonomy tiers, memory objects |
| `ai_service` | Anthropic Claude API sidecar for AI entity execution |
| `copilot` | Statistics-based anomaly detection: missed blocks, vote delays, peer churn |
| `governance` | Proposal types, lifecycle, timelocks, AI autonomy upgrade gates |
| `genesis` | Deterministic chain initialization |

## Consensus

NOVAI uses a clean-room implementation of HotStuff-like Byzantine Fault Tolerant consensus:

**Safety Mechanism:**
- Any two quorums (2f+1 out of 3f+1 validators) intersect in at least one honest node
- Honest validators vote at most once per (height, round), preventing conflicting blocks at the same height
- A QC at (h, r) proves 2f+1 votes exist, blocking conflicting QCs for that (h, r)

**3-Chain Commit Rule:**
```
Block B (h) --QC_h--> B' (h+1) --QC_{h+1}--> B'' (h+2)
Committed when QC_{h+2} is observed
```

**Liveness and View-Change:**
- Deterministic leader rotation: `leader(height, round) = validators[(height + round) % n]`
- Exponential backoff timeouts: `timeout(r) = min(BASE_TIMEOUT_MS * 2^r, MAX_TIMEOUT_MS)` (default 1s, 2s, 4s, ..., 60s)
- Timeout messages broadcast when no valid proposal received within timeout window
- Round advances when 2f+1 timeout votes accumulated for (height, round)

**Signature Domain Separation:**
All message types use domain-separated hashing to prevent cross-context attacks:
- Vote: `blake3("NOVAI_VOTE_V1" || encode_vote_v1_unsigned(vote))`
- Timeout: `blake3("NOVAI_TIMEOUT_V1" || encode_timeout_v1_unsigned(timeout))`
- Proposal: `blake3("NOVAI_PROPOSAL_V1" || encode_proposal_v1_unsigned(proposal))`

## AI Integration

### AI Entities as Protocol Primitives

AI entities are first-class protocol types, not smart contracts:

```rust
AiEntity {
    id: [u8; 32],                    // blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator)
    autonomy_mode: AutonomyMode,     // Advisory (0), Gated (1), Autonomous (2)
    capabilities: u32,               // Bitflags: read_public, read_memory, emit_proposals, etc.
    economic_balance: u64,           // Can pay fees, own assets
    module_manifest_hash: [u8; 32],  // Identity of code/weights
}
```

### Autonomy Modes

1. **Advisory (Tier 0)**: AI proposes only; all actions require human/governance approval
2. **Gated (Tier 1)**: AI executes pre-approved action types; novel actions go through approval gates with multisig/timelock
3. **Autonomous (Tier 2)**: Reserved for future; would require ZK-proof verification

All entities start in Advisory. Upgrades require governance proposals with 5000-block timelocks.

### AI Signals

AI entities emit advisory signals (AiSignalV1) with cryptographic commitments but no direct execution authority:

```rust
AiSignalV1 {
    signal_type: AiSignalType,    // Anomaly, Optimization, Prediction, RiskScore,
                                  // AuditReport, SpamRisk, CongestionForecast
    height: u64,                  // Block height when generated
    issuer: [u8; 32],             // AI entity ID
    confidence: u8,               // 0-255 confidence level
    payload_hash: [u8; 32],       // Off-chain payload (content-addressed)
    zk_proof: Option<Vec<u8>>,    // Optional ZK proof (max 64KB)
    signature: [u8; 64],          // Ed25519 signature
}
```

Signals are advisory only, rate-limited per entity, cryptographically signed, and extensible (7 signal types defined; values 7-255 reserved).

### Validator Co-Pilot

A statistics-based anomaly detector that monitors chain health and publishes signals:

| Metric | Threshold | Signal |
|--------|-----------|--------|
| Missed blocks | > 3x average | Anomaly |
| Vote delay | > 5x p95 | Anomaly |
| Peer churn | > 2x baseline | Anomaly |
| Mempool growth | > 3x normal | Congestion forecast |
| Transaction patterns | Spam indicators | Spam risk |

The co-pilot runs non-blocking in the background and uses only integer math for determinism.

## Privacy (NNPX)

NNPX (Nova Nota Private Exchange) provides privacy-preserving transactions with a hard guarantee: **AI entities NEVER have direct access to raw private data.**

**Storage Architecture:**
- Private data in separate RocksDB column family (`nnpx`)
- All `nnpx/` keys blocked for AI-initiated operations
- Public store contains only commitments and nullifiers

**Commitment and Nullifier Scheme:**
```
commitment_hash = blake3("NOVAI_NNPX_COMMITMENT_V1" || encrypted_payload)
nullifier = blake3("NOVAI_NNPX_NULLIFIER_V1" || spending_secret || counter)
```

**Enforcement Points:**
1. Entity registration: `read_nnpx_derived = false` for all AI entities
2. Execution: Storage operations check caller type; AI entities rejected for `nnpx/` keys
3. Nullifier validation: Duplicates rejected (prevents double-spend)

## Getting Started

### Prerequisites
- Rust 1.70+ (stable)
- RocksDB (auto-compiled via rocksdb-sys)
- Linux or macOS

### Build

```bash
# Build all crates
cargo build --workspace

# Build release binary
cargo build --release -p novai-node

# Run full test suite
cargo test --workspace

# Format and lint
cargo fmt
cargo clippy --all-targets

# Check license compliance
cargo deny check licenses
```

### Run Local Devnet

```bash
# Build the node
cargo build --release -p novai-node

# Start a 4-node local testnet
./scripts/devnet-4.sh
```

### Docker

```bash
docker build -t novai-node:latest .
./scripts/deploy-testnet.sh
```

## Project Status

**Completed:**
- Core consensus (HotStuff BFT with 3-chain commit rule)
- Deterministic execution engine with SMT state root
- Crash-safe persistence with restart recovery
- AI entity types as first-class protocol primitives
- AI signal types (7 categories) with domain-separated commitment hashing
- Validator Co-Pilot with anomaly detection
- Governance scaffolding with 5 proposal types and timelocks
- NNPX privacy foundation with RocksDB column family isolation
- Chaos testing framework (105+ tests: partitions, crashes, Byzantine faults)
- Performance testing infrastructure with tx-generator tool
- 1000+ tests passing across the workspace

**In Progress:**
- Testnet stabilization and operator tooling
- End-to-end AI entity lifecycle testing

## License

NOVAI is licensed under the **Apache License 2.0**. All dependencies must pass `cargo deny check licenses`. GPL/AGPL dependencies are forbidden.

## Documentation

| Document | Description |
|----------|-------------|
| `docs/CONSENSUS_V1.md` | HotStuff-inspired BFT specification |
| `docs/ARCHITECTURE_DECISIONS.md` | SMT design, encoding formats, consensus-critical parameters |
| `docs/AI_SIGNALS_V1.md` | Signal types, approval gates, attack model |
| `docs/AI_AUTONOMY_UPGRADE.md` | Tier progression and governance process |
| `docs/NNPX_PRIVACY_CONTRACT.md` | Storage isolation and AI access prohibition |
| `docs/MAINNET_SPEC.md` | Protocol limits and transaction signing |
| `docs/CLEANROOM_POLICY.md` | No copied consensus code, license gates |
| `docs/OPERATOR_RUNBOOK.md` | Node deployment and operations guide |
| `docs/VALIDATOR_KIT.md` | Validator setup and monitoring |
| `docs/DEVLOG.md` | Weekly development summaries |
