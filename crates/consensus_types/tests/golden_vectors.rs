//! Golden vector tests for consensus message encodings.
//!
//! These tests ensure encoding stability across versions.
//! To regenerate vectors (ONLY when intentionally changing format):
//!   `UPDATE_VECTORS=1` cargo test -p novai-consensus-types

use novai_consensus_types::codec::*;
use novai_consensus_types::{Block, Timeout, Vote, QC};
use std::fs;
use std::path::Path;

fn vectors_dir() -> &'static Path {
    Path::new("tests/vectors")
}

fn should_update_vectors() -> bool {
    std::env::var("UPDATE_VECTORS").is_ok()
}

#[test]
fn golden_vote_unsigned() {
    let vote = Vote {
        height: 42,
        round: 7,
        block_hash: [0xaa; 32],
        voter: [0xbb; 32],
        signature: [0x00; 64],
    };

    let bytes = encode_vote_v1_unsigned(&vote);
    let path = vectors_dir().join("vote_unsigned_v1.bin");

    if should_update_vectors() {
        fs::write(&path, &bytes).unwrap();
        println!("Updated: {path:?}");
    } else {
        let expected = fs::read(&path).expect("golden vector missing");
        assert_eq!(bytes, expected, "Vote unsigned encoding drifted!");
    }
}

#[test]
fn golden_vote_signed() {
    let vote = Vote {
        height: 42,
        round: 7,
        block_hash: [0xaa; 32],
        voter: [0xbb; 32],
        signature: [0xcc; 64],
    };

    let bytes = encode_vote_v1_signed(&vote);
    let path = vectors_dir().join("vote_signed_v1.bin");

    if should_update_vectors() {
        fs::write(&path, &bytes).unwrap();
        println!("Updated: {path:?}");
    } else {
        let expected = fs::read(&path).expect("golden vector missing");
        assert_eq!(bytes, expected, "Vote signed encoding drifted!");
    }
}

#[test]
fn golden_block_empty_txs() {
    let block = Block {
        height: 100,
        round: 5,
        parent_hash: [0x11; 32],
        state_root: [0x22; 32],
        txs: vec![],
    };

    let bytes = encode_block_v1(&block).unwrap();
    let path = vectors_dir().join("block_empty_txs_v1.bin");

    if should_update_vectors() {
        fs::write(&path, &bytes).unwrap();
        println!("Updated: {path:?}");
    } else {
        let expected = fs::read(&path).expect("golden vector missing");
        assert_eq!(bytes, expected, "Block (empty txs) encoding drifted!");
    }
}

#[test]
fn golden_qc_empty_votes() {
    let qc = QC {
        height: 50,
        round: 3,
        block_hash: [0x33; 32],
        votes: vec![],
    };

    let bytes = encode_qc_v1(&qc).unwrap();
    let path = vectors_dir().join("qc_empty_votes_v1.bin");

    if should_update_vectors() {
        fs::write(&path, &bytes).unwrap();
        println!("Updated: {path:?}");
    } else {
        let expected = fs::read(&path).expect("golden vector missing");
        assert_eq!(
            bytes, expected,
            "QC (empty votes - codec only) encoding drifted!"
        );
    }
}

#[test]
fn golden_timeout_no_qc() {
    let timeout = Timeout {
        height: 25,
        round: 2,
        voter: [0x44; 32],
        highest_qc: None,
        signature: [0x55; 64],
    };

    let bytes_unsigned = encode_timeout_v1_unsigned(&timeout).unwrap();
    let bytes_signed = encode_timeout_v1_signed(&timeout).unwrap();

    let path_unsigned = vectors_dir().join("timeout_no_qc_unsigned_v1.bin");
    let path_signed = vectors_dir().join("timeout_no_qc_signed_v1.bin");

    if should_update_vectors() {
        fs::write(&path_unsigned, &bytes_unsigned).unwrap();
        fs::write(&path_signed, &bytes_signed).unwrap();
        println!("Updated: {path_unsigned:?} and {path_signed:?}");
    } else {
        let expected_unsigned = fs::read(&path_unsigned).expect("golden vector missing");
        let expected_signed = fs::read(&path_signed).expect("golden vector missing");
        assert_eq!(
            bytes_unsigned, expected_unsigned,
            "Timeout unsigned encoding drifted!"
        );
        assert_eq!(
            bytes_signed, expected_signed,
            "Timeout signed encoding drifted!"
        );
    }
}

#[test]
fn golden_qc_with_votes() {
    let vote_a = Vote {
        height: 10,
        round: 2,
        block_hash: [0x99; 32],
        voter: [0xaa; 32],
        signature: [0x11; 64],
    };
    let vote_b = Vote {
        height: 10,
        round: 2,
        block_hash: [0x99; 32],
        voter: [0xbb; 32],
        signature: [0x22; 64],
    };
    let vote_c = Vote {
        height: 10,
        round: 2,
        block_hash: [0x99; 32],
        voter: [0xcc; 32],
        signature: [0x33; 64],
    };

    let qc = QC {
        height: 10,
        round: 2,
        block_hash: [0x99; 32],
        votes: vec![vote_a, vote_b, vote_c],
    };

    let bytes = encode_qc_v1(&qc).unwrap();
    let path = vectors_dir().join("qc_with_votes_v1.bin");

    if should_update_vectors() {
        fs::write(&path, &bytes).unwrap();
        println!("Updated: {path:?}");
    } else {
        let expected = fs::read(&path).expect("golden vector missing");
        assert_eq!(bytes, expected, "QC (with votes) encoding drifted!");
    }
}

