# Disaster Recovery Procedures (NOVAI testnet)

This document consolidates the disaster recovery (DR) procedures for the NOVAI
testnet into one place. It indexes the existing incident playbooks and the
operator runbook, and it fills the gaps those documents did not yet cover
(single validator corruption, total state loss, oracle failure, genesis
mismatch, and fork detection).

Scope. This is a testnet recovery runbook. It covers documentation and
read-only verification only. Every destructive action (wiping state, performing
a fresh genesis, restarting the whole validator set) is written as a manual,
gated step that an operator runs deliberately, never as an automated script.

Authorial note on honesty. I have not executed any of these procedures against
the live network while writing this document. Where a step is labelled TESTED,
that label refers to a prior recorded drill or a repository unit test, cited at
the procedure. Everything else is labelled DRY-RUN ONLY or UNTESTED. A procedure
that has not been exercised end to end on this topology is marked UNTESTED on
purpose, so that nobody mistakes a plausible procedure for a proven one.

Voice. Numbered recovery steps are written in the imperative ("Restart the
unit"). Notes written in the first person ("I have not tested this") are my
authorial commentary, not instructions.

---

## 1. Placeholders and conventions

This document uses placeholders for every operational specific. Do not paste
real IP addresses, hostnames, or secrets into this file.

- `<i>` is a validator index in the range 0 to 3.
- `<STATE_DIR>` is a validator data directory. On this testnet the structural
  form is `/var/lib/novai/node<i>` (RocksDB lives here).
- `<ORACLE_KEYFILE>` is the oracle key file at `/etc/novai/oracle-keys.json`
  (mode 0600, owned by root). This file is the single precious, non-reproducible
  secret on the box.
- `<NODE_HOST>` is a node host. All operator commands in this runbook target the
  loopback interface (`127.0.0.1`) on the box itself.
- Metrics port is `8080 + <i>`. P2P port is `9000 + <i>`. RPC port, where
  enabled, is `3030 + <i>`.
- `<BACKUP_DIR>` is an operator-chosen directory that lives outside any state
  directory (ideally off the box entirely).
- `<HEIGHT>`, `<N>` are example numeric values in sample output.

---

## 2. Topology and facts at a glance

These facts were confirmed by a read-only diagnostic run on the live box. They
are the assumptions the rest of this runbook is built on. If the deployment
changes, revisit this section first.

- Deployment is systemd. The four validators run as instances of the
  `novai-node@.service` template: `novai-node@0`, `novai-node@1`,
  `novai-node@2`, `novai-node@3`.
- Supporting services: `novai-monitor.service` (metrics poller and alerter) and
  `novai-price-oracle.service` (BTC/USD anchor publisher) are active.
  `novai-txgen.service` (load generator) exists but is inactive, which is why a
  resting mempool size of zero is normal here and is not an incident.
- Topology is a full mesh. Every validator carries all four peer flags
  (`--peer` to ports 9000, 9001, 9002, 9003), so each validator connects to
  every other. There is no single seed node, and the symmetric topology means no
  "seed died" failure mode exists.
- Consensus tolerance is f equal to 1: quorum is 3 of 4. One validator can be
  down without halting the chain. Losing a second validator halts consensus.
- Validators run with `--dev-keys --allow-insecure-dev-keys --validator <i>`.
  These keys are deterministic and reproducible, so wiping a validator data
  directory does not destroy an irreplaceable signing key on this testnet.
- The oracle uses a real key file at `<ORACLE_KEYFILE>`. It is NOT reproducible.
  Losing it means abandoning the oracle entity identity (the entity signing key
  cannot be rotated). Treat it as the one secret that must survive any recovery.
- Persisted state is RocksDB under `/var/lib/novai/node<i>`. Configuration and
  the oracle and monitor secrets live under `/etc/novai/` (`oracle.env`,
  `monitor.env`, `oracle-keys.json`), all mode 0600 owned by root.
- There is no operator governance CLI on the box. The only transaction
  submission path is `novai-node submit-tx <payload>`. Any procedure that needs
  a governance proposal is therefore BLOCKED until that CLI exists.
- There is no automated backup and no protocol-level state snapshot or fast
  sync. A crashed or lagging validator recovers by chunked block sync from its
  healthy peers. Total simultaneous state loss has no fast-restore path; it
  requires a manual backup restore (Scenario 9) or a fresh genesis (Scenario 4).

---

## 3. Golden rules

Read these before acting. They are the invariants that keep a recoverable
incident from becoming an unrecoverable one.

1. Preserve the oracle key before any wipe. Before deleting any state, confirm
   that `<ORACLE_KEYFILE>` and the `/etc/novai/*.env` files are backed up off the
   box. Validator dev-keys are reproducible; the oracle key is not. Use the
   pre-wipe guard in Appendix B.1.
2. Protect quorum. Quorum is 3 of 4. Never stop or restart more than one
   validator at a time unless consensus is already halted. When consensus is
   live, recover one node, wait for it to rejoin and catch up, then move to the
   next.
3. Diagnose before destroying. Wiping state and restarting are irreversible for
   that node. Confirm the failure mode (Section 4) and capture forensic copies
   (logs, and where relevant a read-only copy of the data directory) before any
   destructive step.
4. Destructive steps are manual. Steps marked [MANUAL, DESTRUCTIVE] are never
   automated. Run them by hand, deliberately, after the guard checks pass.
5. A resting mempool is normal. With `novai-txgen` inactive, `mempool_size` of
   zero is expected. Do not treat it as a failure.
6. Honesty in incident notes. If a procedure was not tested, say so in the
   postmortem. File incidents with `docs/POSTMORTEM_TEMPLATE.md`.

---

## 4. Detection

Two systems detect trouble, plus a manual read-only check.

Monitoring. `novai-monitor.service` polls a node metrics endpoint and raises
alerts over the configured chat channel. The Prometheus rules in
`monitoring/alerts.yml` and the standalone monitor in
`monitoring/novai-monitor/` cover the conditions below.

Key metrics (read from `http://127.0.0.1:<8080+i>/metrics`):

| Metric | Healthy | Trouble signal |
|--------|---------|----------------|
| `novai_committed_height` | increases steadily, equal across nodes within a few blocks | flat across all nodes (stall), or one node frozen while others advance |
| `novai_current_round` | 0 most of the time | greater than 5 sustained (consensus struggling) |
| `novai_peer_count` | stable, at or above quorum; the existing alert fires below 3 | below 3 (lost quorum visibility) |
| `novai_consensus_view_changes_total` | nearly flat | rising quickly (more than about 10 per minute) |
| `novai_mempool_size` | 0 at rest here (txgen inactive) | sustained near the cap (about 800 to 1000) |

Documented alert thresholds (from `monitoring/alerts.yml` and the monitor):
ConsensusStalled (no height change in 10 minutes), ConsensusDelayed (5 minutes),
InsufficientPeers (peer_count below 3), HighConsensusRound (round above 5),
FrequentViewChanges, MempoolNearFull, ValidatorLagging (more than 5 blocks
behind the max).

Manual check. Run the read-only health check in Appendix B.2 to poll all four
nodes at once and flag a stall or a height divergence. I have not executed this
snippet against the live network; it is read-only by construction.

Note on `peer_count`. The naive expectation for a four node full mesh is three
peers per node, but the live reading was higher than three. I did not confirm
the exact counting semantics (inbound plus outbound, or connections rather than
distinct peers). Treat the documented alert threshold (below 3 is bad) as
authoritative and confirm the healthy baseline against your own monitor history.
See Section 13.

---

## 5. Scenario 1: Single validator down or crashed

Severity: P2 (chain stays live on the remaining 3 of 4).

Applies when: one `novai-node@<i>` is stopped, crash-looping, or frozen while
the other three keep committing blocks.

Detection: the affected node metrics endpoint is unreachable or its
`novai_committed_height` is frozen; `systemctl is-active novai-node@<i>` is not
active, or the unit restarts repeatedly in `journalctl`.

Recovery:
1. Confirm quorum is intact. Verify the other three nodes are committing blocks
   (Appendix B.2). If two or more nodes are down, go to Scenario 2 instead.
2. Inspect the failed unit:
   ```bash
   systemctl status novai-node@<i>
   journalctl -u novai-node@<i> -n 200 --no-pager
   ```
3. If the cause is transient (a one-off panic, an OOM, a host hiccup), restart
   the single unit:
   ```bash
   systemctl restart novai-node@<i>
   ```
4. Watch it rejoin and catch up. The node resyncs missing blocks from its peers
   by chunked block sync; `novai_committed_height` should climb toward the
   others.
5. If the unit crash-loops with a RocksDB open or corruption error, this is not
   a transient fault. Go to Scenario 3.

Verification (read-only): re-run Appendix B.2 and confirm the recovered node
height converges to within a few blocks of the others and `current_round` is 0.

Status: PARTIALLY TESTED. Chunked block sync catch-up from peers is TESTED per
the drill recorded in `docs/playbooks/VALIDATOR_COMPROMISE.md`. The systemd
restart procedure on this topology is UNTESTED by me against the live network.

Related: `docs/OPERATOR_RUNBOOK.md` (node lifecycle and node-behind sections).

---

## 6. Scenario 2: Full consensus halt or stall

Severity: P0 (no block progress network wide).

Applies when: `novai_committed_height` is flat across all four nodes for minutes
and `novai_current_round` is climbing. ConsensusStalled or ConsensusDelayed
alerts fire.

Detection: Appendix B.2 shows no node advancing; `current_round` rising;
`view_changes` rising.

Recovery:
1. Determine the shape of the halt before acting:
   - If two or more validators are down, consensus lost quorum. Recover the down
     validators (Scenario 1 for each, or Scenario 3 if their state is corrupt).
     Bringing the count back to at least three restores liveness.
   - If all four are up but stuck, suspect a recent deploy or a bad parameter.
     If a binary was just rolled out, go to Scenario 5 (rollback). 
2. Capture forensics first. Save recent logs for all nodes:
   ```bash
   journalctl -u 'novai-node@*' --since '20 min ago' --no-pager > <BACKUP_DIR>/halt-logs.txt
   ```
   I note that writing this file is the operator's own forensic capture outside
   the repository; it is not part of the repository content.
3. If all nodes are up and stuck with no obvious bad deploy, perform a rolling
   restart, one validator at a time, waiting for each to rejoin before the next:
   ```bash
   systemctl restart novai-node@0   # wait for it to rejoin and current_round to settle
   systemctl restart novai-node@1   # then the next, and so on
   ```
   Restarting one at a time preserves any quorum that still exists and avoids a
   cold start.
4. If the halt is caused by AI execution misbehaviour (not a consensus bug),
   escalate to the emergency freeze playbook.

BLOCKED note: the governance based EmergencyFreeze in
`docs/playbooks/EMERGENCY_FREEZE.md` requires a governance proposal, which has no
operator CLI on this box. The available interim action is the manual rolling
restart in step 3, plus a raw `novai-node submit-tx <payload>` if a freeze
payload can be constructed by hand.

Verification (read-only): Appendix B.2 shows `committed_height` advancing again
on all nodes and `current_round` returning to 0.

Status: UNTESTED by me. The consensus-stuck diagnosis steps mirror
`docs/OPERATOR_RUNBOOK.md`.

Related: `docs/OPERATOR_RUNBOOK.md` (consensus stuck), 
`docs/playbooks/EMERGENCY_FREEZE.md` (BLOCKED governance path).

---

## 7. Scenario 3: Corrupted persisted state on one validator

Severity: P2 (one node down, quorum of 3 of 4 holds).

Applies when: one `novai-node@<i>` crash-loops with a RocksDB open, manifest, or
corruption error in its journal, while the other three nodes are healthy.

Detection: `journalctl -u novai-node@<i>` shows a database error on startup; that
node never reaches a healthy height while peers advance.

Recovery:
1. Confirm the other three nodes are healthy and committing (Appendix B.2). The
   recovery below relies on resyncing the bad node from these peers.
2. Run the pre-wipe key-safety guard (Appendix B.1). Do not proceed until it
   confirms `<ORACLE_KEYFILE>` and the env files are backed up off the box. On
   this testnet the validator keys are reproducible dev-keys, so the validator
   data wipe itself is recoverable; the guard protects the oracle key, which
   shares the same host.
3. Stop the affected unit:
   ```bash
   systemctl stop novai-node@<i>
   ```
4. [MANUAL, DESTRUCTIVE] Optionally take a read-only forensic copy of the corrupt
   directory for postmortem, then remove the corrupted state for that one node:
   ```bash
   # forensic copy first (optional, read-only source):
   cp -a /var/lib/novai/node<i> <BACKUP_DIR>/node<i>-corrupt-$(date -u +%Y%m%dT%H%M%SZ)
   # then clear the node state (run by hand, deliberately):
   rm -rf /var/lib/novai/node<i>/*
   ```
   Wipe only the single affected `node<i>` directory. Never wipe more than the
   one corrupted node here.
5. Start the unit. It re-initializes from genesis and resyncs the chain from the
   three healthy peers by chunked block sync:
   ```bash
   systemctl start novai-node@<i>
   ```
6. Watch it catch up to the tip.

Verification (read-only): Appendix B.2 shows the recovered node converging to the
others height and `current_round` 0.

Status: PARTIALLY TESTED. Catch-up by chunked block sync is TESTED per the
`docs/playbooks/VALIDATOR_COMPROMISE.md` drill. The single-node wipe and rejoin
sequence is UNTESTED by me against the live network.

Related: Scenario 1, `docs/OPERATOR_RUNBOOK.md` (data corruption).

---

## 8. Scenario 4: Total state loss or cold restart from genesis

Severity: P0 (whole network down, no healthy peer to sync from).

Applies when: all four nodes have lost or cannot open their state, and no peer
retains usable history. This is the worst case. There is no fast snapshot
restore, so recovery is either a backup restore (Scenario 9, if a good backup
exists) or a fresh genesis, which relaunches the chain and discards all history.

Decision: prefer a verified backup restore (Scenario 9) if one exists, because it
preserves chain history. Use fresh genesis only when no usable backup exists and
a clean relaunch is acceptable. On a testnet a relaunch is usually acceptable,
but it must be a deliberate decision.

Recovery (fresh genesis):
1. Preserve secrets. Confirm `<ORACLE_KEYFILE>` and `/etc/novai/*.env` are backed
   up off the box (Appendix B.1). Reusing the same oracle key file lets the
   oracle keep its identity across the relaunch.
2. Obtain the canonical genesis file and verify it. All four nodes must use byte
   identical genesis. Compare hashes across the nodes and against the published
   canonical hash (Appendix B.3). Investigate any mismatch before continuing.
3. Stop all validators:
   ```bash
   systemctl stop novai-node@0 novai-node@1 novai-node@2 novai-node@3
   ```
4. [MANUAL, DESTRUCTIVE] Clear each node state directory:
   ```bash
   for i in 0 1 2 3; do rm -rf /var/lib/novai/node$i/*; done
   ```
5. Place the verified canonical genesis where each unit expects it (the path is
   set in the `novai-node@.service` template; inspect it with
   `systemctl cat novai-node@0`). Caveat: this genesis path is an unconfirmed
   OPEN ITEM (Section 16). The diagnostic found the two probed genesis paths
   absent, so confirm the real `--genesis` path from the unit file before
   proceeding, and do not guess it.
6. Start the validators. Each recomputes the genesis state root, verifies it, and
   begins consensus from height 1:
   ```bash
   systemctl start novai-node@0 novai-node@1 novai-node@2 novai-node@3
   ```
7. Re-bootstrap the oracle. A fresh genesis is a new chain, so the oracle entity
   must be re-registered. Because the key file was preserved, the entity keeps
   its derived identity. Follow Scenario 6, step "entity not registered".
8. Verify chain liveness (Appendix B.2) and that the oracle resumes posting
   anchors (Appendix B.4).

Verification (read-only): all four nodes advance from height 1 together;
`current_round` settles to 0; oracle `submission_success` advances.

Status: UNTESTED. I have not performed a fresh genesis recovery on this network.
The genesis production and independent state-root verification process is
documented in `docs/GENESIS_CEREMONY.md`.

Related: `docs/GENESIS_CEREMONY.md`, Scenario 6 (oracle re-bootstrap),
Scenario 9 (backup restore).

---

## 9. Scenario 5: Failed deploy or bad release rollback

Severity: P1 (a deploy degraded one or more validators).

Applies when: after deploying a new `novai-node` binary, a validator crash-loops
or consensus degrades (rising round, view changes).

Recovery:
1. Identify the previous known-good binary version.
2. Roll back one validator at a time, preserving quorum:
   ```bash
   # replace the binary with the known-good version, then:
   systemctl restart novai-node@<i>
   # wait for this node to rejoin and current_round to settle before the next
   ```
3. Proceed node by node until all four run the known-good binary.
4. If the regression was a parameter change rather than a binary, the revert is a
   governance action.

BLOCKED note: parameter and module rollback via governance
(`docs/playbooks/ROLLBACK_BAD_PARAM.md`, `docs/playbooks/ROLLBACK_BAD_MODULE.md`)
require a governance proposal, which has no operator CLI here. The interim path
is a hand-built `novai-node submit-tx <payload>`. Keep these two playbooks marked
BLOCKED until a `governance propose` CLI exists.

Verification (read-only): all nodes healthy on the rolled-back binary; Appendix
B.2 shows steady height and round 0.

Status: UNTESTED by me. The rolling upgrade and rollback pattern follows
`docs/OPERATOR_RUNBOOK.md`.

Related: `docs/OPERATOR_RUNBOOK.md` (rolling upgrade),
`docs/playbooks/ROLLBACK_BAD_PARAM.md`, `docs/playbooks/ROLLBACK_BAD_MODULE.md`.

---

## 10. Scenario 6: Oracle failure and re-bootstrap

Severity: P2 (price anchors stop; chain consensus is unaffected).

Applies when: the oracle stops posting anchors. Symptoms include
`novai-price-oracle.service` inactive or crash-looping, oracle
`last_submission_height` falling far behind the chain `committed_height`, or a
drained funder or entity balance.

Detection: Appendix B.4 reports the oracle service state and how stale its last
submission is relative to the chain tip.

Recovery (work down this tree to the first matching cause):
1. Service down, chain healthy. Restart and inspect:
   ```bash
   systemctl restart novai-price-oracle
   journalctl -u novai-price-oracle -n 100 --no-pager
   ```
2. RPC endpoint moved. If the node RPC address or port changed, edit
   `PRICE_ORACLE_RPC_ENDPOINT` in `/etc/novai/oracle.env`, then restart the
   service. No re-bootstrap is needed.
3. Entity not registered (for example after a fresh genesis) or missing the
   anchor capability. Re-run the idempotent bootstrap:
   ```bash
   set -a && source /etc/novai/oracle.env && set +a
   <oracle venv python> <oracle dir>/bootstrap.py
   ```
   Re-running is safe: if the entity already exists with the anchor capability,
   bootstrap is a no-op. If it does not, bootstrap funds the funder and registers
   the entity.
4. Funder drained. Top up the funder address (faucet or manual funding). The
   oracle then tops up the entity balance on its own.
5. Key file lost. This is the unrecoverable case. The entity signing key cannot
   be rotated, so a lost `<ORACLE_KEYFILE>` means re-bootstrapping a brand new
   entity under a fresh funder address. This is why the key file must be backed
   up (Golden Rule 1).

Recovery ordering: the node RPC must be live, then the entity must be registered,
then the funder must be funded, then the oracle service can run cleanly.

Verification (read-only): Appendix B.4 shows the service active, and oracle
`submission_success_total` advancing with `last_submission_height` tracking the
chain `committed_height`.

Status: PARTIALLY TESTED. Bootstrap idempotency is TESTED by the repository unit
test `agents/price-oracle/tests/test_bootstrap_idempotent.py`. The live restart
and re-bootstrap as a recovery action are UNTESTED by me against the live
network.

Related: `docs/deploy-two-key-oracle.md`, `docs/AGENT_FUNDING_PLAYBOOK.md`,
`agents/price-oracle/README.md`.

---

## 11. Scenario 7: Genesis mismatch or wrong genesis on startup

Severity: P1 (a node cannot join, or joins and diverges immediately).

Applies when: a node refuses to start with a genesis or state-root validation
error, or a node starts but immediately diverges from the others.

Detection: the node journal shows a genesis or state-root validation failure; or
Appendix B.2 shows one node that cannot peer or sits at an incompatible height.
Use Appendix B.3 to compare genesis file hashes across nodes.

Recovery:
1. Compare each node genesis hash (Appendix B.3). The node whose hash differs has
   the wrong genesis.
2. Replace the wrong genesis with the canonical file, verifying the hash matches
   the canonical published value before restarting:
   ```bash
   systemctl stop novai-node@<i>
   # install the verified canonical genesis at the path from `systemctl cat novai-node@<i>`
   systemctl start novai-node@<i>
   ```
   Caveat: the genesis path is an unconfirmed OPEN ITEM (Section 16). Confirm the
   real `--genesis` path from the unit file before installing, and do not guess it.
3. If all four nodes differ from the canonical hash, this is a distribution
   failure. Re-distribute the canonical genesis to all nodes and treat it as a
   coordinated restart (Scenario 4 if state must also be reset).

Verification (read-only): Appendix B.3 shows all nodes share one genesis hash
that equals the canonical value; Appendix B.2 shows the node rejoining.

Status: UNTESTED. Hash comparison detection is straightforward and read-only.

Related: `docs/GENESIS_CEREMONY.md`, Scenario 4.

---

## 12. Scenario 8: Validator key compromise (cross-reference)

Severity: depends on deployment.

Primary procedure: `docs/playbooks/VALIDATOR_COMPROMISE.md`, which carries a
threat model, an isolation procedure, a key rotation procedure, and recorded
drill results.

Testnet caveat. On this testnet the validators run with `--dev-keys
--allow-insecure-dev-keys`. There is no production secret validator signing key
to steal here; the dev-keys are deterministic. For this testnet, a misbehaving or
suspect validator is handled by isolating and stopping that one node, after which
quorum of 3 of 4 keeps the chain live (Scenario 1 covers the mechanics). The key
rotation steps in the compromise playbook apply to a real-key deployment such as
mainnet, not to this dev-keys testnet.

Status: cross-reference. The compromise playbook records its own drills.

---

## 13. Scenario 9: Manual state backup and restore

Severity: procedure, used by Scenario 4 and for routine safety before risky
changes.

Context. There is no automated backup and no protocol snapshot or fast sync on
this testnet. The mechanism below is the interim manual one. A future state
snapshot export and import would replace it (Section 14).

Backup (manual):
1. Prefer a cold backup. Stop the node so RocksDB is quiescent, archive its state
   directory, then start it again:
   ```bash
   systemctl stop novai-node@<i>
   tar -czf <BACKUP_DIR>/node<i>-$(date -u +%Y%m%dT%H%M%SZ).tar.gz -C /var/lib/novai node<i>
   systemctl start novai-node@<i>
   ```
   I note a hot archive of a running RocksDB can be internally inconsistent.
   Stopping the node first is the safe choice; if you must hot-copy, expect that
   the archive may not restore cleanly.
2. Always back up the precious secrets alongside the state: `<ORACLE_KEYFILE>`
   and `/etc/novai/*.env`.

Restore (manual):
1. Stop the node:
   ```bash
   systemctl stop novai-node@<i>
   ```
2. [MANUAL, DESTRUCTIVE] Replace the state directory from the archive:
   ```bash
   rm -rf /var/lib/novai/node<i>/*
   tar -xzf <BACKUP_DIR>/node<i>-<timestamp>.tar.gz -C /var/lib/novai
   ```
3. Start the node and verify it resumes and catches up:
   ```bash
   systemctl start novai-node@<i>
   ```

Verification (read-only): check archive integrity before relying on it
(Appendix B.5), then confirm the node rejoins (Appendix B.2).

Status: UNTESTED by me. The backup and restore shapes follow
`docs/OPERATOR_RUNBOOK.md`.

Related: `docs/OPERATOR_RUNBOOK.md` (backup and restore), Scenario 4.

---

## 14. Scenario 10: Fork detected or state divergence

Severity: P0 (a safety event: nodes disagree on history).

Applies when: a node panics with a fork-detection error (the node has an internal
no-fork check that panics on detection), or two nodes commit different blocks at
the same height.

Detection: the node journal shows a fork-detection panic. Metrics alone cannot
confirm a fork, because the metrics endpoint does not expose block hashes; a fork
may surface as two subsets of nodes at the same height that cannot agree. I note
this is a detection gap (Section 14 caveat below).

Recovery:
1. Stop the diverged minority to stop it from spreading its fork, while keeping
   the majority (which holds quorum) running:
   ```bash
   systemctl stop novai-node@<minority-i>
   ```
2. Capture forensics before any wipe. Take a read-only copy of the minority node
   data directory and its full logs for postmortem:
   ```bash
   cp -a /var/lib/novai/node<minority-i> <BACKUP_DIR>/fork-node<minority-i>-$(date -u +%Y%m%dT%H%M%SZ)
   journalctl -u novai-node@<minority-i> --no-pager > <BACKUP_DIR>/fork-node<minority-i>-log.txt
   ```
3. Recover the minority node by wiping its state and resyncing from the majority,
   exactly as Scenario 3 (run the pre-wipe guard first).
4. Root-cause the divergence with `docs/POSTMORTEM_TEMPLATE.md`. A fork on a BFT
   chain is a serious safety event and warrants a full postmortem, not just a
   restart.

BLOCKED note: pausing the chain with an on-chain emergency freeze while you
investigate is a governance action and is BLOCKED (no operator CLI). The
available interim controls are stopping the minority node manually and, if a
freeze payload can be built by hand, `novai-node submit-tx <payload>`.

Verification (read-only): after recovery, Appendix B.2 shows all nodes converging
on one height with `current_round` 0 and no further fork panics in the journals.

Status: UNTESTED operator response. The fork-detection panic itself is present in
the code and exercised by the chaos test suite (`docs/CHAOS_TEST_REPORT.md`); the
operator recovery here is UNTESTED, and the governance-based pause is BLOCKED.

Caveat I want to flag: the lack of a block-hash signal in the metrics means a
fork may be detected only by the panic, not proactively. A future read-only
"compare head block hash across nodes" check would close this gap (Section 15).

---

## 15. Future scenarios (noted, not covered this pass)

These were deliberately deferred. I am noting them so they are not forgotten.

- Network partition that does not heal: validators split into partitions that do
  not reconnect by peer discovery. Chaos tests exercise partition flapping, but
  there is no operator recovery procedure yet.
- Mempool exhaustion or spam: `novai_mempool_size` near its cap with no operator
  response documented. Low priority while `novai-txgen` is inactive and the
  resting mempool is empty.

---

## 16. Known gaps and open items

- Genesis file location is UNCONFIRMED, and this blocks Scenario 4. The read-only
  diagnostic probed `/etc/novai/genesis/genesis.json` and `./testnet/genesis.json`
  and found both NOT PRESENT, so the canonical genesis path the node actually
  loads is unknown. Scenarios 4 and 7 depend on installing the canonical genesis
  at that path. Before executing Scenario 4, confirm the real path from the unit
  file (`systemctl cat novai-node@<i>`, read the `--genesis` argument on the
  ExecStart line). Do not guess the path.
- No automated or scheduled backup, and no protocol state snapshot or fast sync.
  Recovery from total state loss is manual restore or fresh genesis. A state
  snapshot export and import is the highest-value addition.
- No operator governance CLI. Three existing playbooks (emergency freeze, bad
  param rollback, bad module rollback) and the governance-based parts of
  Scenario 2 and Scenario 10 are BLOCKED until a `governance propose` CLI exists.
  The interim path everywhere is raw `novai-node submit-tx`.
- Fork detection is reactive (a panic), not a proactive head-hash comparison.
- The healthy `peer_count` baseline for this four node mesh was observed higher
  than the naive value of three. Confirm the exact semantics against monitor
  history before treating any specific value as the healthy baseline.

---

## 17. Appendix A: Quick command reference

All commands target the box itself. Replace `<i>` with 0 to 3.

Validator lifecycle (systemd):
```bash
systemctl status novai-node@<i>
systemctl restart novai-node@<i>
systemctl stop novai-node@<i>
systemctl start novai-node@<i>
journalctl -u novai-node@<i> -f
systemctl cat novai-node@<i>          # inspect ExecStart, data dir, genesis path
```

Supporting services:
```bash
systemctl status novai-price-oracle
systemctl status novai-monitor
journalctl -u novai-price-oracle -f
```

Health read (per node):
```bash
curl -fsS --max-time 2 http://127.0.0.1:$((8080 + <i>))/metrics
curl -fsS --max-time 2 http://127.0.0.1:$((8080 + <i>))/health
```

---

## 18. Appendix B: Read-only verification snippets

Every snippet here is read-only by construction: it inspects and reports, and
changes nothing. None has been executed against the live network by me; treat
them as UNTESTED until you run them. They can be promoted to standalone files
under `scripts/` on request.

### B.1 Pre-wipe key-safety guard

Refuses (exits non-zero) unless the precious oracle secrets are backed up outside
any state directory. Run before any wipe (Scenarios 3, 4, 9, 10). It never writes
or deletes anything.

```bash
#!/usr/bin/env bash
# read-only guard: verifies precious secrets are backed up before any wipe
# usage: BACKUP_DIR=/path/to/offbox/backup bash pre-wipe-guard.sh
set -u
BACKUP_DIR="${BACKUP_DIR:-}"
ok=1
need=( /etc/novai/oracle-keys.json /etc/novai/oracle.env /etc/novai/monitor.env )
if [ -z "$BACKUP_DIR" ]; then
  echo "FAIL: set BACKUP_DIR to an off-box backup location first"; exit 2
fi
case "$BACKUP_DIR" in
  /var/lib/novai*) echo "FAIL: BACKUP_DIR must be outside any state directory"; exit 2;;
esac
for f in "${need[@]}"; do
  base="$(basename "$f")"
  if [ ! -e "$f" ]; then
    echo "WARN: source $f not present (skip if expected)"; continue
  fi
  if [ -e "$BACKUP_DIR/$base" ]; then
    echo "OK: backup present for $base"
  else
    echo "FAIL: no backup of $base in BACKUP_DIR"; ok=0
  fi
done
[ "$ok" = 1 ] && { echo "GUARD PASSED: safe to proceed with a deliberate wipe"; exit 0; } \
              || { echo "GUARD FAILED: back up the missing secrets before wiping"; exit 1; }
```

### B.2 Multi-node health check

Polls all four nodes, prints height, round, peers, mempool, and flags a height
divergence greater than five blocks. Read-only.

```bash
#!/usr/bin/env bash
# read-only: poll all four validators and flag stalls or divergence
set -u
hi=-1; lo=-1
for i in 0 1 2 3; do
  m="$(curl -fsS --max-time 2 "http://127.0.0.1:$((8080 + i))/metrics" 2>/dev/null)"
  if [ -z "$m" ]; then echo "node$i: UNREACHABLE"; continue; fi
  h="$(printf '%s\n' "$m" | awk '/^novai_committed_height /{print $2}')"
  r="$(printf '%s\n' "$m" | awk '/^novai_current_round /{print $2}')"
  p="$(printf '%s\n' "$m" | awk '/^novai_peer_count /{print $2}')"
  q="$(printf '%s\n' "$m" | awk '/^novai_mempool_size /{print $2}')"
  echo "node$i: height=${h:-?} round=${r:-?} peers=${p:-?} mempool=${q:-?}"
  if [ -n "${h:-}" ]; then
    [ "$hi" = -1 ] && hi="$h"; [ "$lo" = -1 ] && lo="$h"
    [ "$h" -gt "$hi" ] 2>/dev/null && hi="$h"
    [ "$h" -lt "$lo" ] 2>/dev/null && lo="$h"
  fi
done
if [ "$hi" != -1 ] && [ "$lo" != -1 ]; then
  spread=$((hi - lo))
  echo "height spread across nodes: $spread"
  [ "$spread" -gt 5 ] && echo "WARN: height divergence greater than 5 blocks"
fi
```

### B.3 Genesis hash comparison

Confirms all nodes share one genesis file and lets you compare against the
canonical published hash. Read-only. Set the genesis path from
`systemctl cat novai-node@0`.

```bash
#!/usr/bin/env bash
# read-only: compare genesis file hashes across nodes
# usage: GENESIS_GLOB='/var/lib/novai/node*/genesis.json' CANONICAL_SHA256=<hash> bash genesis-check.sh
set -u
glob="${GENESIS_GLOB:-/var/lib/novai/node*/genesis.json}"
canon="${CANONICAL_SHA256:-}"
found=0
for f in $glob; do
  [ -e "$f" ] || continue
  found=1
  s="$(sha256sum "$f" | awk '{print $1}')"
  echo "$f : $s"
  [ -n "$canon" ] && { [ "$s" = "$canon" ] && echo "  matches canonical" || echo "  MISMATCH vs canonical"; }
done
[ "$found" = 0 ] && echo "no genesis files matched $glob (adjust the path from systemctl cat)"
```

### B.4 Oracle status

Reports the oracle service state and how far its last submission lags the chain
tip. Read-only.

```bash
#!/usr/bin/env bash
# read-only: oracle liveness and staleness vs chain tip
set -u
echo "service: $(systemctl is-active novai-price-oracle 2>/dev/null)"
om="$(curl -fsS --max-time 2 http://127.0.0.1:9201/metrics 2>/dev/null)"
cm="$(curl -fsS --max-time 2 http://127.0.0.1:8080/metrics 2>/dev/null)"
osub="$(printf '%s\n' "$om" | awk '/^novai_oracle_last_submission_height /{print $2}')"
tip="$(printf '%s\n' "$cm" | awk '/^novai_committed_height /{print $2}')"
echo "oracle last_submission_height: ${osub:-unknown}"
echo "chain committed_height:        ${tip:-unknown}"
if [ -n "${osub:-}" ] && [ -n "${tip:-}" ]; then
  lag=$((tip - osub))
  echo "submission lag (blocks): $lag"
  [ "$lag" -gt 50 ] 2>/dev/null && echo "WARN: oracle submissions are stale"
fi
```

### B.5 Backup integrity check

Verifies a backup archive is readable and contains RocksDB artifacts before you
rely on it. Read-only (lists the archive, does not extract it).

```bash
#!/usr/bin/env bash
# read-only: validate a state backup archive
# usage: bash backup-check.sh <archive.tar.gz>
set -u
ar="${1:-}"
[ -z "$ar" ] && { echo "usage: backup-check.sh <archive.tar.gz>"; exit 2; }
[ -e "$ar" ] || { echo "FAIL: archive not found"; exit 1; }
if tar -tzf "$ar" >/dev/null 2>&1; then
  echo "OK: archive is a readable gzip tar"
else
  echo "FAIL: archive is not readable"; exit 1
fi
if tar -tzf "$ar" 2>/dev/null | grep -Eq 'CURRENT|MANIFEST|\.sst$|\.log$'; then
  echo "OK: archive contains RocksDB artifacts"
else
  echo "WARN: no obvious RocksDB files in archive (verify the contents)"
fi
```

---

## 19. Appendix C: Status summary

Honest status per procedure. No procedure in this document was executed against
the live network by me during authoring.

| Scenario | Status | Basis |
|----------|--------|-------|
| 1 Single validator down | PARTIALLY TESTED | catch-up TESTED per VALIDATOR_COMPROMISE drill; restart UNTESTED by me |
| 2 Full consensus halt | UNTESTED | diagnosis mirrors OPERATOR_RUNBOOK; governance pause BLOCKED |
| 3 Corrupted state, one node | PARTIALLY TESTED | catch-up TESTED; wipe and rejoin UNTESTED by me |
| 4 Cold restart from genesis | UNTESTED | ceremony documented in GENESIS_CEREMONY |
| 5 Bad release rollback | UNTESTED | rolling pattern from OPERATOR_RUNBOOK; param and module rollback BLOCKED |
| 6 Oracle failure and re-bootstrap | PARTIALLY TESTED | bootstrap idempotency TESTED by unit test; live restart UNTESTED by me |
| 7 Genesis mismatch | UNTESTED | read-only hash compare detection |
| 8 Validator key compromise | CROSS-REFERENCE | drills recorded in VALIDATOR_COMPROMISE |
| 9 Manual backup and restore | UNTESTED | shapes from OPERATOR_RUNBOOK |
| 10 Fork detected | UNTESTED | detection coded and chaos-tested; operator response UNTESTED; pause BLOCKED |

---

## 20. Appendix D: Cross-reference map

- Node lifecycle, consensus-stuck, backup and restore, rolling upgrade:
  `docs/OPERATOR_RUNBOOK.md`
- Emergency freeze (BLOCKED governance path): `docs/playbooks/EMERGENCY_FREEZE.md`
- Parameter and module rollback (BLOCKED): `docs/playbooks/ROLLBACK_BAD_PARAM.md`,
  `docs/playbooks/ROLLBACK_BAD_MODULE.md`
- Validator compromise and recorded drills: `docs/playbooks/VALIDATOR_COMPROMISE.md`
- Genesis production and verification: `docs/GENESIS_CEREMONY.md`
- Oracle deployment and funding: `docs/deploy-two-key-oracle.md`,
  `docs/AGENT_FUNDING_PLAYBOOK.md`, `agents/price-oracle/README.md`
- Fault-injection coverage: `docs/CHAOS_TEST_REPORT.md`
- Incident write-up: `docs/POSTMORTEM_TEMPLATE.md`
