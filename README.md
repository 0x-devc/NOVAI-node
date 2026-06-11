# NOVAI - L1 Blockchain with First-Class AI Entities

NOVAI is a Layer-1 blockchain where AI entities are protocol primitives, not smart contracts.

Most blockchain projects that claim "AI integration" bolt AI onto an existing smart contract VM - the AI runs off-chain and pokes the chain through oracle calls or contract wrappers. NOVAI takes a different approach: AI entities exist at the same level as accounts and validators. They have on-chain identity, persistent memory, economic balance, capability flags, and governance-controlled autonomy modes, all enforced at the protocol layer.

There is no smart contract VM. There is no WASM runtime. Every transaction type is a native protocol operation. This is a deliberate design choice - it means you cannot deploy arbitrary code, but it also means the chain understands the semantics of every operation it executes.

The entire codebase is clean-room: no code copied or adapted from Substrate, Tendermint, Cosmos SDK, Diem, Aptos, Sui, or any other blockchain implementation. Concepts are drawn from published papers (HotStuff BFT, Sparse Merkle Trees), but every line is written from first principles.

## What is Currently Live

The private testnet has been running since early 2026. Current state:

- **BFT consensus** producing blocks continuously (16M+ blocks committed)
- **11 transaction types** fully executing with deterministic state transitions
- **AI entity registration** with on-chain identity, balance, and capabilities
- **AI memory objects** (persistent on-chain storage for AI entities)
- **AI signal commitments** (on-chain indexing of off-chain AI outputs)
- **Governance proposals** with timelocked execution and approval gates
- **Crash-safe persistence** with RocksDB and automatic node restart recovery
- **Timeout view-change** with exponential backoff for leader failure handling
- **Block pruning** (100K block retention) for bounded disk usage
- **Developer CLI** (`novai-cli`) for the full AI entity lifecycle

What is **not** live: smart contracts, dynamic code execution, the NNPX privacy layer (types defined, logic not active), and AI autonomous execution (requires governance gate work).

## Architecture

```
                           ┌───────────────────────────┐
                           │      novai-node binary     │
                           └─────────┬─────────────────┘
                                     │
              ┌──────────────────────┼──────────────────────┐
              │                      │                      │
     ┌────────▼────────┐   ┌────────▼────────┐   ┌────────▼────────┐
     │   P2P Layer      │   │   RPC Server    │   │   Metrics       │
     │   (libp2p/TCP)   │   │   (JSON-RPC)    │   │   (Prometheus)  │
     │   Port: 9000+    │   │   Port: 3030    │   │   Port: 8080    │
     └────────┬─────────┘   └────────┬────────┘   └─────────────────┘
              │                      │
              ▼                      ▼
     ┌─────────────────────────────────────────┐
     │           Consensus Engine               │
     │   HotStuff BFT, 3-chain commit rule,    │
     │   timeout view-change, leader rotation   │
     └────────────────┬────────────────────────┘
                      │
                      ▼
     ┌─────────────────────────────────────────┐
     │           Execution Engine               │
     │   Deterministic tx dispatch, nonce       │
     │   validation, fee enforcement, SMT       │
     └────────────────┬────────────────────────┘
                      │
        ┌─────────────┼─────────────┐
        │             │             │
   ┌────▼───┐   ┌────▼───┐   ┌────▼────┐
   │Accounts│   │   AI   │   │  State  │
   │Balances│   │Entities│   │   SMT   │
   │ Nonces │   │Memory  │   │  Root   │
   └────────┘   │Signals │   └─────────┘
                └────────┘
```

### Crate Map

| Crate | Purpose |
|-------|---------|
| `crates/node` | Validator binary: consensus loop, RPC server, metrics, peer management |
| `crates/consensus` | HotStuff BFT engine: proposal/vote/QC cycle, timeout view-change |
| `crates/consensus_types` | Wire protocol: `SignedProposal`, `Vote`, `QC`, `Timeout`, canonical codecs |
| `crates/execution` | Deterministic state transitions: all 11 tx types, fee schedule, SMT updates |
| `crates/smt` | 256-bit Sparse Merkle Tree with domain-separated hashing |
| `crates/state` | Storage abstraction: `Kv`/`KvBatch` traits, RocksDB + in-memory backends |
| `crates/mempool` | Transaction pool: signature verification, nonce ordering, size limits |
| `crates/p2p` | TCP networking: Noise XX encryption, message broadcast, peer discovery |
| `crates/codec` | Canonical binary encoding: versioned, golden-vector tested |
| `crates/crypto` | Ed25519 signatures, Blake3 hashing, domain-separated address derivation |
| `crates/types` | Protocol primitives: `TxV1`, `Block`, `Address`, constants |
| `crates/ai_entities` | AI types: entities, signals (23 types), memory objects (16 types), approval gates |
| `crates/governance` | Proposal lifecycle: 5 types, timelocks, AI autonomy upgrade gates |
| `crates/genesis` | Deterministic genesis state generation |
| `crates/copilot` | Validator advisory: statistics-based anomaly detection (non-binding) |
| `crates/ai_service` | Anthropic Claude API sidecar framework (not wired to consensus) |
| `tools/novai-cli` | Developer CLI: 17 commands for the full AI entity lifecycle |
| `tools/tx-generator` | Load testing tool: configurable TPS, retry logic, metrics |
| `tools/genesis-generator` | Deterministic genesis JSON + state root generation |

