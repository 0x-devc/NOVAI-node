//! Gate SOAK phase 5 (B6 adaptive throttle, B7 stall monitor honesty).

use tx_generator::submitter::poll_is_progress;
use tx_generator::throttle::{
    next_level, Throttle, LOOSEN_BELOW, MAX_LEVEL, MAX_SPARSE_WINDOWS, MIN_SAMPLES, TIGHTEN_ABOVE,
};

/// Build a window with the given rejection ratio and enough samples to count.
fn window(ratio: f64) -> (u64, u64) {
    let total = MIN_SAMPLES * 10;
    let rejected = (total as f64 * ratio).round() as u64;
    (total - rejected, rejected)
}

// ===========================================================================
// B6: it cannot oscillate.
// ===========================================================================

/// THE ANTI-OSCILLATION PIN. Any ratio inside the dead band leaves the level
/// exactly where it is, from every level. A single-threshold controller would
/// step on one side of the threshold and step back on the other, flapping the
/// offered rate forever on a steady input.
#[test]
fn a_ratio_inside_the_dead_band_never_moves_the_level() {
    let mid = (LOOSEN_BELOW + TIGHTEN_ABOVE) / 2.0;
    for level in 0..=MAX_LEVEL {
        for ratio in [LOOSEN_BELOW, mid, TIGHTEN_ABOVE] {
            let (a, r) = window(ratio);
            assert_eq!(
                next_level(level, a, r),
                level,
                "ratio {ratio} at level {level} is inside the dead band and must hold"
            );
        }
    }
}

/// A signal that alternates across the middle of the band is exactly what
/// makes a naive controller flap. Here it must produce a flat line.
#[test]
fn an_alternating_in_band_signal_does_not_flap() {
    let mut level = 3;
    let seen: Vec<u32> = (0..40)
        .map(|i| {
            let ratio = if i % 2 == 0 {
                LOOSEN_BELOW + 0.01
            } else {
                TIGHTEN_ABOVE - 0.01
            };
            let (a, r) = window(ratio);
            level = next_level(level, a, r);
            level
        })
        .collect();

    assert!(
        seen.iter().all(|l| *l == 3),
        "an in-band signal must produce a constant level, got {seen:?}"
    );
}

/// Too few outcomes to judge: hold. Without this, one rejection in a quiet
/// window reads as a 100 percent rejection rate and slams the throttle on.
#[test]
fn a_sparse_window_never_moves_the_level() {
    assert_eq!(next_level(2, 0, MIN_SAMPLES - 1), 2);
    assert_eq!(next_level(2, 0, 0), 2);
    assert_eq!(next_level(0, 1, 1), 0);
}

// ===========================================================================
// B6: it cannot starve.
// ===========================================================================

/// Sustained refusal tightens, then STOPS. An unbounded throttle would take
/// the generator to zero, which is indistinguishable from the generator
/// having died.
#[test]
fn sustained_rejection_tightens_to_the_cap_and_stops() {
    let (a, r) = window(0.95);
    let mut level = 0;
    for _ in 0..50 {
        level = next_level(level, a, r);
    }
    assert_eq!(level, MAX_LEVEL, "the throttle must saturate, not run away");

    let t = Throttle::new();
    for _ in 0..MAX_LEVEL {
        for _ in 0..(MIN_SAMPLES * 2) {
            t.record_rejected();
        }
        t.sample();
    }
    assert_eq!(t.level(), MAX_LEVEL);
    assert_eq!(
        t.delay_multiplier(),
        1 << MAX_LEVEL,
        "the slowest the generator can run is a bounded multiple of its interval, \
         never a stop"
    );
}

