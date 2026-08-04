//! Gate ACCEL Stage B: the per-block execution applier.
//!
//! The single choke point for applying a committed block's execution output to
//! durable state (docs/gate-accel-stageB-execution-batching-design.md, section
//! 2.4). The commit callback and boot replay both route through it, so the
//! commit-path and replay-path write shapes cannot drift.

use novai_consensus::PendingExec;
use novai_consensus_types::Block;
use novai_execution::TxOutcome;
use novai_state::{KvBatch, WriteOp, KEY_EXECUTED_HEIGHT};

/// A block's vote-time execution handed to the commit callback (gate ACCEL
/// Stage B): the cached write set flattened to apply-ready sorted ops, plus
/// the per-tx outcomes for log parity. Produced from a `PendingExec` the
/// commit site took via `ConsensusState::take_pending_execs`.
pub struct CachedExec {
    pub write_ops: Vec<WriteOp>,
    pub outcomes: Vec<TxOutcome>,
}

impl CachedExec {
    /// Flatten a taken `PendingExec` into apply-ready ops. `BTreeMap`
    /// iteration is sorted, so the op order matches
    /// `BlockExecution::write_ops` exactly. Returns `None` when the write set
    /// was not retained; the caller treats that as a re-execution cache miss.
    #[must_use]
    pub fn from_pending(pe: PendingExec) -> Option<Self> {
        let ws = pe.write_set?;
        let write_ops = ws
            .into_iter()
            .map(|(k, v)| match v {
                Some(val) => WriteOp::Put(k, val),
                None => WriteOp::Delete(k),
            })
            .collect();
        Some(Self {
            write_ops,
            outcomes: pe.outcomes,
        })
    }
}

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

/// Resolve a committed block's execution and apply it as one batch: the
/// production commit core (gate ACCEL Stage B). A cached hit applies the
/// vote-time write set as-is (its binding to this block and to the current
/// parent state was verified by the caller: `take_pending_execs` checked the
/// hash, the header-bound post root, and the write-set checksum, and
/// `execute_committed_blocks` checked the parent binding). A miss re-executes
/// the block once in the non-persisting overlay over the committed database
/// and REFUSES BEFORE APPLYING if the computed post root does not match the
/// committed header, leaving state untouched (strictly earlier than the
/// apply-then-detect shape this replaces; the outer lag-0 post-execution
/// check in `execute_committed_blocks` remains as the unchanged second belt).
///
/// Returns the per-tx outcomes for the caller's logging and side effects.
///
/// # Errors
/// Returns the CONSENSUS SAFETY HALT error on a miss-path root mismatch, or
/// a formatted error if re-execution or the batch write fails.
pub fn resolve_and_apply_block<K: KvBatch>(
    db: &mut K,
    block: &Block,
    cached: Option<CachedExec>,
) -> Result<Vec<TxOutcome>, String>
where
    K::Error: std::fmt::Debug,
{
    let (write_ops, outcomes) = match cached {
        Some(c) => (c.write_ops, c.outcomes),
        None => {
            let exec = novai_execution::execute_block_to_root(&*db, &block.txs, block.height)
                .map_err(|e| {
                    format!(
                        "Commit re-execution failed at height {}: {e:?}",
                        block.height
                    )
                })?;
            if exec.post_root != block.state_root {
                return Err(format!(
                    "CONSENSUS SAFETY HALT: pre-apply state root mismatch at height {} \
                     (computed={}, header={}). Local execution diverged from the committed \
                     header; refusing to apply. Reseed from a good snapshot or wipe the data \
                     dir and resync from peers.",
                    block.height,
                    hex::encode(&exec.post_root[..8]),
                    hex::encode(&block.state_root[..8]),
                ));
            }
            let write_ops = exec.write_ops();
            (write_ops, exec.outcomes)
        }
    };
    apply_block_execution(db, block.height, write_ops)?;
    Ok(outcomes)
}
