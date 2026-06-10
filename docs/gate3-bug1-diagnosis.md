# Gate 3 diagnosis: state_root divergence (Bug 1)

Author: Claude (Opus 4.7 1M ctx), session 2026-06-09
Repo: `~/NOVAI-node` at HEAD `9ac23c4`
Status: writeup-only. NO patch code applied. NO commits.
Awaiting operator approval before Gate 4.

---

## 1. Root cause

The `state_root` computed by the executor authenticates only the writes produced by `apply_tx_v1_transfer_inner`; every other tx-type handler in `crates/execution/src/lib.rs` writes state via direct `db.apply_batch(&ops)` without calling `append_smt_ops_for_state_ops`. Consequently `KEY_SMT_ROOT` reflects only the slice of state touched by Transfer (the AI-sender entity record or the normal-sender account, plus recipient account, plus fee pool), and any drift in non-SMT-authenticated state on a single validator (reverse-index, entity balance / capabilities / code_hash / is_active / last_active_at, signal records, oracle anchors, memory objects, governance state, entity upgrade records) surfaces as a `state_root` divergence on the next Transfer FROM the affected entity because the Transfer's AI-sender branch reads that drifted state and writes it back through `append_smt_ops_for_state_ops`, producing a different SMT root.

This mechanism is proven by harness workloads W11 (reverse-index drift), W12 (economic_balance drift), W16 (capability drift), W17 (code_hash drift), W18 (is_active drift), and W19 (last_active_at drift), each of which injects the named drift on validator 3 only and asserts `assert_ne!(r0, r3)` on the resulting SMT roots. W13 demonstrates that `entity.nonce` drift specifically is absorbed by β4-A's `entity.nonce = tx.nonce + 1` semantic and does not produce SMT divergence.

The original production TRIGGER on validator @3 at block 1745004 (which initial-state and drift mechanism is unknown given no wipe occurred at the 15:40 binary swap and continuous operation through the fork window) is NOT identified in this writeup. The candidate trigger "forced RocksDB compaction at block 1745000 silently dropped a write on @3" was ruled out by R1-R7 in the RocksKv harness, all of which preserve byte-equal state across 4 validators including under uneven and per-validator compaction.

## 2. File:line citations for every handler that will be modified

All citations refer to `crates/execution/src/lib.rs` at HEAD `9ac23c4` (line numbers pre-fix). I will modify exactly the following 13 production sites:

| # | File:line | Handler | State-relevant keys written |
|---|-----------|---------|------------------------------|
| 1 | `lib.rs:6828` | `apply_module_activation` | `ai_entity_key` (entity record with `is_active=true`) |
| 2 | `lib.rs:6852` | `apply_module_rollback` | `ai_entity_key` (entity record with `is_active=false`) |
| 3 | `lib.rs:7042` | `apply_governance_submit_tx` | `governance_proposal_key` |
| 4 | `lib.rs:7126` | `apply_governance_execute_tx` (EmergencyFreeze global kill switch branch) | `KEY_AI_KILL_SWITCH` |
| 5 | `lib.rs:7170` | `apply_governance_execute_tx` (proposal-state update at end) | `governance_proposal_key` |
| 6 | `lib.rs:9008` | `apply_signal_commitment_tx_inner` | `ai_signal_key`, `ai_signal_by_type_key`, `ai_signal_by_issuer_key`, plus per-sub-type: `oracle_anchor_*`, `payment_record_*`, `channel_*`, `sla_*`, `subscription_*`, `ai_entity_key` (post-mutation) |
| 7 | `lib.rs:9146` | `apply_register_ai_entity_tx` (Type 8) | `account_key`, `KEY_FEE_POOL`, `ai_entity_key`, `ai_entity_by_address_key` |
| 8 | `lib.rs:9260` | `apply_credit_ai_entity_tx` (Type 9) | `account_key`, `KEY_FEE_POOL`, `ai_entity_key` |
| 9 | `lib.rs:9407` | `apply_register_ai_entity_with_key_tx` (Type 10) | `account_key`, `KEY_FEE_POOL`, `ai_entity_key`, `ai_entity_by_address_key` |
| 10 | `lib.rs:9527` | `apply_entity_upgrade_tx` (Type 11) | `account_key`, `KEY_FEE_POOL`, `ai_entity_key`, `entity_upgrade_summary_key`, `entity_upgrade_by_entity_key` |
| 11 | `lib.rs:10685` | `apply_create_memory_object_tx_inner` | `ai_memory_object_key`, `ai_memory_by_type_key`, `ai_memory_count_key`, `ai_entity_key`, plus per-type (`sla_*`, `channel_*`, `service_descriptor_by_category_key`, `vk_registry_by_id_key`, `ai_delegation_by_delegate_key`) |
| 12 | `lib.rs:10857` | `apply_update_memory_object_tx_inner` | `ai_memory_object_key`, `ai_entity_key` |
| 13 | `lib.rs:11122` | `apply_delete_memory_object_tx_inner` | `ai_memory_object_key` (delete), `ai_memory_by_type_key` (delete), `ai_memory_count_key`, `ai_entity_key`, plus per-type deletes |

