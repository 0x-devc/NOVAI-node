//! Gate SOAK phase 4 (B3 reconciliation, B4 shared failure counter,
//! B5 shared resync cooldown).
//!
//! Phase 1 stopped the generator writing a nonce it had not read. That made
//! the reactive path safe, but it left two holes:
//!
//! - The reactive path only fires on a run of rejections, and the dangerous
//!   desync direction (running AHEAD of the chain) produced no rejection at
//!   all until the A5 horizon landed. A sender could sit wrong indefinitely.
//! - The streak counter and cooldown lived in a per-worker map, so with four
//!   workers a sender needed roughly four times as many rejections before
//!   anyone reacted, and all four could then fire their own resync at once,
//!   against an endpoint that was usually already struggling.
//!
//! Phase 4 closes both: a periodic sweep that converges a drifted sender to
//! chain truth without touching a healthy one, and per-sender health that
//! lives on the account so every worker shares one view.

use tx_generator::sender::{SenderAccount, SenderPool};
use tx_generator::submitter::{classify_drift, reconcile_sender_nonces, Drift};

use mempool::MAX_PENDING_PER_SENDER;
use std::sync::Arc;
use std::time::Duration;

const CAP: u64 = MAX_PENDING_PER_SENDER as u64;

// ===========================================================================
// B3: the reconciliation predicate.
// ===========================================================================

/// A sender below chain truth is grinding: every submission is refused and
/// burns another nonce. It must be corrected up.
#[test]
fn a_sender_behind_the_chain_is_corrected() {
    assert_eq!(classify_drift(0, 300), Drift::Behind);
    assert_eq!(classify_drift(299, 300), Drift::Behind);
}

/// THE PIN THAT STOPS THE SWEEP DOING HARM. A working sender's local nonce
/// legitimately leads the chain by its in-flight depth, up to the node's
/// per-sender slot cap. The sweep must leave that completely alone. A sweep
/// that "corrected" a healthy lead would rewind past live submissions every
/// time it ran, turning a working generator into a broken one.
#[test]
fn a_healthy_in_flight_lead_is_never_touched() {
    assert_eq!(classify_drift(300, 300), Drift::Healthy, "no lead");
    assert_eq!(classify_drift(301, 300), Drift::Healthy, "one in flight");
    assert_eq!(
        classify_drift(300 + CAP, 300),
        Drift::Healthy,
        "a full slot cap of in-flight transactions is the maximum healthy lead"
    );
}

/// One past the cap is a lead the sender could not possibly be holding, so
/// those nonces were burned on rejections and can never commit.
#[test]
fn a_lead_beyond_the_slot_cap_is_drift() {
    assert_eq!(classify_drift(300 + CAP + 1, 300), Drift::TooFarAhead);
    assert_eq!(classify_drift(300 + 5_000, 300), Drift::TooFarAhead);
}

// ===========================================================================
// B3: the sweep converges, and stays converged.
// ===========================================================================

fn mock_nonce_endpoint(server: &mut mockito::ServerGuard, nonce: u64) -> mockito::Mock {
    server
        .mock("POST", "/")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"jsonrpc":"2.0","result":{{"nonce":{nonce}}},"id":1}}"#
        ))
        .expect_at_least(1)
        .create()
}

/// A sender that has drifted in either direction converges to chain truth in
/// one sweep, and a second sweep is a no-op: it does not oscillate, and it
/// does not keep "correcting" a sender that is already right.
#[tokio::test(start_paused = true)]
async fn the_sweep_converges_and_then_leaves_the_sender_alone() {
    let mut server = mockito::Server::new_async().await;
    let _m = mock_nonce_endpoint(&mut server, 300);

    let pool = SenderPool::new(2);
    // Sender 0 is far behind, sender 1 is far ahead. Both are wrong.
    pool.get_sender(0).unwrap().reset_nonce(0);
    pool.get_sender(1).unwrap().reset_nonce(300 + CAP + 99);

    let client = reqwest::Client::new();
    let corrected = reconcile_sender_nonces(&client, &server.url(), &pool, Duration::ZERO).await;
    assert_eq!(corrected, 2, "both drifted senders must be corrected");

    assert_eq!(pool.get_sender(0).unwrap().current_nonce(), 300);
    assert_eq!(pool.get_sender(1).unwrap().current_nonce(), 300);

    // Converged: a second pass finds nothing to do.
    let again = reconcile_sender_nonces(&client, &server.url(), &pool, Duration::ZERO).await;
    assert_eq!(
        again, 0,
        "a converged pool must not be corrected again; the sweep is idempotent"
    );
    assert_eq!(pool.get_sender(0).unwrap().current_nonce(), 300);
    assert_eq!(pool.get_sender(1).unwrap().current_nonce(), 300);
}

