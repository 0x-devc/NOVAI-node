use novai_smt::hash::{empty_hash_at_height, hash_internal, Hash32};
use novai_smt::smt::{MemoryStore, Smt, SmtError};

#[test]
fn missing_internal_node_is_hard_error() {
    // Create a non-empty-looking root hash, but do NOT store the corresponding node bytes.
    // This simulates DB corruption / missing records.
    let left = empty_hash_at_height(255);
    let right = empty_hash_at_height(255);
    let root = hash_internal(&left, &right);

    let mut smt = Smt::with_root(MemoryStore::default(), root);

    let key: Hash32 = [0x11u8; 32];
    let err = smt.update(key, b"hello").unwrap_err();

    match err {
        SmtError::MissingNode { hash } => assert_eq!(hash, root),
        other => panic!("expected MissingNode, got: {other:?}"),
    }
}
