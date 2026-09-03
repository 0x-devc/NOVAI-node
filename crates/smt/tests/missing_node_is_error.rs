use std::cell::RefCell;
use std::rc::Rc;

use novai_smt::hash::{empty_hash_at_height, hash_internal, Hash32};
use novai_smt::node::{Node, NodeChild};
use novai_smt::smt::{MemoryStore, Smt, SmtError, SmtStore};

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

/// Store that supports removal, so a test can take a node back out of a tree
/// that was built by real updates.
///
/// Removal is the operation a future garbage collector performs, and the point
/// of R6 is that removing a node that is still reachable must be loud. The
/// store is shared through `Rc<RefCell<..>>` because `Smt` owns its store by
/// value and the removal has to happen between two operations on the same tree.
#[derive(Clone, Default)]
struct RemovableStore {
    nodes: Rc<RefCell<Vec<(Hash32, [u8; Node::ENCODED_LEN])>>>,
}

impl RemovableStore {
    fn remove(&self, node_hash: &Hash32) -> bool {
        let mut nodes = self.nodes.borrow_mut();
        match nodes.iter().position(|(k, _)| k == node_hash) {
            Some(i) => {
                nodes.remove(i);
                true
            }
            None => false,
        }
    }

    fn len(&self) -> usize {
        self.nodes.borrow().len()
    }
}

impl SmtStore for RemovableStore {
    type Error = ();

    fn get_node(&self, node_hash: &Hash32) -> Result<Option<[u8; Node::ENCODED_LEN]>, Self::Error> {
        Ok(self
            .nodes
            .borrow()
            .iter()
            .find(|(k, _)| k == node_hash)
            .map(|(_, v)| *v))
    }

    fn put_node(
        &mut self,
        node_hash: &Hash32,
        node_bytes: &[u8; Node::ENCODED_LEN],
    ) -> Result<(), Self::Error> {
        let mut nodes = self.nodes.borrow_mut();
        for (k, v) in nodes.iter_mut() {
            if k == node_hash {
                *v = *node_bytes;
                return Ok(());
            }
        }
        nodes.push((*node_hash, *node_bytes));
        Ok(())
    }
}

/// Bit `bit_index` of `key`, MSB first, matching the path convention in
/// `recompute_path`: index 0 is the high bit of `key[0]`.
fn bit_msb_first(key: &Hash32, bit_index: u16) -> u8 {
    let byte_index = (bit_index / 8) as usize;
    let bit_in_byte = 7 - (bit_index % 8);
    (key[byte_index] >> bit_in_byte) & 1
}

fn child_hash_of(c: &NodeChild) -> Hash32 {
    match c {
        NodeChild::Hash(h) => *h,
        NodeChild::Empty { height } => empty_hash_at_height(*height),
    }
}

/// R6: removing a node that is genuinely reachable from a real root must be a
/// hard error on the next traversal, not a silent empty subtree.
///
/// The test above plants a root whose bytes were never written, which is
/// synthetic corruption. This one builds a real tree with real updates, then
/// takes one real node back out. That is exactly the failure mode a garbage
/// collector can introduce: the pruner mistakenly frees a live node, and the
/// question is whether the tree then screams or silently produces a different
/// state root. A wrong-but-plausible root is the catastrophic outcome because
/// it diverges the chain with no signal, so the assertion is positive: the call
/// must return `Err(MissingNode { .. })` naming the removed hash.
#[test]
fn deleting_one_reachable_node_is_a_hard_error() {
    let store = RemovableStore::default();
    let mut smt = Smt::new(store.clone());

    let k1: Hash32 = [0x01u8; 32];
    let k2: Hash32 = [0x02u8; 32];

    smt.update(k1, b"v1").unwrap();
    smt.update(k2, b"v2").unwrap();

    let root_before = smt.root();
    let nodes_before = store.len();

    // Descend the real tree along k1's own path, exactly as `recompute_path`
    // will descend it on the next update of k1: start at the root (height 256),
    // decode, follow the bit for this level, repeat. Every hash collected this
    // way is reachable from the root BY CONSTRUCTION, because reaching it is
    // how it was found, and it is traversed on the next update of k1 because
    // the descent is driven by k1's bits and those bits do not change.
    let mut path: Vec<(u16, Hash32)> = Vec::new();
    let mut cur = root_before;
    let mut height: u16 = 256;
    while height > 0 && path.len() < 8 {
        assert_ne!(
            cur,
            empty_hash_at_height(height),
            "descent hit an empty subtree at height {height}; the chosen node \
             must be a real stored node, not an empty placeholder"
        );
        path.push((height, cur));
        let bytes = store
            .get_node(&cur)
            .unwrap()
            .expect("node on the live path must be present before removal");
        let node = Node::decode(&bytes).unwrap();
        let bit = bit_msb_first(&k1, 256 - height);
        cur = if bit == 0 {
            child_hash_of(&node.left)
        } else {
            child_hash_of(&node.right)
        };
        height -= 1;
    }

    // Take a node three levels below the root rather than the root itself. The
    // root is trivially traversed, so removing it would also pass a much weaker
    // implementation; a mid-path node proves the descent checks presence at
    // every level, not only at the entry point.
    let (victim_height, victim_hash) = path[3];
    assert_eq!(victim_height, 253);
    assert!(
        store.remove(&victim_hash),
        "the node selected for removal must actually be in the store"
    );
    assert_eq!(store.len(), nodes_before - 1);

    let result = smt.update(k1, b"v1-changed");

    match result {
        Err(SmtError::MissingNode { hash }) => assert_eq!(
            hash, victim_hash,
            "the error must name the node that was removed"
        ),
        Ok(root) => panic!(
            "update returned Ok with root {root:?} after a reachable node was \
             removed; a silently synthesised subtree is a state-root divergence"
        ),
        Err(other) => panic!("expected MissingNode, got: {other:?}"),
    }

    // The failed update must not have moved the tree. `update` assigns
    // `self.root` only on success, and a partially applied walk would be just
    // as silent a divergence as a wrong return value.
    assert_eq!(
        smt.root(),
        root_before,
        "a failed update must leave the root untouched"
    );
}
