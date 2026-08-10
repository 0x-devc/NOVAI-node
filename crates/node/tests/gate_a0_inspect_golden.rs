//! Golden pin on the `a0 inspect` CLI output (gate F5 Stage 1).
//!
//! `inspect` is the offline forensic reader. It is what gets pointed at a
//! preserved data directory to answer "what height was this node at, what
//! root did it hold, how high had it voted, who signed its QCs" in every
//! snapshot investigation, and the F4 restore procedure reads its
//! `voted_view` line to establish the equivocation precondition before an
//! install. Its output is therefore a contract with a human and with a
//! runbook, not an incidental print, and nothing pinned it: `audit` has five
//! RED suites, `inspect` had one assertion about QC voters.
//!
//! This pins the SHAPE exhaustively (which lines, in which order, with which
//! field names) and the VALUES that are semantically determined by the
//! fixture. The one exception is the `operational` count, which counts SMT
//! internal node records; that number is a function of the tree's internal
//! shape, so pinning it would turn any unrelated SMT change into a failure
//! here while adding nothing to the contract. It is asserted present and
//! positive instead, and the reasoning is recorded so a future reader does
//! not mistake the gap for an oversight.

#[path = "a0_common/mod.rs"]
mod a0_common;

use a0_common::{build_fixture, run_a0, Evidence, FixtureSpec};

/// The exact ordered line prefixes `inspect` emits for a fixture with dense
/// QC-row evidence. Order is part of the contract: a runbook reader scans
/// top to bottom.
const EXPECTED_SHAPE: &[&str] = &[
    "committed_height=",
    "executed_height=",
    "smt_root=",
    "voted_view=",
    "locked_qc_height=",
    "highest_qc_height=",
    "highest_qc_voters=",
    "qc_row height=",
    "class_counts ",
];

fn lines(stdout: &str) -> Vec<&str> {
    stdout.lines().filter(|l| !l.is_empty()).collect()
}

#[test]
fn inspect_emits_the_pinned_shape_in_order() {
    let fx = build_fixture("inspect_golden_shape", FixtureSpec::default());
    let (code, stdout, _stderr) = run_a0(&["inspect", "--db", &fx.db_arg()]);
    assert_eq!(code, 0, "inspect must succeed on a well formed copy:\n{stdout}");

    let got = lines(&stdout);
    assert_eq!(
        got.len(),
        EXPECTED_SHAPE.len(),
        "inspect emitted {} lines, expected {}. A line was added or removed; \
         if that is intended, update EXPECTED_SHAPE and say why in the commit.\n{stdout}",
        got.len(),
        EXPECTED_SHAPE.len()
    );
    for (i, prefix) in EXPECTED_SHAPE.iter().enumerate() {
        assert!(
            got[i].starts_with(prefix),
            "line {i} must start with {prefix:?}, got {:?}\n{stdout}",
            got[i]
        );
    }
}

#[test]
fn inspect_reports_the_fixture_values_exactly() {
    let fx = build_fixture("inspect_golden_values", FixtureSpec::default());
    let (code, stdout, _stderr) = run_a0(&["inspect", "--db", &fx.db_arg()]);
    assert_eq!(code, 0, "stdout:\n{stdout}");

    let t = fx.t;
    for expected in [
        format!("committed_height={t}"),
        format!("executed_height={t}"),
        // The stored root is post-state(T) under the post-state convention.
        format!("smt_root={}", hex::encode(fx.r1)),
        // A synthetic copy has never voted and never locked.
        "voted_view=absent".to_string(),
        "locked_qc_height=absent".to_string(),
        // Evidence::QcRow stores the QC over block(T+1) as both the dense row
        // and KEY_HIGHEST_QC.
        format!("highest_qc_height={}", t + 1),
        // Voter ORDER is part of the contract and is not the validator index
        // order the fixture asks for. The canonical QC encoding sorts votes by
        // voter ADDRESS, and the dev addresses sort 3 < 1 < 2 < 0
        // (096094.. < 33de7a.. < 4ad1ba.. < afb672.., see the golden pin in
        // snapshot/valset.rs), so a fixture signed by validators 0, 1 and 3
        // reads back as 3, 1, 0. Pinned explicitly because an operator
        // comparing two directories by eye would otherwise read a reordering
        // as a difference.
        "highest_qc_voters=validator-3,validator-1,validator-0".to_string(),
        format!("qc_row height={} voters=validator-3,validator-1,validator-0", t + 1),
    ] {
        assert!(
            stdout.lines().any(|l| l == expected),
            "missing exact line {expected:?}\n{stdout}"
        );
    }
}

