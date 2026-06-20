//! Phase 1 RED tests (gate-syncpath-commit-safety).
//!
//! The sync commit path must finalize a block ONLY on a 3-chain (a QC two
//! heights above that chains back to the block via parent pointers), exactly
//! as the live path does in cache_qc_and_check_commit, never on a single
//! certifying QC (a 1-chain). A block certified in one round but abandoned by
//! a same-height view change carries a valid QC yet has no canonical
//! descendant, so no 3-chain ever reaches it. The live path refuses it; the
//! sync certified-prefix loop in handle_block_response finalizes it on a
//! 1-chain. That is the node1 height 1,460,002 divergence.
//!
//! On current HEAD BOTH tests FAIL: HEAD over-commits via the 1-chain loop.
//! Phase 2 (the fix) makes both pass by driving sync finality through the
//! same 3-chain rule. These tests are added, not committed.

use ed25519_dalek::SigningKey;
use novai_consensus_types::{Block, BlockResponse, Vote, QC};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::ConsensusNode;
use novai_state::Kv;
use novai_types::Address;
use rand_core::OsRng;
use std::collections::HashMap;

// Helpers mirrored from crates/node/tests/sync_test.rs (each integration test
// file is its own crate, so the helpers are replicated, not shared).

/// A domain-separated, validly signed vote.
fn signed_vote(
    signer: &SigningKey,
    voter: Address,
    height: u64,
    round: u64,
    block_hash: [u8; 32],
) -> Vote {
    let unsigned = Vote {
        height,
        round,
        block_hash,
        voter,
        signature: [0u8; 64],
        ai_signal_commitment: None,
    };
    let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned);
    let mut to_sign = Vec::new();
    to_sign.extend_from_slice(b"NOVAI_VOTE_V1");
    to_sign.extend_from_slice(&unsigned_bytes);
    let signature = novai_crypto::sign_bytes(signer, &to_sign);
    Vote {
        signature,
        ..unsigned
    }
}

/// A single-vote QC that certifies `block` (quorum is 1 for a 2-validator set).
fn certifying_qc(signer: &SigningKey, voter: Address, block: &Block) -> QC {
    let block_hash = novai_consensus_types::block_hash(block);
    QC {
        height: block.height,
        round: block.round,
        block_hash,
        votes: vec![signed_vote(signer, voter, block.height, block.round, block_hash)],
    }
}

/// A 2-validator receiver (node2) behind at committed_height 0, plus the addr1
/// signing key to mint certifying QCs. Mirrors a2_receiver_fixture.
fn receiver() -> (ConsensusNode, SigningKey, Address) {
    let sk1 = SigningKey::generate(&mut OsRng);
    let sk2 = SigningKey::generate(&mut OsRng);
    let pk1 = sk1.verifying_key();
    let pk2 = sk2.verifying_key();
    let addr1 = address_from_pubkey(&pk1);
    let addr2 = address_from_pubkey(&pk2);
    let validator_set = vec![addr1, addr2];
    let mut validator_pubkeys = HashMap::new();
    validator_pubkeys.insert(addr1, pk1);
    validator_pubkeys.insert(addr2, pk2);
    let node2 = ConsensusNode::new(sk2, validator_set, validator_pubkeys, 1000);
    (node2, sk1, addr1)
}

/// Build a contiguous chain of `n` blocks at heights 1..=n with matching
/// parent hashes. Height 1 carries state_root [0xaa] so the receiver's SMT
/// root can be set to match the existing C-01 state-root check.
fn chain(n: u64) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut parent = [0u8; 32];
    for h in 1..=n {
        let block = Block {
            height: h,
            round: 0,
            parent_hash: parent,
            state_root: if h == 1 { [0xaa; 32] } else { [h as u8; 32] },
            txs: vec![],
        };
        parent = novai_consensus_types::block_hash(&block);
        blocks.push(block);
    }
    blocks
}

/// Set the receiver's committed SMT root so the C-01 state-root check on the
/// first synced block passes and execution reaches the commit logic.
fn set_smt_root(node: &ConsensusNode, root: [u8; 32]) {
    let mut db = node.db.lock().unwrap();
    db.put(
        novai_state::KEY_SMT_ROOT,
        &novai_state::encode_smt_root_v1(&root),
    )
    .unwrap();
}

/// RED on HEAD. The abandoned-fork case, minimal form. A peer serves blocks
/// at heights 1 and 2, each with a valid certifying QC, but NO QC at height 3
/// or 4. Neither block has a 3-chain, so neither is final. The height-2 block
/// here stands for the abandoned round-0 block at node1's 1,460,002: a block
/// that earned a QC but, having no canonical descendant, can never gather a
/// QC two heights above. The sync path must REFUSE to finalize it
/// (committed_height stays 0). On HEAD the 1-chain certified-prefix loop
/// finalizes both (committed_height advances to 2), reproducing the divergence.
#[test]
fn sync_does_not_finalize_without_three_chain() {
    let (node2, sk1, addr1) = receiver();
    let blocks = chain(2); // heights 1, 2; height 2 is the abandoned tip
    set_smt_root(&node2, blocks[0].state_root);
    let qc1 = certifying_qc(&sk1, addr1, &blocks[0]);
    let qc2 = certifying_qc(&sk1, addr1, &blocks[1]);
    let response = BlockResponse {
        responder: addr1,
        request_start: 1,
        request_end: 2,
        blocks: blocks.clone(),
        qcs: vec![Some(qc1), Some(qc2)],
    };

    node2.handle_block_response(response).unwrap();

    let committed = node2.state.lock().unwrap().committed_height;
    assert_eq!(
        committed, 0,
        "1-chain finalization defect: blocks at heights 1 and 2 carry a valid \
         certifying QC but have no QC at height 3 or 4 (no 3-chain), so neither \
         is final and the sync path must defer (committed_height must stay 0). \
         Got committed_height={committed}. On HEAD the certified-prefix loop \
         finalizes them on a single QC, the node1 1,460,002 divergence."
    );
}

/// RED on HEAD. The catch-up guarantee. A genuinely-behind node re-syncs
/// canonical history up to the 3-chain safety margin. With QCs up to height 4,
/// heights 1 and 2 each have a QC two heights above (heights 3 and 4) that
/// chains back to them, so they finalize; heights 3 and 4 are within the
/// 2-block margin of the top QC and must defer until their descendants arrive.
/// Expected committed_height is 2. On HEAD the 1-chain loop over-commits to 4.
/// This proves the fix does not break legitimate catch-up.
#[test]
fn sync_catches_up_to_three_chain_margin() {
    let (node2, sk1, addr1) = receiver();
    let blocks = chain(4); // heights 1..=4, all canonical
    set_smt_root(&node2, blocks[0].state_root);
    let qcs: Vec<Option<QC>> = blocks
        .iter()
        .map(|b| Some(certifying_qc(&sk1, addr1, b)))
        .collect();
    let response = BlockResponse {
        responder: addr1,
        request_start: 1,
        request_end: 4,
        blocks: blocks.clone(),
        qcs,
    };

    node2.handle_block_response(response).unwrap();

    let committed = node2.state.lock().unwrap().committed_height;
    assert_eq!(
        committed, 2,
        "3-chain catch-up: with QCs up to height 4, heights 1 and 2 have a QC \
         two heights above and must finalize, while heights 3 and 4 sit within \
         the 2-block safety margin of the top QC and must defer. Expected \
         committed_height=2, got {committed}. On HEAD the 1-chain loop \
         over-commits to 4."
    );
}
