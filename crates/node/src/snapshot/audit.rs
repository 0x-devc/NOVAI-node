//! The audit pipeline: checks A1..A8 from the F4 diagnosis, section 3.
//!
//! Trust chain: the rebuilt root equals a QC-certified header's state_root,
//! where header(T+1).state_root is the canonical commitment to post-state(T)
//! (the lag-1 identity proven in the diagnosis, section 2). Every failure
//! mode lands in a failed equality or signature check; nothing passes by
//! default.
//!
//! Certification evidence, in priority order:
//!   1. qc_row: the dense qc row at T+1 plus the block row at T+1
//!      (installed-dir shape; rows written by persist_commit_atomic).
//!   2. highest_qc: KEY_HIGHEST_QC certifying pipeline height Q > T, with
//!      stored block rows T+1..=Q parent-linked down to T+1 (the shape of a
//!      fresh healthy-node copy, where blocks above the committed tip are
//!      stored at proposal receipt and the commit lags the highest QC by the
//!      3-chain rule).
//!
//! Memory note: A3 materializes the full default-CF key set via scan_prefix.
//! On a real ~1 GB copy this peaks at a few GB of RAM; acceptable for the
//! testnet-scale copies this gate targets, revisit before larger fleets.

use novai_consensus::ConsensusState;
use novai_consensus_types::codec::{decode_qc_v1, hash_block_v1};
use novai_consensus_types::{Block, QC};
use novai_execution::empty_smt_root;
use novai_state::{
    decode_smt_root_v1, Kv, RocksKv, KEY_COMMITTED_HEIGHT, KEY_EXECUTED_HEIGHT, KEY_HIGHEST_QC,
    KEY_SMT_ROOT,
};

use crate::snapshot::classify::{classify, Class};
use crate::snapshot::rebuild::rebuild_root;
use crate::snapshot::valset::{dev_valset, quorum};

/// Sanity bound on the highest-QC descent. The live pipeline runs two blocks
/// ahead of the committed tip (3-chain rule); anything remotely near this
/// bound means the copy is not what this tool expects.
const MAX_DESCENT: u64 = 1024;

struct Report {
    lines: Vec<String>,
    ok: bool,
}

impl Report {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            ok: true,
        }
    }
    fn pass(&mut self, code: &str, detail: &str) {
        self.lines.push(format!("{code} PASS {detail}"));
    }
    fn fail(&mut self, code: &str, detail: &str) {
        self.ok = false;
        self.lines.push(format!("{code} FAIL {detail}"));
    }
    fn skip(&mut self, code: &str, why: &str) {
        self.lines.push(format!("{code} SKIP {why}"));
    }
}

enum Cursor {
    Absent,
    Bad(usize),
    Val(u64),
}

fn read_cursor(db: &RocksKv, key: &[u8]) -> Result<Cursor, String> {
    match db.get(key).map_err(|e| format!("db get cursor: {e:?}"))? {
        None => Ok(Cursor::Absent),
        Some(b) if b.len() == 8 => {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b);
            Ok(Cursor::Val(u64::from_be_bytes(a)))
        }
        Some(b) => Ok(Cursor::Bad(b.len())),
    }
}

enum EvidenceOutcome {
    Found {
        qc: QC,
        chain: Vec<Block>,
        source: &'static str,
    },
    Broken(String),
}