#[test]
fn inspect_classification_counts_are_pinned_where_they_are_determined() {
    let fx = build_fixture("inspect_golden_counts", FixtureSpec::default());
    let (_code, stdout, _stderr) = run_a0(&["inspect", "--db", &fx.db_arg()]);

    let line = stdout
        .lines()
        .find(|l| l.starts_with("class_counts "))
        .unwrap_or_else(|| panic!("no class_counts line\n{stdout}"));

    // The fixture writes three pre-state accounts plus one block-T account.
    assert!(
        line.contains("smt_committed=4"),
        "the fixture's four accounts are the whole authenticated leaf set: {line}"
    );
    // Fail-closed classes must be empty on a well formed copy. These two are
    // the ones that block a snapshot export, so they are the load-bearing
    // half of this line.
    assert!(line.contains("defined_unwritten=0"), "{line}");
    assert!(line.contains("unknown=0"), "{line}");

    // operational counts SMT internal node records, a function of tree shape
    // rather than of the contract. Present and positive is the assertion that
    // stays true across unrelated SMT work.
    let operational: u64 = line
        .split_whitespace()
        .find_map(|f| f.strip_prefix("operational="))
        .unwrap_or_else(|| panic!("no operational field: {line}"))
        .parse()
        .unwrap_or_else(|e| panic!("operational is not a number in {line:?}: {e}"));
    assert!(
        operational > 0,
        "a copy with state must carry SMT and consensus infrastructure rows: {line}"
    );
}

#[test]
fn inspect_omits_the_qc_row_line_when_there_is_no_dense_row() {
    // Evidence::HqcDescent is the shape of a fresh healthy-node copy: blocks
    // above the committed tip stored at receipt, KEY_HIGHEST_QC certifying
    // the pipeline tip, and no dense QC row at T or T+1. inspect must then
    // emit no qc_row line at all rather than an empty or placeholder one,
    // because a runbook reader treats a qc_row line as evidence a row exists.
    let fx = build_fixture(
        "inspect_golden_hqc",
        FixtureSpec {
            evidence: Evidence::HqcDescent,
            ..FixtureSpec::default()
        },
    );
    let (code, stdout, _stderr) = run_a0(&["inspect", "--db", &fx.db_arg()]);
    assert_eq!(code, 0, "stdout:\n{stdout}");

    assert!(
        !stdout.contains("qc_row height="),
        "no dense QC row exists in this shape, so no qc_row line may appear\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("highest_qc_height={}", fx.t + 2)),
        "the pipeline tip QC is still reported\n{stdout}"
    );
}

#[test]
fn inspect_never_fails_a_verdict_only_reports() {
    // Contract from the module doc: "Inspect never fails a verdict; it
    // reports what is there. Only IO errors exit nonzero." A copy with a
    // sub-quorum QC is diagnostically interesting and must still exit 0, or
    // an operator reading a broken directory would get no reading at all.
    let fx = build_fixture(
        "inspect_golden_subquorum",
        FixtureSpec {
            voters: vec![0, 1],
            ..FixtureSpec::default()
        },
    );
    let (code, stdout, _stderr) = run_a0(&["inspect", "--db", &fx.db_arg()]);
    assert_eq!(code, 0, "inspect must report, never judge:\n{stdout}");
    // Address order again: validator-1 (33de7a..) sorts before validator-0
    // (afb672..).
    assert!(stdout.contains("highest_qc_voters=validator-1,validator-0"));
}