### Consensus

NOVAI uses a HotStuff-inspired BFT consensus protocol with a 3-chain commit rule:

1. **Propose**: Leader creates a block from mempool transactions
2. **Vote**: Validators verify the proposal and broadcast votes
3. **QC**: Once 2f+1 votes are collected, a Quorum Certificate is formed
4. **Commit**: After 3 consecutive QCs (the "3-chain"), the earliest block commits

Safety properties:
- Tolerates up to f Byzantine validators in a 3f+1 validator set
- Deterministic leader rotation based on `height % validator_count`
- Exponential backoff timeouts: `min(base_timeout * 2^round, 60s)`
- View change advances the round on leader failure without consensus split

### Execution

All transactions are deterministic. There are no floats, no `HashMap` iteration order dependencies, and all arithmetic is checked for overflow. Every transaction updates the Sparse Merkle Tree, producing a deterministic 32-byte state root that all honest validators agree on.

### Encoding

All protocol types use canonical binary encoding - one valid byte sequence per logical value. Encodings are versioned (type prefix byte) and locked by golden vector tests. Domain-separated hashing prevents cross-protocol attacks.

## Transaction Types

NOVAI has 11 native transaction types. Each is identified by the first byte of the transaction payload.

### Wire Format

Every transaction follows the `TxV1` structure:

```
[version:1][from:32][pubkey:32][nonce:8 LE][fee:8 LE][payload_len:4 LE][payload:N][sig:64]
```

- **version**: Always `0x01` (TxV1)
- **from**: Sender address = `blake3("NOVAI_ADDRESS_V1" || pubkey)`
- **pubkey**: Ed25519 public key (32 bytes)
- **nonce**: Monotonically increasing per sender (prevents replay)
- **fee**: Transaction fee in base units (minimum enforced per type)
- **payload**: Type-specific data (identified by first byte)
- **sig**: Ed25519 signature over `"NOVAI_TX_V1" || unsigned_bytes`

Total overhead: 149 bytes + payload. Max transaction size: 128 KB.

### Type 1: Transfer

Move tokens between accounts.

```
Payload: [0x01][to:32][amount:8 BE]
Size: 41 bytes
Min fee: 100
```

### Type 2: Signal Commitment

Publish an on-chain commitment to an AI signal (full payload stored off-chain).

```
Payload: [0x02][signal_hash:32][signal_type:1][issuer_entity_id:32]
Size: 66 bytes
Min fee: 1000
```

Signal types (0-22, 23 total): Anomaly, Optimization, Prediction, RiskScore, AuditReport, SpamRisk, CongestionForecast, and 16 additional types covering reputation, marketplace, staking, composition, proof submission, subscriptions, payments, SLAs, channels, and oracle anchors. See `crates/ai_entities/src/signals.rs` for the canonical enum.

### Type 3: Create Memory Object

Create a persistent on-chain storage object for an AI entity.

```
Payload: [0x03][object_type:1][data_len:4 BE][data:N]
Size: 6+ bytes
Min fee: 500
```

Memory object types (0-15, 16 total): ChainSummary, LabelIndex, EmbeddingCommitment, AnomalyLog, StatisticsSnapshot, and 11 additional types covering reputation events, ratings, signal catalogs, composition graphs, verification records, delegation grants, subscriptions, service descriptors, VK registrations, SLA agreements, and payment channels. See `crates/ai_entities/src/memory.rs` for the canonical enum. Max 64 KB per object, max 100 objects per entity.

### Type 4: Update Memory Object

Replace the data in an existing memory object.

```
Payload: [0x04][object_id:32][data_len:4 BE][new_data:N]
Size: 37+ bytes
Min fee: 500
```

### Type 5: Delete Memory Object

Remove a memory object.

```
Payload: [0x05][object_id:32]
Size: 33 bytes
Min fee: 500
```