fn gather_evidence(db: &RocksKv, t: u64) -> Result<EvidenceOutcome, String> {
    use EvidenceOutcome::{Broken, Found};

    // Priority 1: dense qc row at T+1.
    match ConsensusState::load_qc_at_height(db, t + 1) {
        Ok(Some(qc)) => {
            return Ok(match ConsensusState::load_block(db, t + 1) {
                Ok(Some(block)) => match hash_block_v1(&block) {
                    Ok(bh) if qc.height == t + 1 && qc.block_hash == bh => Found {
                        qc,
                        chain: vec![block],
                        source: "qc_row",
                    },
                    Ok(_) => Broken(format!(
                        "qc row at {} does not certify the stored block (height {})",
                        t + 1,
                        qc.height
                    )),
                    Err(e) => Broken(format!("hash block {}: {e:?}", t + 1)),
                },
                Ok(None) => Broken(format!("qc row present but block row {} absent", t + 1)),
                Err(e) => Broken(format!("load block {}: {e:?}", t + 1)),
            });
        }
        Ok(None) => {}
        Err(e) => return Ok(Broken(format!("load qc row {}: {e:?}", t + 1))),
    }

    // Priority 2: highest-QC pipeline descent.
    let Some(hqc_bytes) = db
        .get(KEY_HIGHEST_QC)
        .map_err(|e| format!("db get highest qc: {e:?}"))?
    else {
        return Ok(Broken(
            "no qc row at T+1 and no highest qc: no certification evidence".to_string(),
        ));
    };
    let qc = match decode_qc_v1(&hqc_bytes) {
        Ok(q) => q,
        Err(e) => return Ok(Broken(format!("decode highest qc: {e:?}"))),
    };
    if qc.height <= t {
        return Ok(Broken(format!(
            "highest qc height {} not above committed {t}",
            qc.height
        )));
    }
    if qc.height - t > MAX_DESCENT {
        return Ok(Broken(format!(
            "highest qc height {} implausibly far above committed {t} (bound {MAX_DESCENT})",
            qc.height
        )));
    }
    let mut chain: Vec<Block> = Vec::new();
    for h in (t + 1)..=qc.height {
        match ConsensusState::load_block(db, h) {
            Ok(Some(b)) => chain.push(b),
            Ok(None) => {
                return Ok(Broken(format!(
                    "pipeline block row {h} absent; cannot descend from highest qc"
                )))
            }
            Err(e) => return Ok(Broken(format!("load block {h}: {e:?}"))),
        }
    }
    for i in 1..chain.len() {
        let parent_hash = match hash_block_v1(&chain[i - 1]) {
            Ok(h) => h,
            Err(e) => return Ok(Broken(format!("hash block: {e:?}"))),
        };
        if chain[i].parent_hash != parent_hash {
            return Ok(Broken(format!(
                "parent link broken at height {}",
                chain[i].height
            )));
        }
    }
    let tip_hash = match hash_block_v1(chain.last().expect("chain nonempty")) {
        Ok(h) => h,
        Err(e) => return Ok(Broken(format!("hash block: {e:?}"))),
    };
    if qc.block_hash != tip_hash {
        return Ok(Broken(
            "highest qc does not certify the stored pipeline tip".to_string(),
        ));
    }
    Ok(Found {
        qc,
        chain,
        source: "highest_qc",
    })
}

