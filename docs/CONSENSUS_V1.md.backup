# CONSENSUS_V1 — HotStuff-Like BFT Consensus Specification

## Overview
This document defines the consensus protocol for NOVAI v1. The protocol is inspired by HotStuff but implemented clean-room from first principles. It provides Byzantine Fault Tolerant (BFT) consensus with deterministic finality.

## Version
- **Protocol Version:** 1
- **Status:** Week 5 implementation (message types + leader schedule)

## Assumptions

### Network Model
- Partial synchrony: messages eventually delivered within bounded time Δ (unknown)
- Authenticated channels: all messages are signed and verified
- No network partitions lasting indefinitely

### Validator Set
- Static validator set for V1 (dynamic set deferred to future weeks)
- Known validator public keys at genesis
- Minimum 4 validators required (f < n/3 where f is max Byzantine nodes)

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
    height: u64,
    round: u64,
    parent_hash: [u8; 32],
    state_root: [u8; 32],
    txs: Vec<TxV1>,
}
```

### Proposal
```
Proposal {
    block: Block,
    justify_qc: QC,  // QC for parent block
}
```

### Vote
```
Vote {
    height: u64,
    round: u64,
    block_hash: [u8; 32],
    voter: Address,  // [u8; 32]
    signature: [u8; 64],
}
```

### Quorum Certificate (QC)
```
QC {
    height: u64,
    round: u64,
    block_hash: [u8; 32],
    votes: Vec<Vote>,  // Must contain 2f+1 valid votes
}
```

### Timeout (view-change)
```
Timeout {
    height: u64,
    round: u64,
    voter: Address,
    highest_qc: Option<QC>,  // Highest QC seen by voter
    signature: [u8; 64],
}
```

## Message Validity Rules

### Proposal Validity
A Proposal is valid if:
1. `block.height > 0`
2. `block.round >= 0`
3. `justify_qc.height == block.height - 1` (or height 0 has no justify)
4. `justify_qc.block_hash == block.parent_hash`
5. `justify_qc` is valid (see QC validity)
6. `sender == leader(block.height, block.round)`
7. State transition is valid: `execute(parent_state, block.txs) == block.state_root`

### Vote Validity
A Vote is valid if:
1. `height > 0`
2. `voter` is in validator set
3. Signature verifies: `verify(voter_pubkey, vote_bytes, signature)`
4. Vote bytes are canonical unsigned encoding

### QC Validity
A QC is valid if:
1. Contains at least `2f+1` votes
2. All votes have matching `(height, round, block_hash)`
3. All votes are individually valid
4. No duplicate voters

### Timeout Validity
A Timeout is valid if:
1. `height > 0`
2. `voter` is in validator set
3. Signature verifies
4. `highest_qc` (if present) is valid

## Consensus Rules

### Commit Rule
A block at height `h` is **committed** when:
1. A QC exists for height `h`
2. A QC exists for height `h+1` (direct child)
3. The h+1 QC's block has `parent_hash == hash(h_block)`

**Rationale:** Two-chain commit rule (simplified from HotStuff's three-chain).

### Vote Rule (Honest Validator)
An honest validator votes for a proposal if:
1. Proposal is valid
2. Validator has not voted for a different block at `(height, round)`
3. `justify_qc.height >= highest_qc_height_seen`

### Timeout Rule
A validator issues a timeout for `(height, round)` if:
1. Timer expires without seeing a valid proposal
2. Validator has not seen a QC for `(height, round)`

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
- Validators are sorted by `Address` (lexicographic byte order)
- Sorting is canonical and deterministic

## Safety Proof Sketch

**Theorem:** If two honest nodes commit blocks B and B' at height h, then B == B'.

**Proof:**
1. Assume B ≠ B' both committed at height h
2. By commit rule: QC_h exists for B, QC_{h+1} exists for child of B
3. Similarly: QC_h' exists for B', QC_{h+1}' exists for child of B'
4. QC_h and QC_h' both require 2f+1 votes at (h, round_B) and (h, round_B')
5. If round_B == round_B': quorums intersect → honest node voted twice (contradiction)
6. If round_B ≠ round_B': WLOG round_B < round_B'
7. QC_h formed before QC_h' → honest voters at round_B' saw QC_h via justify
8. Honest voters only vote if justify_qc.height >= highest_qc_height_seen
9. QC_h proves block at h → honest voters won't vote for conflicting B' (contradiction)

## Liveness Dependencies

### Timeouts
- Initial timeout: 2 seconds
- Exponential backoff: `timeout(r) = 2^r seconds` (capped at 60s)
- Reset on progress (new QC formed)

### Synchrony Requirement
After GST:
- Messages delivered within Δ
- Honest leader can collect 2f+1 votes → form QC
- Progress guaranteed within O(Δ) rounds

## Implementation Notes (Week 5 Scope)

### This Week
- Define message structs
- Implement canonical encoding
- Create golden vectors
- Implement deterministic leader schedule
- **No** actual consensus logic (voting/QC formation)

### Future Weeks
- Week 6-8: Consensus engine (voting, QC aggregation, commit detection)
- Week 9+: View-change, timeout handling, recovery

## Open Questions (To Be Resolved)

1. **Block size limits?** (Week 5: unbounded, add limits in Week 6)
2. **Vote aggregation scheme?** (Week 5: simple list, optimize later)
3. **Timeout certificate (TC) needed?** (Defer to Week 7)

## References
- HotStuff paper (conceptual inspiration only, no code reuse)
- PBFT (safety properties)
- BFT-SMaRt (deterministic leader rotation)

---
**Document Status:** Living document, updated as protocol evolves.
**Last Updated:** Week 5 implementation start.
