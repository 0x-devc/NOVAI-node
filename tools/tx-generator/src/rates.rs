//! Per-sender rate heterogeneity.
//!
//! Every sender used to fire at exactly the same rate. The generator runs one
//! ticker at `1/target_tps` and hands each tick to the next sender in strict
//! round robin, so with N senders each one gets precisely `target_tps / N`.
//! That is not what the chain will meet in production and it fails in a
//! specific way: identical senders reach the per-sender ceiling on the same
//! block, so the fleet does not degrade, it tips over all at once.
//!
//! Real load is many independent agents at different rates. Some are chatty,
//! most are not, a few are very chatty.
//!
//! WHAT MUST NOT CHANGE: the aggregate. `--tps` is the dial on the
//! instrument, and this whole session exists because the generator was
//! delivering 0.43 TPS when asked for 3. Heterogeneity that also made the
//! aggregate approximate would be trading one broken dial for another.
//!
//! So the tick rate is left exactly as it is and only the CHOICE OF SENDER
//! changes. One global ticker still fires at `target_tps` and still hands out
//! exactly one transaction per tick; it just no longer hands them out in
//! equal shares. The aggregate is preserved by construction rather than by
//! tuning, which is the only version of this worth having.
//!
//! Selection is seeded and lock free, so a run is reproducible from its seed
//! and two runs with the same seed place the same load. An instrument whose
//! readings cannot be reproduced is not an instrument.

use std::sync::atomic::{AtomicU64, Ordering};

/// Narrowest and widest share any sender may draw, relative to an equal
/// share. Bounded on both ends deliberately.
///
/// The floor is the important one. An unbounded heavy tail (a true Zipf or
/// Pareto) eventually draws a weight indistinguishable from zero, and a
/// sender that never fires is not a slow agent, it is a dead one: it holds a
/// funded account, it is swept for nonce drift every pass, and it contributes
/// nothing to the measurement. The ceiling keeps one sender from swallowing
/// the run and hitting its own per-sender ceiling while the rest idle.
pub const MIN_WEIGHT: f64 = 0.125;
pub const MAX_WEIGHT: f64 = 8.0;

/// How per-sender shares are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateDistribution {
    /// Every sender identical. The old behaviour, kept so a run can be
    /// compared against the homogeneous baseline it replaces.
    Uniform,
    /// Shares spread evenly between the bounds. Heterogeneous but with no
    /// realistic shape: useful for isolating whether an effect is caused by
    /// spread itself or by the tail.
    Spread,
    /// THE DEFAULT. Most senders near the middle, a thin tail of chatty ones.
    ///
    /// Chosen over a power law because a power law's realism is all in a tail
    /// this cannot use: the interesting part of a Zipf population is the
    /// thousands of near-silent members, and the generator runs hundreds of
    /// senders, not thousands. Truncating a power law hard enough to keep
    /// every sender alive removes the property it was chosen for. Lognormal
    /// is right-skewed, strictly positive, and stays that shape after
    /// clamping, so what the flag promises is what the run does.
    Lognormal,
}

impl RateDistribution {
    /// Parse the `--sender-rate-distribution` flag.
    ///
    /// Deliberately not `FromStr`: an unknown value must stop the run rather
    /// than fall back to a default, because silently loading the wrong shape
    /// is exactly the class of failure this gate is about.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "uniform" => Some(Self::Uniform),
            "spread" => Some(Self::Spread),
            "lognormal" => Some(Self::Lognormal),
            _ => None,
        }
    }
}

/// splitmix64. Small, seedable, and good enough to draw a few hundred
/// weights; written out rather than pulled in so the generator keeps its
/// dependency list and, more importantly, so the exact sequence is pinned
/// here and cannot change under a version bump.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform in [0, 1).
fn unit(state: &mut u64) -> f64 {
    (splitmix64(state) >> 11) as f64 / (1u64 << 53) as f64
}

