//! Gate F5 Stage 2: turning a checkpoint into a servable bundle.
//!
//! THE COMMIT-PATH PROPERTY. Everything in this module takes a filesystem
//! PATH and nothing else. It has no handle to the live database, no `Storage`,
//! no `Arc<Mutex<..>>`, and it cannot acquire the node's database lock because
//! it is never given anything that holds one. That is the type-level reason
//! the expensive work (a full key scan, the classification pass, the
//! from-scratch SMT rebuild inside the audit) cannot run on the commit path.
//! The commit path's entire contribution is creating the checkpoint this
//! module later opens, which is a memtable flush plus hard links.
//!
//! THE FAIL-CLOSED PROPERTY. Production runs the FULL A0 audit against the
//! checkpoint before a single byte is cached as servable, and refuses on any
//! failed check. A node never serves state it has not proven. Separately, the
//! leaf extraction refuses any key that classifies as unknown or as
//! defined-but-unwritten, rather than dropping it: dropping a leaf produces a
//! wrong root, which is a silent corruption, while refusing produces a loud
//! availability failure that names the key. That trade is the whole reason the
//! rule exists.

use std::path::Path;

use novai_consensus::ConsensusState;
use novai_consensus_types::{Block, QC};
use novai_state::{Kv, RocksKv, KEY_COMMITTED_HEIGHT, KEY_EXECUTED_HEIGHT};

use crate::snapshot::audit;
use crate::snapshot::bundle::{
    chunk_digest, chunk_pairs, BundleError, FlatPairs, SnapshotBundle, SnapshotManifest,
    SNAPSHOT_FORMAT_VERSION,
};
use crate::snapshot::classify::{classify, Class};

#[derive(Debug)]
pub enum ProduceError {
    /// The checkpoint could not be opened or read.
    Io(String),
    /// The mandatory self-audit did not pass. Production stops: a node never
    /// serves state it has not proven.
    AuditFailed {
        result_line: String,
        failures: Vec<String>,
    },
    /// The cursors disagree, so the checkpoint is not at a clean block
    /// boundary. Redundant with the audit's A1 and deliberately so: the
    /// extraction must not depend on check ordering inside another module.
    CursorsDiffer {
        committed: Option<u64>,
        executed: Option<u64>,
    },
    /// A key matched nothing in the classification table, or matched a family
    /// with no production writer at this HEAD. Fail closed, named.
    UnclassifiableKey { key: String, class: &'static str },
    /// A row the certification chain needs is missing from the checkpoint.
    MissingEvidence(String),
    /// A chunk exceeds what the wire will carry. Fails at production so the
    /// limit is discovered once, named, rather than every time a send is tried.
    ChunkTooLarge {
        index: usize,
        len: usize,
        max: usize,
    },
    Bundle(BundleError),
}

impl std::fmt::Display for ProduceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "snapshot production io: {e}"),
            Self::AuditFailed {
                result_line,
                failures,
            } => write!(
                f,
                "snapshot production refused: the self-audit did not pass ({result_line}); {}",
                failures.join("; ")
            ),
            Self::CursorsDiffer {
                committed,
                executed,
            } => write!(
                f,
                "snapshot production refused: committed={committed:?} executed={executed:?}, \
                 the checkpoint is not at a clean block boundary"
            ),
            Self::UnclassifiableKey { key, class } => write!(
                f,
                "snapshot production refused: key {key} is {class}; refusing to drop it, \
                 because a dropped leaf is a wrong root"
            ),
            Self::MissingEvidence(e) => {
                write!(f, "snapshot production refused: missing evidence: {e}")
            }
            Self::ChunkTooLarge { index, len, max } => write!(
                f,
                "snapshot production refused: chunk {index} is {len} bytes, above the {max} \
                 byte wire bound; a single state value is too large to transfer"
            ),
            Self::Bundle(e) => write!(f, "snapshot production: {e}"),
        }
    }
}