Reference sites that ALREADY call `append_smt_ops_for_state_ops` (no functional change needed; will be refactored to use the same helper for consistency):

| File:line | Handler | Branch |
|-----------|---------|--------|
| `lib.rs:6672` + `:6673` | `apply_tx_v1_transfer_inner` | AI-sender |
| `lib.rs:6716` + `:6717` | `apply_tx_v1_transfer_inner` | normal-sender |

## 3. Estimated diff size per handler

The proposed pattern at each non-Transfer site is a 1-line replacement.

For 8 of the 13 sites, the existing line is:
```rust
db.apply_batch(&ops).map_err(ExecError::Db)?;
```
and it becomes:
```rust
apply_state_ops_with_smt(db, ops)?;
```

For the 5 single-op sites (`apply_module_activation`, `apply_module_rollback`, `apply_governance_submit_tx`, the EmergencyFreeze branch of `apply_governance_execute_tx`, and the final write in `apply_governance_execute_tx`), the existing line is:
```rust
db.apply_batch(&[op]).map_err(ExecError::Db)?;
```
and it becomes:
```rust
apply_state_ops_with_smt(db, vec![op])?;
```

Per-handler delta: **1 line replaced** (no other change to handler logic). Total handler diff: **13 lines replaced** = +13 / -13 = 0 net.

The Transfer refactor: the 2 sites currently look like
```rust
let mut all_ops = ops;
let state_ops_snapshot = all_ops.clone();
let _new_root = append_smt_ops_for_state_ops(db, &state_ops_snapshot, &mut all_ops)?;
db.apply_batch(&all_ops).map_err(ExecError::Db)?;
```
(5 lines each). Refactoring to use the helper makes each 1 line:
```rust
apply_state_ops_with_smt(db, ops)?;
```
Net Transfer delta: +2 / -8 = -6.

The helper itself: ~25 lines including docstring.

**Total `lib.rs` delta: about +27 lines added, -8 lines removed = +19 net.**

## 4. Fix shape proposal

### Option A: Centralized helper (RECOMMENDED)
A single helper function placed adjacent to `append_smt_ops_for_state_ops` at `lib.rs:6517`:

```rust
fn apply_state_ops_with_smt<K: KvBatch>(
    db: &mut K,
    state_ops: Vec<WriteOp>,
) -> Result<(), ExecError<K::Error>> {
    let mut all_ops = state_ops;
    let snapshot = all_ops.clone();
    let _new_root = append_smt_ops_for_state_ops(db, &snapshot, &mut all_ops)?;
    db.apply_batch(&all_ops).map_err(ExecError::Db)
}
```

Every handler that writes state-relevant keys calls this helper.

**Pros:**
- Consistent pattern across all 15 sites (13 newly fixed + 2 Transfer)
- Single audit point: "every state-mutating handler must call `apply_state_ops_with_smt`"
- Future handlers inherit the fix if they use the helper
- Minimal per-site diff (1 line)
- Transfer's 5-line inline pattern collapses to 1 line

