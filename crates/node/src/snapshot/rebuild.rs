//! From-scratch SMT rebuild over the exported flat leaf set.
//!
//! Drives the canonical execution path (append_smt_ops_for_state_ops,
//! crates/execution/src/lib.rs:6530-6573) one pair per walk, mirroring the
//! node's per-transaction batching, against a fresh in-memory store. The SMT
//! root is a pure function of the final leaf map (order independence is
//! pinned by crates/smt tests and re-pinned here), so this reproduces the
//! chain root if and only if the pair set equals the chain's authenticated
//! state.

use std::collections::BTreeSet;

use novai_execution::{append_smt_ops_for_state_ops, empty_smt_root};
use novai_smt::{Node, NodeChild};
use novai_state::{
    decode_smt_root_v1, smt_node_key, Kv, KvBatch, WriteOp, KEY_PREFIX_SMT_NODE, KEY_SMT_ROOT,
};

use crate::snapshot::store::BTreeKv;

/// What a from-scratch rebuild produced: the root, the rows it left in its
/// store, and the rows that root actually reaches.
///
/// THE TWO FIGURES ARE NOT THE SAME, and assuming they were is a trap I walked
/// into. The rebuild replays leaves ONE AT A TIME, matching the node's
/// per-transaction batching, so each walk rewrites the root path of every walk
/// before it and orphans the previous version. A rebuild over K leaves
/// therefore leaves slightly MORE than the live set in its store: the live set
/// plus the orphans the rebuild itself created on the way. The excess is small
/// (measured at 12 rows over 1,268 live for a 5 leaf tree) and it is bounded by
/// the leaf count rather than by the churn the reclaim exists to remove, but it
/// is not zero, and a report that called `stored_rows` the live count would
/// overstate the live tree and understate the reclaim.
///
/// `live_rows` is therefore computed by WALKING from the root, which is the
/// definition of reachable rather than a proxy for it. Nothing in this repo
/// measured that before: A3 counts the whole family, live and dead together,
/// and the G0 gauge measures bytes in a key range without separating the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebuiltTree {
    pub root: [u8; 32],
    /// Rows the rebuild left in its store, which is exactly what a rebuild into
    /// a real directory writes: the live set plus its own transient orphans.
    pub stored_rows: u64,
    pub stored_bytes: u64,
    /// Rows reachable from `root`. THE live set. Leaves are never stored as
    /// rows, so this counts internal nodes only.
    pub live_rows: u64,
    pub live_bytes: u64,
}

impl RebuiltTree {
    /// Rows the rebuild wrote that its own root does not reach.
    #[must_use]
    pub const fn rebuild_orphan_rows(&self) -> u64 {
        self.stored_rows.saturating_sub(self.live_rows)
    }
}

/// Rebuild the root from flat (key, value) pairs. The empty set yields the
/// canonical empty root, matching every consensus absent-root default.
pub fn rebuild_root(pairs: &[(Vec<u8>, Vec<u8>)]) -> Result<[u8; 32], String> {
    rebuild_tree(pairs).map(|t| t.root)
}

/// The same rebuild, reporting the tree it built as well as its root.
///
/// `rebuild_root` delegates here, so the audit and the reclaim census share one
/// rebuild. A second implementation that counted while rebuilding would be a
/// second thing that can disagree with the root the audit trusts.
pub fn rebuild_tree(pairs: &[(Vec<u8>, Vec<u8>)]) -> Result<RebuiltTree, String> {
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
    let root = match db.get(KEY_SMT_ROOT) {
        Ok(Some(bytes)) => decode_smt_root_v1(&bytes).map_err(|e| format!("{e:?}"))?,
        Ok(None) => empty_smt_root(),
        Err(e) => return Err(format!("{e:?}")),
    };
    let stored = db
        .scan_prefix(KEY_PREFIX_SMT_NODE)
        .map_err(|e| format!("{e:?}"))?;
    let stored_bytes = stored
        .iter()
        .map(|(k, v)| (k.len() + v.len()) as u64)
        .sum::<u64>();
    let (live_rows, live_bytes) = walk_reachable(&db, root)?;
    Ok(RebuiltTree {
        root,
        stored_rows: stored.len() as u64,
        stored_bytes,
        live_rows,
        live_bytes,
    })
}

