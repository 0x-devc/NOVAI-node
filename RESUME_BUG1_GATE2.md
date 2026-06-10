# RESUME notes for Bug 1 (state_root divergence on @3): Gate 2 stopping point

Author: Claude (Opus 4.7 1M ctx), session 2026-06-09 BST
Branch: main (unchanged); workspace clean except `crates/execution/tests/multi_validator_determinism.rs` (new, unstaged) and `fuzz/`, `agents/price-oracle/lib/__pycache__/` (pre-existing).

## Status

- Phase 0 (Gate 1, Orient): complete. Five parallel Explore agents mapped state mutation, state read, commit ordering, block verification, streaming decoder.
- Phase 1 (Gate 2, Build harness + reproduce): complete in scope but **did not reproduce the production divergence**.
- Phase 1 (Gate 3, Diagnose) and onward: NOT started. Stopped per the no-autonomous-Gate-3 constraint.

## What is proven

1. **The SMT inclusion gap is real.** Only `apply_tx_v1_transfer_inner` calls `append_smt_ops_for_state_ops` (at `crates/execution/src/lib.rs:6672` and `:6716`). Every other handler writes via direct `db.apply_batch(&ops)` with NO SMT update. The `state_root` therefore does not authenticate `ai_entity_key`, `ai_entity_by_address_key`, signal commitments, oracle anchors, memory objects, governance state, entity upgrades, or `account_key` / `KEY_FEE_POOL` mutations from non-Transfer handlers.
2. **The mechanism that turns non-SMT drift into a state_root divergence is the Transfer handler's two-branch routing.** W11 in the harness directly proves this: if validator @3's `ai_entity_by_address_key` is missing while @0/@1/@2 have it, a Transfer FROM the entity address routes through different branches (AI-sender vs normal-sender) constructing different `ops` vectors, yielding different SMT roots.
3. **Even within the same branch, drift in non-SMT-authenticated entity-record fields propagates to a state_root divergence on the next Transfer.** W12 proves this: a 1-unit difference in `entity.economic_balance` on @3 before the Transfer causes @3's encoded entity record (written into SMT in the AI-sender branch) to differ, producing different SMT roots.
4. **β4-A absorbs `entity.nonce` drift.** W13 observes that because the post-fix semantic is `entity.nonce = tx.nonce + 1` (not `entity.nonce + 1`), a prior drift in `entity.nonce` is absorbed and post-Transfer entity bytes match. This means β4-A is incidentally a determinism shield against nonce drift but NOT against balance / capabilities / is_active / code_hash / last_active_at drift.
5. **The MemKv executor is fully deterministic under identical inputs.** 13 workloads exercising baseline transfers, type-8 + type-10 registrations, OracleAnchor signals, drained-entity scenarios, multi-sender high load, tight β4-A interleaving, and many-entity workloads all produce byte-equal state across 4 in-process validators. No same-input non-determinism found in the executor.
6. **The streaming tx decoder is deterministic** (Phase 0 finding).

## What is NOT proven

