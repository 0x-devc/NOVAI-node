//! Gate SOAK phase 3 (A4 displacement, A5 nonce horizon).
//!
//! A4 lets a lower nonce from a sender displace that same sender's highest
//! pending nonce when the sender is at its slot cap. This is what breaks the
//! jam: when a desynced client resyncs and submits the one transaction the
//! chain is actually waiting for, that transaction must be able to get in
//! even though the sender's 16 slots are full of transactions it cannot use.
//!
//! Displacement is held to a DIFFERENT and weaker standard than the dead-past
//! eviction in phase 2, and the difference matters:
//!
//! - Dead-past eviction removes a transaction that can NEVER be included.
//!   Absolute.
//! - Displacement removes a transaction that is included strictly LATER than
//!   the one being admitted, in every possible future. It does not claim the
//!   victim is dead. It claims that swapping it for a lower nonce never costs
//!   an inclusion, and can only buy one.
//!
//! Two guards make that claim true, and both are pinned here:
//!
//! - P6: displacement runs only AFTER signature verification. Sender
//!   addresses are public, so if it ran earlier anyone could evict a chosen
//!   victim's queue for free with garbage transactions.
//! - P13: the incoming nonce must be one the sender does not already hold.
//!   Without it, a same-nonce duplicate at a low nonce would evict a healthy
//!   waiting transaction at a high nonce, shortening the reachable run to
//!   admit something that can never add an inclusion.
//!
//! A5 bounds how far ahead a nonce may be admitted. It is an ADMISSION rule,
//! not an eviction rule: it never removes anything from the pool, and it
//! defers rather than refuses (see `horizon_defers_it_does_not_refuse`).

use ed25519_dalek::{SigningKey, VerifyingKey};
use mempool::{NonceProvider, TxMempool, TxMempoolError, MAX_PENDING_PER_SENDER};
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

/// Admit a transaction regardless of the nonce window, by briefly telling the
/// pool the sender sits at that nonce. Used only to BUILD a starting pool
/// shape; the behaviour under test is always exercised through a normal
/// `insert` afterwards.
fn seed_pool(mp: &mut TxMempool, np: &mut Nonces, tx: TxV1) {
    let from = tx.from;
    let saved = np.expected_nonce(&from);
    np.set(from, tx.nonce);
    mp.insert(tx, np).expect("seeded");
    np.set(from, saved);
}

/// A sender at its slot cap whose pooled nonces are NOT contiguous, so a
/// displacement is genuinely possible. Holds `expected` plus a gap at
/// `expected + 1`, then a contiguous tail. 16 transactions, max nonce
/// `expected + 16`.
fn victim_at_cap(mp: &mut TxMempool, np: &mut Nonces, sk: &SigningKey, vk: &VerifyingKey, e: u64) {
    seed_pool(mp, np, signed(sk, vk, e, b"held"));
    for n in (e + 2)..=(e + 16) {
        seed_pool(mp, np, signed(sk, vk, n, b"held"));
    }
    let from = address_from_pubkey(vk);
    assert_eq!(
        mp.pending_count(&from),
        MAX_PENDING_PER_SENDER,
        "victim must start exactly at the slot cap"
    );
}

// ===========================================================================
// P6: the security pin. Displacement runs after signature verification.
// ===========================================================================