/// Run the full audit. Ok(true) = PASS (exit 0), Ok(false) = FAIL (exit 1),
/// Err = IO/environment error (exit 2).
pub fn run(db_path: &str, expected_height: Option<u64>) -> Result<bool, String> {
    let db = RocksKv::open(db_path).map_err(|e| format!("open db copy: {e:?}"))?;
    let mut r = Report::new();

    // A1: cursor consistency.
    let committed = read_cursor(&db, KEY_COMMITTED_HEIGHT)?;
    let executed = read_cursor(&db, KEY_EXECUTED_HEIGHT)?;
    let t: Option<u64> = match (&committed, &executed) {
        (Cursor::Val(c), Cursor::Val(e)) if c == e => {
            if let Some(want) = expected_height {
                if *c == want {
                    r.pass("A1", &format!("committed={c} executed={e} height={c}"));
                    Some(*c)
                } else {
                    r.fail("A1", &format!("committed={c} but expected height {want}"));
                    None
                }
            } else {
                r.pass("A1", &format!("committed={c} executed={e} height={c}"));
                Some(*c)
            }
        }
        (Cursor::Val(c), Cursor::Val(e)) => {
            r.fail(
                "A1",
                &format!("committed={c} executed={e} (cursors differ: torn or crash-window copy)"),
            );
            None
        }
        (Cursor::Absent, _) => {
            r.fail("A1", "committed cursor absent (not an auditable copy)");
            None
        }
        (_, Cursor::Absent) => {
            r.fail("A1", "executed cursor absent (not an auditable copy)");
            None
        }
        (Cursor::Bad(n), _) | (_, Cursor::Bad(n)) => {
            r.fail("A1", &format!("cursor has invalid length {n}"));
            None
        }
    };

    // A2: stored root readback. Absent is the canonical empty root, matching
    // every consensus read site.
    let stored_root: Option<[u8; 32]> = match db
        .get(KEY_SMT_ROOT)
        .map_err(|e| format!("db get root: {e:?}"))?
    {
        Some(bytes) => match decode_smt_root_v1(&bytes) {
            Ok(root) => {
                r.pass("A2", &format!("root={}", hex::encode(root)));
                Some(root)
            }
            Err(e) => {
                r.fail("A2", &format!("stored root undecodable: {e:?}"));
                None
            }
        },
        None => {
            let root = empty_smt_root();
            r.pass("A2", &format!("root={} absent=empty", hex::encode(root)));
            Some(root)
        }
    };

    // A3: full enumeration and classification. The nnpx column family is
    // reachable through the Kv trait only under its routing prefix; any
    // reachable nnpx/ key is DefinedUnwritten and fails the audit.
    let mut smt_pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut operational = 0usize;
    let mut unwritten: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut scan = db
        .scan_prefix(b"")
        .map_err(|e| format!("scan default cf: {e:?}"))?;
    scan.extend(
        db.scan_prefix(b"nnpx/")
            .map_err(|e| format!("scan nnpx cf: {e:?}"))?,
    );
    for (k, v) in scan {
        match classify(&k) {
            Some(Class::SmtCommitted) => smt_pairs.push((k, v)),
            Some(Class::Operational) => operational += 1,
            Some(Class::DefinedUnwritten) => {
                unwritten.push(String::from_utf8_lossy(&k).into_owned());
            }
            None => unknown.push(String::from_utf8_lossy(&k).into_owned()),
        }
    }
    if unknown.is_empty() && unwritten.is_empty() {
        r.pass(
            "A3",
            &format!(
                "smt_committed={} operational={operational} defined_unwritten=0 unknown=0",
                smt_pairs.len()
            ),
        );
    } else {
        for k in unknown.iter().take(10) {
            r.fail("A3", &format!("unknown key: {k}"));
        }
        for k in unwritten.iter().take(10) {
            r.fail(
                "A3",
                &format!("defined-but-unwritten key present (no production writer at this HEAD): {k}"),
            );
        }
    }

    // A4: from-scratch rebuild over the SMT-committed pairs.
    let rebuilt: Option<[u8; 32]> = match rebuild_root(&smt_pairs) {
        Ok(root) => {
            r.pass(
                "A4",
                &format!("rebuilt={} pairs={}", hex::encode(root), smt_pairs.len()),
            );
            Some(root)
        }
        Err(e) => {
            r.fail("A4", &format!("rebuild failed: {e}"));
            None
        }
    };

    // A5: rebuild equals stored root.
    match (rebuilt, stored_root) {
        (Some(a), Some(b)) if a == b => r.pass("A5", &format!("root={}", hex::encode(a))),
        (Some(a), Some(b)) => r.fail(
            "A5",
            &format!("rebuilt={} stored={}", hex::encode(a), hex::encode(b)),
        ),
        _ => r.skip("A5", "missing rebuild or stored root"),
    }

    // A6..A8: certification evidence and the lag-1 identity.
    let mut audited_root_hex: Option<String> = rebuilt.map(hex::encode);
    match t {
        Some(t) => match gather_evidence(&db, t)? {
            EvidenceOutcome::Found { qc, chain, source } => {
                let vs = dev_valset();
                let q = quorum(vs.len());
                match ConsensusState::verify_qc_well_formed(&qc, &vs, q) {
                    Ok(()) => r.pass(
                        "A6",
                        &format!(
                            "qc_height={} voters={} quorum={q} source={source}",
                            qc.height,
                            qc.votes.len()
                        ),
                    ),
                    Err(e) => r.fail("A6", &format!("qc verification failed: {e:?}")),
                }
                let block_t1 = &chain[0];
                // A7 (gate wedge-276272, lag-0): under the post-state convention the
                // header at the AUDITED height T carries post-state(T) == rebuilt.
                // (Was lag-1: the successor header(T+1) carried post-state(T).) A8
                // still anchors the successor to block(T).
                match ConsensusState::load_block(&db, t) {
                    Ok(Some(block_t)) => {
                        match rebuilt {
                            Some(root) if block_t.state_root == root => {
                                r.pass(
                                    "A7",
                                    &format!("header({}).state_root={}", t, hex::encode(root)),
                                );
                            }
                            Some(root) => r.fail(
                                "A7",
                                &format!(
                                    "header({}).state_root={} rebuilt={} (lag-0 identity violated)",
                                    t,
                                    hex::encode(block_t.state_root),
                                    hex::encode(root)
                                ),
                            ),
                            None => r.skip("A7", "no rebuilt root"),
                        }
                        match hash_block_v1(&block_t) {
                            Ok(h) if block_t1.parent_hash == h => {
                                r.pass("A8", &format!("block({}) anchors block({})", t, t + 1));
                            }
                            Ok(_) => r.fail(
                                "A8",
                                &format!("block({}) does not anchor block({})", t, t + 1),
                            ),
                            Err(e) => r.fail("A8", &format!("hash block {t}: {e:?}")),
                        }
                    }
                    Ok(None) => {
                        r.skip("A7", "no block at audited height");
                        r.fail("A8", &format!("block row {t} absent"));
                    }
                    Err(e) => {
                        r.skip("A7", "load block failed");
                        r.fail("A8", &format!("load block {t}: {e:?}"));
                    }
                }
            }
            EvidenceOutcome::Broken(why) => {
                r.fail("A6", &why);
                r.skip("A7", "no certification evidence");
                r.skip("A8", "no certification evidence");
            }
        },
        None => {
            r.skip("A6", "no audited height");
            r.skip("A7", "no audited height");
            r.skip("A8", "no audited height");
        }
    }

    for line in &r.lines {
        println!("{line}");
    }
    if r.ok {
        let t = t.expect("ok implies height known");
        let root_hex = audited_root_hex.take().expect("ok implies rebuilt root");
        println!("RESULT PASS height={t} root={root_hex}");
    } else {
        println!("RESULT FAIL");
    }
    Ok(r.ok)
}