### Type 6: Submit Governance Proposal

Submit a governance proposal for timelocked execution.

```
Payload: [0x06][proposal_type:1][gate_id:32][data_len:4 BE][proposal_data:N]
Size: 38+ bytes
Min fee: 2000
```

Proposal types: ParamChange, ModuleActivation, ModuleRollback, PolicyChange, EmergencyFreeze.

### Type 7: Execute Governance Proposal

Execute an approved proposal after its timelock has elapsed.

```
Payload: [0x07][proposal_id:32]
Size: 33 bytes
Min fee: 500
```

### Type 8: Register AI Entity

Create a new AI entity (without its own signing key).

```
Payload: [0x08][code_hash:32][autonomy_mode:1][capabilities:1][initial_balance:16 BE]
Size: 51 bytes
Min fee: 5000
```

Autonomy modes: Advisory (0) - can only emit proposals; Gated (1) - proposals go through approval gates. The entity ID is deterministically derived: `blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator_address)`.

Capabilities (8-bit bitfield, bits 0-6 defined, bit 7 reserved): `read_public_chain` (bit 0), `read_memory_objects` (bit 1), `emit_proposals` (bit 2), `request_execution` (bit 3), `read_nnpx_derived` (bit 4), `submit_reputation_updates` (bit 5, oracle entities only), `post_oracle_anchors` (bit 6).

### Type 9: Credit AI Entity

Transfer balance from your account to an AI entity.

```
Payload: [0x09][entity_id:32][amount:16 BE]
Size: 49 bytes
Min fee: 100
```

### Type 10: Register AI Entity with Key

Create a new AI entity with its own Ed25519 signing key, allowing it to sign its own transactions.

```
Payload: [0x0A][code_hash:32][pubkey:32][autonomy_mode:1][capabilities:1][initial_balance:16 BE]
Size: 83 bytes
Min fee: 5000
```

### Type 11: Entity Upgrade

In-place upgrade of an AI entity's code hash, capabilities, or autonomy mode. Payload version byte: `0x0B` (`ENTITY_UPGRADE_PAYLOAD_V1` in `crates/execution/src/lib.rs`). See source for the canonical encoding and fee.

## Getting Started (5 Minutes)

This guide takes you from zero to a registered AI entity with memory and signals on a local devnet.

### Prerequisites

- Rust 1.84+ (`rustup update`)
- A terminal

### Step 1: Build

```bash
git clone <repo-url> && cd NOVAI-node
cargo build --release -p novai-node -p novai-cli
```

### Step 2: Start a Local Devnet

```bash
# Start 4 validator nodes with deterministic dev keys
./scripts/devnet.sh

# Verify nodes are running (check logs)
tail -f /tmp/node0.log
```

The devnet starts 4 validators on ports 9000-9003 with RPC on port 3030 (node 0). Each validator has a pre-funded account with 1 billion tokens.

### Step 3: Generate Keys

```bash
# Generate your account key
./target/release/novai-cli keygen --output my.key

# Generate a key for your AI entity
./target/release/novai-cli keygen --output entity.key
```

### Step 4: Get Testnet Tokens

```bash
# Request tokens from the faucet (10M tokens)
./target/release/novai-cli faucet --address <your-address-from-step-3>
```

### Step 5: Register an AI Entity

```bash
# Register an AI entity with its own signing key
./target/release/novai-cli ai register-with-key \
  --key-file my.key \
  --entity-key-file entity.key \
  --code-hash 0000000000000000000000000000000000000000000000000000000000000001 \
  --autonomy advisory \
  --capabilities read_chain,read_memory,emit_proposals \
  --initial-balance 100000
```

The CLI prints the entity ID (deterministically computed). Save it.

### Step 6: Create a Memory Object

```bash
# The entity signs its own transactions using entity.key
./target/release/novai-cli memory create \
  --key-file entity.key \
  --type statistics-snapshot \
  --data "block_count:1000,avg_tps:50,timestamp:2026-04-03"
```

### Step 7: Publish a Signal

```bash
# Publish a prediction signal commitment
./target/release/novai-cli signal publish \
  --key-file entity.key \
  --signal-hash 0000000000000000000000000000000000000000000000000000000000000abc \
  --signal-type prediction \
  --issuer-entity-id <entity-id-from-step-5>
```

### Step 8: Verify

```bash
# Check entity state
./target/release/novai-cli ai info --entity-id <entity-id>

# List memory objects
./target/release/novai-cli memory list --entity-id <entity-id>

# Query signals
./target/release/novai-cli signal by-issuer \
  --issuer <entity-id> --start 0 --end 1000000
```