/// A forged transaction claiming a public address cannot evict anything.
///
/// Sender addresses are public. If displacement ran before signature
/// verification, anyone could pick a victim, submit garbage at a low nonce,
/// and knock the victim's highest-nonce transaction out of the pool for
/// free, repeatedly. Both forgery shapes are covered: a bad signature under
/// the victim's own pubkey, and an attacker pubkey pasted onto the victim's
/// address.
#[test]
fn a_forged_transaction_cannot_displace_a_victims_queue() {
    let (sk_v, vk_v) = keypair(11);
    let (sk_a, vk_a) = keypair(12);
    let victim = address_from_pubkey(&vk_v);
    let e: u64 = 100;

    for shape in ["bad-signature", "address-mismatch"] {
        let mut mp = TxMempool::new(1, 1000);
        let mut np = Nonces::default();
        np.set(victim, e);
        victim_at_cap(&mut mp, &mut np, &sk_v, &vk_v, e);

        let before_count = mp.pending_count(&victim);
        let before_top = mp.contains(&novai_codec::txid_v1(&signed(&sk_v, &vk_v, e + 16, b"held")).unwrap());
        assert!(before_top, "{shape}: victim's highest-nonce tx is pooled to start");

        // The gap at e+1 is exactly what a displacement would be allowed to
        // use, so this forgery is the strongest possible version of the
        // attack: it targets a nonce that WOULD displace if it were genuine.
        let forged = match shape {
            "bad-signature" => {
                let mut tx = signed(&sk_v, &vk_v, e + 1, b"forged");
                tx.sig[0] ^= 0xFF; // break the signature, keep the real pubkey
                tx
            }
            _ => {
                // Attacker signs with its own key but claims the victim's
                // address. Caught by the address/pubkey binding check.
                let mut tx = signed(&sk_a, &vk_a, e + 1, b"forged");
                tx.from = victim;
                tx
            }
        };

        // The rejection reason is not the point and deliberately not asserted:
        // before displacement exists the slot cap refuses this at the cheap
        // pre-check, and after it exists the transaction reaches verification
        // and is refused there. Either is correct. What must hold in BOTH
        // worlds is that the victim pays nothing for it.
        let _err = mp
            .insert(forged, &np)
            .expect_err("a forged transaction must never be admitted");

        assert_eq!(
            mp.pending_count(&victim),
            before_count,
            "{shape}: a forged transaction must not cost the victim a slot"
        );
        assert!(
            mp.contains(&novai_codec::txid_v1(&signed(&sk_v, &vk_v, e + 16, b"held")).unwrap()),
            "{shape}: the victim's highest-nonce transaction must still be pooled. \
             If this fails, displacement is running before signature verification \
             and any public address can have its queue destroyed for free."
        );
    }
}

// ===========================================================================
// P13: a same-nonce duplicate cannot displace a healthy waiting transaction.
// ===========================================================================

/// A sender holding a FULL contiguous run is at its most valuable: every one
/// of its 16 transactions is reachable and will drain in order. A second
/// transaction at a nonce it already holds adds nothing, because only one
/// transaction per nonce can ever be included. Admitting it by evicting the
/// top of the run would shorten the run to buy nothing.
#[test]
fn a_same_nonce_duplicate_cannot_displace_a_waiting_transaction() {
    let (sk, vk) = keypair(13);
    let from = address_from_pubkey(&vk);
    let e: u64 = 100;

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();
    np.set(from, e);

    // A full contiguous run: e .. e+15, all reachable.
    for n in e..(e + MAX_PENDING_PER_SENDER as u64) {
        seed_pool(&mut mp, &mut np, signed(&sk, &vk, n, b"run"));
    }
    assert_eq!(mp.pending_count(&from), MAX_PENDING_PER_SENDER);
    let top_id = novai_codec::txid_v1(&signed(&sk, &vk, e + 15, b"run")).unwrap();
    assert!(mp.contains(&top_id));

    // A genuine, correctly signed duplicate at a nonce already held. Its
    // nonce IS lower than the top of the run, so a displacement rule that
    // only compared nonces would happily evict e+15 for it.
    let dup = signed(&sk, &vk, e + 3, b"different-payload-same-nonce");
    let err = mp
        .insert(dup, &np)
        .expect_err("a duplicate nonce must not buy its way in");
    assert!(
        matches!(err, TxMempoolError::SenderLimitExceeded { .. }),
        "expected SenderLimitExceeded, got {err:?}"
    );

    assert!(
        mp.contains(&top_id),
        "the waiting transaction at the top of the run must survive: evicting it \
         to admit a duplicate nonce shortens the reachable run and buys nothing"
    );
    assert_eq!(mp.pending_count(&from), MAX_PENDING_PER_SENDER);

    // And the run is still whole: all 16 drain in nonce order.
    let mut drained = 0;
    for n in e..(e + MAX_PENDING_PER_SENDER as u64) {
        np.set(from, n);
        drained += mp.drain_ready(10, &np).len();
    }
    assert_eq!(
        drained, MAX_PENDING_PER_SENDER,
        "the full run must still drain, one transaction per nonce"
    );
}

