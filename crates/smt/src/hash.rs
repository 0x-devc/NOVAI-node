use blake3::Hasher;

/// 32-byte hash type used by the SMT.
pub type Hash32 = [u8; 32];

/// Domain tags (single byte) for canonical hashing.
///
/// Rationale: domain separation prevents collisions between:
/// - empty nodes at different heights
/// - leaf hashes vs internal hashes
/// - internal nodes vs arbitrary concatenations
const TAG_EMPTY: u8 = 0x00;
const TAG_LEAF: u8 = 0x01;
const TAG_INTERNAL: u8 = 0x02;

/// Canonical hash for an "empty" subtree at a given height.
///
/// Height definition:
/// - height 0 corresponds to a leaf position (no remaining bits).
/// - height increases as you move up toward the root.
/// - root height is 256 (because keys are 256-bit).
///
/// Rule:
/// empty(h) = blake3( TAG_EMPTY || [h as u16 be] )
///
/// This is intentionally simple and fixed (no recursion) to avoid ambiguity and
/// allow fast computation.
pub fn empty_hash_at_height(height: u16) -> Hash32 {
    let mut hasher = Hasher::new();
    hasher.update(&[TAG_EMPTY]);
    hasher.update(&height.to_be_bytes());
    hasher.finalize().into()
}

// Height contract:
// - height is subtree height / remaining bits.
// - leaf height = 0.
// - root height = 256 (because keys are 256-bit).
// The SMT code treats the root as height 256 and decrements by 1 per level.

/// Canonical leaf hash.
///
/// leaf = blake3( TAG_LEAF || key32 || blake3(value_bytes) )
///
/// Note: we hash the value_bytes first to support arbitrary lengths without
/// changing leaf layout.
pub fn hash_leaf(key: &Hash32, value_bytes: &[u8]) -> Hash32 {
    let value_hash: Hash32 = blake3::hash(value_bytes).into();
    let mut hasher = Hasher::new();
    hasher.update(&[TAG_LEAF]);
    hasher.update(key);
    hasher.update(&value_hash);
    hasher.finalize().into()
}

/// Canonical internal node hash.
///
/// internal = blake3( TAG_INTERNAL || left32 || right32 )
pub fn hash_internal(left: &Hash32, right: &Hash32) -> Hash32 {
    let mut hasher = Hasher::new();
    hasher.update(&[TAG_INTERNAL]);
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_hash_is_stable_and_distinct_by_height() {
        let e0 = empty_hash_at_height(0);
        let e1 = empty_hash_at_height(1);
        let e255 = empty_hash_at_height(255);
        let e256 = empty_hash_at_height(256);

        assert_ne!(e0, e1);
        assert_ne!(e1, e255);
        assert_ne!(e255, e256);

        // determinism: same input => same output
        assert_eq!(e0, empty_hash_at_height(0));
        assert_eq!(e256, empty_hash_at_height(256));
    }

    #[test]
    fn leaf_and_internal_domain_separation() {
        let k: Hash32 = [0x11u8; 32];
        let v = b"hello";
        let leaf = hash_leaf(&k, v);

        let e0 = empty_hash_at_height(0);
        let internal = hash_internal(&e0, &e0);

        assert_ne!(leaf, internal);
        assert_ne!(leaf, e0);
        assert_ne!(internal, e0);
    }
}