## Running a Local Devnet

### Quick Start

```bash
./scripts/devnet.sh
```

This starts 4 validators using deterministic dev keys (insecure, for testing only). Each node:
- Listens on P2P port 9000 + index
- Exposes metrics on port 8080 + index
- Exposes RPC on port 3030 (node 0 only by default)
- Uses RocksDB storage at `~/.novai/data/validator-<index>/`
- Has 100 pre-funded accounts (1B tokens each) for testing

### Manual Setup

```bash
# Build release binary
cargo build --release -p novai-node

# Start node 0 (seed node)
./target/release/novai-node run \
  --port 9000 \
  --dev-keys --allow-insecure-dev-keys --validator 0 &

# Start node 1 (connects to node 0)
./target/release/novai-node run \
  --port 9001 \
  --peer 127.0.0.1:9000 \
  --dev-keys --allow-insecure-dev-keys --validator 1 &

# Start nodes 2 and 3 similarly
./target/release/novai-node run \
  --port 9002 \
  --peer 127.0.0.1:9000 --peer 127.0.0.1:9001 \
  --dev-keys --allow-insecure-dev-keys --validator 2 &

./target/release/novai-node run \
  --port 9003 \
  --peer 127.0.0.1:9000 --peer 127.0.0.1:9001 --peer 127.0.0.1:9002 \
  --dev-keys --allow-insecure-dev-keys --validator 3 &
```

### Stopping

```bash
pkill -f 'novai-node run'
```

### Node Configuration

| Flag | Default | Description |
|------|---------|-------------|
| `--port` | (required) | P2P listen port |
| `--peer <addr>` | none | Peer to connect to (repeatable) |
| `--rpc-port` | 3030 | JSON-RPC server port |
| `--metrics-port` | 8080 | Prometheus metrics port |
| `--storage` | rocksdb | Storage backend (`rocksdb` or `memory`) |
| `--data-dir` | `~/.novai/data` | RocksDB data directory |
| `--base-timeout` | 1000 | Base consensus timeout in ms |
| `--proposal-interval` | 100 | Minimum ms between block proposals (>= 5) |
| `--genesis` | (required) | Path to genesis JSON (or use `--dev-keys`) |
| `--key-file` | auto | Path to 32-byte Ed25519 seed file |
| `--dev-keys` | off | Use deterministic keys (requires `--allow-insecure-dev-keys`) |
| `--no-encryption` | off | Disable Noise XX transport encryption |

### RPC Endpoints

All RPC calls use JSON-RPC 2.0 over HTTP POST. The node exposes 29 methods; the 9 most common are listed below. The full reference (every method with request/response shapes) lives in `docs/RPC_REFERENCE.md`.

| Method | Parameters | Description |
|--------|-----------|-------------|
| `novai_submitTransaction` | `{ "tx": "<hex>" }` | Submit a signed transaction |
| `novai_getNonce` | `{ "address": "<hex>" }` | Query expected nonce |
| `novai_getBalance` | `{ "address": "<hex>" }` | Query balance and nonce |
| `novai_getAiEntity` | `{ "entity_id": "<hex>" }` | Query AI entity state |
| `novai_getMemoryObjects` | `{ "entity_id": "<hex>" }` | List memory objects |
| `novai_getSignalsByHeight` | `{ "height": <u64> }` | Query signals at height |
| `novai_getSignalsByIssuer` | `{ "issuer": "<hex>", "start_height": <u64>, "end_height": <u64> }` | Query signals by issuer |
| `novai_getSignalsByType` | `{ "signal_type": <u8>, "start_height": <u64>, "end_height": <u64> }` | Query signals by type |
| `novai_faucet` | `{ "address": "<hex>" }` | Request testnet tokens (dev mode only) |
<!-- TODO: enumerate remaining 20 methods (payments, services, VK, SLA, channels, oracle anchors, blocks, transactions, etc.). Canonical list in crates/node/src/rpc.rs. -->


## Becoming a Validator on the Public Testnet

The public testnet is not yet launched. When it launches, the process will be:

### 1. Generate Your Validator Key

```bash
cargo build --release -p novai-node
./target/release/novai-node generate-key --output ~/.novai/validator.key
```

This creates a 32-byte Ed25519 seed file and prints your public key and address. The file is created with `0600` permissions (owner read/write only). Keep it safe.

### 2. Submit Your Validator Public Key