/// Draw one share per sender.
///
/// Always returns exactly `count` strictly positive weights, whatever the
/// distribution. Callers rely on both: a zero would starve a sender and a
/// short vector would panic the selector.
pub fn draw_weights(count: usize, dist: RateDistribution, seed: u64) -> Vec<f64> {
    if count == 0 {
        return Vec::new();
    }
    let mut state = seed;
    (0..count)
        .map(|_| {
            let w = match dist {
                RateDistribution::Uniform => 1.0,
                RateDistribution::Spread => {
                    MIN_WEIGHT + unit(&mut state) * (MAX_WEIGHT - MIN_WEIGHT)
                }
                RateDistribution::Lognormal => {
                    // Box-Muller for the normal, then exponentiate. sigma is
                    // 0.75: wide enough that the fast senders are several
                    // times the slow ones, narrow enough that the clamp is
                    // reached rarely rather than routinely, which would pile
                    // senders up on the bounds and flatten the shape back out.
                    let u1 = unit(&mut state).max(f64::MIN_POSITIVE);
                    let u2 = unit(&mut state);
                    let normal = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
                    (0.75 * normal).exp()
                }
            };
            w.clamp(MIN_WEIGHT, MAX_WEIGHT)
        })
        .collect()
}

/// Picks a sender per tick, in proportion to the drawn weights.
///
/// Selection is random rather than a smooth deterministic schedule, and that
/// is the burstiness. A smooth schedule would give each sender its exact
/// share spread as evenly as possible, which is a third kind of unrealistic:
/// real agents clump. Drawing per tick lets a sender take several ticks in a
/// row and then go quiet, while its long run share still converges on its
/// weight.
#[derive(Debug)]
pub struct WeightedSelector {
    /// Cumulative weights. `cumulative[i]` is the sum through sender `i`.
    cumulative: Vec<f64>,
    total: f64,
    seed: u64,
    /// Tick counter. The draw is derived from it rather than from a running
    /// RNG state so selection needs no lock and stays reproducible under any
    /// number of workers.
    ticks: AtomicU64,
}

impl WeightedSelector {
    pub fn new(weights: &[f64], seed: u64) -> Option<Self> {
        if weights.is_empty() || !weights.iter().all(|w| *w > 0.0) {
            return None;
        }
        let mut cumulative = Vec::with_capacity(weights.len());
        let mut running = 0.0;
        for w in weights {
            running += *w;
            cumulative.push(running);
        }
        Some(Self {
            cumulative,
            total: running,
            seed,
            ticks: AtomicU64::new(0),
        })
    }

    pub fn len(&self) -> usize {
        self.cumulative.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cumulative.is_empty()
    }

    /// The share sender `index` is expected to receive.
    pub fn share(&self, index: usize) -> f64 {
        let lower = if index == 0 {
            0.0
        } else {
            self.cumulative[index - 1]
        };
        (self.cumulative[index] - lower) / self.total
    }

    /// Pick the sender for the next tick.
    pub fn next_index(&self) -> usize {
        let tick = self.ticks.fetch_add(1, Ordering::Relaxed);
        self.index_for_tick(tick)
    }

    /// The sender for a given tick number. Pure, so a schedule can be
    /// replayed and compared without running the generator.
    pub fn index_for_tick(&self, tick: u64) -> usize {
        let mut state = self.seed ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        // Discard one draw: splitmix64 is weak in its lowest-index outputs
        // for closely-spaced seeds, and consecutive ticks are exactly that.
        let _ = splitmix64(&mut state);
        let target = unit(&mut state) * self.total;
        match self
            .cumulative
            .binary_search_by(|c| c.partial_cmp(&target).unwrap_or(std::cmp::Ordering::Equal))
        {
            Ok(i) => i.min(self.cumulative.len() - 1),
            Err(i) => i.min(self.cumulative.len() - 1),
        }
    }
}
