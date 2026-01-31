# NOVAI Week 26: Adversarial Privacy Testing Report

**Date**: 2026-01-31
**Week**: 26 - Adversarial Week #2 (Privacy Attacks)
**Status**: COMPLETE

---

## Executive Summary

The NOVAI NNPX privacy system has been subjected to comprehensive adversarial testing covering all major attack vectors against private transaction data, commitment formats, derived views, and AI entity access boundaries. The test suite consists of **35 adversarial tests across 5 test suites**, all passing.

**Key Findings:**
- 0 vulnerabilities discovered
- 1 known gap documented (privacy budget stub)
- All cryptographic formats are fixed-size (no information leakage)
- NNPX boundary enforcement is structurally sound
- Derived view schemas are aggregate-only by design

---

## Test Suites Created

| Test Suite | File | Lines | Tests |
|------------|------|-------|-------|
| A26.1 Timing Correlation | `crates/execution/tests/adversarial_timing_correlation.rs` | 494 | 8 |
| A26.2 Size Leak | `crates/execution/tests/adversarial_size_leak.rs` | 435 | 7 |
| A26.3 Access Pattern Analysis | `crates/execution/tests/adversarial_access_pattern.rs` | 603 | 8 |
| A26.4 Derived View Accumulation | `crates/execution/tests/adversarial_derived_view_accumulation.rs` | 617 | 6 |
| A26.5 Malicious Module | `crates/execution/tests/adversarial_malicious_module.rs` | 461 | 6 |
| **TOTAL** | | **2,610** | **35** |

### Execution Results

```
Running tests/adversarial_timing_correlation.rs
test result: ok. 8 passed; 0 failed; 0 ignored

Running tests/adversarial_size_leak.rs
test result: ok. 7 passed; 0 failed; 0 ignored

Running tests/adversarial_access_pattern.rs
test result: ok. 8 passed; 0 failed; 0 ignored

Running tests/adversarial_derived_view_accumulation.rs
test result: ok. 6 passed; 0 failed; 0 ignored

Running tests/adversarial_malicious_module.rs
test result: ok. 6 passed; 0 failed; 0 ignored
```

**Total: 35 adversarial tests passed | 0 failed**

---

## Attack Scenarios & Results

### A26.1: Timing Correlation Attack

**File**: `crates/execution/tests/adversarial_timing_correlation.rs`
**Tests**: 8
**Result**: SECURE

**Attack**: Attempt to correlate commitment creation times with transaction activity to deanonymize users by linking commitments to specific transactions or addresses.

**Tests Performed:**
| Test | Attack Vector | Result |
|------|--------------|--------|
| `test_commitment_contains_no_block_height` | Check if block height leaks into commitment | SECURE |
| `test_commitment_contains_no_timestamp` | Check if timestamp leaks into commitment | SECURE |
| `test_commitments_created_at_different_times_are_unlinkable` | Link commitments by creation order | SECURE |
| `test_nullifier_contains_no_timing_info` | Check nullifier for timing metadata | SECURE |
| `test_commitment_encoding_is_fixed_not_timestamped` | Encoded format has no time fields | SECURE |
| `test_same_payload_different_secrets_are_unlinkable` | Link same-value transactions | SECURE |
| `test_batch_commitments_are_unlinkable` | Correlate batch-created commitments | SECURE |
| `test_delayed_commitment_publishing_safe` | Infer timing from delayed publication | SECURE |

**Finding**: The `PrivatePayloadCommitment` struct contains no timing metadata (no block height, no timestamp, no sequence number). The commitment format is `[version:1][commitment_hash:32][nullifier:32][encryption_pubkey:32][zk_proof:32]` - purely cryptographic fields. Different secrets produce completely unlinkable commitments even for identical payloads.

---

### A26.2: Size Leak Attack

**File**: `crates/execution/tests/adversarial_size_leak.rs`
**Tests**: 7
**Result**: SECURE