**Cons:**
- Adds one layer of indirection over `apply_batch`
- The helper takes `Vec<WriteOp>` by value, so callers must construct a Vec rather than pass a slice. This is a minor ergonomic cost; every existing call site already builds a Vec.

### Option B: Per-handler inline modifications
Copy the existing Transfer inline pattern (5 lines) into each of the 13 sites.

**Pros:**
- No new abstraction
- Each handler is self-explanatory at the call site (no need to read into a helper)
- Exactly matches the existing Transfer pattern

**Cons:**
- ~4 net lines added per site × 13 = +52 LOC of boilerplate
- Inconsistent variable naming across handlers (`ops` vs inlined `vec![op]` vs other names)
- Higher chance of one-off mistakes during review
- Future handlers may omit the pattern by mistake; no compile-time enforcement
- Harder to audit: requires checking each site individually for the SMT walk

### Option C: Hybrid
Helper for vec-form sites, inline pattern for single-op sites.

**Pros:**
- Each site optimized for clarity

**Cons:**
- Two patterns in the codebase, inconsistent
- Reviewers must check both forms
- Loses the audit-by-helper-call benefit
- Marginal complexity savings vs Option A

### Recommendation
**Option A.** The cost (one layer of indirection) is trivial; the benefits (single audit point, consistent pattern, safe-by-default for future handlers, smallest diff) are large. Refactoring Transfer to use the helper is a free net-LOC reduction and removes the only existing variant of the SMT-update pattern.

## 5. Regression test plan

### Existing tests that act as the regression bed (no behavior change expected)

| Test file / workload | Verifies |
|---|---|
| `multi_validator_determinism.rs` W1-W7, W9-W10, W14-W15 | Same-input determinism across 4 MemKv validators must still hold. |
| W11 (reverse-index drift) | Injection on @3 produces SMT root divergence. The fix does not change W11's outcome because the injection bypasses the handler; `assert_ne!(r0, r3)` still fires. |
| W12 (balance drift) | Same as W11 for `entity.economic_balance`. |
| W13 (nonce drift) | β4-A absorbs the drift; SMT roots match. Unchanged. |
| W16 (capability), W17 (code_hash), W18 (is_active), W19 (last_active_at) | Same as W11 for each field. |
| W8 (should_panic positive control) | Harness still detects injected divergence. |
| `multi_validator_determinism_rocksdb.rs` R1-R7 | RocksDB compaction still benign across 4 validators. |
| `crates/execution/tests/determinism.rs` | Transfer-only determinism. Unchanged because Transfer's behavior is unchanged after refactor (helper is semantically equivalent to inline pattern). |
| Most existing tx-handler integration tests (~50 test files) | Each verifies handler-output state (entity records, balances, signal records, memory objects, etc.). Post-fix, those outputs are unchanged; only the additional SMT writes are new. Tests that assert on entity / account / fee_pool / signal / memory state should pass without modification. |

### Existing tests likely to require updates

| Test file | Expected impact |
|---|---|
| `crates/execution/tests/smt_root_recompute_matches.rs` | The "recompute from full state" path likely walks only Transfer-touched keys today. Post-fix, the recompute must walk ALL state-relevant keys. Either the recompute function in this test needs to be updated, or the assertion needs adjustment. **Inspect before fix.** |
| `crates/execution/tests/smt_root_updates.rs` | Likely asserts specific SMT root values after Transfer. May or may not break depending on whether non-Transfer txs run in the same test. **Inspect before fix.** |
| `crates/execution/tests/smt_deterministic_ordering.rs` | Algorithm-level; the SMT itself is unchanged. **Probably safe; verify.** |
| `crates/execution/tests/invariants_v1.rs` | High-level invariants. **Probably safe; verify.** |
| Any test that hardcodes a specific 32-byte `KEY_SMT_ROOT` value via golden vectors | Will need regeneration. **Inspect during fix run.** |

