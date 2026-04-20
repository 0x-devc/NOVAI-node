use crate::hash::{empty_hash_at_height, hash_internal, hash_leaf, Hash32};
use crate::node::{Node, NodeChild, NodeEncodingError};
use novai_state::{smt_node_key, Kv};

/// Storage abstraction for SMT nodes.
///
/// Nodes are addressed by their hash (content-addressed):
/// - key: 32-byte hash
/// - value: canonical node encoding (67 bytes)
pub trait SmtStore {
    type Error;

    fn get_node(&self, node_hash: &Hash32) -> Result<Option<[u8; Node::ENCODED_LEN]>, Self::Error>;
    fn put_node(
        &mut self,
        node_hash: &Hash32,
        node_bytes: &[u8; Node::ENCODED_LEN],
    ) -> Result<(), Self::Error>;
}

/// Errors from SMT operations.
#[derive(Debug)]
pub enum SmtError<E> {
    Store(E),
    CorruptNode(NodeEncodingError),
    MissingNode { hash: Hash32 },
    BadNodeBytesLen { expected: usize, got: usize },
    BadEmptyHeight { expected: u16, got: u16 },
    HeightOutOfRange { height: u16 },
}

/// SMT store adapter backed by a generic `novai-state::Kv`.
///
/// Keys:
/// - Node key: `smt/node/<hash32>`
/// - Value: canonical `Node::encode()` bytes
pub struct DbStore<'a, K: Kv> {
    db: &'a mut K,
}

impl<'a, K: Kv> DbStore<'a, K> {
    pub fn new(db: &'a mut K) -> Self {
        Self { db }
    }
}

impl<'a, K: Kv> SmtStore for DbStore<'a, K> {
    type Error = K::Error;

    fn get_node(&self, node_hash: &Hash32) -> Result<Option<[u8; Node::ENCODED_LEN]>, Self::Error> {
        let key = smt_node_key(node_hash);
        match self.db.get(&key)? {
            None => Ok(None),
            Some(v) => {
                // Strict length check. We cannot "guess" or truncate.
                if v.len() != Node::ENCODED_LEN {
                    // H-09: Log corruption explicitly so operators can detect it.
                    // We cannot return SmtError here because trait returns Self::Error.
                    // Treating as missing is deterministic but WRONG — alert via stderr
                    // so monitoring detects corruption rather than silent divergence.
                    // Uses eprintln because smt crate has no tracing dependency.
                    eprintln!(
                        "CORRUPTED SMT NODE: hash={:?} expected_len={} actual_len={} — \
                         treating as missing. This may cause state root divergence!",
                        &node_hash[..8],
                        Node::ENCODED_LEN,
                        v.len(),
                    );
                    return Ok(None);
                }
                let mut out = [0u8; Node::ENCODED_LEN];
                out.copy_from_slice(&v);
                Ok(Some(out))
            }
        }
    }

    fn put_node(
        &mut self,
        node_hash: &Hash32,
        node_bytes: &[u8; Node::ENCODED_LEN],
    ) -> Result<(), Self::Error> {
        let key = smt_node_key(node_hash);
        self.db.put(&key, node_bytes)?;
        Ok(())
    }
}

/// Simple in-memory store for tests.
///
/// NOTE: consensus-critical behavior must not rely on iteration order. This is test-only.
#[derive(Default)]
pub struct MemoryStore {
    nodes: Vec<(Hash32, [u8; Node::ENCODED_LEN])>,
}

impl MemoryStore {
    fn find(&self, h: &Hash32) -> Option<[u8; Node::ENCODED_LEN]> {
        self.nodes.iter().find(|(k, _)| k == h).map(|(_, v)| *v)
    }
}

impl SmtStore for MemoryStore {
    type Error = ();

    fn get_node(&self, node_hash: &Hash32) -> Result<Option<[u8; Node::ENCODED_LEN]>, Self::Error> {
        Ok(self.find(node_hash))
    }