Your public key must be included in the genesis configuration before the network starts. During the testnet registration period, submit your hex-encoded public key through the registration process (details will be announced on [@NOVAInetwork](https://x.com/NOVAInetwork)).

### 3. Genesis Configuration

Your validator entry in genesis looks like:

```json
{
  "validators": [
    {
      "pubkey": "<your-64-char-hex-pubkey>",
      "initial_stake": "1000000",
      "name": "your-validator-name"
    }
  ]
}
```

The genesis state root is deterministically computed. All validators must start from the same genesis file.

### 4. Start Your Node

```bash
./target/release/novai-node run \
  --port 9090 \
  --genesis testnet/genesis.json \
  --key-file ~/.novai/validator.key \
  --peer <seed-node-address>:9090 \
  --metrics-port 8080 \
  --rpc-port 3030
```

### 5. Verify

Your node should:
- Connect to peers and begin receiving blocks
- Participate in consensus (propose blocks when you are the leader)
- Commit blocks (visible in logs and metrics)

Prometheus metrics are available at `http://localhost:8080/metrics` for monitoring block height, round number, peer count, and mempool size.

### Hardware Requirements

The testnet runs on modest hardware. A single VPS with 2 CPU cores, 4 GB RAM, and 50 GB SSD is sufficient. RocksDB write buffers are tuned to 16 MB with an 8 MB LRU cache, so memory usage is bounded.

### Current Limitations

- Validators cannot be added or removed after genesis (dynamic validator sets are planned)
- There is no staking or slashing mechanism yet
- All validators have equal weight in consensus

## Docker

```bash
# Build the image (multi-stage, ~50 MB final)
docker build -t novai-node:latest .

# Deploy a 5-validator testnet via Docker
./scripts/deploy-testnet.sh

# Deploy a single validator
./scripts/deploy-validator.sh --validator-id 0
```

## Build and Test

```bash
# Build all crates
cargo build --workspace

# Build release binaries
cargo build --release -p novai-node -p novai-cli -p tx-generator

# Run full test suite (1100+ tests)
cargo test --workspace

# Format and lint
cargo fmt --all
cargo clippy --workspace --all-targets

# License compliance check
cargo deny check licenses
```

## CLI Reference

```
novai-cli keygen --output <path>
novai-cli key-info --key-file <path>
novai-cli balance --address <hex>
novai-cli nonce --address <hex>
novai-cli faucet --address <hex>
novai-cli transfer --key-file <path> --to <hex> --amount <u64> [--fee <u64>]

novai-cli ai register --key-file <path> --code-hash <hex> --autonomy <advisory|gated> \
  --capabilities <flags> --initial-balance <u128> [--fee <u64>]
novai-cli ai register-with-key --key-file <path> --entity-key-file <path> \
  --code-hash <hex> --autonomy <advisory|gated> --capabilities <flags> \
  --initial-balance <u128> [--fee <u64>]
novai-cli ai credit --key-file <path> --entity-id <hex> --amount <u128> [--fee <u64>]
novai-cli ai info --entity-id <hex>

novai-cli memory create --key-file <path> --type <type> --data <string> [--fee <u64>]
novai-cli memory update --key-file <path> --object-id <hex> --data <string> [--fee <u64>]
novai-cli memory delete --key-file <path> --object-id <hex> [--fee <u64>]
novai-cli memory list --entity-id <hex>

novai-cli signal publish --key-file <path> --signal-hash <hex> --signal-type <type> \
  --issuer-entity-id <hex> [--fee <u64>]
novai-cli signal by-height --height <u64>
novai-cli signal by-issuer --issuer <hex> --start <u64> --end <u64>
novai-cli signal by-type --type <type> --start <u64> --end <u64>
```

Global flags: `--endpoint <url>` (default: `http://localhost:3030`), `--json` (JSON output).

## Documentation

| Document | Description |
|----------|-------------|
| `docs/CONSENSUS_V1.md` | HotStuff-inspired BFT specification |
| `docs/ARCHITECTURE_DECISIONS.md` | SMT design, encoding formats, consensus parameters |
| `docs/AI_SIGNALS_V1.md` | Signal types, approval gates, attack model |
| `docs/AI_AUTONOMY_UPGRADE.md` | Tier progression and governance process |
| `docs/NNPX_PRIVACY_CONTRACT.md` | Storage isolation and AI access prohibition |
| `docs/SECURITY_MODEL.md` | Threat model and security mechanisms |
| `docs/CLEANROOM_POLICY.md` | No copied consensus code, license gates |
| `docs/OPERATOR_RUNBOOK.md` | Node deployment and operations guide |
| `docs/DEVLOG.md` | Weekly development summaries |

## License

Apache License 2.0. All dependencies must pass `cargo deny check licenses` (GPL/AGPL forbidden).