// ===========================================================================
// A4: the jam breaker, and the run-never-shortens property.
// ===========================================================================

/// THE JAM. A desynced client ran ahead and filled all 16 slots with nonces
/// the chain cannot use, because the one it is waiting for is missing. The
/// client then resyncs and submits exactly that transaction. Before A4 it was
/// refused SenderLimitExceeded: the transaction that would unjam the sender
/// could not get in because the jam was full.
#[test]
fn the_resync_transaction_can_always_break_in() {
    let (sk, vk) = keypair(14);
    let from = address_from_pubkey(&vk);
    let e: u64 = 100;

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();
    np.set(from, e);

    // The client ran ahead: it holds e+1 .. e+16 and never sent e.
    for n in (e + 1)..=(e + 16) {
        seed_pool(&mut mp, &mut np, signed(&sk, &vk, n, b"ahead"));
    }
    assert_eq!(mp.pending_count(&from), MAX_PENDING_PER_SENDER);

    // Nothing is drainable: the chain wants e, and e is not pooled.
    assert!(
        mp.drain_ready(10, &np).is_empty(),
        "the sender is producing nothing: every pooled nonce is unreachable"
    );

    // The client resyncs and sends the transaction the chain is waiting for.
    let rescue = signed(&sk, &vk, e, b"rescue");
    mp.insert(rescue, &np)
        .expect("the resync transaction must be admitted even at the slot cap");

    // The whole run now drains in order, one nonce per commit.
    let mut drained = 0;
    for n in e..(e + MAX_PENDING_PER_SENDER as u64) {
        np.set(from, n);
        drained += mp.drain_ready(10, &np).len();
    }
    assert_eq!(
        drained, MAX_PENDING_PER_SENDER,
        "after the rescue, the sender drains a full contiguous run"
    );
}

/// Displacement never shortens the reachable run. Here it lengthens it from
/// one transaction to sixteen, which is the whole point.
#[test]
fn displacement_never_shortens_the_reachable_run() {
    let (sk, vk) = keypair(15);
    let from = address_from_pubkey(&vk);
    let e: u64 = 100;

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();
    np.set(from, e);

    // Run rooted at e has length 1: e is present, e+1 is not, and the sender
    // is at its slot cap so nothing more can normally get in.
    victim_at_cap(&mut mp, &mut np, &sk, &vk, e);

    // Supply the missing e+1 WITHOUT first draining anything, so the sender is
    // genuinely at the cap and the only way in is by displacing the top.
    let filler = signed(&sk, &vk, e + 1, b"filler");
    mp.insert(filler, &np)
        .expect("the gap-filling nonce must be admitted by displacing the top");
    assert_eq!(
        mp.pending_count(&from),
        MAX_PENDING_PER_SENDER,
        "displacement swaps one for one, it does not grow the sender's footprint"
    );

    // The run is now e .. e+15: sixteen reachable transactions where there
    // was one. Displacement lengthened it and shortened nothing.
    let mut drained = 0;
    for n in e..(e + MAX_PENDING_PER_SENDER as u64) {
        np.set(from, n);
        drained += mp.drain_ready(10, &np).len();
    }
    assert_eq!(
        drained, MAX_PENDING_PER_SENDER,
        "filling the gap turns a one-transaction run into a full sixteen"
    );
}