#[test]
fn golden_block_with_tx() {
    // Deterministic tx for golden vector (no random keypair)
    let tx = novai_types::TxV1 {
        version: novai_types::TxVersion::V1,
        from: [0xAA; 32],
        pubkey: [0xBB; 32],
        nonce: 0,
        fee: 10,
        payload: b"test".to_vec(),
        sig: [0xCC; 64],
    };

    let block = Block {
        height: 5,
        round: 1,
        parent_hash: [0xcc; 32],
        state_root: [0xdd; 32],
        txs: vec![tx],
    };

    let bytes = encode_block_v1(&block).unwrap();
    let path = vectors_dir().join("block_with_tx_v1.bin");

    if should_update_vectors() {
        fs::write(&path, &bytes).unwrap();
        println!("Updated: {path:?}");
    } else {
        let expected = fs::read(&path).expect("golden vector missing");
        assert_eq!(bytes, expected, "Block (with tx) encoding drifted!");
    }
}

#[test]
fn golden_proposal_v1() {
    let block = Block {
        height: 10,
        round: 3,
        parent_hash: [0x11; 32],
        state_root: [0x22; 32],
        txs: vec![],
    };

    let qc = QC {
        height: 9,
        round: 2,
        block_hash: [0x11; 32],
        votes: vec![],
    };

    let proposal = novai_consensus_types::Proposal {
        block,
        justify_qc: qc,
    };

    let bytes = encode_proposal_v1(&proposal).unwrap();
    let path = vectors_dir().join("proposal_v1.bin");

    if should_update_vectors() {
        fs::write(&path, &bytes).unwrap();
        println!("Updated: {path:?}");
    } else {
        let expected = fs::read(&path).expect("golden vector missing");
        assert_eq!(bytes, expected, "Proposal encoding drifted!");
    }
}

#[test]
fn golden_timeout_with_qc() {
    let qc = QC {
        height: 5,
        round: 2,
        block_hash: [0xaa; 32],
        votes: vec![],
    };

    let timeout = Timeout {
        height: 6,
        round: 3,
        voter: [0xbb; 32],
        highest_qc: Some(qc),
        signature: [0xcc; 64],
    };

    let bytes_unsigned = encode_timeout_v1_unsigned(&timeout).unwrap();
    let bytes_signed = encode_timeout_v1_signed(&timeout).unwrap();

    let path_unsigned = vectors_dir().join("timeout_with_qc_unsigned_v1.bin");
    let path_signed = vectors_dir().join("timeout_with_qc_signed_v1.bin");

    if should_update_vectors() {
        fs::write(&path_unsigned, &bytes_unsigned).unwrap();
        fs::write(&path_signed, &bytes_signed).unwrap();
        println!("Updated: {path_unsigned:?} and {path_signed:?}");
    } else {
        let expected_unsigned = fs::read(&path_unsigned).expect("golden vector missing");
        let expected_signed = fs::read(&path_signed).expect("golden vector missing");
        assert_eq!(
            bytes_unsigned, expected_unsigned,
            "Timeout (with QC) unsigned encoding drifted!"
        );
        assert_eq!(
            bytes_signed, expected_signed,
            "Timeout (with QC) signed encoding drifted!"
        );
    }
}

#[test]
fn golden_proposal_unsigned() {
    let block = Block {
        height: 5,
        round: 2,
        parent_hash: [0xaa; 32],
        state_root: [0xbb; 32],
        txs: vec![],
    };

    let qc = QC {
        height: 4,
        round: 1,
        block_hash: [0xaa; 32],
        votes: vec![],
    };

    let proposal = novai_consensus_types::Proposal {
        block,
        justify_qc: qc,
    };

    let bytes = encode_proposal_v1_unsigned(&proposal).unwrap();
    let path = vectors_dir().join("proposal_unsigned_v1.bin");

    if should_update_vectors() {
        fs::write(&path, &bytes).unwrap();
        println!("Updated: {path:?}");
    } else {
        let expected = fs::read(&path).expect("golden vector missing");
        assert_eq!(bytes, expected, "Proposal unsigned encoding drifted!");
    }
}

#[test]
fn golden_signed_proposal() {
    use novai_consensus_types::SignedProposal;

    let block = Block {
        height: 5,
        round: 2,
        parent_hash: [0xaa; 32],
        state_root: [0xbb; 32],
        txs: vec![],
    };

    let qc = QC {
        height: 4,
        round: 1,
        block_hash: [0xaa; 32],
        votes: vec![],
    };

    let signed_proposal = SignedProposal {
        proposer: [0xcc; 32],
        proposal: novai_consensus_types::Proposal {
            block,
            justify_qc: qc,
        },
        signature: [0xdd; 64],
    };

    let bytes = encode_signed_proposal_v1(&signed_proposal).unwrap();
    let path = vectors_dir().join("signed_proposal_v1.bin");

    if should_update_vectors() {
        fs::write(&path, &bytes).unwrap();
        println!("Updated: {path:?}");
    } else {
        let expected = fs::read(&path).expect("golden vector missing");
        assert_eq!(bytes, expected, "Signed proposal encoding drifted!");
    }
}
