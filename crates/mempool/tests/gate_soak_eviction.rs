//! Gate SOAK phase 2 (A2): the dead-past eviction boundary.
//!
//! These pins exist to hold one line: eviction fires ONLY on DEAD-PAST
//! (nonce strictly below the chain's expected nonce), and NEVER on READY
//! (nonce == expected), WAITING (in the reachable run) or GAPPED
//! (unreachable from the current pool contents).
//!
//! Why the boundary is where it is:
//!
//! - DEAD-PAST is provably dead. Inclusion requires `tx.nonce == expected`
//!   at some future drain (`drain_ready`), and `expected` is monotonically
//!   non-decreasing: it advances only in the node's commit callback, and
//!   commits are final under the 3-chain rule, so nothing lowers it. A
//!   transaction already below `expected` can therefore never be selected.
//!
//! - GAPPED is NOT provably dead. A transaction at nonce n whose
//!   predecessor m is missing becomes includable the moment m arrives and
//!   commits, and m may legitimately be in flight, retried by a wallet, or
//!   simply arriving out of order. Evicting it on classification alone
//!   would destroy transactions the chain would otherwise have accepted.
//!
//! That asymmetry is the whole safety argument for this gate, so it gets
//! pins on both sides rather than only on the eviction side.

use ed25519_dalek::{SigningKey, VerifyingKey};
use mempool::{NonceProvider, TxMempool};
use novai_crypto::{address_from_pubkey, sign_tx_v1};
use novai_types::{Address, TxV1, TxVersion};
use std::collections::HashMap;

#[derive(Default)]
struct Nonces {
    map: HashMap<Address, u64>,
}

impl Nonces {
    fn set(&mut self, from: Address, nonce: u64) {
        self.map.insert(from, nonce);
    }
}

impl NonceProvider for Nonces {
    fn expected_nonce(&self, from: &Address) -> u64 {
        *self.map.get(from).unwrap_or(&0)
    }
}

fn keypair(seed: u8) -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let vk = sk.verifying_key();
    (sk, vk)
}

fn signed(sk: &SigningKey, vk: &VerifyingKey, nonce: u64, payload: &[u8]) -> TxV1 {
    let mut tx = TxV1 {
        version: TxVersion::V1,
        from: address_from_pubkey(vk),
        pubkey: vk.to_bytes(),
        nonce,
        fee: 1,
        payload: payload.to_vec(),
        sig: [0u8; 64],
    };
    sign_tx_v1(sk, &mut tx).expect("sign");
    tx
}

/// Admit a transaction regardless of the nonce floor, by temporarily
/// telling the pool the sender is at that nonce. Lets a test build an
/// arbitrary pooled nonce set (including nonces below the eventual
/// expected value) before exercising eviction.
fn admit(mp: &mut TxMempool, np: &mut Nonces, tx: TxV1) {
    let from = tx.from;
    let saved = np.expected_nonce(&from);
    np.set(from, tx.nonce);
    mp.insert(tx, np).expect("admitted");
    np.set(from, saved);
}

