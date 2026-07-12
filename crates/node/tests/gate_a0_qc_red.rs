//! Gate F4 (A0 scope) RED tests: T6, the QC gauntlet.
//!
//! Every mutation below produces certification evidence that must be
//! rejected by A6 (which wires verify_qc_well_formed plus the hash-binding
//! and parent-linkage checks). The audit must exit 1 with an A6 FAIL line.

#[path = "a0_common/mod.rs"]
mod a0_common;

use a0_common::{
    build_fixture, dev_signing_keys, make_qc, make_qc_with_keys, run_a0, sign_vote, Evidence,
    Fixture, FixtureSpec,
};
use ed25519_dalek::SigningKey;
use novai_consensus_types::codec::{encode_block_v1, encode_qc_v1, encode_vote_v1_signed, hash_block_v1};
use novai_consensus_types::{Block, QC};
use novai_state::{block_key, qc_key, Kv, KEY_HIGHEST_QC};

/// Overwrite both the dense qc row at T+1 and KEY_HIGHEST_QC with `qc_bytes`.
fn put_qc_evidence(fx: &Fixture, qc_bytes: &[u8]) {
    let mut db = fx.reopen();
    db.put(&qc_key(fx.t + 1), qc_bytes).expect("put qc row");
    db.put(KEY_HIGHEST_QC, qc_bytes).expect("put highest qc");
}

fn assert_a6_fail(fx: &Fixture, label: &str) {
    let (code, stdout, _stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(code, 1, "{label}: audit must fail; stdout:\n{stdout}");
    assert!(stdout.contains("A6 FAIL"), "{label}: stdout:\n{stdout}");
    assert!(stdout.contains("RESULT FAIL"), "{label}: stdout:\n{stdout}");
}

#[test]
fn t6_sub_quorum_qc_fails() {
    let fx = build_fixture("t6_subq", FixtureSpec::default());
    let qc = make_qc(&fx.block_t1, &[0, 1]);
    put_qc_evidence(&fx, &encode_qc_v1(&qc).expect("encode"));
    assert_a6_fail(&fx, "sub-quorum");
}

#[test]
fn t6_duplicate_voter_qc_fails() {
    let fx = build_fixture("t6_dup", FixtureSpec::default());
    let keys = dev_signing_keys();
    let bh = hash_block_v1(&fx.block_t1).expect("hash");
    let v0 = sign_vote(&keys[0], fx.t + 1, 0, bh);
    let v1 = sign_vote(&keys[1], fx.t + 1, 0, bh);

    // encode_qc_v1 refuses duplicate voters, so assemble the bytes by hand:
    // [version][height][round][block_hash][vote_count][votes...]
    let mut bytes = Vec::new();
    bytes.push(1u8);
    bytes.extend_from_slice(&(fx.t + 1).to_be_bytes());
    bytes.extend_from_slice(&0u64.to_be_bytes());
    bytes.extend_from_slice(&bh);
    bytes.extend_from_slice(&3u32.to_be_bytes());
    bytes.extend_from_slice(&encode_vote_v1_signed(&v0));
    bytes.extend_from_slice(&encode_vote_v1_signed(&v0));
    bytes.extend_from_slice(&encode_vote_v1_signed(&v1));

    put_qc_evidence(&fx, &bytes);
    assert_a6_fail(&fx, "duplicate voter");
}

#[test]
fn t6_unknown_voter_qc_fails() {
    let fx = build_fixture("t6_unknown", FixtureSpec::default());
    let keys = dev_signing_keys();
    let outsider = SigningKey::from_bytes(&[9u8; 32]);
    let qc = make_qc_with_keys(&fx.block_t1, &[&keys[0], &keys[1], &outsider]);
    put_qc_evidence(&fx, &encode_qc_v1(&qc).expect("encode"));
    assert_a6_fail(&fx, "unknown voter");
}

#[test]
fn t6_invalid_signature_qc_fails() {
    let fx = build_fixture("t6_badsig", FixtureSpec::default());
    let mut qc = make_qc(&fx.block_t1, &[0, 1, 3]);
    qc.votes[0].signature[0] ^= 0x01;
    put_qc_evidence(&fx, &encode_qc_v1(&qc).expect("encode"));
    assert_a6_fail(&fx, "invalid signature");
}

#[test]
fn t6_vote_bound_to_different_block_fails() {
    let fx = build_fixture("t6_binding", FixtureSpec::default());
    let keys = dev_signing_keys();
    // Votes are validly signed, but over block T's hash, not block T+1's.
    let other_hash = hash_block_v1(&fx.block_t).expect("hash t");
    let votes = vec![
        sign_vote(&keys[0], fx.t + 1, 0, other_hash),
        sign_vote(&keys[1], fx.t + 1, 0, other_hash),
        sign_vote(&keys[3], fx.t + 1, 0, other_hash),
    ];
    let qc = QC {
        height: fx.t + 1,
        round: 0,
        block_hash: hash_block_v1(&fx.block_t1).expect("hash t1"),
        votes,
    };
    put_qc_evidence(&fx, &encode_qc_v1(&qc).expect("encode"));
    assert_a6_fail(&fx, "vote bound to different block");
}

#[test]
fn t6_hqc_descent_broken_parent_link_fails() {
    let fx = build_fixture(
        "t6_link",
        FixtureSpec {
            evidence: Evidence::HqcDescent,
            ..FixtureSpec::default()
        },
    );
    {
        let mut db = fx.reopen();
        // Corrupt the pipeline block at T+2: parent no longer links to T+1.
        let corrupt = Block {
            height: fx.t + 2,
            round: 0,
            parent_hash: [0xFF; 32],
            state_root: fx.r1,
            txs: vec![],
        };
        db.put(
            &block_key(fx.t + 2),
            &encode_block_v1(&corrupt).expect("encode corrupt"),
        )
        .expect("put corrupt block");
        let qc = make_qc(&corrupt, &[0, 1, 3]);
        db.put(KEY_HIGHEST_QC, &encode_qc_v1(&qc).expect("encode"))
            .expect("put hqc");
    }
    assert_a6_fail(&fx, "broken parent link");
}
