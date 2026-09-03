//! Gate R9 (W3): a missing SMT node must name the hash it could not find.
//!
//! `append_smt_ops_for_state_ops` used to collapse every non-`Store` SMT
//! failure onto `ExecError::Overflow`. That is the single worst evidence bug in
//! the state layer: `SmtError::MissingNode` is exactly the symptom of an SMT
//! garbage collector that freed a live node, and it reached the operator's logs
//! wearing the name of an arithmetic overflow, which points a reader at balance
//! math instead of at storage. Three root causes have already been lost to bad
//! evidence in this project, so the error class is pinned here rather than left
//! to a code reading.
//!
//! Both walks are covered. `update` and `delete` both descend through
//! `recompute_path`, so a delete walk can land on a freed node exactly as an
//! update walk can, and a fix applied to only one arm would leave half the
//! surface still reporting an overflow.

use novai_execution::{append_smt_ops_for_state_ops, ExecError};
use novai_smt::hash::{empty_hash_at_height, hash_internal, Hash32};
use novai_state::{
    account_key, encode_account_v1, encode_smt_root_v1, AccountStateV1, Kv, MemKv, WriteOp,
    KEY_SMT_ROOT,
};
use novai_types::Address;

const HOLDER: Address = [0x5Au8; 32];

/// A store whose persisted root names an internal node that was never written.
///
/// `hash_internal` of two height-255 empty hashes is a well-formed internal
/// node hash that is distinct from `empty_hash_at_height(256)`, so the descent
/// in `recompute_path` cannot short-circuit on the empty-subtree branch and is
/// forced to ask the store for bytes that are absent. This is the same fixture
/// shape as `crates/smt/tests/missing_node_is_error.rs`, lifted one layer up so
/// it exercises the execution crate's error mapping instead of the SMT's own.
fn db_with_dangling_root() -> (MemKv, Hash32) {
    let mut db = MemKv::default();
    let dangling = hash_internal(&empty_hash_at_height(255), &empty_hash_at_height(255));
    db.put(KEY_SMT_ROOT, &encode_smt_root_v1(&dangling))
        .unwrap();
    (db, dangling)
}

/// An account-shaped write, so the state key hashes through
/// `smt_key_for_state_key` the way a real transfer's does.
fn account_write() -> Vec<u8> {
    encode_account_v1(&AccountStateV1 {
        balance: 1_000,
        nonce: 0,
    })
    .to_vec()
}

#[test]
fn a_missing_node_names_the_hash_it_could_not_find() {
    // Put arm: the `smt.update` mapping site in `append_smt_ops_for_state_ops`.
    let (db, dangling) = db_with_dangling_root();
    let mut out_ops = Vec::new();
    let err = append_smt_ops_for_state_ops(
        &db,
        &[WriteOp::Put(account_key(&HOLDER), account_write())],
        &mut out_ops,
    )
    .unwrap_err();
    match err {
        ExecError::SmtMissingNode { hash } => assert_eq!(
            hash, dangling,
            "Put arm named the wrong hash: an error that reports a hash the \
             operator cannot look up in the node store is no better evidence \
             than Overflow was"
        ),
        other => panic!(
            "Put arm: expected SmtMissingNode naming {dangling:?}, got {other:?}; \
             a missing node points at storage, an overflow points at balance math"
        ),
    }

    // Delete arm: the `smt.delete` mapping site. Same descent, separate match.
    let (db, dangling) = db_with_dangling_root();
    let mut out_ops = Vec::new();
    let err =
        append_smt_ops_for_state_ops(&db, &[WriteOp::Delete(account_key(&HOLDER))], &mut out_ops)
            .unwrap_err();
    match err {
        ExecError::SmtMissingNode { hash } => {
            assert_eq!(hash, dangling, "Delete arm named the wrong hash")
        }
        other => panic!("Delete arm: expected SmtMissingNode naming {dangling:?}, got {other:?}"),
    }
}
