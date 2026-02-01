# Playbook: Rollback Bad Parameter Change

**Scenario**: A governance ParamChange proposal was executed and the new parameter value is causing issues (e.g., MIN_FEE too high, BLOCK_SIZE_LIMIT too low).

**Severity**: P1 (High)
**Expected Recovery**: 2-3 actions
**Prerequisites**: Operator has Docker access to validator nodes

---

## Detection

### Symptoms

- Transactions rejected due to new fee/limit: check logs for `❌` errors
- Mempool draining or accumulating abnormally
- Block production slowed or halted
- `novai_current_round` elevated (consensus struggling)
- Users reporting failed transaction submissions

### Identify the Bad Parameter

```bash
# Check governance audit log for recent Executed proposals
docker logs novai-validator-0 | grep -E "Executed|ParamChange" | tail -10

# Check current metrics for anomalies
curl -s http://localhost:8080/metrics

# Compare block production rate before and after change
# If committed_height is stalling, the parameter is likely the cause
watch -n 5 'curl -s http://localhost:8080/metrics | grep committed_height'
```

---

## Response

### Action 1: Submit Revert ParamChange

```bash
# Submit a new ParamChange proposal that reverts to the previous value
# ProposalType::ParamChange = 0
# Timelock: 1000 blocks (~2.8 hours at 10s/block)

# The proposal_data field should contain the PREVIOUS parameter value
# Governance audit log records what the value was before the change
```

**If 1000-block timelock is too slow** and consensus is actively degraded:

### Action 2: Emergency Restart with Override (if consensus stalled)

If the bad parameter has stalled consensus entirely (no new blocks being committed), the governance path cannot work because proposals require block production.

```bash
# Restart all validators to clear in-memory state
# The parameter change is persisted in state DB, so restart alone
# does NOT revert it. But it clears any stuck consensus rounds.

for i in {0..4}; do docker restart novai-validator-$i; done

# Wait 30s for reconnection
sleep 30

# Verify consensus resumes
for i in {0..4}; do
  echo "V$i: $(curl -s http://localhost:808$i/metrics | grep committed_height | awk '{print $2}')"
done
```

### Action 3: Verify Revert

```bash
# After revert proposal executes:

# Check block production is normal
watch -n 5 'curl -s http://localhost:8080/metrics | grep committed_height'

# Check mempool is processing
curl -s http://localhost:8080/metrics | grep mempool_size

# Check consensus round is normal
curl -s http://localhost:8080/metrics | grep current_round
```

---

## Recovery Verification

| Check | Command | Expected |
|-------|---------|----------|
| Block production | `curl -s localhost:8080/metrics \| grep committed_height` | Increasing steadily |
| Consensus round | `curl -s localhost:8080/metrics \| grep current_round` | 0 or 1 |
| Mempool processing | `curl -s localhost:8080/metrics \| grep mempool_size` | Decreasing or stable |
| All validators synced | Compare `committed_height` across all 5 | Within 1 block |

---

## Post-Incident

1. File postmortem using `docs/POSTMORTEM_TEMPLATE.md`
2. Review the governance proposal that introduced the bad parameter
3. Add parameter bounds validation if not present
4. Consider tighter acceptance criteria for ParamChange proposals
5. Update tuning parameters documentation: `docs/TUNING_PARAMETERS.md`

---

## Escalation

If parameter revert does not resolve the issue:

1. Check if the parameter change corrupted state (unlikely but possible)
2. Consider restoring from backup: see `docs/OPERATOR_RUNBOOK.md` Section 6
3. File P0 postmortem

---

## Architecture Reference

- ProposalType::ParamChange: `crates/governance/src/lib.rs:287`
- GovernanceConfig timelocks: `crates/governance/src/lib.rs:31-103`
- Governance audit log: `crates/governance/src/lib.rs:119-266`
- Tuning parameters: `docs/TUNING_PARAMETERS.md`