/// Count the rows reachable from `root`, and their logical bytes.
///
/// This is the definition of the live set rather than a proxy for it: descend
/// from the root, resolve every internal child, and count what you touch.
///
/// GENERIC OVER THE STORE ON PURPOSE. It runs against the in-memory rebuild to
/// produce the live figure, and against a real RocksDB directory to prove the
/// tree that landed there is complete. Those are the same question and they
/// must not be answered by two implementations that can disagree.
///
/// WHY THE ON-DISK CALLER EXISTS AT ALL. The A0 audit cannot do this job. A4
/// rebuilds the root from the LEAVES into a fresh store and A5 compares that to
/// the stored root; neither ever reads the directory's own `smt/node/` rows. So
/// a directory whose node store was empty, truncated or corrupt still audits
/// PASS at the right height and the right root. The plan this work implements
/// says a deleted live node would be "caught before the rename by A5", and that
/// is not true. This walk is what makes it true.
///
/// HEIGHT IS TRACKED because the tree is a fixed 256 levels with no path
/// compression. The root sits at height 256 and a node at height H has children
/// at H-1, so the children of a height-1 node are LEAF hashes at height 0, and
/// leaves are never stored as rows (`crates/smt/src/node.rs`). Descending into
/// one looking for a node row finds nothing, and reading that absence as
/// corruption would condemn every healthy tree.
///
/// A genuinely absent INTERNAL node is an error, not a smaller count. Returning
/// a number there would let a broken tree report a plausible live set, which is
/// the failure this whole gate exists to make impossible.
///
/// # Errors
/// Returns an error naming the hash and height of the first reachable node that
/// is absent or undecodable.
pub fn walk_reachable<K>(db: &K, root: [u8; 32]) -> Result<(u64, u64), String>
where
    K: Kv,
    K::Error: std::fmt::Debug,
{
    let mut seen: BTreeSet<[u8; 32]> = BTreeSet::new();
    let mut bytes = 0u64;
    let mut stack: Vec<([u8; 32], u16)> = vec![(root, 256)];

    while let Some((hash, height)) = stack.pop() {
        if !seen.insert(hash) {
            continue;
        }
        let key = smt_node_key(&hash);
        let Some(encoded) = db.get(&key).map_err(|e| format!("{e:?}"))? else {
            // An empty tree's root is an empty hash with no row behind it, and
            // that is the one absence that is legitimate at the top.
            if height == 256 && hash == empty_smt_root() {
                seen.remove(&hash);
                continue;
            }
            return Err(format!(
                "rebuilt tree is incomplete: no node row for {} at height {height}",
                hex::encode(hash)
            ));
        };
        bytes += (key.len() + encoded.len()) as u64;
        let node = Node::decode(&encoded).map_err(|e| format!("decode rebuilt node: {e:?}"))?;
        for child in [&node.left, &node.right] {
            if let NodeChild::Hash(ch) = child {
                if height > 1 {
                    stack.push((*ch, height - 1));
                }
            }
        }
    }
    Ok((seen.len() as u64, bytes))
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

    #[test]
    fn rebuild_root_and_rebuild_tree_agree() {
        // The delegation is the point: one rebuild, so the count can never be
        // taken from a tree the audit did not trust.
        let pairs = vec![pair(0x01, 10), pair(0x02, 20)];
        assert_eq!(
            rebuild_root(&pairs).unwrap(),
            rebuild_tree(&pairs).unwrap().root
        );
    }

    #[test]
    fn one_leaf_spans_exactly_256_rows_and_orphans_nothing() {
        // The tree is 256 levels with no path compression and the collapse rule
        // cannot fire on an update, so one leaf is exactly 256 internal nodes.
        // A single walk has nothing before it to strand.
        let t = rebuild_tree(&[pair(0x01, 10)]).unwrap();
        assert_eq!(t.stored_rows, 256);
        assert_eq!(t.live_rows, 256);
        assert_eq!(t.rebuild_orphan_rows(), 0);
    }

    #[test]
    fn replaying_leaves_one_at_a_time_strands_the_shared_top_of_the_tree() {
        // THE reason `stored_rows` and `live_rows` are separate fields. Two
        // walks write 256 rows each. But the second walk descends through the
        // levels the two paths share and rewrites them, because a changed child
        // gives a parent a new content hash, so the first walk's versions of
        // those shared levels are stranded the moment the second walk lands.
        // The excess is the shared depth, which is bounded by the leaf count,
        // NOT by the churn this reclaim exists to remove.
        let t = rebuild_tree(&[pair(0x01, 10), pair(0x02, 20)]).unwrap();
        assert_eq!(t.stored_rows, 512, "two walks write 256 rows each");
        assert!(
            t.live_rows < t.stored_rows,
            "the second walk must strand the shared top: stored {} live {}",
            t.stored_rows,
            t.live_rows
        );
        assert!(
            t.live_rows >= 256,
            "two leaves cannot span fewer rows than one"
        );
        assert_eq!(
            t.live_rows + t.rebuild_orphan_rows(),
            t.stored_rows,
            "the two figures must partition what the rebuild wrote"
        );
    }

    #[test]
    fn a_rewritten_key_costs_a_walk_but_adds_no_live_node() {
        // The tool never hits this case, because `extract_leaf_set` yields
        // DISTINCT keys and so drives exactly one walk per key. It is pinned
        // anyway: it is the in-miniature version of what the source database
        // did over its whole life, and it states the invariant that matters,
        // which is that rewriting a key moves the live set sideways rather than
        // growing it.
        let clean = vec![pair(0x01, 99), pair(0x02, 20)];
        let churned = vec![pair(0x01, 10), pair(0x02, 20), pair(0x01, 99)];
        let a = rebuild_tree(&clean).unwrap();
        let b = rebuild_tree(&churned).unwrap();

        assert_eq!(a.root, b.root, "same final leaf map, same root");
        assert_eq!(
            a.live_rows, b.live_rows,
            "and the same live set: the rewrite orphaned a path, it did not add one"
        );
        assert_eq!(b.stored_rows, 768, "but it did cost a third walk");
        assert!(b.rebuild_orphan_rows() > a.rebuild_orphan_rows());
    }

    #[test]
    fn the_empty_set_spans_no_nodes() {
        let t = rebuild_tree(&[]).unwrap();
        assert_eq!(t.root, empty_smt_root());
        assert_eq!(t.stored_rows, 0);
        assert_eq!(t.live_rows, 0);
        assert_eq!(t.live_bytes, 0);
    }
}
