# Playbook: Emergency Freeze — Halt All AI Execution

**Scenario**: Critical situation requiring immediate halt of all AI entity execution. Use when a module is causing consensus instability, data corruption risk, or any situation where continuing AI execution is dangerous.

**Severity**: P0 (Critical)
**Expected Recovery**: 2 actions to freeze, 1 action to unfreeze
**Prerequisites**: Operator has Docker access to validator nodes

---

## When to Use

- AI module causing consensus splits or forks
- Suspected data corruption from AI execution
- Multiple modules misbehaving simultaneously
- ModuleRollback is too slow (1000-block timelock vs 100-block for freeze)
- Unknown root cause but AI execution is the common factor

---

## Detection

### Symptoms Requiring Emergency Freeze

- `novai_current_round > 5` AND `novai_anomaly_signals_total` spiking
- Consensus stalled with AI-related errors in logs
- Fork detection panic in logs: `CONSENSUS SAFETY VIOLATION: FORK DETECTED`
- Multiple validators showing different `committed_height` values (divergence > 5 blocks)

### Confirm AI Is the Cause

```bash
# Check if anomaly signals correlate with consensus issues
curl -s http://localhost:8080/metrics | grep -E "current_round|anomaly_signals_total|committed_height"

# Check logs for AI-related errors
docker logs novai-validator-0 | grep -E "entity|module|AI|anomaly" | tail -20

# Compare validator heights (should be within 1 block normally)
for i in {0..4}; do
  echo "V$i: $(curl -s http://localhost:808$i/metrics | grep committed_height | awk '{print $2}')"
done
```

---

## Response

### Action 1: Submit Emergency Freeze

```bash
# EmergencyFreeze is the fastest governance action
# ProposalType::EmergencyFreeze = 4
# Timelock: 100 blocks (~17 minutes at 10s/block)
#
# Effect: Sets is_active = false on ALL AI entities
# All AI proposals, signals, and execution halt immediately after execution
```

**If consensus is completely stalled** (no new blocks being produced), the governance path cannot work. Proceed to Action 2.

### Action 2: Manual Recovery (if consensus stalled)

If blocks are not being produced, governance proposals cannot be submitted or executed. Manual intervention is required:

```bash
# Step 1: Stop all validators
for i in {0..4}; do docker stop novai-validator-$i; done

# Step 2: Restart validators (clears in-memory state, triggers catch-up)
for i in {0..4}; do docker start novai-validator-$i; done

# Step 3: Wait for reconnection (30 seconds)
sleep 30

# Step 4: Verify consensus resumes
for i in {0..4}; do
  echo "V$i: $(curl -s http://localhost:808$i/metrics | grep committed_height | awk '{print $2}')"
done

# Step 5: Once blocks are flowing, submit EmergencyFreeze via governance
```

### Action 3: Verify Freeze

```bash
# Confirm no AI signals being published
curl -s http://localhost:8080/metrics | grep anomaly_signals_published
# Take two readings 60s apart — should be stable (not increasing)

# Confirm consensus is healthy
curl -s http://localhost:8080/metrics | grep -E "committed_height|current_round"
# committed_height increasing, current_round = 0 or 1

# Confirm all validators synced
for i in {0..4}; do
  echo "V$i: $(curl -s http://localhost:808$i/metrics | grep committed_height | awk '{print $2}')"
done
# All within 1 block of each other
```

---

## Unfreeze Procedure

After root cause is identified and fixed:

```bash
# Submit ModuleActivation proposal to re-enable specific modules
# ProposalType::ModuleActivation = 1
# Timelock: 5000 blocks (~14 hours at 10s/block) — intentionally slow
#
# Re-enable modules ONE AT A TIME
# Monitor for 30+ minutes between each activation
# If any module causes issues again, re-freeze immediately
```

**Unfreeze verification**:
```bash
# After each module re-activation, monitor:
watch -n 10 'curl -s http://localhost:8080/metrics | grep -E "committed_height|current_round|anomaly"'

# Watch for anomaly spikes
# If anomaly_signals_total spikes after activation, re-freeze
```

---

## Recovery Verification

| Check | Command | Expected |
|-------|---------|----------|
| Block production | `curl -s localhost:8080/metrics \| grep committed_height` | Increasing |
| Consensus round | `curl -s localhost:8080/metrics \| grep current_round` | 0 or 1 |
| AI signals halted | `curl -s localhost:8080/metrics \| grep anomaly_signals_published` | Stable |
| All validators synced | Compare `committed_height` across all 5 | Within 1 block |
| No fork detected | `docker logs novai-validator-0 \| grep FORK` | No output |

---

## Post-Incident

1. File P0 postmortem using `docs/POSTMORTEM_TEMPLATE.md`
2. Root cause analysis of why AI execution caused instability
3. Review all active modules before unfreezing
4. Consider adding circuit breaker logic (auto-freeze on anomaly threshold)
5. Review and tighten AI entity capabilities

---

## Architecture Reference

- ProposalType::EmergencyFreeze: `crates/governance/src/lib.rs:299`
- Emergency timelock (100 blocks): `crates/governance/src/lib.rs:41`
- AiEntity::is_active: `crates/ai_entities/src/lib.rs:233`
- ModuleActivation timelock (5000 blocks): `crates/governance/src/lib.rs:39`
- AutonomyMode: `crates/ai_entities/src/lib.rs:82-110`
- Capabilities: `crates/ai_entities/src/lib.rs:112-198`

---

## Drill Results

### Drill Status: BLOCKED (2026-02-01)

**Reason:** No operator-facing governance CLI exists. The `novai-node submit-tx` command accepts raw payloads but there is no way to construct properly encoded governance proposals (ParamChange, ModuleRollback, EmergencyFreeze) from the command line.

**Action Items:**
- [ ] Implement `novai-node governance propose <type> <params>` CLI subcommand
- [ ] Once governance CLI exists, re-run drills for this playbook
