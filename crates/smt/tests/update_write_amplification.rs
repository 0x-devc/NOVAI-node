use std::cell::RefCell;
use std::rc::Rc;

use novai_smt::hash::{empty_hash_at_height, Hash32};
use novai_smt::node::Node;
use novai_smt::smt::{Smt, SmtStore};

/// Counting store for the write-amplification gates.
///
/// `MemoryStore` in the production crate keeps its `nodes` field private, and a
/// public accessor would be production API surface that exists only to serve a
/// test. This store therefore reimplements the same content-addressed dedup
/// (linear scan, overwrite on hash match, otherwise push) and additionally
/// exposes the two numbers the disk claim rests on:
/// - `distinct()`: rows that would exist under `smt/node/<hash>` on disk;
/// - `puts()`: raw write calls, which is the write amplification.
///
/// The store is shared through `Rc<RefCell<..>>` because `Smt` takes its store
/// by value; the counts must be readable between operations without consuming
/// the tree.
#[derive(Default)]
struct StoreInner {
    nodes: Vec<(Hash32, [u8; Node::ENCODED_LEN])>,
    puts: usize,
}

#[derive(Clone, Default)]
struct CountingStore {
    inner: Rc<RefCell<StoreInner>>,
}

impl CountingStore {
    fn distinct(&self) -> usize {
        self.inner.borrow().nodes.len()
    }

    fn puts(&self) -> usize {
        self.inner.borrow().puts
    }
}

impl SmtStore for CountingStore {
    type Error = ();

    fn get_node(&self, node_hash: &Hash32) -> Result<Option<[u8; Node::ENCODED_LEN]>, Self::Error> {
        Ok(self
            .inner
            .borrow()
            .nodes
            .iter()
            .find(|(k, _)| k == node_hash)
            .map(|(_, v)| *v))
    }

    fn put_node(
        &mut self,
        node_hash: &Hash32,
        node_bytes: &[u8; Node::ENCODED_LEN],
    ) -> Result<(), Self::Error> {
        let mut inner = self.inner.borrow_mut();
        inner.puts += 1;
        for (k, v) in inner.nodes.iter_mut() {
            if k == node_hash {
                *v = *node_bytes;
                return Ok(());
            }
        }
        inner.nodes.push((*node_hash, *node_bytes));
        Ok(())
    }
}

/// R1: every `update` walks all 256 levels and writes a node at each one.
///
/// The tree is a fixed 256-level SMT with no path compression. `update` seeds
/// the upward rebuild with `hash_leaf(&key, value_bytes)`, a TAG_LEAF blake3
/// output, which is never equal to `empty_hash_at_height(0)`, a TAG_EMPTY
/// output. The collapse rule in `recompute_path` requires BOTH children to
/// equal the empty hash for the child height, so it can never fire on the first
/// level of an update. The parent it writes instead is a TAG_INTERNAL hash,
/// which is again never an empty hash, so by induction the collapse cannot fire
/// at any of the 256 levels either. Every level writes.
///
/// 256 is the constant every reclaim estimate in the SMT garbage-collection
/// work rests on, so it is pinned exactly rather than as a bound.
#[test]
fn smt_update_writes_exactly_256_nodes() {
    let store = CountingStore::default();
    let mut smt = Smt::new(store.clone());

    let k1: Hash32 = [0x01u8; 32];
    let k2: Hash32 = [0x02u8; 32];

    // One update into a fresh empty tree.
    smt.update(k1, b"v1").unwrap();
    assert_eq!(
        store.distinct(),
        256,
        "one update into an empty tree must store 256 distinct nodes"
    );
    assert_eq!(
        store.puts(),
        256,
        "one update must issue exactly 256 raw put calls"
    );

    // A second update at a DIFFERENT key. The two keys share a prefix, so the
    // upper levels are recomputed rather than untouched, and a recomputed
    // parent is a new content address: nothing is overwritten.
    smt.update(k2, b"v2").unwrap();
    assert_eq!(
        store.distinct(),
        512,
        "a second update at a different key must bring distinct nodes to 512"
    );
    assert_eq!(
        store.puts(),
        512,
        "two updates must issue exactly 512 raw put calls"
    );

    // THIS IS THE ORPHANING, and it is the most important assertion in the file.
    // Re-updating a key that is already in the tree does not reuse or replace a
    // single row. The new leaf hash differs, so every one of the 256 parents
    // above it hashes differently, so the store is addressed at 256 new keys and
    // the 256 predecessors become unreachable while staying on disk forever.
    // This is the mechanism behind 26.5 GB of store holding a 3 MB live tree.
    smt.update(k1, b"v1-changed").unwrap();
    assert_eq!(
        store.distinct(),
        768,
        "re-updating an existing key must ADD 256 nodes, not replace them"
    );
    assert_eq!(
        store.puts(),
        768,
        "three updates must issue exactly 768 raw put calls"
    );
}

/// R1 asymmetry: `delete` seeds the rebuild with `empty_hash_at_height(0)`, so
/// the collapse rule CAN fire, and the 256-per-walk constant is a property of
/// `update` specifically rather than of any walk through the tree.
#[test]
fn smt_delete_collapses_and_writes_far_fewer_than_256() {
    let k1: Hash32 = [0x01u8; 32];
    let k2: Hash32 = [0x02u8; 32];

    // Case one: the deleted key is the only key. Every sibling on its path is
    // empty, so the collapse fires at level 0 and again at every level above,
    // and the walk writes nothing at all. This count is not shape dependent:
    // a single-key tree always collapses the whole way back to the empty root.
    let solo_store = CountingStore::default();
    let mut solo = Smt::new(solo_store.clone());
    solo.update(k1, b"v1").unwrap();
    let after_insert = solo_store.puts();
    assert_eq!(after_insert, 256);

    solo.delete(k1).unwrap();
    let delete_puts = solo_store.puts() - after_insert;
    assert_eq!(
        delete_puts, 0,
        "deleting the only key must collapse every level and write nothing"
    );
    assert_eq!(
        solo.root(),
        empty_hash_at_height(256),
        "deleting the only key must return the tree to the empty root"
    );

    // Case two: a sibling key keeps the upper levels alive, so the collapse
    // stops at the divergence point and only the levels from there to the root
    // are rewritten. That count depends on where the two keys first differ, so
    // it is asserted as a bound rather than pinned: the point is that it is
    // nowhere near 256. For these two keys it is 7, because 0x01 and 0x02 first
    // differ at bit index 6 and there are 7 levels from there up to the root.
    let pair_store = CountingStore::default();
    let mut pair = Smt::new(pair_store.clone());
    pair.update(k1, b"v1").unwrap();
    pair.update(k2, b"v2").unwrap();
    let before_delete = pair_store.puts();

    pair.delete(k1).unwrap();
    let pair_delete_puts = pair_store.puts() - before_delete;
    assert!(
        pair_delete_puts < 32,
        "an isolated delete must write on the order of the divergence depth, \
         not 256; wrote {pair_delete_puts}"
    );
    assert!(
        pair_delete_puts > 0,
        "the sibling key keeps the upper levels non-empty, so some levels must \
         still be rewritten; wrote {pair_delete_puts}"
    );
}
