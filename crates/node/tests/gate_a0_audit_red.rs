//! Gate F4 (A0 scope) RED tests: the audit pipeline A1..A8.
//!
//! Happy paths cover both certification evidence shapes (dense qc row and
//! highest-QC pipeline descent). Failure cases pin the exact check that must
//! catch each defect class from the diagnosis T-list:
//!   T3 value tamper        -> A5 (rebuild != stored root)
//!   T4 dropped leaf        -> A5
//!   T5 off-by-one trap     -> A7 (identity: header(T+1) is the certified
//!                              commitment to post-state(T), never header(T))
//!   T7 torn cursors        -> A1; unauthenticated extra flat key -> A5
//!   T8 unknown key         -> A3 hard fail, key named
//!      defined-but-unwritten prefix present -> A3 hard fail
//!      every known SMT-committed class      -> audit passes

#[path = "a0_common/mod.rs"]
mod a0_common;

use a0_common::{acct, build_fixture, make_qc, run_a0, Evidence, FixtureSpec};
use novai_consensus_types::codec::{encode_block_v1, encode_qc_v1, hash_block_v1};
use novai_consensus_types::Block;
use novai_state::{
    account_key, ai_delegation_by_delegate_key, ai_entity_by_address_key, ai_entity_key,
    ai_memory_by_type_key, ai_memory_count_key, ai_memory_key, ai_memory_object_key,
    ai_params_key, ai_signal_by_issuer_key, ai_signal_by_type_key, ai_signal_key,
    approval_gate_key, block_key, derived_view_key, encode_account_v1, governance_log_key,
    governance_proposal_by_state_key, governance_proposal_key, qc_key, AccountStateV1, Kv,
    KEY_AI_KILL_SWITCH, KEY_EXECUTED_HEIGHT, KEY_HIGHEST_QC,
};