**Attack**: Observe on-chain commitment sizes to infer transaction values, types, or payload sizes. Even a single-byte variation could leak information about the underlying transaction.

**Tests Performed:**
| Test | Attack Vector | Result |
|------|--------------|--------|
| `test_all_commitments_same_encoded_size` | Wide variety of payloads (empty to 1MB) | All 129 bytes |
| `test_small_payload_same_size_as_large` | Compare 1-byte vs 1MB payload | Identical size |
| `test_different_value_amounts_same_commitment_size` | Values from 0 to u128::MAX | All 129 bytes |
| `test_empty_payload_same_size` | Empty vs non-empty payload | Identical size |
| `test_nullifier_fixed_size` | Various secrets and counters | All 32 bytes |
| `test_commitment_hash_fixed_size` | Various payload sizes | All 32 bytes |
| `test_encoded_commitment_golden_size` | Golden vector format lock | Exactly 129 bytes |

**Finding**: All commitments encode to exactly 129 bytes (`PRIVATE_PAYLOAD_COMMITMENT_LEN`). All nullifiers are exactly 32 bytes. All commitment hashes are exactly 32 bytes. The blake3 hash function compresses any input to a fixed 32-byte output, completely hiding payload size. The golden vector test locks the format to prevent accidental introduction of variable-length fields.

**Format**:
```
[version:1][commitment_hash:32][nullifier:32][encryption_pubkey:32][zk_proof:32] = 129 bytes
```

---

### A26.3: Access Pattern Analysis

**File**: `crates/execution/tests/adversarial_access_pattern.rs`
**Tests**: 8
**Result**: SECURE

**Attack**: Analyze AI entity query patterns (which keys they access, how often, in what order) to infer private data. Even if individual queries return aggregate data, the pattern of queries could reveal information.

**Tests Performed:**
| Test | Attack Vector | Result |
|------|--------------|--------|
| `test_ai_entity_blocked_from_all_nnpx_paths` | Access nnpx/commitments, nullifiers, encrypted | All BLOCKED |
| `test_audit_log_reveals_only_view_id` | Check audit entry contents | Only view_id stored |
| `test_derived_view_schemas_are_aggregate_only` | Schema field inspection | No per-address fields |
| `test_capability_bit_cannot_grant_nnpx_access` | Max capabilities vs NNPX boundary | Still BLOCKED |
| `test_account_caller_can_access_nnpx` | Verify accounts are not blocked | ALLOWED (by design) |
| `test_entity_without_derived_cap_blocked` | Missing read_nnpx_derived | BLOCKED |
| `test_key_prefix_boundary_exact` | Edge cases (nnpx, nnpx/, nnpx/x) | Correct boundary |
| `test_query_pattern_cannot_narrow_to_individual` | Repeated queries same view | Same aggregate data |

**Finding**: The `validate_nnpx_access()` function enforces a hard boundary: any key starting with `nnpx/` is denied to `Caller::AiEntity`. This is a structural check - the `Caller` enum is constructed by the host, not by the entity itself, so it cannot be forged. Audit logs store only `(entity_id, block_height) -> view_id`, revealing which view was read but not what data it contained.

---

### A26.4: Derived View Accumulation

**File**: `crates/execution/tests/adversarial_derived_view_accumulation.rs`
**Tests**: 6
**Result**: SECURE (with known gap)

**Attack**: Accumulate multiple derived views over time or across schemas to reconstruct individual private data from aggregates. Cross-reference AggregateVolume, ActivityCount, and PoolSize to narrow down individual transactions.

