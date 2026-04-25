# Investigation: Block Slowdown + Tx Failures — 2026-04-25

## Symptoms (from operator)

1. **Disk bloat**: 6.3 GB per node (each of 4 nodes) — was ~88 MB stable.
2. **State root mismatch**: validators reject incoming proposals with
   `InvalidBlock("State root mismatch")`.
3. **Tx commits stalled**: `Committed blocks with transactions` count fell to 0 in
   recent 10-second windows; `state_root` field stays constant across blocks
   (no execution side-effects).
4. **Chain still produces blocks** (~70-80 blocks/sec) but they are effectively
   empty for the diverged validators.
5. **Old testnet ran 34M+ blocks stable** before recent security/perf rework.
   New testnet stuck around 4.3M blocks, never recovers.

---

## Diagnosis

### A) Pruning is *adding the deletes* but not *freeing disk space*

`crates/consensus/src/lib.rs:1597-1609` does write `WriteOp::Delete(block_key(h))`
and `WriteOp::Delete(qc_key(h))` for every height that crosses the
`PRUNE_RETAIN_BLOCKS = 100_000` window. The atomic batch is correctly applied
to RocksDB.

**But:**

1. **No explicit compaction.** RocksDB tombstones occupy SST files until a
   compaction visits the relevant ranges. With 4M+ heights of point deletes
   spread across the LSM, background compaction is slow to actually free the
   physical bytes. Searched: zero `compact_range`, zero `delete_range_cf`,
   zero `compact_files` calls anywhere in repo.
2. **SMT nodes are never garbage-collected.** Acknowledged in
   `crates/smt/src/smt.rs:264-267`:
   > L-03: Old nodes from previous updates become unreachable ("dead") but
   > remain in storage indefinitely. SMT compaction/garbage collection is
   > future work.

   Every state-changing tx writes ~256 SMT nodes (path leaf→root). At millions
   of state-changing txs over the chain's lifetime, this is multi-GB of dead
   nodes that will never be reclaimed.
3. **Recent paranoid_checks tail.** `bb0457e` (Apr 20) enabled
   `set_paranoid_checks(true)`; `b105358` (Apr 24) reverted it. During the
   four days in between, RocksDB internal compaction was throttled — those
   tombstones from that window are still on disk waiting for compaction.
4. **Per-block tombstone overhead.** Even after compaction, every committed
   block adds 2 deletes + 4 puts. For 4M heights, that's 24M ops the LSM has
   to materialize.

So the **pruning logic is correct**, but the side effects (SMT growth +
deferred compaction) make disk size grow.

### B) State root mismatch — root cause: `persist_commit_atomic` is **not** atomic with execution

This is the more dangerous bug. Trace the commit pipeline in any of:

- `crates/node/src/consensus_node.rs:1303` (handle_proposal QC catch-up)
- `crates/node/src/consensus_node.rs:1576` (handle_vote — local QC formed)
- `crates/node/src/consensus_node.rs:1671` (handle_qc — peer QC arrived)
- `crates/node/src/consensus_node.rs:898` + `:909` + `:936` (sync)

The pattern is always:

```rust
state.persist_commit_atomic(...)?;          // 1. WRITES committed_height = N
state.apply_commits(...)?;                   // 2. in-memory only
drop(state);
self.execute_committed_blocks(&mut db, ...); // 3. dispatch_tx → updates SMT, accounts
```

`persist_commit_atomic` writes `KEY_COMMITTED_HEIGHT = N` **as part of its
WriteBatch**, before any of block N's transactions have been applied to
account state or to the SMT. Each transaction in `execute_committed_blocks`
does its own `db.apply_batch(...)` — atomic per-tx, but **not** atomic with
the height bump.

**Crash window:** if the process is killed between step 1 and step 3
completing (or anywhere inside step 3 between txs), the database has:

- `committed_height = N` (durable)
- blocks 0..N stored (durable)
- account/SMT state **only reflecting txs up through some block M < N**

`crates/consensus/src/lib.rs:1899` `recover()` reads `committed_height` from
disk and **does not re-execute** blocks. Account/SMT state is silently
divergent from the rest of the network forever after.

Once a validator is in this state, every incoming proposal with
`block.state_root == network's SMT` will fail
`crates/consensus/src/lib.rs:371` because `current_root` (the diverged
local SMT) ≠ network root. That validator votes for nothing, proposes blocks
with the wrong root (which other validators reject), and the chain only
keeps moving because the other 3 nodes form 2f+1 = 3 quorum without it.

**Why was it stable at 34M blocks?** Before `e1d1355` (C-01) and
`bb0457e` (H-09 logging), divergent state could pass through sync
silently, the SMT corruption detection was a silent `Ok(None)`, and the
chain limped along on agreement-by-coincidence. The recent commits closed
those silent paths — the bug was always there, but now it stops the chain
the moment a single node has a bad shutdown.

**Plus the OS itself reported `*** System restart required ***` on the
testnet host.** Any kernel-level reboot, OOM kill, or systemd restart
during commit will land in this window with high probability.

