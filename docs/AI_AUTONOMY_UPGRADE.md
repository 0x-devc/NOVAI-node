# AI Autonomy Upgrade Path

This document describes the governance process for upgrading AI entity autonomy levels in the NOVAI protocol.

## Overview

### Autonomy Tiers

AI entities in NOVAI operate under one of three autonomy tiers, each with different levels of independence:

| Tier | Name | Description | Human Oversight |
|------|------|-------------|-----------------|
| 0 | **Advisory** | AI provides recommendations only; all actions require human approval | Full oversight |
| 1 | **Gated** | AI can execute pre-approved action types; novel actions require approval | Partial oversight |
| 2 | **Autonomous** | AI operates independently within defined policy bounds | Audit-only |

### Tier Progression

```
Advisory (Tier 0)
    │
    ▼ [PolicyChange or ModuleActivation proposal]
    │ [5000 block timelock]
    │
Gated (Tier 1)
    │
    ▼ [PolicyChange proposal]
    │ [5000 block timelock]
    │
Autonomous (Tier 2)
```

Upgrades require governance proposals with extended timelocks. Downgrades can be immediate via `EmergencyFreeze`.

## Upgrade Path: Advisory → Gated

### Requirements

1. **Proposal Type**: `PolicyChange` (for policy updates) or `ModuleActivation` (for new AI modules)
2. **Gate Approvals**: Must reach threshold defined by the controlling gate
3. **Timelock**: 5000 blocks (~13.9 hours at 10s block time) - high-risk category

### Governance Types Reference

From `novai-governance` crate:

```rust
// Proposal types (crates/governance/src/lib.rs)
pub enum ProposalType {
    ParamChange = 0,        // Standard timelock (1000 blocks)
    ModuleActivation = 1,   // High-risk timelock (5000 blocks)
    ModuleRollback = 2,     // Standard timelock (1000 blocks)
    PolicyChange = 3,       // High-risk timelock (5000 blocks)
    EmergencyFreeze = 4,    // Emergency timelock (100 blocks)
}

// Timelock configuration (crates/governance/src/lib.rs)
pub struct GovernanceConfig {
    pub default_timelock_blocks: u64,      // 1000 blocks
    pub high_risk_timelock_blocks: u64,    // 5000 blocks
    pub emergency_timelock_blocks: u64,    // 100 blocks
    pub default_expiry_blocks: u64,        // 50000 blocks
}
```

### Proposal Data Format

For autonomy upgrades, the `proposal_data` field should contain:

```
[action:1][entity_id:32][new_tier:1][policy_hash:32]
```

| Field | Size | Description |
|-------|------|-------------|
| `action` | 1 byte | `0x01` = upgrade tier, `0x02` = set policy |
| `entity_id` | 32 bytes | Target AI entity identifier |
| `new_tier` | 1 byte | New autonomy tier (0, 1, or 2) |
| `policy_hash` | 32 bytes | Blake3 hash of the new policy document |

**Example**: Upgrade entity `0xABCD...` to Gated (tier 1):

```
01                              // action: upgrade tier
ABCD...ABCD                     // entity_id (32 bytes)
01                              // new_tier: Gated
1234...5678                     // policy_hash (32 bytes)
```

## Proposal Workflow

### Step 1: Submit Proposal Transaction

Create a proposal targeting the appropriate gate:

```rust
use novai_governance::{Proposal, ProposalType};

let proposal = Proposal::new(
    ProposalType::PolicyChange,     // High-risk, 5000 block timelock
    proposal_data,                  // Encoded upgrade parameters
    proposer_address,               // Who is proposing
    gate_id,                        // Gate that must approve
    current_height,                 // Block height at submission
    50000,                          // Expiry: 50000 blocks
);

// Proposal ID is deterministically computed
let proposal_id = proposal.id;
```

**State after submission**: `Submitted`

**Storage key**: `governance/proposals/{proposal_id}`

### Step 2: Collect Gate Approvals

Gate members submit approval transactions:

```rust
// Each approval is recorded
proposal.add_approval(approver_address);

// Check if threshold reached
if proposal.has_threshold(gate.threshold) {
    // Ready for approval transition
}
```

**Approval events are logged**:
```rust
let log_entry = AuditLogEntry::approved(
    proposal_id,
    current_height,
    final_approver,
);
// Stored at: governance/log/{proposal_id}
```

### Step 3: Approve and Start Timelock

Once threshold is reached, transition to `Approved` state:

```rust
// Timelock calculated based on proposal type
let timelock = config.timelock_for_proposal_type(proposal.proposal_type);
// For PolicyChange: 5000 blocks

proposal.approve(current_height, timelock)?;

// executable_at = current_height + 5000
```

**State after approval**: `Approved`

### Step 4: Wait for Timelock

The proposal cannot be executed until `current_height >= executable_at`.

```rust
// Check executability
if proposal.can_execute_at(current_height) {
    // Timelock elapsed, not expired
    proposal.make_executable(current_height)?;
}
```

**State after timelock**: `Executable`

