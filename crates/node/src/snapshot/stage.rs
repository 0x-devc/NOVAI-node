//! Gate F5 Stage 2: materialising a bundle into a staging directory.
//!
//! SCOPE. This builds a directory and stops. It does not rename anything, does
//! not touch a live data directory, does not write an install marker and does
//! not restart anything. The install (the boot-time atomic rename, the
//! preserved old directory, the crash-idempotent completion rule) is Stage 3
//! and is separately gated.
//!
//! Stage 2 needs this much because the gate's equivalence claim is exactly
//! "a bundle materialised into a directory audits to PASS with the SAME height
//! and root as the source". Without a materialiser that claim cannot be
//! executed, and an unexecuted equivalence claim is where a snapshot design
//! quietly goes wrong.
//!
//! The SMT is rebuilt here from the leaves rather than shipped: the receiver
//! never reads the sender's internal nodes, so a forged node cannot survive.
//! The walk is driven through `append_smt_ops_for_state_ops`, the same
//! canonical execution path genesis and every state handler use, so the
//! materialised root is produced by the node's own code and not by a parallel
//! implementation that could drift.

use std::path::Path;

use novai_execution::append_smt_ops_for_state_ops;
use novai_state::{
    block_key, qc_key, KvBatch, RocksKv, WriteOp, KEY_COMMITTED_HEIGHT, KEY_EXECUTED_HEIGHT,
    KEY_HIGHEST_QC, KEY_LOCKED_QC,
};

use crate::snapshot::bundle::SnapshotBundle;

#[derive(Debug)]
pub enum StageError {
    Io(String),
    DigestMismatch(String),
    Decode(String),
    NotEmpty(String),
}

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "staging io: {e}"),
            Self::DigestMismatch(e) => write!(f, "staging refused, chunk integrity: {e}"),
            Self::Decode(e) => write!(f, "staging refused, decode: {e}"),
            Self::NotEmpty(p) => write!(
                f,
                "staging refused: {p} already holds data; a snapshot is only ever \
                 installed into a FRESH directory, because a surviving stale flat row \
                 would be read by execution and diverge the state"
            ),
        }
    }
}

/// Materialise `bundle` into a fresh directory at `dir`.
///
/// The caller audits the result; this function deliberately does not, so that
/// the audit at the gate is run against the bytes on disk rather than against
/// an in-memory belief about them.
///
/// # Errors
/// Refuses a directory that already holds a database, a chunk that fails its
/// manifest digest, an undecodable chunk, or any write failure.
pub fn materialize(bundle: &SnapshotBundle, dir: &Path) -> Result<(), StageError> {
    // Integrity before anything is written. A chunk that does not match the
    // digest the manifest claims never reaches disk.
    bundle.verify_digests().map_err(StageError::DigestMismatch)?;

    if dir.join("CURRENT").exists() {
        return Err(StageError::NotEmpty(dir.display().to_string()));
    }
    std::fs::create_dir_all(dir).map_err(|e| StageError::Io(format!("create {}: {e}", dir.display())))?;

    let pairs = bundle
        .pairs()
        .map_err(|e| StageError::Decode(e.to_string()))?;
    if pairs.len() != bundle.manifest.leaf_count as usize {
        return Err(StageError::Decode(format!(
            "manifest claims {} leaves, chunks carry {}",
            bundle.manifest.leaf_count,
            pairs.len()
        )));
    }

    let mut db = RocksKv::open(dir).map_err(|e| StageError::Io(format!("open staging: {e:?}")))?;

    // Leaves plus the tree, one walk per pair, matching the node's own
    // per-transaction batching. The final KEY_SMT_ROOT falls out of the last
    // walk; it is never written by hand, so it cannot disagree with the tree.
    for (k, v) in &pairs {
        let state_ops = vec![WriteOp::Put(k.clone(), v.clone())];
        let mut all_ops = state_ops.clone();
        append_smt_ops_for_state_ops(&db, &state_ops, &mut all_ops)
            .map_err(|e| StageError::Io(format!("smt walk for a leaf: {e:?}")))?;
        db.apply_batch(&all_ops)
            .map_err(|e| StageError::Io(format!("apply leaf batch: {e:?}")))?;
    }

    let m = &bundle.manifest;
    let mut ops = vec![
        // Both cursors at H. Equal is load bearing twice over: the boot replay
        // then loops over an empty range and cannot reach its fatal missing-block
        // path, and A1 treats any inequality as a torn capture.
        WriteOp::Put(KEY_COMMITTED_HEIGHT.to_vec(), m.height.to_be_bytes().to_vec()),
        WriteOp::Put(KEY_EXECUTED_HEIGHT.to_vec(), m.height.to_be_bytes().to_vec()),
    ];
    ops.push(WriteOp::Put(
        block_key(m.height),
        encode_block(&m.block_h)?,
    ));
    ops.push(WriteOp::Put(
        block_key(m.height + 1),
        encode_block(&m.block_h1)?,
    ));
    let qc_h1_bytes = encode_qc(&m.qc_h1)?;
    ops.push(WriteOp::Put(qc_key(m.height + 1), qc_h1_bytes.clone()));
    // The certifying QC is both the highest QC and the lock. Stage 3 will carry
    // max(own, donor) for the lock and the vote mark; Stage 2 writes only what
    // the bundle itself carries, because it has no node identity to merge with.
    ops.push(WriteOp::Put(KEY_HIGHEST_QC.to_vec(), qc_h1_bytes.clone()));
    ops.push(WriteOp::Put(KEY_LOCKED_QC.to_vec(), qc_h1_bytes));
    if let Some(qc_h) = &m.qc_h {
        ops.push(WriteOp::Put(qc_key(m.height), encode_qc(qc_h)?));
    }

    db.apply_batch(&ops)
        .map_err(|e| StageError::Io(format!("apply consensus rows: {e:?}")))?;

    Ok(())
}

fn encode_block(b: &novai_consensus_types::Block) -> Result<Vec<u8>, StageError> {
    novai_consensus_types::codec::encode_block_v1(b)
        .map_err(|e| StageError::Io(format!("encode block: {e:?}")))
}

fn encode_qc(q: &novai_consensus_types::QC) -> Result<Vec<u8>, StageError> {
    novai_consensus_types::codec::encode_qc_v1(q)
        .map_err(|e| StageError::Io(format!("encode qc: {e:?}")))
}
