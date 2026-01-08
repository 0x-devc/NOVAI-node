# CONSENSUS_V1 — HotStuff-Like BFT Consensus Specification

## Overview
This document defines the consensus protocol for NOVAI v1. The protocol is inspired by HotStuff but implemented clean-room from first principles. It provides Byzantine Fault Tolerant (BFT) consensus with deterministic finality.

## Version
- **Protocol Version:** 1
- **Status:** Week 5 implementation (message types + leader schedule)

## Normative Language
This specification uses RFC 2119 keywords:
- **MUST** / **SHALL** = absolute requirement
- **MUST NOT** / **SHALL NOT** = absolute prohibition
- **SHOULD** / **SHOULD NOT** = recommended but not required
- **MAY** = optional

Sections marked **(Informative)** are explanatory only. All other sections are normative.

## Assumptions

### Network Model
- Partial synchrony: messages eventually delivered within bounded time Δ (unknown)
- Authenticated channels: all messages MUST be signed and verified
- No network partitions lasting indefinitely

### Validator Set
- Static validator set for V1 (dynamic set deferred to future weeks)
- Known validator public keys at genesis
- **MUST have at least 4 validators** (n >= 4)
- **MUST satisfy n = 3f + 1** where f is max Byzantine nodes
- Validators MUST be sorted by Address (lexicographic byte order) for canonical ordering

### Cryptographic Assumptions
- Ed25519 signatures are unforgeable
- Blake3 hashes are collision-resistant
- Private keys are not compromised

## Safety and Liveness Guarantees

### Safety (Never violated)
**Property:** Two honest nodes never commit conflicting blocks at the same height.

**Mechanism:** Quorum intersection
- Any two quorums (2f+1 out of 3f+1 validators) intersect in at least one honest node
- Honest nodes vote at most once per (height, round)
- QC at (h, r) proves 2f+1 votes exist → blocks conflicting QC for same (h, r)

### Liveness (Eventually satisfied under partial synchrony)
**Property:** The protocol eventually makes progress and commits new blocks.

**Mechanism:**
- View-change (timeout) allows recovery from faulty leaders
- Deterministic leader schedule prevents deadlock
- After GST (Global Stabilization Time), synchrony allows QC formation

## Core Data Structures

### Block
```
Block {
    height: u64,          // Block height (genesis = 0)
    round: u64,           // Consensus round
    parent_hash: [u8; 32], // Hash of parent block
    state_root: [u8; 32],  // SMT root after executing txs
    txs: Vec<TxV1>,       // Transactions (MUST be <= 10000)
}
```

**Constraints:**
- `txs.len()` MUST be <= 10000 (prevent unbounded messages)

### Proposal
```
Proposal {
    block: Block,
    justify_qc: QC,  // QC for parent block (or genesis QC for height 1)
}
```

### Vote
```
Vote {
    height: u64,
    round: u64,
    block_hash: [u8; 32],
    voter: Address,        // [u8; 32]
    signature: [u8; 64],   // Ed25519 signature over unsigned bytes
}
```

**Signing Rule:**
- Vote signature MUST be computed over: `encode_vote_v1_unsigned(vote)`
- Domain separation tag: `b"NOVAI_VOTE_V1"`
- Full signed bytes: `domain_tag || encode_vote_v1_unsigned(vote)`

### Quorum Certificate (QC)
```
QC {
    height: u64,
    round: u64,
    block_hash: [u8; 32],
    votes: Vec<Vote>,  // MUST contain exactly 2f+1 valid votes
}
```

**Canonical Ordering Requirements:**
- `votes` MUST be sorted by `voter` (Address lexicographic order)
- `votes` MUST NOT contain duplicate voters
- All votes MUST have matching `(height, round, block_hash)`

**Constraints:**
- `votes.len()` MUST equal quorum threshold (2f+1)
- `votes.len()` MUST be <= 10000

### Timeout (view-change)
```
Timeout {
    height: u64,
    round: u64,
    voter: Address,
    highest_qc: Option<QC>,  // Highest QC seen by voter
    signature: [u8; 64],      // Ed25519 signature over unsigned bytes
}
```

**Signing Rule:**
- Timeout signature MUST be computed over: `encode_timeout_v1_unsigned(timeout)`
- Domain separation tag: `b"NOVAI_TIMEOUT_V1"`
- Full signed bytes: `domain_tag || encode_timeout_v1_unsigned(timeout)`

## Canonical Encoding Rules

### Vector Encoding
All `Vec<T>` fields MUST be encoded as:
```
[count:u32][item_1][item_2]...[item_n]
```
- `count` is big-endian u32
- Items encoded consecutively with no separators
- For variable-length items, each item includes its own length prefix

### Vote Ordering in QC
Before encoding a QC:
1. Votes MUST be sorted by `voter` (lexicographic byte order)
2. Duplicate voters MUST be rejected (encoding returns error)
3. All votes MUST have matching `(height, round, block_hash)`

This ensures: **same logical QC → same encoded bytes → same hash**

## Message Validity Rules

### Proposal Validity
A Proposal is valid if:
1. `block.height > 0` (genesis block not proposed)
2. `block.round >= 0`
3. `block.txs.len() <= 10000`
4. If `block.height > 1`: `justify_qc.height == block.height - 1`
5. If `block.height > 1`: `justify_qc.block_hash == block.parent_hash`
6. `justify_qc` is valid (see QC validity)
7. `sender == leader(block.height, block.round)`
8. State transition is valid: `execute(parent_state, block.txs) == block.state_root`