### C) Empty-block effect (compounding factor)

In the recent log snapshots `state_root` is constant for many heights, which
means most blocks are empty (no txs). This is consistent with the H-08
per-sender mempool cap (16 entries) + H-06 RPC error sanitization regression
trail — txgen lost nonce sync intermittently. Empty blocks are **not** the
cause of state-root mismatch (an empty block keeps `state_root` constant on
both sides), but they amplify the symptom: when a tx-bearing block does
finally appear, the diverged validator's SMT diverges further on that single
block and the cascade of mismatch errors begins.

### D) Genesis–SMT inconsistency (latent issue, not cause)

`apply_dev_genesis` (crates/node/src/main.rs:471) writes 100 funded accounts
**directly** as `account_key` puts, bypassing the SMT. All four validators
do the same thing on a fresh DB so they agree among themselves — but the SMT
root never reflects those accounts. Today this is harmless because the
on-chain `state_root` is just "what every validator computes", and they
all compute the same wrong-but-consistent thing. It becomes a real problem
the moment we want to verify a state root against an externally computed
genesis state, and it's worth fixing now while we're touching this.

### E) Non-transfer txs don't update SMT (latent issue)

Only `apply_tx_v1_transfer_inner` calls `append_smt_ops_for_state_ops`.
`apply_signal_commitment_tx`, `apply_register_ai_entity_tx`,
`apply_create_memory_object_tx`, `apply_governance_*` — all write account
state and entity records to the DB without updating the SMT. So the chain's
state_root only authenticates account balances + fee_pool, not entities,
signals, memory, or governance. **This is a real determinism gap** for
auditability but **not** the immediate cause of state-root mismatch. Note
it for follow-up.

---

## Fix Plan

Three batches. Batch 1 + 2 unblock the testnet today. Batch 3 is the
cleanup. Batches 4 / 5 are the latent issues — log them, address later.

### Batch 1 — Crash-safe commit/execute (fixes state-root mismatch)

**Goal:** make commit and execution part of one atomic `WriteBatch` so a
crash either applies both or neither. State on disk can never be ahead of
state in the SMT.

**Approach:** refactor `dispatch_tx` to **return** a `Vec<WriteOp>` instead of
calling `db.apply_batch` itself. The consensus-node commit callback collects
all WriteOps from all txs in all committed blocks, hands them to
`persist_commit_atomic` as the existing `ai_ops` parameter (rename it to
`exec_ops`), and the whole bundle is one atomic batch.

This is a **non-trivial refactor** because every `apply_*_tx` function in
`crates/execution/src/lib.rs` currently calls `db.apply_batch` internally
*and reads from db between its own writes* (e.g.,
`apply_tx_v1_transfer_inner` calls `append_smt_ops_for_state_ops` which
reads SMT nodes). The `SmtOverlayStore` already supports the
read-after-buffered-write semantics — we extend that pattern across whole
blocks.

**Files:**

- `crates/execution/src/lib.rs`: convert each `apply_*_tx` to take an
  `&mut Vec<WriteOp>` (or similar accumulator) and an overlay-aware `Kv`.
  No more `db.apply_batch` inside these functions.
- `crates/node/src/main.rs`: `ExecutionCommitCallback::on_commit` collects
  all WriteOps from the dispatch_tx calls and **returns** them.
- `crates/node/src/consensus_node.rs`: `execute_committed_blocks` returns
  `Vec<WriteOp>`. Each of the four call sites passes them as the
  `ai_ops` (rename: `exec_ops`) argument of `persist_commit_atomic`.
  The call to `execute_committed_blocks` moves to **before**
  `persist_commit_atomic`, and the height-bump + state-bump go into the
  same single batch.

**Risk:** large blast radius — every tx type's apply function changes signature
and every call site updates. Tests will catch most issues but golden vector
tests are the safety net.

**Smaller, lower-risk alternative (recommended for the immediate fix):**
add a separate `KEY_EXECUTED_HEIGHT` durable cursor.

- After `execute_committed_blocks` finishes successfully, write
  `KEY_EXECUTED_HEIGHT = committed_height` in its own atomic batch.
- On startup, if `executed_height < committed_height`, **re-execute**
  blocks `(executed_height+1)..=committed_height` before letting the node
  participate in consensus.
- `recover()` (`crates/consensus/src/lib.rs:1899`) gets a sibling
  `recover_and_replay()` that loads each missing block from disk and
  feeds it through `dispatch_tx`. The result is identical to a node that
  never crashed.

This adds ~80 lines, is O(min(crash window, committed_height) txs) on
restart, and **no** existing apply function or call site changes.

I recommend the cursor approach. It's the right primitive (it's how every
production blockchain handles this), and we can layer the larger refactor
on later as a clean-up.

### Batch 2 — Genesis SMT consistency + state recovery tool

**Goal:** make sure all 4 validators have an *identical* SMT from block 0,
and give us a way to wipe-and-resync any node that's already diverged.