/// S1. THE BOUNDARY. One sender holding, relative to expected E:
/// two DEAD-PAST, one READY, one WAITING, one GAPPED.
/// Exactly the two dead-past transactions may be evicted.
#[test]
fn evicts_dead_past_only_and_spares_ready_waiting_and_gapped() {
    let (sk, vk) = keypair(1);
    let from = address_from_pubkey(&vk);
    let e: u64 = 10;

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();

    // DEAD-PAST: below E.
    admit(&mut mp, &mut np, signed(&sk, &vk, e - 2, b"dead-a"));
    admit(&mut mp, &mut np, signed(&sk, &vk, e - 1, b"dead-b"));
    // READY: exactly E.
    admit(&mut mp, &mut np, signed(&sk, &vk, e, b"ready"));
    // WAITING: E+1, contiguous with the ready one.
    admit(&mut mp, &mut np, signed(&sk, &vk, e + 1, b"waiting"));
    // GAPPED: E+3, unreachable because E+2 is absent.
    admit(&mut mp, &mut np, signed(&sk, &vk, e + 3, b"gapped"));

    assert_eq!(mp.pending_count(&from), 5, "all five admitted");

    let evicted = mp.evict_dead_past(&from, e);

    assert_eq!(
        evicted, 2,
        "exactly the two transactions below expected are dead, no more and no fewer"
    );
    assert_eq!(
        mp.pending_count(&from),
        3,
        "READY, WAITING and GAPPED must all survive dead-past eviction"
    );

    // Prove which three survived: only nonce == E is drainable now, and
    // draining it must yield the READY transaction, not a resurrected
    // dead-past one.
    np.set(from, e);
    let drained = mp.drain_ready(10, &np);
    assert_eq!(drained.len(), 1, "exactly one transaction is ready at E");
    assert_eq!(
        drained[0].payload, b"ready",
        "the surviving ready transaction must be the one at nonce == expected"
    );

    // WAITING and GAPPED are still pooled after the ready one left.
    assert_eq!(
        mp.pending_count(&from),
        2,
        "WAITING and GAPPED remain pooled after the ready transaction drains"
    );
}

/// S2. GAPPED IS NOT PROVABLY DEAD. A sender holding nothing but gapped
/// transactions loses none of them, no matter how many times the
/// commit-triggered sweep runs, because its expected nonce never advanced.
#[test]
fn gapped_transactions_are_never_evicted_on_classification_alone() {
    let (sk, vk) = keypair(2);
    let from = address_from_pubkey(&vk);
    let e: u64 = 100;

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();

    // Every one of these is unreachable: nonce 100 is absent, so nothing
    // here can ever be selected until it arrives.
    for n in [e + 1, e + 2, e + 5] {
        admit(&mut mp, &mut np, signed(&sk, &vk, n, b"gap"));
    }
    assert_eq!(mp.pending_count(&from), 3);

    // The sweep runs on every commit. Run it many times over. None of these
    // is below expected, so none of them is provably dead, so none may go.
    for _ in 0..50 {
        assert_eq!(
            mp.evict_dead_past(&from, e),
            0,
            "a gapped transaction is not provably dead and must never be evicted"
        );
    }
    assert_eq!(
        mp.pending_count(&from),
        3,
        "the whole gapped set survives an unbounded number of sweeps"
    );

    // And they are genuinely still usable: once the missing predecessor
    // arrives and commits, the run becomes reachable and drains in order.
    admit(&mut mp, &mut np, signed(&sk, &vk, e, b"filler"));
    np.set(from, e);
    assert_eq!(mp.drain_ready(10, &np).len(), 1, "nonce E drains");
    np.set(from, e + 1);
    assert_eq!(
        mp.drain_ready(10, &np).len(),
        1,
        "the previously gapped nonce E+1 is now reachable and drains, which \
         is exactly why it could not be evicted earlier"
    );
}

/// S3. THE FOLLOWER LEAK THIS CLOSES. Two transactions share nonce 5.
/// One commits and is removed by txid through the deferred queue. The
/// other is now below expected and its txid never committed, so nothing
/// on a follower removes it. That is the slot leak A2 exists to close.
#[test]
fn same_nonce_loser_is_reclaimed_after_its_rival_commits() {
    let (sk, vk) = keypair(3);
    let from = address_from_pubkey(&vk);

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();
    np.set(from, 5);

    let winner = signed(&sk, &vk, 5, b"winner");
    let loser = signed(&sk, &vk, 5, b"loser");
    let winner_id = novai_codec::txid_v1(&winner).unwrap();

    mp.insert(winner, &np).expect("winner admitted");
    mp.insert(loser, &np).expect("loser admitted at the same nonce");
    assert_eq!(mp.pending_count(&from), 2);

    // The winner commits: removed by txid, and the chain's expected nonce
    // advances past 5. This is exactly what the deferred queue does.
    mp.remove(&winner_id);
    np.set(from, 6);

    assert_eq!(
        mp.pending_count(&from),
        1,
        "pre-sweep the loser still holds a slot: its txid never committed, \
         so the committed-txid queue cannot reach it"
    );

    let evicted = mp.evict_dead_past(&from, 6);
    assert_eq!(evicted, 1, "the loser is below expected and provably dead");
    assert_eq!(
        mp.pending_count(&from),
        0,
        "the sender's slot is reclaimed on every node, not only on the leader"
    );
}

