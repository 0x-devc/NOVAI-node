//! Gate LOAD: the reconciliation sweep must be able to tell converging from
//! thrashing.
//!
//! The live trace of 2026-08-28 reads as a sweep that does nothing:
//!
//! ```text
//! 20:43  reconciled sender 0  local=43022 chain=43004 drift=TooFarAhead
//! 20:46  reconciled sender 0  local=43022 chain=43004 drift=TooFarAhead
//!        reconciliation sweep corrected senders corrected=10 senders=10
//! ```
//!
//! Identical values three minutes apart, and a success line over the top of
//! them. The correction is in fact applied every time. What the log cannot
//! say is that it did not last: the sender burns a nonce per submission
//! because the claim is optimistic, nothing commits so the chain never moves,
//! and by the next pass it has climbed back to exactly where it was.
//!
//! `corrected` counts decisions to correct. A sweep that corrects the same
//! sender on every pass forever is reported identically to one that fixed a
//! pool and found it clean afterwards, and those are opposite conditions.

use tx_generator::sender::SenderPool;
use tx_generator::submitter::reconcile_sender_nonces;

use std::time::Duration;

/// Chain nonce pinned: this is a node that is accepting nothing from us, so
/// the sender's expected nonce never advances.
fn mock_pinned_nonce(server: &mut mockito::ServerGuard, nonce: u64) -> mockito::Mock {
    server
        .mock("POST", "/")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"nonce":{nonce}}}}}"#
        ))
        .expect_at_least(1)
        .create()
}

/// The disambiguation. Two readings can produce an unmoving log line: the
/// write never lands, or it lands and is immediately undone. This pins the
/// first one shut, so the failure can only be the second.
#[tokio::test(start_paused = true)]
async fn the_sweep_really_does_move_the_nonce() {
    let mut server = mockito::Server::new_async().await;
    let _m = mock_pinned_nonce(&mut server, 43004);

    let pool = SenderPool::new(1);
    let sender = pool.get_sender(0).unwrap();
    sender.reset_nonce(43022);

    let client = reqwest::Client::new();
    let corrected = reconcile_sender_nonces(&client, &server.url(), &pool, Duration::ZERO).await;

    assert_eq!(corrected, 1);
    assert_eq!(
        sender.current_nonce(),
        43004,
        "the correction is applied to the shared account, not to a copy"
    );
}

/// THE PIN THIS DEFECT IS ABOUT. Replay the live shape: correct the sender,
/// let it burn its way back to the same offset against a chain that never
/// moves, sweep again. The sweep must be able to say that this sender is not
/// converging.
///
/// Today every pass reports plain success, which is why sixteen hours of logs
/// showed a sweep working perfectly on a generator that was delivering
/// almost nothing.
#[tokio::test(start_paused = true)]
async fn a_sender_that_redrifts_every_pass_is_reported_as_thrashing() {
    let mut server = mockito::Server::new_async().await;
    let _m = mock_pinned_nonce(&mut server, 43004);

    let pool = SenderPool::new(1);
    let sender = pool.get_sender(0).unwrap();
    let client = reqwest::Client::new();

    for pass in 1..=3 {
        // Burn back up to the observed +18 lead: every submission claimed a
        // nonce and none of them committed.
        sender.reset_nonce(43004);
        for _ in 0..18 {
            sender.claim_nonce();
        }
        assert_eq!(sender.current_nonce(), 43022);

        let corrected =
            reconcile_sender_nonces(&client, &server.url(), &pool, Duration::ZERO).await;
        assert_eq!(corrected, 1, "pass {pass} corrects the same sender again");
        assert_eq!(sender.current_nonce(), 43004);
    }

    assert!(
        sender.consecutive_corrections() >= 3,
        "three passes correcting the same sender is thrashing, not convergence; \
         the sweep reported {} consecutive corrections",
        sender.consecutive_corrections()
    );
    assert!(
        sender.is_thrashing(),
        "a sender corrected on every consecutive pass must be flagged, not logged as success"
    );
}

/// The counter must not be a one way ratchet. A sender that gets corrected
/// once and then works is exactly what a healthy sweep looks like, and it
/// must not be indistinguishable from one that never recovers.
#[tokio::test(start_paused = true)]
async fn a_correction_that_sticks_clears_the_thrash_signal() {
    let mut server = mockito::Server::new_async().await;
    let _m = mock_pinned_nonce(&mut server, 43004);

    let pool = SenderPool::new(1);
    let sender = pool.get_sender(0).unwrap();
    sender.reset_nonce(43022);

    let client = reqwest::Client::new();
    reconcile_sender_nonces(&client, &server.url(), &pool, Duration::ZERO).await;
    assert_eq!(sender.consecutive_corrections(), 1);

    // The node accepts something from this sender: the correction held.
    sender.record_accepted();

    assert_eq!(
        sender.consecutive_corrections(),
        0,
        "an accepted submission is proof the correction stuck"
    );
    assert!(!sender.is_thrashing());
}