/// The sweep never rewinds a sender that is merely mid-flight. This is the
/// end-to-end version of `a_healthy_in_flight_lead_is_never_touched`.
#[tokio::test(start_paused = true)]
async fn the_sweep_does_not_disturb_a_working_sender() {
    let mut server = mockito::Server::new_async().await;
    let _m = mock_nonce_endpoint(&mut server, 300);

    let pool = SenderPool::new(1);
    let sender = pool.get_sender(0).unwrap();
    sender.reset_nonce(300 + CAP); // fully saturated, every slot in flight

    let client = reqwest::Client::new();
    let corrected =
        reconcile_sender_nonces(&reqwest::Client::clone(&client), &server.url(), &pool, Duration::ZERO)
            .await;

    assert_eq!(corrected, 0, "a saturated but healthy sender is left alone");
    assert_eq!(
        sender.current_nonce(),
        300 + CAP,
        "the sweep must not rewind a sender that simply has transactions in flight"
    );
}

/// A failed query must not move anything, exactly as in the reactive path.
/// The R1 rule (never write a nonce we did not read) holds for the sweep too.
#[tokio::test(start_paused = true)]
async fn the_sweep_never_writes_a_nonce_it_did_not_read() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("POST", "/")
        .with_status(500)
        .with_body("nope")
        .expect_at_least(1)
        .create_async()
        .await;

    let pool = SenderPool::new(1);
    pool.get_sender(0).unwrap().reset_nonce(4242);

    let client = reqwest::Client::new();
    let corrected = reconcile_sender_nonces(&client, &server.url(), &pool, Duration::ZERO).await;

    assert_eq!(corrected, 0);
    assert_eq!(
        pool.get_sender(0).unwrap().current_nonce(),
        4242,
        "an unreachable endpoint must leave every local nonce untouched"
    );
}

// ===========================================================================
// B4: the failure counter is shared across workers.
// ===========================================================================

/// Every worker must count into the same streak. With a per-worker map, four
/// workers each reach the threshold a quarter as fast, so a sender stays
/// broken roughly four times longer than the threshold implies.
#[test]
fn the_nonce_error_streak_is_shared_across_workers() {
    let account = Arc::new(SenderAccount::from_index(0));

    // Four "workers" each observe one nonce rejection for this sender.
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let a = Arc::clone(&account);
            std::thread::spawn(move || a.record_nonce_error())
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }

    assert_eq!(
        account.nonce_error_streak(),
        4,
        "four workers observing one rejection each must add up to four, not one each"
    );

    account.record_accepted();
    assert_eq!(
        account.nonce_error_streak(),
        0,
        "an accepted submission proves the nonce is right and clears the streak"
    );
}

/// An unrelated rejection says nothing about the nonce and must not wipe the
/// evidence. Clearing here is what let an interleaved fee-floor or duplicate
/// rejection push recovery out indefinitely.
#[test]
fn an_unrelated_rejection_does_not_clear_the_streak() {
    let account = SenderAccount::from_index(1);
    account.record_nonce_error();
    account.record_nonce_error();
    account.record_unrelated_rejection();
    assert_eq!(
        account.nonce_error_streak(),
        2,
        "a fee-floor or duplicate rejection must not erase a nonce-error streak"
    );
}

// ===========================================================================
// B5: the cooldown prevents a resync storm.
// ===========================================================================

/// THE THUNDERING-HERD PIN. Many workers can notice the same sick sender in
/// the same instant. Exactly one of them may query the chain. The resync
/// fires precisely when the endpoint is already struggling, so N concurrent
/// queries per sender is the worst possible response.
#[test]
fn only_one_worker_may_resync_a_sender_per_cooldown() {
    let account = Arc::new(SenderAccount::from_index(2));
    let cooldown = Duration::from_secs(5);

    let winners: Vec<bool> = {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let a = Arc::clone(&account);
                std::thread::spawn(move || a.try_begin_resync(cooldown))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    };

    assert_eq!(
        winners.iter().filter(|w| **w).count(),
        1,
        "exactly one of eight concurrent workers may begin a resync; the rest \
         must back off rather than pile onto a degraded endpoint"
    );
}

/// The cooldown expires, so a sender that is still sick gets another attempt.
/// A cooldown that never reopened would be a permanent lockout.
#[test]
fn the_cooldown_reopens_after_it_elapses() {
    let account = SenderAccount::from_index(3);
    let tiny = Duration::from_millis(20);

    assert!(account.try_begin_resync(tiny), "first attempt is granted");
    assert!(
        !account.try_begin_resync(tiny),
        "an immediate second attempt is refused"
    );

    std::thread::sleep(Duration::from_millis(40));
    assert!(
        account.try_begin_resync(tiny),
        "once the cooldown elapses the sender may be retried; it must not be \
         locked out permanently"
    );
}