Estimated test updates: **~50 LOC** spread across 1-3 test files. To be confirmed during Gate 4 by running the suite and triaging failures.

### New tests to add (W20-W26 in `multi_validator_determinism.rs`)

| # | Test | Purpose | Est. LOC |
|---|------|---------|----------|
| W20 | `type8_register_changes_smt_root_post_fix` | Capture `KEY_SMT_ROOT` before and after a Type-8 register; assert different. Pre-fix this would equal (no SMT update on register). Proves the fix on register handler. | ~30 |
| W21 | `type10_register_changes_smt_root_post_fix` | Same for Type-10. | ~30 |
| W22 | `signal_commit_changes_smt_root_post_fix` | Same for `apply_signal_commitment_tx_inner` (use OracleAnchor as the concrete signal). | ~35 |
| W23 | `memory_create_changes_smt_root_post_fix` | Same for `apply_create_memory_object_tx_inner`. | ~35 |
| W24 | `memory_update_changes_smt_root_post_fix` | Same for `apply_update_memory_object_tx_inner`. | ~35 |
| W25 | `memory_delete_changes_smt_root_post_fix` | Same for `apply_delete_memory_object_tx_inner`. | ~35 |
| W26 | `credit_ai_entity_changes_smt_root_post_fix` | Same for `apply_credit_ai_entity_tx`. | ~30 |
| W27 | `entity_upgrade_changes_smt_root_post_fix` | Same for `apply_entity_upgrade_tx`. | ~30 |
| W28 | `governance_submit_execute_changes_smt_root_post_fix` | Same for governance Submit + Execute. | ~40 |

Each test follows the same shape: setup → snapshot `KEY_SMT_ROOT` → run the handler → snapshot again → assert different. Optional secondary assertion: run the same workload on all 4 validators and verify the snapshots match across validators (sanity-check that the fix did not introduce non-determinism).

Estimated W20-W28 total: **~300 LOC**.

### Why these tests

Each W20+ test pins ONE specific handler as "now authenticates its writes in the SMT root." Together they form a positive proof that the fix landed on every targeted handler. If a future change accidentally removes the helper call from one handler, the corresponding W test will fail.

W11-W19 are kept unchanged: they continue to prove the MECHANISM (drift in non-SMT-authenticated state causes divergence). After the fix, drift in handler-output state is still possible if injected externally (the injection bypasses the handler), so W11-W19 still demonstrate the mechanism. The fix does not eliminate the mechanism; it ensures that all HANDLER-PRODUCED writes are authenticated.

## 6. Folding latent bug A and latent concern B

