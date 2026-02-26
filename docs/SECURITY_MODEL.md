# NOVAI Security Model

## Threat Model

### Threats Addressed

1. **Byzantine validators** — Up to f faulty validators in a 3f+1 set
2. **Network-level adversaries** — Eavesdropping, message injection, replay attacks
3. **Malformed or malicious messages** — Oversized blocks, invalid signatures, duplicate votes
4. **AI entity overreach** — AI accessing private data or exceeding capability boundaries
5. **License contamination** — GPL/AGPL dependencies entering the clean-room codebase
6. **Resource exhaustion** — Connection flooding, mempool spam, oversized messages

### Non-Goals (Current Phase)

- Economic attacks and incentive design
- Slashing logic
- Sybil resistance at the network layer (validator set is permissioned)
- Side-channel attacks on cryptographic operations

---

## Transport Security

**Implementation**: `crates/p2p/src/noise.rs`

All peer-to-peer connections use the **Noise XX** handshake pattern with:

- **Key exchange**: X25519 (derived from Ed25519 validator keys via SHA-256)
- **Encryption**: ChaCha20-Poly1305 with independent send/receive nonces
- **Authentication**: Mutual — both sides prove they hold a validator key
- **Handshake timeout**: 10 seconds (prevents slow-loris attacks)
- **Nonce overflow**: `checked_add()` with panic — connections terminate before nonce reuse
- **Message framing**: `[chunk_len: u16 BE][ciphertext]`, max 65535 bytes per chunk; large messages are automatically chunked
- **Remote key verification**: After handshake, the remote public key is checked against the known validator set

**Plaintext mode** (`--no-encryption`) exists for local testing only and logs a warning at startup.

---

## Connection Limiting

**Implementation**: `crates/p2p/src/lib.rs`

- **Per-IP limit**: `MAX_CONNECTIONS_PER_IP = 3` — prevents a single IP from exhausting resources
- **Per-peer message rate**: `MAX_MESSAGES_PER_SECOND = 100` — rate-limited at the transport layer
- **Socket timeout**: `PEER_SOCKET_TIMEOUT_SECS = 30` — dead connections are reaped
- **Max wire message**: `MAX_WIRE_MSG_BYTES = 2 MB` — oversized messages rejected before parsing
- **TCP_NODELAY**: Enabled on both incoming and outgoing connections for low-latency consensus messaging
- **RAII guards**: `ConnectionLimiter::try_acquire()` returns a guard that auto-releases the connection slot on drop

---

## Cryptographic Signatures

**Implementation**: `crates/crypto/src/lib.rs`, `crates/consensus/src/lib.rs`

### Ed25519 with Strict Verification

All signature verification uses `verify_strict()` (not `verify()`), which enforces canonical signature encoding and rejects malleable signatures.

### Domain-Separated Signing

Every message type uses a unique domain tag to prevent cross-context signature reuse:

| Message Type | Domain Tag | Implementation |
|-------------|------------|----------------|
| Transaction | `NOVAI_TX_V1` | `crates/crypto/src/lib.rs` |
| Vote | `NOVAI_VOTE_V1` | `crates/consensus/src/lib.rs` |
| Timeout | `NOVAI_TIMEOUT_V1` | `crates/consensus/src/lib.rs` |
| Proposal | `NOVAI_PROPOSAL_V1` | Specified in `docs/CONSENSUS_V1.md` |
| Address | `NOVAI_ADDRESS_V1` | `crates/crypto/src/lib.rs` |
| AI Entity ID | `NOVAI_AI_ENTITY_ID_V1` | `crates/ai_entities/src/lib.rs` |
| NNPX Commitment | `NOVAI_NNPX_COMMITMENT_V1` | `crates/execution/src/lib.rs` |
| NNPX Nullifier | `NOVAI_NNPX_NULLIFIER_V1` | `crates/execution/src/lib.rs` |

**Signed bytes format**: `domain_tag || canonical_unsigned_encoding(message)`

---

## Equivocation Detection

**Implementation**: `crates/consensus/src/lib.rs`

- **Vote equivocation**: Tracked via per-round `voted_in_round` HashSet. A second vote from the same validator in the same round is rejected with an error.
- **Timeout equivocation**: Tracked via `pending_timeouts` HashMap keyed by `(height, round)`. Duplicate timeouts from the same voter are rejected.
- **Fork detection**: `check_for_conflicting_commits()` validates no two blocks are committed at the same height.