/// S4. Eviction is strictly per sender. Sweeping one sender must not
/// touch another, even one whose nonces sit in the same numeric range.
#[test]
fn eviction_never_touches_another_sender() {
    let (sk_a, vk_a) = keypair(4);
    let (sk_b, vk_b) = keypair(5);
    let a = address_from_pubkey(&vk_a);
    let b = address_from_pubkey(&vk_b);

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();

    for n in [1u64, 2, 3] {
        admit(&mut mp, &mut np, signed(&sk_a, &vk_a, n, b"a"));
        admit(&mut mp, &mut np, signed(&sk_b, &vk_b, n, b"b"));
    }
    assert_eq!(mp.pending_count(&a), 3);
    assert_eq!(mp.pending_count(&b), 3);

    // Sweep A only, with an expected nonce that would condemn all of B's
    // transactions too if the sweep were not sender scoped.
    let evicted = mp.evict_dead_past(&a, 99);
    assert_eq!(evicted, 3, "all of A's transactions are below 99");
    assert_eq!(mp.pending_count(&a), 0, "A is cleared");
    assert_eq!(
        mp.pending_count(&b),
        3,
        "B is untouched: eviction is scoped to the swept sender"
    );
}

/// S5. The derived per-sender count and the pool contents cannot drift
/// apart across the new eviction path. This is the H-08 leak class, which
/// has bitten this file once already.
#[test]
fn pending_count_tracks_contents_across_eviction() {
    let (sk, vk) = keypair(6);
    let from = address_from_pubkey(&vk);

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();

    for n in 0..8u64 {
        admit(&mut mp, &mut np, signed(&sk, &vk, n, b"x"));
    }
    assert_eq!(mp.pending_count(&from), 8);
    assert_eq!(mp.len(), 8);

    mp.evict_dead_past(&from, 3);
    assert_eq!(mp.pending_count(&from), 5, "nonces 3..7 remain");
    assert_eq!(
        mp.len(),
        mp.pending_count(&from),
        "the derived per-sender count equals the pool size for a single sender"
    );

    mp.evict_dead_past(&from, 8);
    assert_eq!(mp.pending_count(&from), 0);
    assert_eq!(mp.len(), 0);
    assert_eq!(mp.total_bytes(), 0, "byte accounting returns to zero");

    // The sender must be able to fill its slots again afterwards, which is
    // the property the original H-08 leak destroyed.
    np.set(from, 8);
    for n in 8..8 + (mempool::MAX_PENDING_PER_SENDER as u64) {
        let tx = signed(&sk, &vk, n, b"refill");
        let saved = np.expected_nonce(&from);
        np.set(from, n);
        mp.insert(tx, &np)
            .unwrap_or_else(|e| panic!("refill at nonce {n} must be admitted, got {e:?}"));
        np.set(from, saved);
    }
    assert_eq!(mp.pending_count(&from), mempool::MAX_PENDING_PER_SENDER);
}

// ===========================================================================
// Gate SOAK phase 5 (C1): the read-only census that gives the monitor the
// distinction novai_mempool_size cannot make.
// ===========================================================================