### Latent bug A: `KEY_COMMITTED_HEIGHT` non-atomic sync-path write
**Site:** `crates/node/src/consensus_node.rs:954-959` (the sync path's commit cursor write outside any atomic batch, after `execute_committed_blocks` has already mutated state).

**Fix shape:** wrap the block-storage WriteOps and the `KEY_COMMITTED_HEIGHT` Put into a single `db.apply_batch`. The block-storage loop currently at `consensus_node.rs:889` writes blocks one-by-one via `db.put`; refactor that loop to accumulate ops into a Vec, then issue one combined `apply_batch` together with the cursor Put.

**Estimated delta:** ~20 LOC in `consensus_node.rs`. Single function.

**Test coverage:** the existing sync-path tests in the node crate should cover this. May need a new crash-recovery test that explicitly simulates the window. Estimated +30 LOC if added.

**Relation to SMT inclusion fix:** independent. Latent A is a durability / crash-recovery bug; the SMT inclusion fix is a state-commitment scope bug. They share no code paths.

### Latent concern B: missing flush before `compact_range_default`
**Site:** `crates/node/src/main.rs:303-304` (the forced compaction call after the executor's writes, with no preceding flush).

**Fix shape:** call `db.flush_async()` (or expose `flush()` from `RocksKv` if not already exposed) before `db.compact_range_default(...)`. Alternatively configure `set_atomic_flush(true)` on the RocksDB options at `crates/state/src/rocksdb_kv.rs`. The simplest is a flush call.

**Estimated delta:** +5 LOC in `main.rs`, possibly +5 LOC in `rocksdb_kv.rs` if a `pub fn flush(&self)` needs to be added.

**Test coverage:** R1-R7 already validate that compaction does not alter state under no-crash conditions. A crash-recovery test would be needed to validate that compaction-then-crash now preserves writes; this is heavy and arguably out-of-scope.

### How they fold into the patch
Both can be in the same PR but as **separate commits** (see section 8). The SMT inclusion fix is a coordinated chain-level change; latent A and B are independent durability hardening. Mixing them in one commit makes the diff hard to review.

**Recommendation:** include B (5-10 LOC) and A (~20 LOC) as commits 2 and 3 of the same PR. Each is small, independently reviewable, and addresses a real durability gap surfaced during the trigger investigation.

## 7. Estimated total patch size

| Component | File(s) | Added | Removed | Net |
|---|---|---:|---:|---:|
| Helper `apply_state_ops_with_smt` | `lib.rs` | 25 | 0 | +25 |
| Apply helper to 13 sites | `lib.rs` | 13 | 13 | 0 |
| Refactor Transfer to helper | `lib.rs` | 2 | 8 | -6 |
| Existing test updates (SMT recompute, golden vectors) | 1-3 test files | ~50 | ~30 | ~+20 |
| W20-W28 new fix-validation tests | `multi_validator_determinism.rs` | ~300 | 0 | +300 |
| Latent concern B (flush before compact) | `main.rs`, optionally `rocksdb_kv.rs` | ~10 | 0 | +10 |
| Latent bug A (sync-path atomic cursor) | `consensus_node.rs` | ~25 | ~5 | +20 |
| **Subtotal** | **5 files** | **~425** | **~56** | **~+370** |

Net: ~370 LOC across 5 files.

## 8. One commit or staged across multiple commits?

**Recommendation: 3 staged commits in one PR.**

| Commit | Title (proposed) | Scope | Why a separate commit |
|---|---|---|---|
| 1 | `fix(execution): close SMT inclusion gap on every state-mutating handler` | Helper + Transfer refactor + 13 handler call-site changes + test updates + W20-W28 | This is one architectural change. Cannot be split safely without leaving the chain in a half-committed-to-SMT state between commits. |
| 2 | `fix(node): flush RocksDB memtable before forced compaction` | Latent concern B | Small, independent, durability hardening. Reviewable in isolation. |
| 3 | `fix(node): atomically persist KEY_COMMITTED_HEIGHT in sync path` | Latent bug A | Independent crash-recovery fix. Reviewable in isolation. |

**Rationale:** the SMT inclusion fix is architecturally atomic and must ship together. Splitting it across commits would leave intermediate states where some handlers authenticate writes and others do not, defeating the purpose of the fix and making bisection harder.

Latent A and B are independent of each other and of the SMT fix. Each merits its own commit so the operator can review them in isolation, and so a future bisect can land exactly the relevant change.

All three commits go in the same PR because they share a common motivation (the Bug 1 fork investigation) and one deployment cycle.

## 9. Risk assessment

### Risk 1: De-facto hard fork at the SMT level (HIGH)
Every block that includes a non-Transfer tx produces a different `state_root` than the pre-fix binary would. Existing validators on the OLD binary will reject blocks from NEW-binary validators with "State root mismatch" at `consensus/src/lib.rs:412-416`. This is a coordinated chain restart at minimum.

**Mitigation:** the operator already documented wipe-as-default for the post-Bug-1 deployment. The deployment artifact at Gate 6 will reiterate this and verify the procedure step-by-step.

### Risk 2: Performance regression (MEDIUM)
Every non-Transfer handler now walks the SMT, costing additional reads, hashing, and writes per tx. The hot path is memory-CRUD and signal-commitment handlers, which see the largest per-tx work increase.

**Mitigation:** during Gate 5 local verification, run the existing `tx-generator` load test and compare pre- vs post-fix throughput. If the regression exceeds the operator's threshold, profile and consider batching SMT updates across txs in a block (deferred optimization).

### Risk 3: Test cascade (MEDIUM)
A small number of existing tests may hardcode SMT root values or rely on assumptions about which handler writes affect `KEY_SMT_ROOT`. Updating them is mechanical but takes time.

**Mitigation:** before the fix lands, inspect `smt_root_recompute_matches.rs`, `smt_root_updates.rs`, `smt_deterministic_ordering.rs`, `invariants_v1.rs` and any golden vector files. Budget 1-2 hours of Gate 4 time for test updates.

### Risk 4: A handler I missed (LOW-MEDIUM)
My grep covered `db.apply_batch` and `db.put` calls inside `crates/execution/src/lib.rs`. A handler that writes state via a different route (e.g., a helper that does its own `apply_batch`, or a direct `db.put` I missed) would not be caught.

**Mitigation:** during Gate 4, before committing, re-run a comprehensive grep across `crates/execution/`: `db\.apply_batch`, `db\.put`, `db\.delete`. Manually verify every site outside the test mod either calls the helper or is a non-state write.

### Risk 5: Double SMT update per tx (LOW)
`apply_governance_execute_tx` calls `apply_module_activation` or `apply_module_rollback`, each of which calls `apply_state_ops_with_smt` after the fix. Then the outer `apply_governance_execute_tx` calls it again for the proposal-state write at `lib.rs:7170`. That's 2 SMT walks per execute tx (one inner, one outer).

This is CORRECT (each captures state at its moment) but doubles SMT work for execute txs. Performance only, not correctness. Could be optimized later by hoisting the inner apply_module_*'s ops up into the caller's batch.

**Mitigation:** document the double-walk for execute txs in the helper docstring as a known acceptable cost. Optimize in a follow-up if profiling shows it matters.

### Risk 6: Refactoring Transfer introduces a regression (LOW)
If the helper's semantics differ from the inline Transfer pattern in any subtle way, Transfer behavior could change.

**Mitigation:** the helper is a byte-for-byte equivalent transcription of the inline pattern. The existing Transfer tests (`crates/execution/tests/transfer_execution_v1.rs`, `determinism.rs`, dozens of integration tests) will catch any regression. Run them explicitly post-refactor.

### Risk 7: External recomputers of state_root (LOW)
If any RPC, indexer, block explorer, or external tool independently computes `state_root` from a known formula (rather than reading `KEY_SMT_ROOT` directly), it must be updated.

**Mitigation:** grep `crates/` for any `state_root` or `KEY_SMT_ROOT` computation outside the execution and consensus crates. Likely none, but worth verifying.

### Risk 8: SMT overlay store side-effects (LOW)
Each `apply_state_ops_with_smt` call instantiates a new `SmtOverlayStore` and walks the SMT. Repeated calls in the same tx (e.g., the double-walk in execute) are independent walks. If the overlay store has any side-effect that's only safe once per tx, the double-walk could break. Unlikely.

**Mitigation:** read `SmtOverlayStore::new` and confirm it has no external side-effect. Already covered by the existing Transfer pattern (Transfer does one walk per tx, but the helper is called only once per tx in Transfer's case too).

---

## Stopping point

Writeup complete. NO patch code applied. lib.rs reverted to HEAD `9ac23c4`. Awaiting operator review of this document and explicit approval before Gate 4 (patch code) begins.

If approved, Gate 4 plan is:
1. Implement the helper at `lib.rs:6517` adjacent area.
2. Apply to the 13 production sites (single-line replacements per the table in section 2).
3. Refactor Transfer's 2 sites to use the helper.
4. Run full execution-crate test suite; triage and fix failures (anticipated: 1-3 SMT-specific tests need updates).
5. Add W20-W28 new fix-validation tests.
6. Re-run the full harness (W1-W19, R1-R7, W20-W28); confirm all pass.
7. Stage commit 1.
8. Implement latent concern B; stage commit 2.
9. Implement latent bug A; stage commit 3.
10. Run final gates (clippy, fmt, em-dash, full workspace test); proceed to Gate 5.

Stopping now.
