//! Gate F4 (A0 scope) RED tests: SMT rebuild determinism and canon.
//!
//! T1: insertion-order independence of the rebuilt root, proven through the
//!     audit (two fixtures with the same final leaf map, different write
//!     order, must both pass and report the same root).
//! T2: the canonical empty root: an empty-state copy (no state keys, no
//!     stored smt/root) audits clean against evidence built on
//!     empty_smt_root(), pinning the absent-root default the consensus read
//!     sites use.
//! T14: one-shot batch fixture (genesis precedent) audits clean against A0's
//!     chunked rebuild, proving builder-vs-node-path equivalence through the
//!     root equality.

#[path = "a0_common/mod.rs"]
mod a0_common;

use a0_common::{build_fixture, default_pre_state, run_a0, Evidence, FixtureSpec};

#[test]
fn t1_rebuild_is_insertion_order_independent() {
    let fx_forward = build_fixture("t1_fwd", FixtureSpec::default());

    let mut reversed = default_pre_state();
    reversed.reverse();
    let fx_reverse = build_fixture(
        "t1_rev",
        FixtureSpec {
            pre_state: reversed,
            ..FixtureSpec::default()
        },
    );

    let (code_a, out_a, err_a) = run_a0(&["audit", "--db", &fx_forward.db_arg()]);
    assert_eq!(code_a, 0, "forward audit; stdout:\n{out_a}\nstderr:\n{err_a}");
    let (code_b, out_b, err_b) = run_a0(&["audit", "--db", &fx_reverse.db_arg()]);
    assert_eq!(code_b, 0, "reverse audit; stdout:\n{out_b}\nstderr:\n{err_b}");

    let root_a = a0_common::parse_result_root(&out_a);
    let root_b = a0_common::parse_result_root(&out_b);
    assert_eq!(
        root_a, root_b,
        "same leaf map must rebuild to the same root regardless of write order"
    );
    assert_eq!(root_a, hex::encode(fx_forward.r1), "audit root must match fixture root");
}

#[test]
fn t2_empty_state_audits_clean_with_canonical_empty_root() {
    let fx = build_fixture(
        "t2_empty",
        FixtureSpec {
            t: 0,
            pre_state: vec![],
            step_state: vec![],
            ..FixtureSpec::default()
        },
    );

    let (code, stdout, stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(
        code, 0,
        "empty-state audit must pass; stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let root = a0_common::parse_result_root(&stdout);
    assert_eq!(
        root,
        hex::encode(novai_execution::empty_smt_root()),
        "empty rebuild must equal empty_hash_at_height(256)"
    );
}

#[test]
fn t14_oneshot_batch_fixture_matches_chunked_rebuild() {
    let fx = build_fixture(
        "t14_oneshot",
        FixtureSpec {
            oneshot: true,
            evidence: Evidence::QcRow,
            ..FixtureSpec::default()
        },
    );

    let (code, stdout, stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(
        code, 0,
        "one-shot fixture must audit clean; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        a0_common::parse_result_root(&stdout),
        hex::encode(fx.r1),
        "chunked rebuild must reproduce the one-shot batch root"
    );
}