    fn put_node(
        &mut self,
        node_hash: &Hash32,
        node_bytes: &[u8; Node::ENCODED_LEN],
    ) -> Result<(), Self::Error> {
        for (k, v) in self.nodes.iter_mut() {
            if k == node_hash {
                *v = *node_bytes;
                return Ok(());
            }
        }
        self.nodes.push((*node_hash, *node_bytes));
        Ok(())
    }
}

/// Sparse Merkle Tree.
///
/// Root of an empty tree is `empty_hash_at_height(256)`.
pub struct Smt<S: SmtStore> {
    store: S,
    root: Hash32,
}

impl<S: SmtStore> Smt<S> {
    pub fn new(store: S) -> Self {
        Self {
            store,
            root: empty_hash_at_height(256),
        }
    }

    pub fn with_root(store: S, root: Hash32) -> Self {
        Self { store, root }
    }

    pub fn root(&self) -> Hash32 {
        self.root
    }

    pub fn into_store(self) -> S {
        self.store
    }

    /// Update key to value (insert/overwrite).
    pub fn update(
        &mut self,
        key: Hash32,
        value_bytes: &[u8],
    ) -> Result<Hash32, SmtError<S::Error>> {
        let leaf = hash_leaf(&key, value_bytes);
        let new_root = self.recompute_path(key, leaf)?;
        self.root = new_root;
        Ok(new_root)
    }

    /// Delete key (sets leaf to empty at height 0).
    pub fn delete(&mut self, key: Hash32) -> Result<Hash32, SmtError<S::Error>> {
        let empty_leaf = empty_hash_at_height(0);
        let new_root = self.recompute_path(key, empty_leaf)?;
        self.root = new_root;
        Ok(new_root)
    }

    fn recompute_path(
        &mut self,
        key: Hash32,
        new_leaf_hash: Hash32,
    ) -> Result<Hash32, SmtError<S::Error>> {
        // Collect sibling hashes from root (height 256) down to leaf (height 0).
        // siblings[i] corresponds to height = 256 - i, and is the sibling hash at that level's child.
        let mut siblings: Vec<Hash32> = Vec::with_capacity(256);

        let mut cur_hash = self.root;
        let mut height: u16 = 256;

        while height > 0 {
            if cur_hash == empty_hash_at_height(height) {
                // Entire subtree is empty at this height, so the sibling at the next level down
                // is also empty at (height-1), for all remaining levels.
                // We can fill the rest deterministically.
                let mut h = height;
                while h > 0 {
                    siblings.push(empty_hash_at_height(h - 1));
                    h -= 1;
                }
                break;
            }

            // Non-empty internal node at this height must exist in store.
            let bytes = self
                .store
                .get_node(&cur_hash)
                .map_err(SmtError::Store)?
                .ok_or_else(|| SmtError::MissingNode { hash: cur_hash })?;

            let node = Node::decode(&bytes).map_err(SmtError::CorruptNode)?;
            let expected_child_height = height - 1;

            // Validate empty child heights (canonical).
            validate_empty_height(&node.left, expected_child_height)?;
            validate_empty_height(&node.right, expected_child_height)?;

            let bit = get_bit_msb_first(&key, 256 - height);
            match bit {
                0 => {
                    // going left; sibling is right
                    siblings.push(child_hash(&node.right));
                    cur_hash = child_hash(&node.left);
                }
                1 => {
                    // going right; sibling is left
                    siblings.push(child_hash(&node.left));
                    cur_hash = child_hash(&node.right);
                }
                _ => unreachable!(),
            }

            height -= 1;
        }

        // Rebuild upward from leaf to root.
        // siblings is in top-down order; we need bottom-up, so iterate reversed.
        let mut child_hash_now = new_leaf_hash;
        let mut h: u16 = 0; // current child height

        for (level_from_leaf, sibling_hash) in siblings.into_iter().rev().enumerate() {
            // We are building parent at height = h+1 (since child is height h).
            let parent_height: u16 = (level_from_leaf as u16) + 1;
            if parent_height > 256 {
                return Err(SmtError::HeightOutOfRange {
                    height: parent_height,
                });
            }

            let bit_index = 255 - (level_from_leaf as u16);
            let bit = get_bit_msb_first(&key, bit_index);

            let (left, right) = if bit == 0 {
                (child_hash_now, sibling_hash)
            } else {
                (sibling_hash, child_hash_now)
            };

            let empty_child = empty_hash_at_height(h);

            // Collapse rule: if both children are empty at height h, parent is empty at height h+1.
            if left == empty_child && right == empty_child {
                child_hash_now = empty_hash_at_height(h + 1);
            } else {
                let parent_hash = hash_internal(&left, &right);

                // Store the internal node for this height (h+1).
                let node = Node {
                    left: child_ptr_for_hash(&left, h),
                    right: child_ptr_for_hash(&right, h),
                };
                let enc = node.encode();
                self.store
                    .put_node(&parent_hash, &enc)
                    .map_err(SmtError::Store)?;

                child_hash_now = parent_hash;
            }

            h = h
                .checked_add(1)
                .ok_or(SmtError::HeightOutOfRange { height: h })?;
        }

        // After 256 levels, h should be 256 and child_hash_now is the root.
        Ok(child_hash_now)
    }
}