- `apply_dev_genesis`: extend it to also call `append_smt_ops_for_state_ops`
  on the 100 account writes, so the genesis SMT root reflects funded
  accounts. Without this, a future external verifier can't reconstruct
  the chain's expected root from canonical genesis data.
- New `novai-node repair` subcommand: takes a peer URL, wipes the local
  data dir except the keypair, restarts in a clean state, syncs from the
  peer. (Safer than "rm -rf and pray" because the operator doesn't have
  to know the data layout.)

After Batch 1 ships, the operator can run `novai-node repair` on any
validator that's already in the diverged state and bring it back into
the network without touching the others.

### Batch 3 — Disk pruning that actually frees bytes

**Goal:** disk usage at the testnet's PRUNE_RETAIN_BLOCKS=100k window
should be bounded by O(retained blocks + live SMT) — not by the total
chain history.

1. **Force compaction** on the pruned range. After
   `persist_commit_atomic`, every N blocks (e.g. N=10_000) call
   `db.compact_range_cf(cf_default, Some(block_key(0)), Some(block_key(prune_below)))`
   and the same for `qc_key(...)`. Background, but this guarantees
   tombstones get materialized.
2. **Switch point deletes to range deletes** for blocks/QCs:
   `WriteOp::DeleteRange(start, end)` (we'd need to extend the
   `WriteOp` enum and `apply_batch` to support it via
   `WriteBatch::delete_range_cf`). Cheaper at write time, faster
   compaction.
3. **SMT garbage collection** (mark-and-sweep). On a low-water schedule
   (every M blocks, M=100k), walk the live SMT root and mark all
   reachable `smt/node/<hash>` keys; delete the unmarked ones. This is
   the only way to bound long-run disk growth.
   - Mark-and-sweep is O(live tree size) per pass, and O(total nodes)
     per scan. Cheap enough to run as a background task while consensus
     proceeds.
   - **Open question:** SMT-GC has to run on every validator with the
     **same** survivor set, otherwise nodes will see different
     `MissingNode` errors and behavior diverges. Either run it
     deterministically tied to height (`if committed_height % M == 0
     and last_gc_height < committed_height - safety_margin`), or run
     it offline (operator command).

Recommended order: (1) is a one-line fix, ship it now. (2) is a
medium refactor; do after Batch 1. (3) is the big one; design carefully,
ship in a follow-up week.

### Batch 4 — (defer) Real SMT inclusion for non-transfer state

Add `append_smt_ops_for_state_ops` calls to every `apply_*_tx`. Today the
state_root only authenticates accounts + fee_pool. For full chain
auditability, every state record (entities, signals, memory, proposals)
must be committed to the SMT. **This will change the state_root genesis
forward and requires a chain reset.** Log as Week-Nx work.

### Batch 5 — (defer) Recovery hardening

- Periodic invariant check: every M blocks compute a "expected" SMT
  root from the just-committed block.txs and assert against
  KEY_SMT_ROOT. Halts the node loudly on mismatch instead of silently
  diverging.
- Replay-from-block-0 mode for offline integrity verification.

---

## Test Plan

**Unit:**

- New test: kill the process between `persist_commit_atomic` and
  `execute_committed_blocks`. Restart. Verify state matches a node that
  never crashed.
- New test: `KEY_EXECUTED_HEIGHT` cursor advances monotonically and equals
  `KEY_COMMITTED_HEIGHT` whenever the node is idle.
- Existing: every `apply_*_tx` test passes unchanged (cursor is additive).

**Integration:**

- `crates/consensus/tests/recovery.rs`: add a "kill mid-execution" case.
- 4-node devnet smoke: run for 1M blocks under simulated SIGKILLs every
  5k heights. After each restart, verify state_root agrees across all
  nodes.

**On-chain verification of fix (operator side):**

- `cargo test --workspace`
- Wipe testnet, redeploy, run for 100k blocks, verify zero
  "State root mismatch" errors in any node's journal.
- Memory + disk: every 10k blocks, log `du -sh /var/lib/novai/node*`
  and `ps aux | grep validator`. Memory should plateau, disk should
  bound near 100k blocks worth.

---

## What I'm asking for approval on

1. **Approach for Batch 1**: cursor (`KEY_EXECUTED_HEIGHT` + replay-on-startup)
   — the smaller fix. Or do you want the full atomic-batch refactor instead?
2. **Should Batch 2 (genesis SMT consistency) ship in the same patch as
   Batch 1?** It changes the genesis state root, so all running validators
   must wipe and resync once it ships.
3. **Order of Batch 3 sub-items**: I recommend (1) compact-range trigger
   first, (2) delete-range refactor second, (3) SMT GC as a separate week
   later. Confirm?
4. **Confirm scope of immediate work**: Batch 1 + Batch 2 + Batch 3.1
   (compact-range trigger) for this PR. Batch 3.2, 3.3, 4, 5 deferred.

I will **not** start writing code until you confirm. Please respond with
specific direction on each of the four points above (or "go ahead with the
recommendation").