fn read_cursor(db: &RocksKv, key: &[u8]) -> Option<u64> {
    match db.get(key) {
        Ok(Some(b)) if b.len() == 8 => {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b);
            Some(u64::from_be_bytes(a))
        }
        _ => None,
    }
}

/// Build a servable bundle from a checkpoint directory.
///
/// The checkpoint must be an offline copy, never a live data directory:
/// opening it takes RocksDB's lock. The caller creates it with
/// `RocksKv::create_checkpoint` and deletes it afterwards.
///
/// # Errors
/// Returns the first refusal, all of which are fail-closed: a failed audit,
/// disagreeing cursors, an unclassifiable key, missing certification evidence,
/// or an encoding failure. None of them can produce a partial or "best effort"
/// bundle.
pub fn build_bundle(checkpoint_dir: &Path) -> Result<SnapshotBundle, ProduceError> {
    let path = checkpoint_dir
        .to_str()
        .ok_or_else(|| ProduceError::Io("checkpoint path is not valid utf8".to_string()))?;

    // 1. The mandatory self-audit, first, before anything is read for content.
    //    This is the "never serve state you have not proven" rule, and it is
    //    the SAME verifier the receiver and the boot installer will run.
    let report = audit::audit(path, None).map_err(ProduceError::Io)?;
    if !report.ok {
        return Err(ProduceError::AuditFailed {
            result_line: report.result_line(),
            failures: report.failures().into_iter().map(str::to_string).collect(),
        });
    }
    let height = report
        .height
        .ok_or_else(|| ProduceError::Io("audit passed without a height".to_string()))?;
    let state_root = report
        .root
        .ok_or_else(|| ProduceError::Io("audit passed without a root".to_string()))?;

    let db = RocksKv::open(path).map_err(|e| ProduceError::Io(format!("open checkpoint: {e:?}")))?;

    // 2. Cursor boundary, checked here rather than inherited from A1 so the
    //    extraction cannot silently depend on another module's check order.
    let committed = read_cursor(&db, KEY_COMMITTED_HEIGHT);
    let executed = read_cursor(&db, KEY_EXECUTED_HEIGHT);
    if committed != executed || committed != Some(height) {
        return Err(ProduceError::CursorsDiffer {
            committed,
            executed,
        });
    }

    // 3. The authenticated leaf set.
    let pairs = extract_leaf_set(&db)?;

    // 4. Certification evidence. The audit already proved these exist and
    //    chain; they are read again here because they must TRAVEL with the
    //    bundle, not merely have been present at the source.
    let block_h = load_block(&db, height)?;
    let block_h1 = load_block(&db, height + 1)?;
    let qc_h1 = ConsensusState::load_qc_at_height(&db, height + 1)
        .map_err(|e| ProduceError::Io(format!("load qc {}: {e:?}", height + 1)))?
        .ok_or_else(|| {
            ProduceError::MissingEvidence(format!(
                "no certifying QC row at height {}; the bundle would carry no trust anchor",
                height + 1
            ))
        })?;
    let qc_h = ConsensusState::load_qc_at_height(&db, height)
        .map_err(|e| ProduceError::Io(format!("load qc {height}: {e:?}")))?;

    // 5. Chunk and digest.
    let chunks = chunk_pairs(&pairs);

    // Gate F5 Stage 4: a chunk that cannot cross the wire must fail here,
    // loudly and named, rather than at send time. `chunk_pairs` lets a single
    // oversized pair travel alone rather than dropping it, which is the right
    // call for the chunker (a dropped leaf is a wrong root); the wire bound is
    // a separate constraint and this is where it is enforced.
    if let Some((i, len)) = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.len()))
        .find(|&(_, len)| len > novai_consensus_types::codec::MAX_SNAPSHOT_CHUNK_BYTES)
    {
        return Err(ProduceError::ChunkTooLarge {
            index: i,
            len,
            max: novai_consensus_types::codec::MAX_SNAPSHOT_CHUNK_BYTES,
        });
    }
    let chunk_digests = chunks.iter().map(|c| chunk_digest(c)).collect();

    let leaf_count = u32::try_from(pairs.len()).map_err(|_| {
        ProduceError::Io(format!("leaf count {} exceeds the manifest field", pairs.len()))
    })?;

    Ok(SnapshotBundle {
        manifest: SnapshotManifest {
            version: SNAPSHOT_FORMAT_VERSION,
            height,
            state_root,
            leaf_count,
            chunk_digests,
            block_h,
            qc_h,
            block_h1,
            qc_h1,
        },
        chunks,
    })
}

