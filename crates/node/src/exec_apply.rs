//! Gate ACCEL Stage B: the per-block execution applier.
//!
//! The single choke point for applying a committed block's execution output to
//! durable state (docs/gate-accel-stageB-execution-batching-design.md, section
//! 2.4). The commit callback and boot replay both route through it, so the
//! commit-path and replay-path write shapes cannot drift.

use novai_state::{KvBatch, WriteOp, KEY_EXECUTED_HEIGHT};

/// Apply one committed block's execution output as ONE atomic write batch: the
/// block's write-set ops (rows, SMT node records, and the final `KEY_SMT_ROOT`
/// when the set is non-empty) plus the `KEY_EXECUTED_HEIGHT` cursor for the
/// block's height. All-or-nothing per block: rows, root, and cursor move
/// together or not at all, so the rows-without-root crash split is impossible
/// by construction, and the executed cursor is atomic with the state it
/// describes (closes accel-C S6).
///
/// This is a SECOND write beside `persist_commit_atomic`'s commit batch,
/// issued under the same held db lock in today's exact ordering. It must NEVER
/// be merged into the commit batch (WEDGE-20260718 class): a poisoned or
/// failed execution write must not be able to veto or corrupt a consensus
/// commit; it errors AFTER the commit batch is durable, the node freezes
/// fail-closed, and replay self-heals on restart.
///
/// For an empty write set (an empty block, or a block whose every tx was
/// skipped) the batch is the cursor put alone: no root rewrite, preserving
/// today's byte behavior where such blocks touch nothing but the cursor.
///
/// # Errors
/// Returns a formatted error if the batch write fails; callers propagate it on
/// the same channel as today's execution errors.
pub fn apply_block_execution<K: KvBatch>(
    db: &mut K,
    block_height: u64,
    exec_write_ops: Vec<WriteOp>,
) -> Result<(), String>
where
    K::Error: std::fmt::Debug,
{
    let mut ops = exec_write_ops;
    ops.push(WriteOp::Put(
        KEY_EXECUTED_HEIGHT.to_vec(),
        block_height.to_be_bytes().to_vec(),
    ));
    db.apply_batch(&ops)
        .map_err(|e| format!("Block execution batch failed at height {block_height}: {e:?}"))
}
