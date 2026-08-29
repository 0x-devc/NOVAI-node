//! Gate LOAD: senders must not all fire at the same rate.
//!
//! Identical senders fail in a specific way. They reach the per-sender
//! ceiling on the same block, so the fleet does not degrade under rising
//! load, it tips over all at once, and the shape of the failure tells you
//! nothing about where the real ceiling is.
//!
//! The property under test that matters most is the one that is NOT about
//! heterogeneity: the aggregate must still be exactly `--tps`. This whole
//! session exists because the generator delivered 0.43 TPS when asked for 3.
//! Adding realism at the cost of the dial would be the same defect again in
//! nicer clothes.

use std::collections::HashMap;
use tx_generator::rates::{
    draw_weights, RateDistribution, WeightedSelector, MAX_WEIGHT, MIN_WEIGHT,
};
use tx_generator::sender::SenderPool;

const SENDERS: usize = 100;
const SEED: u64 = 0;

fn selector(dist: RateDistribution, seed: u64) -> WeightedSelector {
    let w = draw_weights(SENDERS, dist, seed);
    WeightedSelector::new(&w, seed).expect("weights must be usable")
}

// ===========================================================================
// The dial still means what it says.
// ===========================================================================

/// THE PIN THAT PROTECTS THE INSTRUMENT. Heterogeneity changes which sender
/// gets a tick and never how many ticks exist, so the shares are a partition:
/// they sum to one, and every tick goes to exactly one real sender.
#[test]
fn the_shares_partition_the_offered_rate_exactly() {
    for dist in [
        RateDistribution::Uniform,
        RateDistribution::Spread,
        RateDistribution::Lognormal,
    ] {
        let sel = selector(dist, SEED);
        let total: f64 = (0..SENDERS).map(|i| sel.share(i)).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "{dist:?}: shares must sum to exactly the offered rate, got {total}"
        );

        for tick in 0..10_000u64 {
            assert!(
                sel.index_for_tick(tick) < SENDERS,
                "{dist:?}: every tick must land on a real sender"
            );
        }
    }
}

// ===========================================================================
// The load is actually heterogeneous.
// ===========================================================================

/// The default must not quietly be the old behaviour. If the drawn shares are
/// flat then nothing changed and the fleet still tips over together.
#[test]
fn the_default_distribution_is_not_flat() {
    let w = draw_weights(SENDERS, RateDistribution::Lognormal, SEED);
    let min = w.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = w.iter().cloned().fold(0.0, f64::max);
    assert!(
        max / min > 4.0,
        "the fastest sender must be meaningfully faster than the slowest, \
         got a spread of only {:.2}x (min {min}, max {max})",
        max / min
    );
}

/// Uniform is kept deliberately so a run can be compared against the
/// homogeneous baseline. It must really be flat.
#[test]
fn uniform_really_is_the_old_homogeneous_behaviour() {
    let w = draw_weights(SENDERS, RateDistribution::Uniform, SEED);
    assert!(w.iter().all(|x| (*x - w[0]).abs() < f64::EPSILON));

    let sel = selector(RateDistribution::Uniform, SEED);
    for i in 0..SENDERS {
        assert!((sel.share(i) - 1.0 / SENDERS as f64).abs() < 1e-9);
    }
}

/// No sender may be starved. A sender drawing a share of nothing is not a
/// slow agent, it is a dead one: it still holds a funded account and is still
/// swept for nonce drift every pass, and it contributes no load at all.
#[test]
fn no_sender_is_starved() {
    for dist in [
        RateDistribution::Uniform,
        RateDistribution::Spread,
        RateDistribution::Lognormal,
    ] {
        for seed in [0u64, 1, 7, 99, 12345] {
            let w = draw_weights(SENDERS, dist, seed);
            assert_eq!(w.len(), SENDERS);
            for (i, x) in w.iter().enumerate() {
                assert!(
                    *x >= MIN_WEIGHT && *x <= MAX_WEIGHT,
                    "{dist:?} seed {seed}: sender {i} drew {x}, outside the bounds"
                );
            }

            // And every one of them actually gets picked in a realistic run.
            let sel = WeightedSelector::new(&w, seed).unwrap();
            let mut seen = [false; SENDERS];
            for tick in 0..200_000u64 {
                seen[sel.index_for_tick(tick)] = true;
            }
            let starved: Vec<usize> = (0..SENDERS).filter(|i| !seen[*i]).collect();
            assert!(
                starved.is_empty(),
                "{dist:?} seed {seed}: senders {starved:?} never fired"
            );
        }
    }
}

