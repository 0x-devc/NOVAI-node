# NOVAI — Clean-Room L1 Blockchain with First-Class AI Entities

NOVAI is a clean-room Layer-1 blockchain designed from the ground up to support AI entities as native protocol primitives. Unlike traditional blockchains where AI capabilities are bolted on via smart contracts, NOVAI integrates AI entities into its consensus layer with explicit capabilities, economic agency, and governance-controlled autonomy modes.

## Key Features

- **First-Class AI Entities** — AI systems are protocol primitives with stable identities, persistent memory, economic balances, and capability manifests
- **Clean-Room BFT Consensus** — HotStuff-inspired 3-chain commit rule, exponential backoff timeouts, deterministic leader rotation
- **Deterministic Execution** — No floats, no nondeterministic iteration, all arithmetic checked
- **Canonical Encoding** — All protocol types use versioned, canonical encodings with golden vector tests
- **NNPX Privacy Layer** — Encrypted private data with cryptographic commitments; AI entities architecturally prohibited from accessing raw private data
- **Governance** — 5 proposal types (ParamChange, ModuleActivation, ModuleRollback, PolicyChange, EmergencyFreeze) with timelocked execution and AI autonomy upgrade gating
- **Validator Co-Pilot** — Statistics-based anomaly detection for chain health (advisory only, never affects consensus)
- **Sparse Merkle Tree** — Deterministic state authentication via 256-bit SMT with domain-separated hashing

## Architecture

```
novai-node (binary)
├── consensus        HotStuff BFT engine: proposal/vote/QC cycle, timeout view-change
├── consensus_types  Wire protocol: SignedProposal, Vote, QC, Timeout, canonical codecs
├── node             Validator node orchestration: consensus loop, block storage
├── execution        Deterministic state transition: transfers, nonce validation, SMT updates
├── smt              Sparse Merkle Tree: domain-separated hashing, proof generation
├── state            Storage abstraction: account state, SMT nodes, committed_height
├── p2p              TCP networking: Noise XX encryption, message broadcast, peer management
├── mempool          Transaction validation and pending pool management
├── codec            Type serialization: variable-length encoding, version prefixes
├── crypto           Ed25519 signatures, Blake3 hashing, address derivation
├── types            Protocol primitives: TxV1, Block, Address, Nonce
├── ai_entities      AI primitives: signals, approval gates, autonomy tiers, memory objects
├── ai_service       Anthropic Claude API sidecar for AI entity execution
├── copilot          Statistics-based anomaly detection: missed blocks, vote delays, peer churn
├── governance       Proposal types, lifecycle, timelocks, AI autonomy upgrade gates
└── genesis          Deterministic chain initialization
```

## Non-Negotiable Principles

- **Clean-room implementation** — No copied protocol code from Substrate, Tendermint, Diem, or any other blockchain
- **Deterministic execution** — Identical transaction execution across all nodes
- **License-safe dependency graph** — GPL/AGPL forbidden, enforced by `cargo deny`
- **Spec-first, test-first development** — All encodings golden-vector tested

See `docs/CLEANROOM_POLICY.md` for binding rules.

## Build

```bash
# Build all crates
cargo build --workspace

# Build release binary
cargo build --release -p novai-node

# Run full test suite (1000+ tests)
cargo test --workspace

# Format and lint
cargo fmt
cargo clippy --all-targets

# Check license compliance
cargo deny check licenses
```

## Run

```bash
# Generate a validator key
novai-node generate-key --output ~/.novai/validator.key

# Start a node with genesis
novai-node run \
    --port 9000 \
    --genesis testnet/genesis.json \
    --key-file ~/.novai/validator.key \
    --metrics-port 8080 \
    --storage rocksdb \
    --data-dir ~/.novai/data

# Start a 4-node local devnet (dev keys)
./scripts/devnet-4.sh
```

### CLI Reference

```
novai-node run
    --port <port>                  P2P listen port (required)
    --genesis <path>               Path to genesis JSON (required unless --dev-keys)
    --key-file <path>              Validator Ed25519 key file
    --peer <addr>                  Peer address (repeatable)
    --metrics-port <port>          Prometheus metrics port
    --base-timeout <ms>            Consensus timeout (default: 1000)
    --proposal-interval <ms>       Min ms between proposals (default: 100, min: 20)
    --storage <rocksdb|memory>     Storage backend (default: rocksdb)
    --data-dir <path>              RocksDB data directory
    --no-encryption                Disable Noise XX transport (testing only)

novai-node generate-key --output <path>
novai-node submit-tx <payload> [--nonce <n>] [--fee <n>]
novai-node drain-mempool <payload>... [--max <n>]
```

## Docker

```bash
docker build -t novai-node:latest .
./scripts/deploy-testnet.sh
```

## Documentation

| Document | Description |
|----------|-------------|
| `docs/OVERVIEW.md` | Comprehensive protocol overview |
| `docs/CONSENSUS_V1.md` | HotStuff-inspired BFT specification |
| `docs/ARCHITECTURE_DECISIONS.md` | SMT design, encoding formats, consensus parameters |
| `docs/AI_SIGNALS_V1.md` | Signal types, approval gates, attack model |
| `docs/AI_AUTONOMY_UPGRADE.md` | Tier progression and governance process |
| `docs/NNPX_PRIVACY_CONTRACT.md` | Storage isolation and AI access prohibition |
| `docs/MAINNET_SPEC.md` | Protocol limits and transaction signing |
| `docs/SECURITY_MODEL.md` | Threat model and security mechanisms |
| `docs/TUNING_PARAMETERS.md` | Node configuration and performance tuning |
| `docs/CLEANROOM_POLICY.md` | No copied consensus code, license gates |
| `docs/OPERATOR_RUNBOOK.md` | Node deployment and operations guide |
| `docs/VALIDATOR_KIT.md` | Validator setup and monitoring |

## License

Apache License 2.0. All dependencies must pass `cargo deny check licenses`.