#[test]
fn happy_path_qc_row_evidence_passes() {
    let fx = build_fixture("happy_qcrow", FixtureSpec::default());
    let (code, stdout, stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");
    for check in ["A1", "A2", "A3", "A4", "A5", "A6", "A7", "A8"] {
        assert!(
            stdout.contains(&format!("{check} PASS")),
            "missing {check} PASS; stdout:\n{stdout}"
        );
    }
    assert!(stdout.contains("source=qc_row"), "stdout:\n{stdout}");
    assert_eq!(a0_common::parse_result_root(&stdout), hex::encode(fx.r1));
}

#[test]
fn happy_path_highest_qc_descent_passes() {
    let fx = build_fixture(
        "happy_hqc",
        FixtureSpec {
            evidence: Evidence::HqcDescent,
            ..FixtureSpec::default()
        },
    );
    let (code, stdout, stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.contains("source=highest_qc"), "stdout:\n{stdout}");
    assert_eq!(a0_common::parse_result_root(&stdout), hex::encode(fx.r1));
}

#[test]
fn t3_value_tamper_fails_a5() {
    let fx = build_fixture("t3_tamper", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        // Raw put, deliberately bypassing the SMT path: balance 1000 -> 1001.
        db.put(
            &account_key(&[0xA1; 32]),
            &encode_account_v1(&AccountStateV1 {
                balance: 1_001,
                nonce: 0,
            }),
        )
        .expect("tamper put");
    }
    let (code, stdout, _stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(code, 1, "tampered copy must fail; stdout:\n{stdout}");
    assert!(stdout.contains("A5 FAIL"), "stdout:\n{stdout}");
    assert!(stdout.contains("RESULT FAIL"), "stdout:\n{stdout}");
}

#[test]
fn t4_dropped_leaf_fails_a5() {
    let fx = build_fixture("t4_drop", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        db.delete(&account_key(&[0xB2; 32])).expect("drop leaf");
    }
    let (code, stdout, _stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(code, 1, "dropped leaf must fail; stdout:\n{stdout}");
    assert!(stdout.contains("A5 FAIL"), "stdout:\n{stdout}");
}

#[test]
fn t5_off_by_one_trap_fails_a7() {
    // Craft a copy where header(T) carries the POST-state root r1 (the wrong
    // convention) and header(T+1) carries r0. An implementation that compares
    // the rebuilt root against header(T) would pass this copy; the correct
    // identity check against header(T+1) must fail it.
    let fx = build_fixture("t5_trap", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        let block_t_trap = Block {
            state_root: fx.r1,
            ..fx.block_t.clone()
        };
        let block_t1_trap = Block {
            parent_hash: hash_block_v1(&block_t_trap).expect("hash trap t"),
            state_root: fx.r0,
            ..fx.block_t1.clone()
        };
        db.put(
            &block_key(fx.t),
            &encode_block_v1(&block_t_trap).expect("encode trap t"),
        )
        .expect("put trap t");
        db.put(
            &block_key(fx.t + 1),
            &encode_block_v1(&block_t1_trap).expect("encode trap t1"),
        )
        .expect("put trap t1");
        let qc = make_qc(&block_t1_trap, &[0, 1, 3]);
        let qc_bytes = encode_qc_v1(&qc).expect("encode trap qc");
        db.put(&qc_key(fx.t + 1), &qc_bytes).expect("put trap qc");
        db.put(KEY_HIGHEST_QC, &qc_bytes).expect("put trap hqc");
    }
    let (code, stdout, _stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(code, 1, "off-by-one trap must fail; stdout:\n{stdout}");
    assert!(stdout.contains("A7 FAIL"), "stdout:\n{stdout}");
}

#[test]
fn t7_torn_cursors_fail_a1() {
    let fx = build_fixture("t7_torn", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        db.put(KEY_EXECUTED_HEIGHT, &6u64.to_be_bytes())
            .expect("tear cursor");
    }
    let (code, stdout, _stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(code, 1, "torn cursors must fail; stdout:\n{stdout}");
    assert!(stdout.contains("A1 FAIL"), "stdout:\n{stdout}");
}

#[test]
fn t7_unauthenticated_extra_flat_key_fails_a5() {
    let fx = build_fixture("t7_extra", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        let (k, v) = acct(0xEE, 123, 0);
        db.put(&k, &v).expect("raw extra account");
    }
    let (code, stdout, _stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(code, 1, "unauthenticated key must fail; stdout:\n{stdout}");
    assert!(stdout.contains("A5 FAIL"), "stdout:\n{stdout}");
}

#[test]
fn t8_unknown_key_fails_a3_and_is_named() {
    let fx = build_fixture("t8_unknown", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        db.put(b"wat/unknown", b"junk").expect("unknown key");
    }
    let (code, stdout, _stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(code, 1, "unknown key must fail; stdout:\n{stdout}");
    assert!(stdout.contains("A3 FAIL"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("wat/unknown"),
        "offending key must be named; stdout:\n{stdout}"
    );
}

#[test]
fn t8_defined_but_unwritten_prefix_fails_a3() {
    let fx = build_fixture("t8_unwritten", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        db.put(&derived_view_key(&[0x11; 32]), b"x")
            .expect("derived view key");
    }
    let (code, stdout, _stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(code, 1, "defined-but-unwritten must fail; stdout:\n{stdout}");
    assert!(stdout.contains("A3 FAIL"), "stdout:\n{stdout}");
    assert!(stdout.contains("derived_views/"), "stdout:\n{stdout}");
}

#[test]
fn t8_every_known_smt_class_passes() {
    // One exemplar per SMT-committed class, all written through the canonical
    // SMT path as fixture pre-state. Values are arbitrary bytes: the SMT
    // commits to raw value bytes and the classifier sees keys only.
    let id = [0x77; 32];
    let oid = [0x78; 32];
    let mut pre = a0_common::default_pre_state();
    pre.push((ai_entity_key(&id), b"x".to_vec()));
    pre.push((ai_entity_by_address_key(&id), b"x".to_vec()));
    pre.push((ai_memory_key(&id, b"slot"), b"x".to_vec()));
    pre.push((ai_params_key(&id, b"param"), b"x".to_vec()));
    pre.push((ai_signal_key(9, &id), b"x".to_vec()));
    pre.push((ai_signal_by_type_key(1, 9, &id), b"x".to_vec()));
    pre.push((ai_signal_by_issuer_key(&id, 9), b"x".to_vec()));
    pre.push((ai_memory_object_key(&id, &oid), b"x".to_vec()));
    pre.push((ai_memory_count_key(&id), 1u32.to_be_bytes().to_vec()));
    pre.push((ai_memory_by_type_key(1, &id, &oid), vec![]));
    pre.push((ai_delegation_by_delegate_key(&id, &oid), id.to_vec()));
    pre.push((governance_proposal_key(&id), b"x".to_vec()));
    pre.push((governance_log_key(&id), b"x".to_vec()));
    pre.push((governance_proposal_by_state_key(1, &id), vec![]));
    pre.push((approval_gate_key(&id), b"x".to_vec()));
    pre.push((KEY_AI_KILL_SWITCH.to_vec(), vec![1]));

    let fx = build_fixture(
        "t8_classes",
        FixtureSpec {
            pre_state: pre,
            ..FixtureSpec::default()
        },
    );
    let (code, stdout, stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(
        code, 0,
        "all known classes must audit clean; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(a0_common::parse_result_root(&stdout), hex::encode(fx.r1));
}

#[test]
fn explicit_height_mismatch_fails_a1() {
    let fx = build_fixture("height_mismatch", FixtureSpec::default());
    let (code, stdout, _stderr) = run_a0(&["audit", "--db", &fx.db_arg(), "--height", "9"]);
    assert_eq!(code, 1, "height mismatch must fail; stdout:\n{stdout}");
    assert!(stdout.contains("A1 FAIL"), "stdout:\n{stdout}");
}
