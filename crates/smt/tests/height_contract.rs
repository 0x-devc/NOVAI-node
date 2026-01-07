use novai_smt::hash::empty_hash_at_height;
use novai_smt::smt::{MemoryStore, Smt};

#[test]
fn empty_hash_height_is_not_interchangeable() {
    assert_ne!(empty_hash_at_height(0), empty_hash_at_height(1));
    assert_ne!(empty_hash_at_height(1), empty_hash_at_height(2));
    assert_ne!(empty_hash_at_height(2), empty_hash_at_height(256));
}

#[test]
fn empty_tree_root_is_height_256_rule() {
    let s = Smt::new(MemoryStore::default());
    assert_eq!(s.root(), empty_hash_at_height(256));
}
