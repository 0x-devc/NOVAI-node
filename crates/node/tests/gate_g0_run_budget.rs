//! Gate G0: the disk budget guard.
//!
//! A validation run that fills the disk is not a measurement, it is an
//! outage. This is the pre-flight refusal that keeps the two apart.
//!
//! The arithmetic comes from the throughput plan's 3.1, established from
//! source rather than from docs: the SMT is 256 levels with no path
//! compression over blake3-hashed keys, so an update walk writes exactly one
//! node per level with no collapse. A transfer touches three state keys and
//! `append_smt_ops_for_state_ops` walks once per op, giving 768 nodes at 108
//! bytes each. That 82,944 bytes is PERMANENT: the node store is content
//! addressed, so a changed subtree yields a new key and never overwrites its
//! predecessor, and the 50k prune deletes only `consensus/blocks/` and
//! `consensus/qcs/`.
//!
//! The guard exists because disk burn scales with APPLIED TPS and this entire
//! plan exists to raise applied TPS. It moved from G3 into G0 by operator
//! decision, because G2 validation at 32 TPS already burns 2.65 MB/s and can
//! fill the disk on its own.

use novai_node::run_budget::{
    free_bytes, max_run_seconds, parse_df_available_bytes, BudgetRefusal, RunBudget,
    BYTES_PER_APPLIED_TX, DISK_HOLDBACK,
};

/// The plan states the budget at 12 GB free as `129,480 / applied_TPS`. That
/// closed form is only true for one holdback and one per-transaction cost, so
/// reproducing it pins both constants at once.
const TWELVE_GB: u64 = 12_000_000_000;

#[test]
fn budget_reproduces_the_plans_closed_form_at_twelve_gigabytes() {
    let at_one_tps = max_run_seconds(TWELVE_GB, 1.0).expect("1 TPS is a valid rate");
    assert!(
        (at_one_tps - 129_480.9).abs() < 1.0,
        "the plan's 129,480 / applied_TPS must fall out of the formula, got {at_one_tps}"
    );

    // And it is a true reciprocal in the rate, which is what makes the closed
    // form usable at any load.
    for tps in [0.6_f64, 32.0, 150.0, 256.0] {
        let seconds = max_run_seconds(TWELVE_GB, tps).expect("a positive rate is valid");
        assert!(
            (seconds - at_one_tps / tps).abs() < 1e-6,
            "budget at {tps} TPS must be the 1 TPS budget divided by the rate"
        );
    }
}

#[test]
fn budget_matches_the_plans_per_gate_validation_loads() {
    // G2 validation, 32 TPS at 4 bps. The plan budgets a 15 minute run at
    // 2.4 GB and says it fits.
    let g2 = max_run_seconds(TWELVE_GB, 32.0).unwrap();
    assert!(
        g2 > 900.0,
        "a 15 minute G2 run must fit inside a 12 GB budget, got {g2} s"
    );

    // G3 at its full 256 TPS ceiling does NOT fit a 15 minute run. This is the
    // finding that put a time box on G3 rather than blocking it on garbage
    // collection, so the guard has to agree with it.
    let g3 = max_run_seconds(TWELVE_GB, 256.0).unwrap();
    assert!(
        g3 < 900.0,
        "a 15 minute G3 run at the full ceiling must NOT fit, got {g3} s"
    );

    // A soak at the 150 TPS target misses by orders of magnitude.
    let soak = max_run_seconds(TWELVE_GB, 150.0).unwrap();
    assert!(
        soak < 24.0 * 3600.0 / 50.0,
        "a 24 hour soak at target must be nowhere near fitting, got {soak} s"
    );
}

#[test]
fn budget_holds_back_a_tenth_and_charges_the_full_per_tx_cost() {
    // Both constants pinned as numbers, not as shapes. Spending the last tenth
    // of a disk is how a measurement becomes an outage, and undercharging the
    // per-transaction cost is how a run overruns its budget without ever
    // tripping the guard.
    assert_eq!(BYTES_PER_APPLIED_TX, 83_410);
    assert!((DISK_HOLDBACK - 0.9).abs() < f64::EPSILON);

    let free = 1_000_000_000u64;
    let expected = 0.9 * 1_000_000_000.0 / (10.0 * 83_410.0);
    let got = max_run_seconds(free, 10.0).unwrap();
    assert!(
        (got - expected).abs() < 1e-6,
        "expected {expected}, got {got}"
    );
    // A tenth of the disk is left standing.
    assert!(got * 10.0 * 83_410.0 < free as f64);
}

