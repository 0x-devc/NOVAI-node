use novai_smt::hash::{empty_hash_at_height, hash_internal, hash_leaf, Hash32};
use novai_smt::smt::{MemoryStore, Smt};

#[test]
fn golden_vectors_hash_rules_smoke() {
    // This test locks in the hashing rules. We will later extend it to full SMT roots.
    let k1: Hash32 = [0x01u8; 32];
    let k2: Hash32 = [0x02u8; 32];

    let l1 = hash_leaf(&k1, b"v1");
    let l2 = hash_leaf(&k2, b"v2");

    // basic: distinct keys/values => distinct leaf hashes
    assert_ne!(l1, l2);

    let e0 = empty_hash_at_height(0);
    let e1 = empty_hash_at_height(1);
    assert_ne!(e0, e1);

    let n = hash_internal(&l1, &e0);
    assert_ne!(n, l1);
    assert_ne!(n, e0);

    // determinism:
    assert_eq!(hash_leaf(&k1, b"v1"), l1);
    assert_eq!(hash_internal(&l1, &e0), n);
}

#[test]
fn root_is_stable_for_fixed_set() {
    let k1: Hash32 = [0xA1u8; 32];
    let k2: Hash32 = [0xB2u8; 32];
    let k3: Hash32 = [0xC3u8; 32];

    let mut s = Smt::new(MemoryStore::default());
    s.update(k1, b"alice").unwrap();
    s.update(k2, b"bob").unwrap();
    s.update(k3, b"carol").unwrap();

    let r = s.root();
    let expected: Hash32 = [
        94, 174, 116, 217, 48, 19, 193, 41, 252, 227, 8, 43, 156, 132, 85, 91, 250, 86, 51, 86,
        131, 248, 8, 194, 79, 78, 51, 212, 249, 75, 179, 87,
    ];

    assert_eq!(r, expected);
}
