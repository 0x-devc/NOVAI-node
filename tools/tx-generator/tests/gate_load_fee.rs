//! Gate LOAD: the generator must pay a fee the node will actually accept.
//!
//! The generator sent a hard coded fee against a floor the node moves at
//! runtime. `effective_min_fee = max(min_fee, dynamic_fee_floor)` and the
//! congestion responder raises the dynamic half under load, so a fixed fee is
//! correct only until the first congestion episode and wrong for every
//! submission after it. On 2026-08-28 that showed up as
//! `novai_mempool_rejects_fee_too_low` at 13,669 and a sender burning a nonce
//! on every one of them.
//!
//! The floor does not need to be exposed anywhere new. The node already
//! reports it in the rejection itself.

use tx_generator::fee::{parse_fee_floor, FeePolicy};

// ===========================================================================
// Learning the floor from the rejection.
// ===========================================================================

/// The node's own wording, from `crates/node/src/rpc.rs`:
/// `format!("FeeTooLow: minimum {min_fee}, got {got}")`, where `min_fee` is
/// `effective_min_fee()`. This is the whole discovery mechanism: the number
/// the generator needs is already in the refusal.
#[test]
fn the_floor_is_read_out_of_the_node_s_own_rejection() {
    assert_eq!(
        parse_fee_floor("FeeTooLow: minimum 5000, got 1000"),
        Some(5000)
    );
    assert_eq!(parse_fee_floor("FeeTooLow: minimum 1, got 0"), Some(1));
}

/// THE FORMAT THAT ACTUALLY ARRIVES. The node says `FeeTooLow: minimum N`,
/// but -32011 has no dedicated arm in `submit_with_retry`, so it falls to the
/// generic RPC arm and `SubmitError::Rpc` prepends its own wrapper before the
/// reason ever reaches the policy.
///
/// A parser anchored to the front of the string passes every test written
/// from the node's wording and learns nothing at all in production. This
/// pins the string the generator really sees.
#[test]
fn the_floor_is_read_out_of_the_wrapped_reason_the_generator_actually_sees() {
    let wire = "RPC error -32011: FeeTooLow: minimum 5000, got 1000";
    assert_eq!(
        parse_fee_floor(wire),
        Some(5000),
        "this is the exact string submit_with_retry produces for a fee refusal"
    );

    let policy = FeePolicy::new(1000);
    policy.observe_rejection(wire);
    assert!(
        policy.current() > 5000,
        "the wrapped form must teach the policy exactly as the bare form does"
    );
}

/// Anything that is not a fee refusal must yield nothing. A parser that
/// guessed here would move the fee on evidence about something else.
#[test]
fn a_rejection_about_anything_else_teaches_nothing_about_the_fee() {
    for reason in [
        "NonceTooLow: expected 43004, got 43002",
        "NonceTooHigh: expected 43004, got 43030, horizon 43020",
        "SenderLimitExceeded: max 16 pending per sender",
        "MempoolFull",
        "Transaction validation failed",
        "",
    ] {
        assert_eq!(
            parse_fee_floor(reason),
            None,
            "{reason:?} says nothing about the fee"
        );
    }
}

// ===========================================================================
// Paying above the floor.
// ===========================================================================

/// THE PIN THIS DEFECT IS ABOUT. After one refusal the generator must offer a
/// fee the node accepts. Offering the same fee again is the bug: it burns a
/// nonce per attempt and never converges.
#[test]
fn a_refusal_raises_the_offered_fee_above_the_floor() {
    let policy = FeePolicy::new(1000);
    assert_eq!(policy.current(), 1000);

    policy.observe_rejection("FeeTooLow: minimum 5000, got 1000");

    assert!(
        policy.current() > 5000,
        "after learning a floor of 5000 the generator must offer strictly more, got {}",
        policy.current()
    );
}

/// The floor moves up repeatedly during a congestion ramp. Each refusal must
/// take the offer above the newest floor, not oscillate around a stale one.
#[test]
fn a_rising_floor_is_tracked_upward() {
    let policy = FeePolicy::new(1000);
    let mut offered = Vec::new();

    for floor in [2000u64, 4000, 8000, 16000] {
        policy.observe_rejection(&format!("FeeTooLow: minimum {floor}, got 1"));
        assert!(
            policy.current() > floor,
            "offer {} does not clear floor {floor}",
            policy.current()
        );
        offered.push(policy.current());
    }

    assert!(
        offered.windows(2).all(|w| w[1] > w[0]),
        "the offer must rise with the floor, got {offered:?}"
    );
}

/// A refusal that is not about the fee must not move the fee. Otherwise a
/// burst of nonce errors quietly inflates what every later transaction pays.
#[test]
fn an_unrelated_refusal_does_not_move_the_fee() {
    let policy = FeePolicy::new(1000);
    policy.observe_rejection("NonceTooLow: expected 43004, got 43002");
    assert_eq!(policy.current(), 1000);
}

/// It must not ratchet forever. The floor decays on the node once congestion
/// clears, and a generator that never comes back down overpays for the rest
/// of the run and stops measuring what it set out to measure.
#[test]
fn a_quiet_period_brings_the_fee_back_down() {
    let policy = FeePolicy::new(1000);
    policy.observe_rejection("FeeTooLow: minimum 16000, got 1000");
    let raised = policy.current();
    assert!(raised > 16000);

    for _ in 0..10_000 {
        policy.observe_accepted();
    }

    assert!(
        policy.current() < raised,
        "a long clean run must relax the fee, still at {}",
        policy.current()
    );
    assert!(
        policy.current() >= 1000,
        "it must never fall below the configured base, got {}",
        policy.current()
    );
}
