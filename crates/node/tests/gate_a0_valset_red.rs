//! Gate F4 (A0 scope) RED tests: dev valset derivation and inspect mode.
//!
//! RED against the stub a0 binary; flips green with unchanged bodies once A0
//! is implemented. The valset test derives the four dev validator identities
//! independently (same seeds as main.rs:1002-1011) and requires the binary to
//! agree, closing the loop on the valset resolution in the F4 diagnosis
//! (doc section 10): quorum must come from the 2f+1 formula, never a literal.

#[path = "a0_common/mod.rs"]
mod a0_common;

#[test]
fn valset_matches_independent_derivation_and_quorum_formula() {
    let (code, stdout, stderr) = a0_common::run_a0(&["valset"]);
    assert_eq!(
        code, 0,
        "a0 valset must exit 0; stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let keys = a0_common::dev_signing_keys();
    for (i, sk) in keys.iter().enumerate() {
        let addr_hex = hex::encode(novai_crypto::address_from_pubkey(&sk.verifying_key()));
        let pk_hex = hex::encode(sk.verifying_key().as_bytes());
        let expected = format!("validator-{i} addr={addr_hex} pubkey={pk_hex}");
        assert!(
            stdout.contains(&expected),
            "missing valset line: {expected}\nstdout:\n{stdout}"
        );
    }

    assert!(
        stdout.contains("quorum n=4 f=1 q=3"),
        "missing quorum formula line; stdout:\n{stdout}"
    );
}

#[test]
fn inspect_reports_cursors_root_and_qc_voters() {
    let fx = a0_common::build_fixture("inspect", a0_common::FixtureSpec::default());
    let db = fx.db_arg();

    let (code, stdout, stderr) = a0_common::run_a0(&["inspect", "--db", &db]);
    assert_eq!(
        code, 0,
        "a0 inspect must exit 0; stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    assert!(stdout.contains("committed_height=7"), "stdout:\n{stdout}");
    assert!(stdout.contains("executed_height=7"), "stdout:\n{stdout}");
    assert!(
        stdout.contains(&format!("smt_root={}", hex::encode(fx.r1))),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("highest_qc_height=8"), "stdout:\n{stdout}");

    // Voters in the fixture QC are validators 0, 1, 3. Validator 2 (the node2
    // slot) must not appear anywhere in the inspect output.
    assert!(stdout.contains("validator-0"), "stdout:\n{stdout}");
    assert!(stdout.contains("validator-1"), "stdout:\n{stdout}");
    assert!(stdout.contains("validator-3"), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("validator-2"),
        "validator-2 must be absent from voters; stdout:\n{stdout}"
    );
}