/// The authenticated leaf set, derived mechanically from the same table the
/// auditor classifies with, in canonical key order.
///
/// Refuses, rather than drops, any key that is unknown or defined-but-unwritten.
/// A dropped leaf is a wrong root, which is a silent corruption discovered
/// later by a state-root guard on some other node; a refusal is a loud
/// availability failure that names the key. That is always the better trade.
///
/// Note on redundancy, stated rather than hidden: inside [`build_bundle`] this
/// guard is unreachable, because the mandatory audit runs first and its A3
/// check fails closed on exactly the same two classes. It is kept as an
/// independent guard so the extraction does not silently inherit its
/// correctness from another module's check ordering, and it is unit-tested
/// directly rather than through the pipeline, so it is not uncovered code.
///
/// # Errors
/// Returns [`ProduceError::UnclassifiableKey`] naming the first offending key.
pub fn extract_leaf_set(db: &RocksKv) -> Result<FlatPairs, ProduceError> {
    // Both column families: an empty prefix reaches only the default one,
    // because the nnpx routing prefix does not match it. Streamed rather than
    // collected, for the reason spelled out on `for_each_prefix`: the SMT node
    // store dwarfs the leaf set and none of it is wanted here.
    let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut refusal: Option<ProduceError> = None;
    {
        let mut take = |k: &[u8], v: &[u8]| {
            if refusal.is_some() {
                return;
            }
            match classify(k) {
                Some(Class::SmtCommitted) => pairs.push((k.to_vec(), v.to_vec())),
                Some(Class::Operational) => {}
                Some(Class::DefinedUnwritten) => {
                    refusal = Some(ProduceError::UnclassifiableKey {
                        key: String::from_utf8_lossy(k).into_owned(),
                        class:
                            "defined in the schema but written by no production path at this HEAD",
                    });
                }
                None => {
                    refusal = Some(ProduceError::UnclassifiableKey {
                        key: String::from_utf8_lossy(k).into_owned(),
                        class: "unknown to the classification table",
                    });
                }
            }
        };
        db.for_each_prefix(b"", &mut take)
            .map_err(|e| ProduceError::Io(format!("scan default cf: {e:?}")))?;
        db.for_each_prefix(b"nnpx/", &mut take)
            .map_err(|e| ProduceError::Io(format!("scan nnpx cf: {e:?}")))?;
    }
    if let Some(e) = refusal {
        return Err(e);
    }
    // Canonical order. The rebuild is order independent, but the chunk digests
    // are not, so two producers over identical state must chunk identically.
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(pairs)
}

fn load_block(db: &RocksKv, height: u64) -> Result<Block, ProduceError> {
    ConsensusState::load_block(db, height)
        .map_err(|e| ProduceError::Io(format!("load block {height}: {e:?}")))?
        .ok_or_else(|| ProduceError::MissingEvidence(format!("block row {height} absent")))
}

/// Convenience for callers that want the carried QC set without matching.
#[must_use]
pub fn carried_qcs(m: &SnapshotManifest) -> Vec<&QC> {
    let mut v = vec![&m.qc_h1];
    if let Some(q) = &m.qc_h {
        v.push(q);
    }
    v
}