/// Long run shares must converge on the drawn weights, or the distribution
/// flag is decoration rather than a control.
#[test]
fn the_realised_shares_track_the_drawn_shares() {
    const TICKS: u64 = 400_000;
    let sel = selector(RateDistribution::Lognormal, SEED);

    let mut counts = vec![0u64; SENDERS];
    for tick in 0..TICKS {
        counts[sel.index_for_tick(tick)] += 1;
    }

    for (i, count) in counts.iter().enumerate() {
        let expected = sel.share(i);
        let realised = *count as f64 / TICKS as f64;
        assert!(
            (realised - expected).abs() < expected * 0.15 + 0.0005,
            "sender {i}: expected share {expected:.5}, realised {realised:.5}"
        );
    }
}

// ===========================================================================
// The run is reproducible.
// ===========================================================================

/// A measurement that cannot be repeated is not a measurement. The same seed
/// must draw the same population AND deal the same schedule.
#[test]
fn the_same_seed_places_the_same_load() {
    let a = draw_weights(SENDERS, RateDistribution::Lognormal, 42);
    let b = draw_weights(SENDERS, RateDistribution::Lognormal, 42);
    assert_eq!(a, b, "the same seed must draw the same population");

    let sa = WeightedSelector::new(&a, 42).unwrap();
    let sb = WeightedSelector::new(&b, 42).unwrap();
    let deal_a: Vec<usize> = (0..5_000).map(|t| sa.index_for_tick(t)).collect();
    let deal_b: Vec<usize> = (0..5_000).map(|t| sb.index_for_tick(t)).collect();
    assert_eq!(deal_a, deal_b, "the same seed must deal the same schedule");
}

/// Different seeds must give a different population, or the seed flag does
/// nothing and every run is the same run.
#[test]
fn a_different_seed_draws_a_different_population() {
    let a = draw_weights(SENDERS, RateDistribution::Lognormal, 1);
    let b = draw_weights(SENDERS, RateDistribution::Lognormal, 2);
    assert_ne!(a, b);
}

// ===========================================================================
// The pool uses it.
// ===========================================================================

/// A distribution nothing consults is dead code. The pool must actually deal
/// its ticks unevenly once one is attached, and evenly when it is not.
#[test]
fn the_pool_deals_ticks_by_share() {
    let homogeneous = SenderPool::new(10);
    let mut flat: HashMap<usize, u64> = HashMap::new();
    for _ in 0..10_000 {
        *flat.entry(homogeneous.next_sender().index).or_default() += 1;
    }
    assert!(
        flat.values().all(|c| *c == 1_000),
        "with no distribution attached the pool must stay strict round robin"
    );

    let weighted = SenderPool::new(10).with_rate_distribution(RateDistribution::Lognormal, SEED);
    let mut counts: HashMap<usize, u64> = HashMap::new();
    for _ in 0..100_000 {
        *counts.entry(weighted.next_sender().index).or_default() += 1;
    }
    assert_eq!(counts.len(), 10, "every sender must still be used");

    let min = *counts.values().min().unwrap();
    let max = *counts.values().max().unwrap();
    assert!(
        max as f64 / min as f64 > 2.0,
        "the pool must deal unevenly once a distribution is attached, \
         got min {min} max {max}"
    );

    for i in 0..10 {
        let realised = counts[&i] as f64 / 100_000.0;
        let expected = weighted.share_of(i);
        assert!(
            (realised - expected).abs() < expected * 0.2 + 0.002,
            "sender {i}: pool dealt {realised:.4}, share is {expected:.4}"
        );
    }
}
