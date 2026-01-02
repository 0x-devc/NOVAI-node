use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::Arc;

/// Errors returned by [`Mempool`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MempoolError {
    Duplicate,
}

/// A simple FIFO mempool keyed by a transaction id.
///
/// Notes:
/// - Ordering is FIFO by insertion time.
/// - `remove()` is supported; drained items skip anything already removed.
/// - This is intentionally minimal for Week 2 wiring.
pub struct Mempool<Id, Tx>
where
    Id: Eq + Hash + Copy,
{
    id_of: Arc<dyn Fn(&Tx) -> Id + Send + Sync>,
    by_id: HashMap<Id, Tx>,
    order: VecDeque<Id>,
}

impl<Id, Tx> Mempool<Id, Tx>
where
    Id: Eq + Hash + Copy,
{
    /// Create a new mempool with a function that extracts the tx id from a transaction.
    pub fn new(id_of: impl Fn(&Tx) -> Id + Send + Sync + 'static) -> Self {
        Self {
            id_of: Arc::new(id_of),
            by_id: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Insert a transaction. Rejects duplicates by tx id.
    pub fn insert(&mut self, tx: Tx) -> Result<(), MempoolError> {
        let id = (self.id_of)(&tx);
        if self.by_id.contains_key(&id) {
            return Err(MempoolError::Duplicate);
        }

        self.by_id.insert(id, tx);
        self.order.push_back(id);
        Ok(())
    }

    /// Remove a transaction by id.
    pub fn remove(&mut self, id: Id) -> Option<Tx> {
        self.by_id.remove(&id)
    }

    /// Returns true if the mempool currently contains this id.
    pub fn contains(&self, id: Id) -> bool {
        self.by_id.contains_key(&id)
    }

    /// Get a reference to a tx by id.
    pub fn get(&self, id: Id) -> Option<&Tx> {
        self.by_id.get(&id)
    }

    /// Number of currently-stored transactions.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// True if empty.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Drain up to `max` transactions in FIFO order.
    ///
    /// This skips ids that were previously removed.
    pub fn drain_ready(&mut self, max: usize) -> Vec<Tx> {
        if max == 0 {
            return Vec::new();
        }

        // IMPORTANT: never pre-allocate with an untrusted `max`, because it can panic
        // (capacity overflow) if it's huge. Cap it to the current queue length.
        let cap = max.min(self.order.len());
        let mut out = Vec::with_capacity(cap);

        while out.len() < max {
            let Some(id) = self.order.pop_front() else {
                break;
            };

            if let Some(tx) = self.by_id.remove(&id) {
                out.push(tx);
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Tx {
        id: u64,
        payload: &'static str,
    }

    #[test]
    fn insert_and_get_and_contains() {
        let mut mp = Mempool::<u64, Tx>::new(|tx| tx.id);

        mp.insert(Tx {
            id: 1,
            payload: "a",
        })
        .unwrap();
        assert!(mp.contains(1));
        assert_eq!(mp.len(), 1);

        let tx = mp.get(1).unwrap();
        assert_eq!(tx.payload, "a");
    }

    #[test]
    fn duplicate_rejected() {
        let mut mp = Mempool::<u64, Tx>::new(|tx| tx.id);

        mp.insert(Tx {
            id: 7,
            payload: "x",
        })
        .unwrap();
        let err = mp
            .insert(Tx {
                id: 7,
                payload: "y",
            })
            .unwrap_err();
        assert_eq!(err, MempoolError::Duplicate);

        // original remains
        assert_eq!(mp.get(7).unwrap().payload, "x");
        assert_eq!(mp.len(), 1);
    }

    #[test]
    fn remove_works() {
        let mut mp = Mempool::<u64, Tx>::new(|tx| tx.id);

        mp.insert(Tx {
            id: 2,
            payload: "b",
        })
        .unwrap();
        let removed = mp.remove(2).unwrap();
        assert_eq!(removed.payload, "b");
        assert!(!mp.contains(2));
        assert_eq!(mp.len(), 0);
    }

    #[test]
    fn drain_ready_fifo_and_skips_removed() {
        let mut mp = Mempool::<u64, Tx>::new(|tx| tx.id);

        mp.insert(Tx {
            id: 1,
            payload: "a",
        })
        .unwrap();
        mp.insert(Tx {
            id: 2,
            payload: "b",
        })
        .unwrap();
        mp.insert(Tx {
            id: 3,
            payload: "c",
        })
        .unwrap();

        // remove one in the middle before draining
        mp.remove(2);

        let drained = mp.drain_ready(10);
        let payloads: Vec<_> = drained.into_iter().map(|t| t.payload).collect();
        assert_eq!(payloads, vec!["a", "c"]);
        assert_eq!(mp.len(), 0);
        assert!(mp.is_empty());
    }

    #[test]
    fn drain_respects_max() {
        let mut mp = Mempool::<u64, Tx>::new(|tx| tx.id);

        mp.insert(Tx {
            id: 1,
            payload: "a",
        })
        .unwrap();
        mp.insert(Tx {
            id: 2,
            payload: "b",
        })
        .unwrap();
        mp.insert(Tx {
            id: 3,
            payload: "c",
        })
        .unwrap();

        let drained1 = mp.drain_ready(2);
        assert_eq!(drained1.len(), 2);
        assert_eq!(mp.len(), 1);

        let drained2 = mp.drain_ready(2);
        assert_eq!(drained2.len(), 1);
        assert_eq!(mp.len(), 0);
    }
}