/// THE ANTI-STARVATION PIN. Once the node recovers, the throttle must return
/// all the way to full speed and stay there. A controller that ratchets one
/// way would leave a soak permanently crippled after a single bad minute.
#[test]
fn recovery_returns_to_full_speed_and_stays_there() {
    let t = Throttle::new();

    // Drive it to the cap.
    for _ in 0..(MAX_LEVEL + 2) {
        for _ in 0..(MIN_SAMPLES * 2) {
            t.record_rejected();
        }
        t.sample();
    }
    assert_eq!(t.level(), MAX_LEVEL);

    // The node recovers: clean windows from here on.
    for _ in 0..(MAX_LEVEL + 5) {
        for _ in 0..(MIN_SAMPLES * 2) {
            t.record_accepted();
        }
        t.sample();
    }
    assert_eq!(t.level(), 0, "the throttle must fully release");
    assert_eq!(
        t.delay_multiplier(),
        1,
        "full speed means the configured interval, unmultiplied"
    );

    // And it does not underflow past zero.
    for _ in 0..10 {
        for _ in 0..(MIN_SAMPLES * 2) {
            t.record_accepted();
        }
        t.sample();
    }
    assert_eq!(t.level(), 0);
}

/// Sampling starts a fresh window, so one bad minute cannot keep influencing
/// the level after it has passed.
#[test]
fn sampling_resets_the_window() {
    let t = Throttle::new();
    for _ in 0..(MIN_SAMPLES * 2) {
        t.record_rejected();
    }
    assert_eq!(t.sample(), 1, "one bad window, one step");
    // Next window is sparse because the counters were cleared, so it holds
    // rather than tightening again on stale evidence.
    assert_eq!(t.sample(), 1);
}

// ===========================================================================
// B7: the stall monitor stops calling non-progress progress.
// ===========================================================================

/// A response carrying no height is not progress. Treating it as progress is
/// how the monitor stayed silent through an endpoint that answered but
/// reported nothing.
#[test]
fn a_response_without_a_height_is_not_progress() {
    assert!(
        !poll_is_progress(&Ok(None), Some(100)),
        "an empty height response must not reset the stall clock"
    );
    assert!(!poll_is_progress(&Ok(None), None));
}

/// An error is not progress either. This already held, and must keep holding.
#[test]
fn an_rpc_error_is_not_progress() {
    assert!(!poll_is_progress(
        &Err("connection refused".into()),
        Some(100)
    ));
}

/// A genuinely higher height is progress; the same or a lower one is not.
#[test]
fn only_a_rising_height_counts_as_progress() {
    assert!(poll_is_progress(&Ok(Some(101)), Some(100)));
    assert!(poll_is_progress(&Ok(Some(1)), None), "first observation");
    assert!(!poll_is_progress(&Ok(Some(100)), Some(100)), "frozen");
    assert!(
        !poll_is_progress(&Ok(Some(99)), Some(100)),
        "went backwards"
    );
}

// ===========================================================================
// The closed loop. The throttle's own output feeds its next input.
//
// Every test above hands the controller a window whose size is chosen by the
// test. In production nobody chooses it: the window holds whatever the
// generator managed to send during it, and the throttle is what decides how
// much that is. Once the level rises, the window shrinks, and a window below
// MIN_SAMPLES can no longer move the level. That is a latch, and no open-loop
// test can see it.
// ===========================================================================

/// The generator's offered rate under a throttle level, as outcomes per
/// window. This is the loop that the controller sits inside:
/// `generator.rs` waits `interval * multiplier` between sends, so the sample
/// count a window can possibly hold is the full-speed count divided by the
/// multiplier the controller itself chose.
fn outcomes_per_window(target_tps: u64, window_secs: u64, level: u32) -> u64 {
    (target_tps * window_secs) / (1u64 << level)
}

/// THE LATCH PIN. A healthy node, a generator asked for 3 TPS, the default
/// 10 second window: from level 3 the throttle must walk back down to 0.
///
/// This is the live failure of 2026-08-28. The throttle reached level 3 at
/// 04:13:31 and never moved again for the following 16 hours, turning a
/// request for 3 TPS into 0.375 TPS delivered. Not one submission was
/// rejected in that window. The node was fine. The controller had simply
/// slowed the generator below the sample count it needs to decide to speed
/// back up, so the condition for recovery was one only a faster generator
/// could meet, and the throttle was the reason it was not faster.
#[test]
fn a_healthy_node_walks_the_level_back_down_from_the_top() {
    const TPS: u64 = 3;
    const WINDOW_SECS: u64 = 10;

    let t = Throttle::new();
    for _ in 0..3 {
        for _ in 0..(MIN_SAMPLES * 2) {
            t.record_rejected();
        }
        t.sample();
    }
    assert_eq!(t.level(), 3, "precondition: the throttle is at level 3");

    // The node is healthy from here on. Every window is clean. The only
    // thing that varies is how much the throttle lets the generator send.
    for window in 0..60 {
        let sendable = outcomes_per_window(TPS, WINDOW_SECS, t.level());
        for _ in 0..sendable {
            t.record_accepted();
        }
        t.sample();
        if t.level() == 0 {
            break;
        }
        assert!(
            window < 59,
            "still at level {} after 60 clean windows: the throttle has latched",
            t.level()
        );
    }

    assert_eq!(
        t.level(),
        0,
        "a healthy node must return the generator to full speed"
    );
}

