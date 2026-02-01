# Playbook: Rollback Bad AI Module

**Scenario**: An activated AI module is misbehaving — emitting invalid proposals, consuming excessive resources, or producing incorrect signals.

**Severity**: P1 (High)
**Expected Recovery**: 2-3 actions
**Prerequisites**: Operator has Docker access to validator nodes

---

## Detection

### Symptoms

- `novai_anomaly_signals_total` increasing rapidly
- Copilot logs show repeated anomalies: `docker logs novai-validator-0 | grep "ANOMALY"`
- Invalid proposals in logs: `docker logs novai-validator-0 | grep "❌"`
- Mempool backlog growing: `novai_mempool_size` approaching 1000
- Consensus delays: `novai_current_round > 2`

### Confirm the Bad Module

```bash
# Check anomaly signal details
docker logs novai-validator-0 | grep -E "ANOMALY|anomaly" | tail -20

# Check which entity is active
docker logs novai-validator-0 | grep -E "entity|module|AI" | tail -20

# Verify anomaly rate is abnormal
curl -s http://localhost:8080/metrics | grep anomaly_signals_total
```

---

## Response

### Action 1: Submit Emergency Freeze (if critical)

If the module is causing consensus instability, use EmergencyFreeze (100-block timelock):

```bash
# EmergencyFreeze proposal halts ALL AI execution
# ProposalType::EmergencyFreeze = 4
# Timelock: 100 blocks (~17 minutes at 10s/block)

# Submit via governance (requires validator signature)
# The proposal sets is_active = false on all AI entities
```

**Note**: EmergencyFreeze affects ALL AI entities, not just the bad one. Use ModuleRollback for targeted action.

### Action 2: Submit Module Rollback (targeted)

For a single misbehaving module:

```bash
# ModuleRollback proposal targets a specific entity
# ProposalType::ModuleRollback = 2
# Timelock: 1000 blocks (~2.8 hours at 10s/block)

# This sets the target entity's is_active = false
# Other modules continue operating normally
```

### Action 3: Verify Rollback

```bash
# Monitor committed_height is advancing
watch -n 5 'curl -s http://localhost:8080/metrics | grep committed_height'

# Verify anomaly rate has dropped
curl -s http://localhost:8080/metrics | grep anomaly_signals_total
# Take two readings 60s apart — rate should drop to near zero

# Verify consensus is healthy
curl -s http://localhost:8080/metrics | grep current_round
# Should be 0 or 1 (normal)
```

---

## Recovery Verification

| Check | Command | Expected |
|-------|---------|----------|
| Block production | `curl -s localhost:8080/metrics \| grep committed_height` | Increasing |
| Consensus round | `curl -s localhost:8080/metrics \| grep current_round` | 0 or 1 |
| Anomaly rate | `curl -s localhost:8080/metrics \| grep anomaly_signals_total` | Stable (not increasing) |
| All validators synced | Compare `committed_height` across all 5 | Within 1 block |

---

## Post-Incident

1. File postmortem using `docs/POSTMORTEM_TEMPLATE.md`
2. Investigate root cause of module misbehavior
3. Review module code and capabilities before re-activation
4. Consider adding capability restrictions before re-enabling
5. Update anomaly detection thresholds if too slow to detect

---

## Escalation

If module rollback does not stop the misbehavior:

1. Execute `EMERGENCY_FREEZE.md` playbook (halts all AI execution)
2. If consensus itself is affected, restart all validators:
   ```bash
   for i in {0..4}; do docker restart novai-validator-$i; done
   ```
3. File P0 postmortem

---

## Architecture Reference

- AI entity `is_active` field: `crates/ai_entities/src/lib.rs:233`
- ProposalType::ModuleRollback: `crates/governance/src/lib.rs:293`
- ProposalType::EmergencyFreeze: `crates/governance/src/lib.rs:299`
- Timelock config: `crates/governance/src/lib.rs:31-103`
- Copilot anomaly detection: `crates/copilot/src/lib.rs`