### Step 5: Execute Proposal

Execute the autonomy upgrade:

```rust
proposal.execute(current_height)?;

// Log execution
let log_entry = AuditLogEntry::executed(
    proposal_id,
    current_height,
    executor_address,
);
```

**State after execution**: `Executed` (terminal)

### State Machine Summary

```
Submitted ──[threshold reached]──► Approved ──[timelock elapsed]──► Executable ──[execute]──► Executed
    │                                  │                                 │
    ▼                                  ▼                                 ▼
 Rejected                           Rejected                          Expired
```

## Rollback Procedures

### Emergency Freeze

For immediate AI halt when safety concerns arise:

```rust
let proposal = Proposal::new(
    ProposalType::EmergencyFreeze,  // 100 block timelock only
    freeze_data,                     // Target entity/module
    proposer_address,
    emergency_gate_id,               // Dedicated emergency gate
    current_height,
    10000,                           // Shorter expiry for emergencies
);
```

**Characteristics**:
- Shortest timelock: 100 blocks (~16.7 minutes)
- Limited scope: can only freeze, not modify
- Requires dedicated emergency gate approval
- Immediately halts AI execution upon execution

### Module Rollback

To revert to a previous AI module version:

```rust
let proposal = Proposal::new(
    ProposalType::ModuleRollback,   // Standard 1000 block timelock
    rollback_data,                   // Previous version reference
    proposer_address,
    gate_id,
    current_height,
    50000,
);
```

**Rollback data format**:
```
[entity_id:32][target_version:8][rollback_reason:variable]
```

| Field | Size | Description |
|-------|------|-------------|
| `entity_id` | 32 bytes | AI entity to rollback |
| `target_version` | 8 bytes | Version number to restore (big-endian u64) |
| `rollback_reason` | variable | Human-readable reason (UTF-8) |

## Security Considerations

### Why Longer Timelocks for Autonomy Upgrades

1. **Review Period**: 5000 blocks (~13.9 hours) provides time for:
   - Community review of the proposed change
   - Security audit of new policies
   - Detection of malicious proposals

2. **Attack Mitigation**: Prevents rapid escalation attacks where compromised keys could immediately grant AI full autonomy

3. **Coordination Time**: Allows validators and stakeholders to coordinate response if proposal is concerning

### Multi-Sig Gate Requirements

Gates controlling autonomy upgrades should have:

- **Threshold**: At least 2/3 of members (e.g., 4-of-6)
- **Diversity**: Members from different organizations/jurisdictions
- **Key Security**: Hardware security modules for signing keys
- **Rotation**: Regular key rotation with governance proposals

Example gate configuration:
```rust
use novai_ai_entities::ApprovalGate;

let autonomy_gate = ApprovalGate {
    gate_id: blake3::hash(b"AUTONOMY_UPGRADE_GATE_V1").into(),
    members: vec![member1, member2, member3, member4, member5, member6],
    threshold: 4,  // 4-of-6 required
    timelock_blocks: 5000,
    expiry_blocks: 50000,
};
```

### Audit Log Queryability

All governance actions are logged for transparency:

**Storage keys**:
- `governance/proposals/{proposal_id}` - Full proposal data
- `governance/log/{proposal_id}` - Audit log entries
- `governance/proposals_by_state/{state}/{proposal_id}` - State index

**Audit log entry structure**:
```rust
pub struct AuditLogEntry {
    pub proposal_id: [u8; 32],
    pub action: AuditAction,      // Submitted, Approved, Executed, Rejected, Expired
    pub block_height: u64,
    pub actor: Option<Address>,   // Who triggered (None for system actions)
    pub details: Vec<u8>,         // Additional context
}
```

**Querying examples**:

```rust
// Get all proposals in a state
let prefix = format!("governance/proposals_by_state/{}/", state.to_byte());
let proposals = kv.scan_prefix(prefix.as_bytes())?;

// Get audit trail for a proposal
let log_key = governance_log_key(&proposal_id);
let log_data = kv.get(&log_key)?;
let entry = decode_audit_log_v1(&log_data)?;
```

## Quick Reference

| Operation | Proposal Type | Timelock | Use Case |
|-----------|---------------|----------|----------|
| Upgrade to Gated | `PolicyChange` | 5000 blocks | Enable AI gated execution |
| Upgrade to Autonomous | `PolicyChange` | 5000 blocks | Full AI autonomy |
| Activate new module | `ModuleActivation` | 5000 blocks | Deploy new AI version |
| Rollback module | `ModuleRollback` | 1000 blocks | Revert to previous version |
| Emergency halt | `EmergencyFreeze` | 100 blocks | Immediate AI freeze |
| Parameter change | `ParamChange` | 1000 blocks | Adjust protocol params |

## Related Documentation

- `crates/governance/src/lib.rs` - Core governance types
- `crates/governance/src/codec.rs` - Proposal/audit log encoding
- `crates/ai_entities/src/lib.rs` - AI entity types and autonomy modes
- `crates/ai_entities/src/gates.rs` - Approval gate definitions