### Vote Validity
A Vote is valid if:
1. `height > 0`
2. `voter` is in validator set
3. Signature verifies: `verify(voter_pubkey, signed_bytes, signature)`
   - Where `signed_bytes = b"NOVAI_VOTE_V1" || encode_vote_v1_unsigned(vote)`
4. Vote has not been seen before for this `(height, round)` from this `voter`

### QC Validity
A QC is valid if:
1. `votes.len() == quorum_threshold` (exactly 2f+1)
2. `votes.len() <= 10000`
3. All votes have matching `(height, round, block_hash)` with QC fields
4. All votes are individually valid
5. No duplicate voters (each voter appears exactly once)
6. Votes are sorted by `voter` (canonical ordering)

### Timeout Validity
A Timeout is valid if:
1. `height > 0`
2. `voter` is in validator set
3. Signature verifies: `verify(voter_pubkey, signed_bytes, signature)`
   - Where `signed_bytes = b"NOVAI_TIMEOUT_V1" || encode_timeout_v1_unsigned(timeout)`
4. If `highest_qc` is Some: the QC is valid

## Consensus Rules

### Commit Rule (Three-Block Chain)
A block B at height `h` is **committed** when there exists a three-block chain:
1. Block B at height `h` with QC_h
2. Block B' at height `h+1` with QC_{h+1}, where:
   - `B'.parent_hash == hash(B)`
   - QC_h justifies B
3. Block B'' at height `h+2` with QC_{h+2}, where:
   - `B''.parent_hash == hash(B')`
   - QC_{h+1} justifies B'

**Visual:**
```
B (h) --QC_h--> B' (h+1) --QC_{h+1}--> B'' (h+2)
^
|
Committed when B'' is proposed with valid justify QC_{h+1}
```

**Rationale:** Three-chain rule ensures safety via locked QCs.

*Note: This is a simplified form. Full HotStuff has additional locking rules.*

### Vote Rule (Honest Validator)
An honest validator votes for a proposal if:
1. Proposal is valid (see Proposal Validity)
2. Validator has not voted for a different block at `(height, round)`
3. `proposal.justify_qc.height >= highest_qc_height_seen`
4. Safety rule: if validator is locked on a block at height `h`, it only votes for descendants of that block

### Timeout Rule
A validator issues a timeout for `(height, round)` if:
1. Timer expires without seeing a valid proposal
2. Validator has not seen a QC for `(height, round)`
3. Include `highest_qc` seen so far (or None if no QC seen yet)

## Leader Schedule

### Deterministic Leader Selection
```
leader(height, round) = validators[(height + round) % n]
```

**Properties:**
- Deterministic: all nodes compute same leader
- Round-robin with height offset
- No single point of failure
- Predictable (can be precomputed)

**Validator Ordering:**
- Validators MUST be sorted by `Address` (lexicographic byte order)
- Sorting is canonical and deterministic across all nodes

### Validator Set Constraints
A validator set is valid if:
- `n >= 4` (minimum for f=1 Byzantine tolerance)
- `n = 3f + 1` for some integer f >= 1
- All validator addresses are unique

**Quorum Threshold:**
```
quorum = 2f + 1 = 2 * ((n - 1) / 3) + 1
```

## Safety Proof Sketch (Informative)

**Theorem:** If two honest nodes commit blocks B and B' at height h, then B == B'.

**Proof:**
1. Assume B ≠ B' both committed at height h
2. By commit rule: Three-chain exists for B, three-chain exists for B'
3. Both chains require QCs with 2f+1 votes at height h
4. Quorums intersect: at least one honest node voted for both B and B'
5. But honest nodes vote at most once per (height, round)
6. Contradiction → B must equal B'

## Liveness Dependencies (Informative)

### Timeouts
- Initial timeout: 2 seconds
- Exponential backoff: `timeout(r) = min(2^r seconds, 60s)`
- Reset on progress (new QC formed)

### Synchrony Requirement
After GST (Global Stabilization Time):
- Messages delivered within Δ
- Honest leader can collect 2f+1 votes → form QC
- Progress guaranteed within O(Δ) rounds

## Implementation Notes (Week 5 Scope)

### This Week (Week 5)
- Define message structs
- Implement canonical encoding with Result<_, Error> returns
- Create golden vectors (codec stability + validity fixtures)
- Implement deterministic leader schedule with validator constraints
- **No** actual consensus logic (voting/QC formation)

### Future Weeks
- Week 6-8: Consensus engine (voting, QC aggregation, commit detection, locking)
- Week 9+: View-change, timeout handling, recovery

## Size Limits

To prevent DoS attacks and ensure bounded message sizes:

| Field | Limit | Rationale |
|-------|-------|-----------|
| `Block.txs.len()` | 10,000 | Reasonable block size |
| `QC.votes.len()` | 10,000 | Supports up to ~15,000 validators |
| Individual tx size | 10 KB | Enforced by tx validation |

## Open Questions (To Be Resolved)

1. **Vote aggregation optimization?** (Week 5: simple list, optimize later with BLS or threshold sigs)
2. **Timeout certificate (TC) needed?** (Defer to Week 7)
3. **Pipelining depth?** (Week 5: undefined, Week 6+: decide based on latency)

## References (Informative)
- HotStuff paper (conceptual inspiration only, no code reuse)
- PBFT (safety properties)
- BFT-SMaRt (deterministic leader rotation)

---
**Document Status:** Living document, updated as protocol evolves.  
**Last Updated:** Week 5 - post-feedback hardening.
