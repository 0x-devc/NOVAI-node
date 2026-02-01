# NOVAI Incident Postmortem: [TITLE]

**Incident ID**: INC-YYYY-NNN
**Date**: YYYY-MM-DD
**Severity**: P0 (Critical) | P1 (High) | P2 (Medium) | P3 (Low)
**Duration**: HH:MM (from detection to resolution)
**Author**: [Name]
**Status**: Draft | Reviewed | Final

---

## Incident Summary

[1-3 sentence summary of what happened, what was affected, and the outcome.]

**Impact**:
- Consensus: [Stalled / Degraded / Unaffected]
- Block Production: [Halted for X minutes / Slowed / Unaffected]
- Validators Affected: [N of M]
- Transactions Lost: [N / None]
- Data Integrity: [Compromised / Intact]

---

## Timeline

All times in UTC.

| Time | Event |
|------|-------|
| HH:MM | [First anomaly observed — how was it detected?] |
| HH:MM | [Alert fired / operator noticed — which alert?] |
| HH:MM | [Investigation started — who responded?] |
| HH:MM | [Root cause identified] |
| HH:MM | [Mitigation applied — what action?] |
| HH:MM | [Service restored — how was recovery confirmed?] |
| HH:MM | [Post-incident monitoring confirmed stable] |

**Detection Method**: [Prometheus alert / operator observation / user report / log review]

**Time to Detect (TTD)**: [Duration from first anomaly to detection]
**Time to Mitigate (TTM)**: [Duration from detection to mitigation]
**Time to Resolve (TTR)**: [Duration from detection to full resolution]

---

## Root Cause Analysis

### What Happened

[Detailed technical explanation of the failure chain. Reference specific code, configuration, or infrastructure.]

### Why It Happened

[Underlying cause — why did the conditions exist for this failure?]

### Contributing Factors

- [ ] Configuration error
- [ ] Code defect
- [ ] Infrastructure failure (hardware, network, cloud)
- [ ] Operational error (human mistake during procedure)
- [ ] External dependency failure
- [ ] Insufficient monitoring / alerting
- [ ] Missing documentation / runbook
- [ ] Capacity / scaling issue

---

## Actions Taken

### Immediate Mitigation

[Step-by-step actions taken to stop the bleeding. Reference playbook used if applicable.]

1. [Action 1 — command run or change made]
2. [Action 2]
3. [Action 3]

**Playbook Used**: [ROLLBACK_BAD_MODULE / ROLLBACK_BAD_PARAM / EMERGENCY_FREEZE / VALIDATOR_COMPROMISE / None]

### Recovery

[Steps taken to restore normal operation.]

1. [Recovery step 1]
2. [Recovery step 2]

### Verification

[How was successful recovery confirmed?]

```bash
# Example verification commands
curl -s http://localhost:8080/metrics | grep committed_height
for i in {0..4}; do echo "V$i: $(curl -s http://localhost:808$i/metrics | grep committed_height | awk '{print $2}')"; done
```

---

## Prevention Measures

### Short-Term (This Week)

| Action | Owner | Status |
|--------|-------|--------|
| [Fix / patch / config change] | [Name] | [ ] TODO |

### Medium-Term (Next 2-4 Weeks)

| Action | Owner | Status |
|--------|-------|--------|
| [Code change / new test / monitoring improvement] | [Name] | [ ] TODO |

### Long-Term (Backlog)

| Action | Owner | Status |
|--------|-------|--------|
| [Architecture change / process improvement] | [Name] | [ ] TODO |

---

## Metrics During Incident

[Attach or reference Grafana screenshots / Prometheus queries that show the incident.]

**Key Queries**:
```promql
# Block production rate during incident window
rate(novai_committed_height[1m])

# View changes spike
rate(novai_consensus_view_changes_total[1m])

# Peer connectivity
novai_peer_count

# Validator height divergence
max(novai_committed_height) - min(novai_committed_height)
```

---

## Lessons Learned

### What Went Well

- [Thing that worked as expected or helped during response]

### What Went Poorly

- [Thing that made the incident worse or slowed response]

### Where We Got Lucky

- [Thing that could have made this much worse but didn't]

---

## Appendix

### Related Incidents

- [INC-YYYY-NNN: Brief description if related]

### References

- Playbook: `docs/playbooks/[PLAYBOOK_NAME].md`
- Operator Runbook: `docs/OPERATOR_RUNBOOK.md`
- Architecture Decisions: `docs/ARCHITECTURE_DECISIONS.md`
