//! From-scratch SMT rebuild over the exported flat leaf set.
//!
//! Drives the canonical execution path (append_smt_ops_for_state_ops,
//! crates/execution/src/lib.rs:6530-6573) one pair per walk, mirroring the
//! node's per-transaction batching, against a fresh in-memory store. The SMT
//! root is a pure function of the final leaf map (order independence is
//! pinned by crates/smt tests and re-pinned here), so this reproduces the
//! chain root if and only if the pair set equals the chain's authenticated
//! state.

use novai_execution::{append_smt_ops_for_state_ops, empty_smt_root};
use novai_state::{decode_smt_root_v1, Kv, KvBatch, WriteOp, KEY_SMT_ROOT};

use crate::snapshot::store::BTreeKv;

/// Rebuild the root from flat (key, value) pairs. The empty set yields the
/// canonical empty root, matching every consensus absent-root default.
pub fn rebuild_root(pairs: &[(Vec<u8>, Vec<u8>)]) -> Result<[u8; 32], String> {
    let mut db = BTreeKv::default();
    for (k, v) in pairs {
        let state_ops = vec![WriteOp::Put(k.clone(), v.clone())];
        let mut all_ops = state_ops.clone();
        append_smt_ops_for_state_ops(&db, &state_ops, &mut all_ops).map_err(|e| {
            format!(
                "smt rebuild failed at key {}: {e:?}",
                String::from_utf8_lossy(k)
            )
        })?;
        let _ = db.apply_batch(&all_ops);
    }
    match db.get(KEY_SMT_ROOT) {
        Ok(Some(bytes)) => decode_smt_root_v1(&bytes).map_err(|e| format!("{e:?}")),
        Ok(None) => Ok(empty_smt_root()),
        Err(e) => Err(format!("{e:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use novai_state::{account_key, encode_account_v1, AccountStateV1};

    fn pair(tag: u8, balance: u128) -> (Vec<u8>, Vec<u8>) {
        (
            account_key(&[tag; 32]),
            encode_account_v1(&AccountStateV1 { balance, nonce: 0 }).to_vec(),
        )
    }

    #[test]
    fn empty_set_yields_canonical_empty_root() {
        assert_eq!(rebuild_root(&[]).unwrap(), empty_smt_root());
    }

    #[test]
    fn rebuild_is_order_independent() {
        let a = vec![pair(0x01, 10), pair(0x02, 20), pair(0x03, 30)];
        let mut b = a.clone();
        b.reverse();
        assert_eq!(rebuild_root(&a).unwrap(), rebuild_root(&b).unwrap());
    }

    #[test]
    fn last_write_wins_per_key() {
        let twice = vec![pair(0x01, 10), pair(0x01, 99)];
        let once = vec![pair(0x01, 99)];
        assert_eq!(rebuild_root(&twice).unwrap(), rebuild_root(&once).unwrap());
    }
}