#[test]
fn budget_refuses_rather_than_dividing_by_a_rate_it_cannot_use() {
    // Zero, negative, NaN and infinity all mean "I do not know the load".
    // Returning a number for any of them hands the caller an unbounded run
    // wearing a budget.
    assert_eq!(max_run_seconds(TWELVE_GB, 0.0), None);
    assert_eq!(max_run_seconds(TWELVE_GB, -1.0), None);
    assert_eq!(max_run_seconds(TWELVE_GB, f64::NAN), None);
    assert_eq!(max_run_seconds(TWELVE_GB, f64::INFINITY), None);
}

#[test]
fn budget_refuses_a_run_longer_than_the_disk_allows() {
    // The whole point: a pre-flight refusal, not a post-hoc alert.
    let refusal = RunBudget::plan(TWELVE_GB, 256.0, 900.0).unwrap_err();
    match refusal {
        BudgetRefusal::ExceedsBudget { requested, max } => {
            assert!((requested - 900.0).abs() < f64::EPSILON);
            assert!(max < 900.0, "the computed max must be the binding number");
        }
        other => panic!("expected an ExceedsBudget refusal, got {other:?}"),
    }

    // The same run at a rate the disk can carry is allowed.
    let ok = RunBudget::plan(TWELVE_GB, 32.0, 900.0).expect("G2 at 32 TPS fits");
    assert!(ok.max_run_seconds() > 900.0);
}

#[test]
fn budget_refuses_when_free_space_could_not_be_measured() {
    // Fail closed. If the guard cannot read free space it must refuse to run,
    // never assume the disk is roomy: an unmeasured disk is exactly the
    // condition under which a run fills it.
    let refusal = RunBudget::plan_measured(None, 32.0, 900.0).unwrap_err();
    assert!(matches!(refusal, BudgetRefusal::UnmeasurableFreeSpace));
}

#[test]
fn budget_expires_mid_run_at_the_computed_deadline() {
    use std::time::{Duration, Instant};

    let start = Instant::now();
    let budget = RunBudget::plan_at(TWELVE_GB, 256.0, 500.0, start).expect("500 s fits at 256 TPS");
    let max = budget.max_run_seconds();
    assert!((505.0..507.0).contains(&max), "got {max}");

    assert!(!budget.expired_at(start));
    assert!(!budget.expired_at(start + Duration::from_secs(499)));
    // The deadline is the COMPUTED budget, not the requested run length, so a
    // caller that keeps going past its own request is still stopped by disk.
    assert!(!budget.expired_at(start + Duration::from_secs(504)));
    assert!(budget.expired_at(start + Duration::from_secs(507)));
}

#[test]
fn df_parse_reads_available_bytes_from_real_output() {
    // macOS, the shape this repo's own `df -Pk .` produces.
    let darwin = "Filesystem   1024-blocks      Used Available Capacity  Mounted on\n\
                  /dev/disk3s5   239362496 157078088  39450248    80%    /System/Volumes/Data\n";
    assert_eq!(
        parse_df_available_bytes(darwin),
        Some(39_450_248 * 1024),
        "the Available column is the fourth field and is in 1K blocks"
    );

    // Linux, POSIX -P output.
    let linux = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n\
                 /dev/sda1        102687672  89551208  12345678      88% /\n";
    assert_eq!(parse_df_available_bytes(linux), Some(12_345_678 * 1024));
}

#[test]
fn df_parse_fails_closed_on_anything_it_does_not_understand() {
    // Every one of these must be None, never a fabricated size. A guard that
    // guesses on a parse failure is worse than no guard, because the caller
    // believes it is protected.
    assert_eq!(parse_df_available_bytes(""), None);
    assert_eq!(
        parse_df_available_bytes("Filesystem 1024-blocks Used Available Capacity Mounted on\n"),
        None,
        "a header with no data row is not a measurement"
    );
    assert_eq!(parse_df_available_bytes("df: /nope: No such file or directory\n"), None);
    assert_eq!(
        parse_df_available_bytes("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda1 100 50 fifty 50% /\n"),
        None,
        "a non-numeric Available column is not a measurement"
    );
    assert_eq!(
        parse_df_available_bytes("Filesystem Used\n/dev/sda1 100\n"),
        None,
        "too few columns is not a measurement"
    );
}

#[test]
fn free_bytes_reads_this_filesystem_and_agrees_with_the_budget() {
    // The one call that proves the reader is wired to a real filesystem rather
    // than to a constant. Deliberately loose: this asserts the shape of a real
    // measurement, not a specific size.
    let here = free_bytes(std::path::Path::new(".")).expect("df must work in the repo directory");
    assert!(here > 0, "a working filesystem reports some free space");
    assert!(
        here < 1 << 60,
        "a plausible free-space reading, not a sentinel"
    );

    // A nonexistent path must fail closed rather than report the root.
    assert_eq!(
        free_bytes(std::path::Path::new(
            "/definitely/not/a/real/path/for/gate/g0"
        )),
        None
    );
}