**Tests Performed:**
| Test | Attack Vector | Result |
|------|--------------|--------|
| `test_single_derived_view_reveals_only_aggregate` | Inspect all 3 schemas for individual data | Aggregate-only |
| `test_multiple_views_same_schema_reveal_nothing_individual` | Diff consecutive volume windows | Only aggregate diff |
| `test_cross_schema_accumulation_reveals_nothing` | Combine volume + count + pool | Only averages/aggregates |
| `test_temporal_accumulation_across_heights` | Track pool deltas over time | Net change only |
| `test_privacy_budget_stub_documented` | Unlimited query exploitation | KNOWN GAP |
| `test_derived_view_cannot_be_parameterized_by_individual` | Inject address into schema data | Schema rejects (fixed size) |

**Finding**: All three schemas contain only aggregate data with no per-address fields:
- `AggregateVolume`: `[start_height:8][end_height:8][total_volume:16]` = 32 bytes
- `ActivityCount`: `[start_height:8][end_height:8][tx_count:8]` = 24 bytes
- `PoolSize`: `[snapshot_height:8][pool_size:16]` = 24 bytes

Cross-schema correlation yields only aggregate statistics (e.g., average transaction size = total_volume / tx_count). The attacker cannot distinguish between scenarios producing identical aggregates (e.g., 500 transactions of 200 each vs 1 transaction of 100,000 + 499 of zero).

Schema validation enforces fixed data sizes, preventing injection of address-filter parameters.

**Known Gap**: Privacy budget is a stub (see dedicated section below).

---

### A26.5: Malicious Module Attack

**File**: `crates/execution/tests/adversarial_malicious_module.rs`
**Tests**: 6
**Result**: SECURE

**Attack**: A malicious WASM module with maximum capabilities attempts to read raw NNPX data, forge its caller identity, or write to private key spaces.

**Tests Performed:**
| Test | Attack Vector | Result |
|------|--------------|--------|
| `test_max_capability_entity_blocked_from_nnpx` | All capabilities enabled, access nnpx/ | BLOCKED |
| `test_caller_enum_forgery_impossible` | Construct Caller::Account from entity | Structurally impossible |
| `test_write_op_to_nnpx_key_detected` | WriteOp targeting nnpx/ keys | Detected by is_nnpx_key() |
| `test_entity_registration_blocks_nnpx_derived_cap` | Register with read_nnpx_derived=true | BLOCKED |
| `test_multiple_entities_isolated` | Cross-entity NNPX access | Each independently blocked |
| `test_entity_cannot_read_other_entity_memory` | Access other entity's ai/memory/ keys | Keys are entity-scoped |

**Finding**: The `Caller` enum (`Account([u8;32])` vs `AiEntity([u8;32])`) is constructed by the execution host, not by the entity. An AI entity cannot forge a `Caller::Account` variant. The NNPX boundary check (`is_nnpx_key()`) operates on raw key bytes, independent of capabilities. Even with all capability bits set to true, an `AiEntity` caller is blocked from `nnpx/` keys. The `validate_ai_entity_no_nnpx_capability()` function prevents registration of entities with `read_nnpx_derived=true`, providing defense-in-depth.

---

## Known Gap: Privacy Budget

### Current State (D23.4 Stub)

The `PrivacyBudget` struct in `crates/ai_entities/src/derived_views.rs` defines the interface but is **NOT ENFORCED**:

| Method | Expected Behavior | Actual Behavior |
|--------|-------------------|-----------------|
| `can_read()` | Return `false` when budget exhausted | Always returns `true` |
| `consume(amount)` | Block when budget insufficient | Records via `saturating_add`/`saturating_sub`, never blocks |
| `replenish(height)` | Refill based on elapsed blocks | Complete no-op |

Constants defined but unused:
- `MAX_PRIVACY_BUDGET = 1000`
- `PRIVACY_BUDGET_PER_VIEW = 1`
- `BUDGET_REPLENISH_RATE = 10`

### Risk

Without budget enforcement, an AI entity with `read_nnpx_derived` capability can issue unlimited derived view queries. Combined with fine-grained temporal windows (e.g., per-block PoolSize snapshots), this could enable accumulation attacks where pool size deltas reveal individual deposit/withdrawal amounts.