fn validate_empty_height<E>(c: &NodeChild, expected: u16) -> Result<(), SmtError<E>> {
    if let NodeChild::Empty { height } = c {
        if *height != expected {
            return Err(SmtError::BadEmptyHeight {
                expected,
                got: *height,
            });
        }
    }
    Ok(())
}

fn child_hash(c: &NodeChild) -> Hash32 {
    match c {
        NodeChild::Hash(h) => *h,
        NodeChild::Empty { height } => empty_hash_at_height(*height),
    }
}

/// For a node whose children are subtrees of height `child_height`,
/// choose canonical pointer representation.
fn child_ptr_for_hash(h: &Hash32, child_height: u16) -> NodeChild {
    if *h == empty_hash_at_height(child_height) {
        NodeChild::Empty {
            height: child_height,
        }
    } else {
        NodeChild::Hash(*h)
    }
}

/// Return the bit at `bit_index` where:
/// - bit_index 0 is the MSB of key[0]
/// - bit_index 255 is the LSB of key[31]
fn get_bit_msb_first(key: &Hash32, bit_index: u16) -> u8 {
    debug_assert!(bit_index < 256);
    let byte_index = (bit_index / 8) as usize;
    let bit_in_byte = 7 - (bit_index % 8);
    (key[byte_index] >> bit_in_byte) & 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tree_root_is_empty_hash_256() {
        let s = Smt::new(MemoryStore::default());
        assert_eq!(s.root(), empty_hash_at_height(256));
    }

    #[test]
    fn insert_two_keys_order_independent() {
        let k1: Hash32 = [0x01u8; 32];
        let k2: Hash32 = [0x02u8; 32];

        let mut a = Smt::new(MemoryStore::default());
        a.update(k1, b"v1").unwrap();
        a.update(k2, b"v2").unwrap();
        let r_a = a.root();

        let mut b = Smt::new(MemoryStore::default());
        b.update(k2, b"v2").unwrap();
        b.update(k1, b"v1").unwrap();
        let r_b = b.root();

        assert_eq!(r_a, r_b);
        assert_ne!(r_a, empty_hash_at_height(256));
    }

    #[test]
    fn delete_changes_root_and_can_return_to_empty() {
        let k1: Hash32 = [0x11u8; 32];

        let mut s = Smt::new(MemoryStore::default());
        let r0 = s.root();

        s.update(k1, b"hello").unwrap();
        let r1 = s.root();
        assert_ne!(r0, r1);

        s.delete(k1).unwrap();
        let r2 = s.root();
        assert_eq!(r0, r2);
    }
}