/// The census separates a healthy deep backlog from a jam. Both look
/// identical to a pool-size gauge, which is why no threshold on that gauge
/// can be right in both directions.
#[test]
fn census_separates_a_healthy_backlog_from_a_jam() {
    let (sk_h, vk_h) = keypair(21);
    let (sk_j, vk_j) = keypair(22);
    let healthy = address_from_pubkey(&vk_h);
    let jammed = address_from_pubkey(&vk_j);
    let e: u64 = 50;

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();
    np.set(healthy, e);
    np.set(jammed, e);

    // A healthy sender: a full contiguous run from expected.
    for n in e..(e + 8) {
        admit(&mut mp, &mut np, signed(&sk_h, &vk_h, n, b"ok"));
    }
    // A jammed sender: nothing at expected, so none of it is reachable.
    for n in (e + 1)..(e + 9) {
        admit(&mut mp, &mut np, signed(&sk_j, &vk_j, n, b"jam"));
    }

    let c = mp.census(&np);
    assert_eq!(mp.len(), 16, "the pool-size gauge sees one number for both");
    assert_eq!(c.senders, 2);
    assert_eq!(c.ready, 1, "only the healthy sender has a next-block tx");
    assert_eq!(c.waiting, 7, "the rest of the healthy run");
    assert_eq!(c.gapped, 8, "the whole jammed sender is unreachable");
    assert_eq!(c.dead_past, 0);
}

/// THE ASYMMETRY, PINNED. Observation is strict where eviction is lenient.
///
/// Eviction grants a one-nonce grace when `expected` is missing, because it
/// may be in flight and destroying a live transaction is unacceptable. The
/// census must NOT grant it: a client that never sent `expected` is exactly
/// the jam this gate exists to surface, and a grace would report its whole
/// stuck queue as healthy backlog.
///
/// The price is that a leader reads its own just-drained head as a gap for
/// the sub-second window before commit. Time is what separates that from a
/// real jam, and the alarms built on this gauge carry a persistence window of
/// minutes, so the transient never reaches an operator.
#[test]
fn census_is_strict_where_eviction_is_lenient() {
    let (sk, vk) = keypair(23);
    let from = address_from_pubkey(&vk);
    let e: u64 = 50;

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();
    np.set(from, e);
    for n in e..(e + 5) {
        admit(&mut mp, &mut np, signed(&sk, &vk, n, b"run"));
    }
    assert_eq!(mp.census(&np).waiting, 4, "a whole run is healthy backlog");

    // The leader drains the head. Expected does not advance until commit.
    assert_eq!(mp.drain_ready(10, &np).len(), 1);

    let c = mp.census(&np);
    assert_eq!(c.ready, 0, "the head is in flight");
    assert_eq!(
        c.gapped, 4,
        "the census reports the hole honestly rather than papering over it"
    );
    assert_eq!(c.waiting, 0);

    // Eviction, given the same state, destroys nothing: the grace lives
    // there, where being wrong costs a live transaction.
    assert_eq!(mp.evict_dead_past(&from, e), 0);
    assert_eq!(mp.pending_count(&from), 4, "nothing was evicted");
}

/// The census never mutates the pool. It is observation, and eviction must
/// not be reachable through it.
#[test]
fn census_is_read_only() {
    let (sk, vk) = keypair(24);
    let from = address_from_pubkey(&vk);
    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();

    for n in [1u64, 2, 7] {
        admit(&mut mp, &mut np, signed(&sk, &vk, n, b"x"));
    }
    np.set(from, 5); // makes 1 and 2 dead-past, 7 gapped

    let before_len = mp.len();
    let before_bytes = mp.total_bytes();
    let c = mp.census(&np);
    assert_eq!(c.dead_past, 2);
    assert_eq!(c.gapped, 1);
    assert_eq!(mp.len(), before_len, "census must not evict");
    assert_eq!(mp.total_bytes(), before_bytes, "census must not touch bytes");
    assert_eq!(mp.pending_count(&from), 3);
}