The risk is partially mitigated by:
1. Derived views are created periodically, not per-block
2. Each view covers a window of many transactions
3. Audit logs record all reads for forensic analysis

### Recommended Fix (Future Week)

1. `can_read()` -> return `self.available > 0`
2. `consume()` -> return `Result<(), BudgetExhausted>` and fail when `available == 0`
3. `replenish()` -> implement `available += (current_height - last_replenish_height) * BUDGET_REPLENISH_RATE`, capped at `MAX_PRIVACY_BUDGET`
4. Integrate budget check into `read_derived_view_with_audit()` so the execution layer enforces the limit

---

## Defense Summary

| Layer | Protection | Mechanism | Status |
|-------|------------|-----------|--------|
| NNPX Boundary | `Caller::AiEntity` blocked from `nnpx/*` | `validate_nnpx_access()` + `is_nnpx_key()` | SECURE |
| Commitment Format | Fixed 129 bytes, no metadata | blake3 hash compresses all inputs to 32 bytes | SECURE |
| Nullifier Format | Fixed 32 bytes | blake3 with domain separation | SECURE |
| Derived Views | Aggregate-only, fixed-size schemas | Schema validation rejects wrong-size data | SECURE |
| Audit Logs | Only `(entity_id, height) -> view_id` | `create_derived_view_audit_entry()` | SECURE |
| Capability System | `read_nnpx_derived` gated | `validate_derived_view_access()` | SECURE |
| Caller Identity | Host-controlled enum, unforgeable | `Caller::Account` vs `Caller::AiEntity` | SECURE |
| Entity Registration | `read_nnpx_derived=true` blocked | `validate_ai_entity_no_nnpx_capability()` | SECURE |
| Privacy Budget | Rate-limit derived view queries | Stub - not enforced | KNOWN GAP |

---

## Acceptance Criteria Status

| Criterion | Status | Evidence |
|-----------|--------|----------|
| Timing correlation attack tested | TESTED | 8 tests, no timing metadata in commitments |
| Size leak attack tested | TESTED | 7 tests, all outputs fixed-size |
| Access pattern analysis tested | TESTED | 8 tests, hard NNPX boundary enforced |
| Derived view accumulation tested | TESTED | 6 tests, aggregate-only schemas |
| Malicious module attack tested | TESTED | 6 tests, unforgeable Caller enum |
| Known gaps documented | DOCUMENTED | Privacy budget stub (D23.4) |
| All tests passing | VERIFIED | 35/35 tests pass |

---

## Comparison with Week 25

| Metric | Week 25 (Governance) | Week 26 (Privacy) |
|--------|---------------------|-------------------|
| Attack scenarios | 5 | 5 |
| Total tests | 40 | 35 |
| Vulnerabilities found | 4 (2 critical, 2 high) | 0 |
| Known gaps | 0 | 1 (privacy budget) |
| Code changes required | Yes (2 new error types, 3 checks) | No |
| Lines of test code | 2,750 | 2,610 |

The privacy system required no hardening because it was designed with security-first principles: fixed-size cryptographic formats, structural caller identity, and aggregate-only derived views.

---

## Conclusion

Week 26 adversarial testing confirms the NNPX privacy system is robust against the tested attack vectors. The layered defense architecture provides:

- **Confidentiality**: Fixed-size commitments and hashes leak no information about payload size, value, or type
- **Isolation**: Hard NNPX boundary prevents AI entities from accessing raw private data regardless of capabilities
- **Aggregation**: Derived view schemas enforce aggregate-only data with no per-address parameterization
- **Auditability**: All derived view reads are logged for forensic analysis
- **Identity Safety**: Host-controlled `Caller` enum prevents identity forgery

The single known gap (privacy budget stub) should be addressed in a future week to prevent high-frequency query accumulation attacks.

---

Generated: 2026-01-31
Week 26 Adversarial Testing Complete