---

## Message Size Enforcement

**Implementation**: `crates/types/src/lib.rs`, `crates/consensus/src/lib.rs`

Three-layer size enforcement prevents resource exhaustion:

| Limit | Value | Enforcement Point |
|-------|-------|-------------------|
| `MAX_TX_SIZE` | 128 KB | Per-transaction in block verification |
| `MAX_TXS_PER_BLOCK` | 500 | Block transaction count check |
| `MAX_BLOCK_SIZE` | 2 MB | Aggregate block payload check |
| `MAX_MEMPOOL_BYTES` | 64 MB | Mempool admission |
| `MAX_WIRE_MSG_BYTES` | 2 MB | Network layer, before parsing |

Block verification (`verify_block()`) enforces all three layers: transaction count, individual transaction size, and total block size.

---

## NNPX Privacy Enforcement

**Implementation**: `crates/execution/src/lib.rs`

NNPX (Nova Nota Private Exchange) enforces a hard architectural guarantee: **AI entities never access raw private data.**

### Enforcement Points

1. **Entity registration**: AI entities cannot register with `read_nnpx_derived: true` capability
2. **Storage operations**: All reads/writes to `nnpx/` key prefix check the caller type; AI entities are rejected with `NnpxAccessDenied`
3. **Derived views**: AI entities may only access the `derived_views/` namespace (aggregate data), never raw `nnpx/` data
4. **Nullifier validation**: Duplicate nullifiers are rejected (prevents double-spend)

### Protected Namespaces

- `nnpx/commitments/` — Private commitments
- `nnpx/nullifiers/` — Nullifier set
- `nnpx/encrypted/` — Encrypted payloads
- `nnpx/proofs/` — Zero-knowledge proofs
- `nnpx/metadata/` — Transaction metadata
- `nnpx/indices/` — Private indices

---

## Consensus Safety

**Implementation**: `crates/consensus/src/lib.rs`

- **Quorum intersection**: Any two quorums (2f+1 out of 3f+1) intersect in at least one honest node
- **Single vote per round**: Honest validators vote at most once per `(height, round)`
- **3-chain commit rule**: A block at height h is committed only when QC at h+2 is observed
- **Deterministic leader rotation**: `leader(height, round) = validators[(height + round) % n]`
- **Exponential backoff timeouts**: `timeout(r) = min(BASE_TIMEOUT_MS * 2^r, MAX_TIMEOUT_MS)` — prevents livelock without compromising safety
- **Crash-safe persistence**: Committed height and state are persisted atomically via RocksDB write batches

---

## License Enforcement

**Implementation**: `deny.toml`

All dependencies are gated by `cargo deny check licenses`:

**Allowed licenses**: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, CDLA-Permissive-2.0

**Denied**: GPL, AGPL, LGPL, and all other copyleft licenses

**Additional restrictions**:
- Wildcard dependencies: denied
- Unknown registries: denied
- Unknown git sources: denied
- Security advisories: monitored with justified exceptions documented in `deny.toml`

---

## Determinism Guarantees

Consensus-critical code enforces strict determinism:

- **No floating point** — All arithmetic uses integer types with checked operations
- **Canonical encoding** — One valid encoding per logical value, golden-vector tested
- **Deterministic iteration** — No HashMap iteration in consensus-critical paths; sorted collections used where order matters
- **Domain-separated hashing** — Blake3 with unique prefixes prevents cross-context collisions
- **SMT state root** — Sparse Merkle Tree provides deterministic state authentication across all nodes

---

## Key Management

- Validator keys are Ed25519 keypairs stored in PEM files (`--key-file` flag)
- Keys should be generated via `novai-node generate-key --output <path>`
- Key files should be readable only by the node process (`chmod 600`)
- Dev keys (`--dev-keys`) are deterministic and known to everyone — never use in production
- The `--allow-insecure-dev-keys` flag is required as explicit acknowledgment of the risk

---

## Test Coverage

Security mechanisms are validated by 1000+ tests including:

- **Chaos tests** (105+): Network partitions, node crashes, Byzantine fault injection
- **Adversarial tests**: Proposal spam, timelock bypass attempts, equivocation scenarios
- **NNPX boundary tests**: Exhaustive prefix testing, enumeration attacks, host API blocking
- **Golden vector tests**: Locked canonical encodings for all message types
- **Connection limit tests**: Per-IP enforcement, rate limiting, guard lifecycle