/// No level may be a trap. From every level, a healthy node returns the
/// generator to full speed in bounded time, and the only traffic the
/// controller gets to see is the traffic it is itself allowing.
///
/// The bound matters as much as the outcome. "It recovers eventually" is not
/// a property a soak can rely on; sixteen hours was also eventually.
#[test]
fn no_level_is_a_trap_at_any_offered_rate() {
    const WINDOW_SECS: u64 = 10;
    const PATIENCE: usize = 40;

    for tps in [1u64, 3, 10, 100] {
        for start in 1..=MAX_LEVEL {
            let t = Throttle::new();
            while t.level() < start {
                for _ in 0..(MIN_SAMPLES * 2) {
                    t.record_rejected();
                }
                t.sample();
            }
            assert_eq!(t.level(), start);

            let mut windows = 0;
            while t.level() > 0 {
                let sendable = outcomes_per_window(tps, WINDOW_SECS, t.level());
                for _ in 0..sendable {
                    t.record_accepted();
                }
                t.sample();
                windows += 1;
                assert!(
                    windows <= PATIENCE,
                    "tps {tps} from level {start}: still at level {} after \
                     {PATIENCE} clean windows",
                    t.level()
                );
            }
        }
    }
}

/// The anti-latch escape must not become a way to loosen while the node is
/// actually refusing traffic. A thin window with refusals in it holds.
#[test]
fn a_thin_window_with_refusals_in_it_does_not_loosen() {
    let t = Throttle::new();
    for _ in 0..(MIN_SAMPLES * 2) {
        t.record_rejected();
    }
    assert_eq!(t.sample(), 1);

    // Thin windows, but every one of them carries a refusal.
    for _ in 0..(MAX_SPARSE_WINDOWS * 3) {
        t.record_rejected();
        assert_eq!(
            t.sample(),
            1,
            "a window that refused something is not evidence of a healthy node"
        );
    }
}

/// A node that refuses a little is the normal case, and it is the case the
/// escape alone cannot handle. The escape only fires after a run of windows
/// with nothing refused in them; at one eighth speed a window holds about
/// four outcomes, so a steady trickle of refusals lands in enough windows to
/// break every run before it completes, and recovery becomes a coin flip on
/// how the refusals happen to fall.
///
/// Carrying thin windows forward is what makes this deterministic. Twenty
/// outcomes gathered across five windows are judged on their real ratio,
/// which here is well inside the loosen band, so the level comes down on
/// evidence rather than on luck.
#[test]
fn a_node_refusing_below_the_loosen_band_still_recovers_promptly() {
    const TPS: u64 = 3;
    const WINDOW_SECS: u64 = 10;
    // One in ten refused: a real ratio of 0.10, comfortably below
    // LOOSEN_BELOW, so the correct answer is to speed back up.
    const REFUSE_ONE_IN: u64 = 10;

    let t = Throttle::new();
    for _ in 0..3 {
        for _ in 0..(MIN_SAMPLES * 2) {
            t.record_rejected();
        }
        t.sample();
    }
    assert_eq!(t.level(), 3);

    let mut n = 0u64;
    let mut windows = 0;
    while t.level() > 0 {
        for _ in 0..outcomes_per_window(TPS, WINDOW_SECS, t.level()) {
            n += 1;
            if n.is_multiple_of(REFUSE_ONE_IN) {
                t.record_rejected();
            } else {
                t.record_accepted();
            }
        }
        t.sample();
        windows += 1;
        assert!(
            windows <= 30,
            "a 10 percent refusal rate is inside the loosen band, but the \
             throttle is still at level {} after {windows} windows",
            t.level()
        );
    }
}
