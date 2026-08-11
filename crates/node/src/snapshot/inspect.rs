//! Read-only inspection of an offline data-dir copy: heights, roots, vote
//! high-water mark, QC voters, and key classification counts.
//!
//! This is the offline forensic reader the F4 diagnosis assigns to A0: it is
//! how the exact last-committed heights of node2's old subdirs get read
//! without ever starting a node, and how valset check (b) (live QC voters
//! are dev-keys addresses) gets closed from a healthy-node copy.
//!
//! Inspect never fails a verdict; it reports what is there. Only IO errors
//! exit nonzero.

use novai_consensus::ConsensusState;
use novai_consensus_types::codec::{decode_qc_v1, decode_voted_view_v1};
use novai_consensus_types::QC;
use novai_state::{
    decode_smt_root_v1, Kv, RocksKv, KEY_COMMITTED_HEIGHT, KEY_EXECUTED_HEIGHT, KEY_HIGHEST_QC,
    KEY_LOCKED_QC, KEY_SMT_ROOT, KEY_VOTED_VIEW,
};

use crate::snapshot::classify::{classify, Class};
use crate::snapshot::valset::{dev_valset, name_of};

fn read_u64(db: &RocksKv, key: &[u8]) -> Result<Option<u64>, String> {
    match db.get(key).map_err(|e| format!("db get: {e:?}"))? {
        Some(b) if b.len() == 8 => {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b);
            Ok(Some(u64::from_be_bytes(a)))
        }
        _ => Ok(None),
    }
}

fn fmt_u64(v: Option<u64>) -> String {
    v.map_or_else(|| "absent".to_string(), |x| x.to_string())
}

fn voters_of(qc: &QC) -> String {
    let vs = dev_valset();
    qc.votes
        .iter()
        .map(|v| name_of(&vs, &v.voter))
        .collect::<Vec<_>>()
        .join(",")
}

pub fn run(db_path: &str) -> Result<(), String> {
    let db = RocksKv::open(db_path).map_err(|e| format!("open db copy: {e:?}"))?;

    let committed = read_u64(&db, KEY_COMMITTED_HEIGHT)?;
    let executed = read_u64(&db, KEY_EXECUTED_HEIGHT)?;
    println!("committed_height={}", fmt_u64(committed));
    println!("executed_height={}", fmt_u64(executed));

    match db
        .get(KEY_SMT_ROOT)
        .map_err(|e| format!("db get root: {e:?}"))?
    {
        Some(bytes) => match decode_smt_root_v1(&bytes) {
            Ok(root) => println!("smt_root={}", hex::encode(root)),
            Err(e) => println!("smt_root=undecodable({e:?})"),
        },
        None => println!("smt_root=absent"),
    }

    match db
        .get(KEY_VOTED_VIEW)
        .map_err(|e| format!("db get voted view: {e:?}"))?
    {
        Some(bytes) => match decode_voted_view_v1(&bytes) {
            Ok((h, round)) => println!("voted_view=({h},{round})"),
            Err(e) => println!("voted_view=undecodable({e:?})"),
        },
        None => println!("voted_view=absent"),
    }

    match db
        .get(KEY_LOCKED_QC)
        .map_err(|e| format!("db get locked qc: {e:?}"))?
    {
        Some(bytes) => match decode_qc_v1(&bytes) {
            Ok(qc) => println!("locked_qc_height={}", qc.height),
            Err(e) => println!("locked_qc_height=undecodable({e:?})"),
        },
        None => println!("locked_qc_height=absent"),
    }

    match db
        .get(KEY_HIGHEST_QC)
        .map_err(|e| format!("db get highest qc: {e:?}"))?
    {
        Some(bytes) => match decode_qc_v1(&bytes) {
            Ok(qc) => {
                println!("highest_qc_height={}", qc.height);
                println!("highest_qc_voters={}", voters_of(&qc));
            }
            Err(e) => println!("highest_qc_height=undecodable({e:?})"),
        },
        None => println!("highest_qc_height=absent"),
    }

    // Dense qc rows near the committed tip, when present.
    if let Some(t) = committed {
        for h in [t, t + 1] {
            if let Ok(Some(qc)) = ConsensusState::load_qc_at_height(&db, h) {
                println!("qc_row height={h} voters={}", voters_of(&qc));
            }
        }
    }

    // Classification counts over both column families.
    let mut smt_committed = 0usize;
    let mut operational = 0usize;
    let mut defined_unwritten = 0usize;
    let mut unknown: Vec<String> = Vec::new();
    // Streamed: inspect only counts, so it must never pay to materialise a
    // multi-gigabyte key set (see `for_each_prefix`). The printed counts are
    // unchanged, which the inspect golden pin enforces.
    {
        let mut count = |k: &[u8], _v: &[u8]| match classify(k) {
            Some(Class::SmtCommitted) => smt_committed += 1,
            Some(Class::Operational) => operational += 1,
            Some(Class::DefinedUnwritten) => defined_unwritten += 1,
            None => unknown.push(String::from_utf8_lossy(k).into_owned()),
        };
        db.for_each_prefix(b"", &mut count)
            .map_err(|e| format!("scan default cf: {e:?}"))?;
        db.for_each_prefix(b"nnpx/", &mut count)
            .map_err(|e| format!("scan nnpx cf: {e:?}"))?;
    }
    println!(
        "class_counts smt_committed={smt_committed} operational={operational} defined_unwritten={defined_unwritten} unknown={}",
        unknown.len()
    );
    for k in unknown.iter().take(10) {
        println!("unknown_key={k}");
    }

    Ok(())
}