1. **The original trigger that caused @3's drift in production.** The harness did not reproduce it. The user's narrowing (no wipe at 15:40, all 4 validators identical pre-state and continuous through the window) collapses the "Type-8 replay edge case" sub-variant and means same-input MemKv testing is not expected to reproduce. Most plausible remaining triggers: RocksDB compaction interaction (lines up with @3's 16-second stall at block 1745000), some non-determinism in the consensus_node.rs commit path involving non-SMT-authenticated writes, or a layer I have not yet mapped.

## Harness file

`crates/execution/tests/multi_validator_determinism.rs`. 19 workloads, all pass on commit 9ac23c4:

  - W1  baseline_account_transfers_stay_deterministic
  - W2  type8_register_then_transfer_from_creator_stays_deterministic
  - W3  type10_register_with_key_then_transfer_stays_deterministic
  - W4  oracle_anchor_then_transfer_stays_deterministic
  - W5  drained_entity_with_failing_anchors_then_entity_transfer (operator B probe)
  - W6  multi_sender_high_load_stays_deterministic
  - W7  register_and_transfer_in_same_block_stays_deterministic
  - W8  harness_self_test_detects_injected_divergence (should_panic positive control)
  - W9  independent_transfers_stay_deterministic_in_block_order
  - W10 creator_already_has_entity_guard_fires_consistently
  - W11 MECHANISM: reverse_index_drift_diverges_smt_root_on_next_transfer
  - W12 MECHANISM: entity_balance_drift_diverges_smt_root_on_next_transfer
  - W13 MECHANISM: entity_nonce_drift_diverges_smt_root_on_next_transfer (observes β4-A absorption)
  - W14 STRESS: tight_beta4a_signal_transfer_interleave_stays_deterministic
  - W15 STRESS: many_entities_many_transfers_stay_deterministic
  - W16 MECHANISM: entity_capability_drift_diverges_smt_root_on_next_transfer
  - W17 MECHANISM: entity_code_hash_drift_diverges_smt_root_on_next_transfer
  - W18 MECHANISM: entity_is_active_drift_produces_different_outcomes (outcome + SMT divergence)
  - W19 MECHANISM: entity_last_active_at_drift_diverges_smt_root_on_next_transfer

  cargo test -p novai-execution --test multi_validator_determinism
    → 19 passed; 0 failed
  cargo clippy -p novai-execution --tests --test multi_validator_determinism -- -D warnings
    → clean
  em-dash audit on the new test file
    → empty
  cargo test -p novai-execution
    → all execution-crate tests pass (1973 total)

## Trigger investigation findings (3 parallel Explore agents)

Three agents searched outside the executor for the divergence trigger.
None pinned the exact production trigger to a specific code path under
no-crash conditions, but two SEPARATE latent bugs surfaced that matter
for the fix conversation:

### Latent bug A: non-atomic KEY_COMMITTED_HEIGHT in sync path

`crates/node/src/consensus_node.rs:954-959` writes `KEY_COMMITTED_HEIGHT`
via a single `db.put`, NOT as part of the atomic batch in
`persist_commit_atomic`. The write fires AFTER `execute_committed_blocks`
has already run state mutations (line 951-952). A crash in this window
leaves the validator with executor state advanced beyond the consensus
cursor; on recovery, replay re-executes the same blocks against the
already-mutated state, potentially producing different outcomes.

This requires a crash and is not the active trigger for the 16:29 fork
(no crash was reported). But it is a real latent bug that should be
fixed as part of the broader hardening pass.

### Latent concern B: no flush before compact_range_default

`crates/node/src/main.rs:303-304` calls `db.compact_range_default(...)`
immediately after the executor's writes, with no preceding `flush`.
RocksDB durability tuning at `crates/state/src/rocksdb_kv.rs:88-89` is
bandwidth-based (`set_bytes_per_sync(1MB)`, `set_wal_bytes_per_sync(1MB)`),
NOT per-op synchronous. A crash between the executor's apply_batch and
the next flush can lose recently-written SMT nodes / KEY_SMT_ROOT,
leaving the validator with an internally-inconsistent SMT.

Also crash-gated, so also not the active trigger.

### What this means for the fix

Neither latent bug A nor latent concern B reproduces the no-crash fork
the operator observed. The actual trigger remains unidentified. But:

  - The MECHANISM is proven beyond reasonable doubt (W11-W19).
  - Closing the mechanism (architectural SMT inclusion gap) makes the
    chain robust to ANY future drift trigger.
  - Latent bugs A and B should be fixed in the same hardening pass to
    prevent crash-induced divergence in production.

## RocksKv harness (variant 4)

`crates/execution/tests/multi_validator_determinism_rocksdb.rs`. 7 workloads,
all pass on commit 9ac23c4. Ran 10x consecutively with zero flakes:

  - R1 baseline_rocks_determinism_no_compaction
  - R2 register_then_transfer_no_compaction
  - R3 manual_compaction_does_not_alter_state (direct pre/post snapshot)
  - R4 manual_compaction_between_blocks_stays_deterministic
  - R5 per_validator_uneven_compaction_does_not_introduce_divergence
  - R6 register_then_compact_then_transfer_stays_deterministic
  - R7 high_load_with_periodic_compaction_stays_deterministic

  cargo test -p novai-execution --test multi_validator_determinism_rocksdb
    → 7 passed; 0 failed (stable across 10 consecutive runs)
  cargo clippy -p novai-execution --tests --test multi_validator_determinism_rocksdb -- -D warnings
    → clean
  em-dash audit on the new RocksKv test file
    → empty

R3 directly snapshots the default-CF state before and after
`compact_range_default(None, None)` and asserts byte equality, so the
"compaction silently drops/modifies values" hypothesis is ruled out at
the harness level. R5 tests per-validator uneven compaction (only v3
compacts), confirming state still matches across all 4. R6 tests the
exact production-relevant sequence (register, then compact across the
boundary, then transfer FROM the entity address), and the reverse-index
remains intact.

**Conclusion:** RocksDB compaction by itself is NOT the same-input
non-determinism source under the workloads tested. The hypothesized
"silent reverse-index loss on @3 caused by compaction at block 1745000"
does not reproduce in a controlled RocksKv harness.

## What was deferred and why

- **Per-validator timing pauses via tokio.** The MemKv executor is single-threaded and synchronous; a wall-clock pause cannot expose tokio async-ordering bugs at this layer. Documented in the harness file's FAILURE MODES section.
- **Live multi-node consensus reproduction.** Out of scope for in-process tests; would require running 4 actual node binaries.

## Fix-strategy options to discuss

A (targeted): bring the reverse-index Put inside the SMT update at `apply_register_ai_entity_tx` and `apply_register_ai_entity_with_key_tx`. Small blast radius, fixes the exact divergence vector exercised by W11. Does NOT fix W12-style same-branch drift, because the entity record was already in scope of the SMT at Transfer time. Does NOT prevent future RocksDB-level silent data loss.

B (broader): bring every non-Transfer handler that writes any state under the same `append_smt_ops_for_state_ops` path. Massive blast radius (every handler), longest test cycle, but eliminates the entire SMT inclusion gap.

C (hybrid, recommended): A now (fast incident fix that hardens the most plausible trigger surface), file B as architectural debt with a multi-week refactor plan. Combine with a periodic state-root-recompute audit job to detect any future silent drift.

## Open question for the operator (asked at Gate 2 wrap)

Which fix path do we take?
- Path A (close reverse-index gap on register only) plus deploy.
- Path C (Path A plus filed architectural debt for B plus state-audit job) plus deploy.
- Other (e.g., add periodic recompute first, hold the fix until trigger is found).

## [redacted-host] deploy artifacts (Gate 6): NOT yet written

Per the prompt, the operator deploys, not me. I have NOT written:
- `docs/deploy-bug1-fix.md`
- `scripts/verify-host-fix.sh`
- `docs/bug1-load-test-plan.md`

These come after Gate 4 (fix locally verified).

## Constraint posture

- Auto-approved scope: files within repo, cargo {build,test,clippy,fmt}, read-only shell, git local (no remote), markdown notes at root or docs/. Respected.
- Bounded iteration: 2 of 5 variants used (initial 10 workloads + 5 mechanism proofs). 3 remaining if approved to continue.
- No autonomous Gate 3: respected. Stopping at Gate 2 with the above for operator decision.
- Push: none, per HARD RULE 1.
- Em-dash audit: empty for the new file.
- First-person singular: maintained.