/// Displacement is strictly intra-sender: it can never reach another sender.
#[test]
fn displacement_never_touches_another_sender() {
    let (sk_a, vk_a) = keypair(16);
    let (sk_b, vk_b) = keypair(17);
    let a = address_from_pubkey(&vk_a);
    let b = address_from_pubkey(&vk_b);
    let e: u64 = 100;

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();
    np.set(a, e);
    np.set(b, e);

    victim_at_cap(&mut mp, &mut np, &sk_a, &vk_a, e);
    victim_at_cap(&mut mp, &mut np, &sk_b, &vk_b, e);

    let b_top = novai_codec::txid_v1(&signed(&sk_b, &vk_b, e + 16, b"held")).unwrap();

    // A displaces its own top by filling its gap.
    np.set(a, e + 1);
    mp.insert(signed(&sk_a, &vk_a, e + 1, b"fill"), &np)
        .expect("A displaces its own top");

    assert_eq!(mp.pending_count(&b), MAX_PENDING_PER_SENDER, "B is untouched");
    assert!(mp.contains(&b_top), "B's highest-nonce transaction survives");
}

// ===========================================================================
// A5: the nonce horizon.
// ===========================================================================

/// The last admissible nonce is `expected + 15`, so the admission window and
/// the slot cap are the same number: a sender's admissible nonces are exactly
/// the 16 it has slots for. Neither rule shadows the other.
#[test]
fn horizon_window_and_slot_cap_are_the_same_sixteen() {
    let (sk, vk) = keypair(18);
    let from = address_from_pubkey(&vk);
    let e: u64 = 100;

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();
    np.set(from, e);

    // Every nonce in [e, e+15] is admissible, and together they exactly fill
    // the sender's 16 slots.
    for n in e..(e + MAX_PENDING_PER_SENDER as u64) {
        mp.insert(signed(&sk, &vk, n, b"win"), &np)
            .unwrap_or_else(|err| panic!("nonce {n} is inside the window, got {err:?}"));
    }
    assert_eq!(mp.pending_count(&from), MAX_PENDING_PER_SENDER);
}

/// One past the window is refused, and refused with its own error rather
/// than silently accepted as it is today. The silent acceptance is what makes
/// a client's runaway nonce invisible to it.
#[test]
fn horizon_rejects_one_past_the_window() {
    let (sk, vk) = keypair(19);
    let from = address_from_pubkey(&vk);
    let e: u64 = 100;

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();
    np.set(from, e);

    let too_far = e + MAX_PENDING_PER_SENDER as u64; // e+16, first inadmissible
    let err = mp
        .insert(signed(&sk, &vk, too_far, b"far"), &np)
        .expect_err("a nonce past the window must be refused, not silently pooled");
    assert!(
        matches!(err, TxMempoolError::NonceTooHigh { .. }),
        "expected NonceTooHigh, got {err:?}"
    );
    assert_eq!(mp.pending_count(&from), 0, "nothing was pooled");
}

/// THE LIVENESS PROOF FOR A5. The horizon defers; it does not refuse.
///
/// A transaction rejected as too far ahead becomes admissible as soon as the
/// sender's expected nonce advances into range, with no change to the
/// transaction itself. Since inclusion at nonce n requires expected to reach
/// n, and expected is monotone, it must pass through n-15 first, so the
/// window is open for the entire stretch during which the transaction is one
/// of the sender's sixteen nearest-to-includable nonces. The horizon
/// therefore cannot cost an inclusion: it only refuses to buffer a
/// transaction earlier than the sender's own slot budget could have used it.
#[test]
fn horizon_defers_it_does_not_refuse() {
    let (sk, vk) = keypair(20);
    let from = address_from_pubkey(&vk);
    let e: u64 = 100;

    let mut mp = TxMempool::new(1, 1000);
    let mut np = Nonces::default();
    np.set(from, e);

    let far = e + MAX_PENDING_PER_SENDER as u64; // e+16
    let tx = signed(&sk, &vk, far, b"deferred");

    assert!(
        matches!(
            mp.insert(tx.clone(), &np),
            Err(TxMempoolError::NonceTooHigh { .. })
        ),
        "refused while out of range"
    );

    // One commit from this sender moves expected by one, and the very same
    // transaction is now inside the window.
    np.set(from, e + 1);
    mp.insert(tx, &np)
        .expect("the identical transaction is admitted once expected advances by one");
    assert_eq!(mp.pending_count(&from), 1);
}
