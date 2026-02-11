use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::{ConsensusError, ConsensusState};
use novai_crypto::address_from_pubkey;
use novai_state::MemKv;
use novai_types::Address;
use rand_core::OsRng;

// Simple test nonce provider
#[derive(Default)]
struct TestNonceProvider;

impl mempool::NonceProvider for TestNonceProvider {
    fn expected_nonce(&self, _from: &Address) -> u64 {
        0
    }
}

#[test]
fn leader_proposes_block() {
    // Setup
    let sk = SigningKey::generate(&mut OsRng);
    let pk = sk.verifying_key();
    let addr = address_from_pubkey(&pk);

    let validator_set = vec![addr];
    let mut state = ConsensusState::new(addr);
    let mut mempool = mempool::TxMempool::new(1, 100);
    let nonce_provider = TestNonceProvider;
    let db = MemKv::new();

    // Leader proposes
    let block = state
        .propose_block(&mut mempool, &nonce_provider, &db, &validator_set)
        .unwrap();

    assert_eq!(block.height, 1); // Proposals are for next height (self.height + 1)
    assert_eq!(block.round, 0);
    assert_eq!(block.parent_hash, [0u8; 32]); // Genesis
}

#[test]
fn non_leader_cannot_propose() {
    // Setup two validators
    let sk1 = SigningKey::generate(&mut OsRng);
    let pk1 = sk1.verifying_key();
    let addr1 = address_from_pubkey(&pk1);

    let sk2 = SigningKey::generate(&mut OsRng);
    let pk2 = sk2.verifying_key();
    let addr2 = address_from_pubkey(&pk2);

    let validator_set = vec![addr1, addr2];

    // Node 2 tries to propose at height 0 (but leader is addr1)
    let mut state = ConsensusState::new(addr2);
    let mut mempool = mempool::TxMempool::new(1, 100);
    let nonce_provider = TestNonceProvider;
    let db = MemKv::new();

    let result = state.propose_block(&mut mempool, &nonce_provider, &db, &validator_set);

    assert!(matches!(result, Err(ConsensusError::NotLeader)));
}

#[test]
fn vote_and_form_qc() {
    // Setup 4 validators with keys
    let mut validators: Vec<Address> = Vec::new();
    let mut validator_pubkeys: Vec<(Address, VerifyingKey)> = Vec::new();
    let mut keys: Vec<SigningKey> = Vec::new();

    for _ in 0..4 {
        let sk = SigningKey::generate(&mut OsRng);
        let pk = sk.verifying_key();
        let addr = address_from_pubkey(&pk);
        validators.push(addr);
        validator_pubkeys.push((addr, pk));
        keys.push(sk);
    }

    // Node 0 proposes
    let mut state0 = ConsensusState::new(validators[0]);
    let mut mempool = mempool::TxMempool::new(1, 100);
    let nonce_provider = TestNonceProvider;
    let db = MemKv::new();

    let block = state0
        .propose_block(&mut mempool, &nonce_provider, &db, &validators)
        .unwrap();

    // All 4 validators vote
    for i in 0..4 {
        let state = ConsensusState::new(validators[i]);
        let vote = state.create_vote(&block, &keys[i]).unwrap();

        // Add vote to node 0's state
        state0.add_vote(vote, &validator_pubkeys).unwrap();
    }

    // Node 0 forms QC
    let block_hash = novai_consensus_types::codec::hash_block_v1(&block).unwrap();
    let qc = state0.try_form_qc(&block_hash, &validators).unwrap();

    assert!(qc.is_some());
    let qc = qc.unwrap();
    assert_eq!(qc.height, 1); // QC is for the block's height (self.height + 1)
    assert_eq!(qc.round, 0);
    assert_eq!(qc.votes.len(), 3); // Quorum = 2f+1 = 3 for n=4
}

#[test]
fn equivocation_detected() {
    // Setup
    let sk = SigningKey::generate(&mut OsRng);
    let pk = sk.verifying_key();
    let addr = address_from_pubkey(&pk);

    let validator_pubkeys = vec![(addr, pk)];
    let validator_set = vec![addr];

    let mut state = ConsensusState::new(addr);
    let mut mempool = mempool::TxMempool::new(1, 100);
    let nonce_provider = TestNonceProvider;
    let db = MemKv::new();

    let block = state
        .propose_block(&mut mempool, &nonce_provider, &db, &validator_set)
        .unwrap();

    // Vote once
    let vote1 = state.create_vote(&block, &sk).unwrap();
    state.add_vote(vote1.clone(), &validator_pubkeys).unwrap();

    // Try to vote again (equivocation)
    let result = state.add_vote(vote1, &validator_pubkeys);

    assert!(matches!(result, Err(ConsensusError::InvalidVote(_))));
}
#[test]
fn leader_computation_is_consistent() {
    // Setup 4 validators
    let mut validators: Vec<Address> = Vec::new();
    for _ in 0..4 {
        let sk = SigningKey::generate(&mut OsRng);
        let pk = sk.verifying_key();
        let addr = address_from_pubkey(&pk);
        validators.push(addr);
    }

    // Test that leader computation gives same result for same inputs
    for height in 0..10 {
        for round in 0..5 {
            let leader1 =
                ConsensusState::compute_leader_for_view(height, round, &validators).unwrap();
            let leader2 =
                ConsensusState::compute_leader_for_view(height, round, &validators).unwrap();
            assert_eq!(leader1, leader2, "Leader computation must be deterministic");

            // Verify it's actually in the validator set
            assert!(
                validators.contains(&leader1),
                "Leader must be from validator set"
            );
        }
    }
}

#[test]
fn leader_rotates_with_height_and_round() {
    // Setup 4 validators
    let mut validators: Vec<Address> = Vec::new();
    for _ in 0..4 {
        let sk = SigningKey::generate(&mut OsRng);
        let pk = sk.verifying_key();
        let addr = address_from_pubkey(&pk);
        validators.push(addr);
    }

    // Test that leader changes as height/round advance
    let leader_h0_r0 = ConsensusState::compute_leader_for_view(0, 0, &validators).unwrap();
    let leader_h1_r0 = ConsensusState::compute_leader_for_view(1, 0, &validators).unwrap();
    let leader_h0_r1 = ConsensusState::compute_leader_for_view(0, 1, &validators).unwrap();

    // Leader should rotate (with 4 validators, they should be different)
    assert_ne!(
        leader_h0_r0, leader_h1_r0,
        "Leader should change with height"
    );
    assert_ne!(
        leader_h0_r0, leader_h0_r1,
        "Leader should change with round"
    );
}
#[test]
fn duplicate_vote_rejected() {
    // Setup
    let sk = SigningKey::generate(&mut OsRng);
    let pk = sk.verifying_key();
    let addr = address_from_pubkey(&pk);

    let validator_pubkeys = vec![(addr, pk)];
    let validator_set = vec![addr];

    let mut state = ConsensusState::new(addr);
    let mut mempool = mempool::TxMempool::new(1, 100);
    let nonce_provider = TestNonceProvider;
    let db = MemKv::new();

    let block = state
        .propose_block(&mut mempool, &nonce_provider, &db, &validator_set)
        .unwrap();

    // Vote once
    let vote = state.create_vote(&block, &sk).unwrap();
    state.add_vote(vote.clone(), &validator_pubkeys).unwrap();

    // Try to vote again with same vote (replay attack)
    let result = state.add_vote(vote, &validator_pubkeys);

    // Should be rejected as duplicate
    assert!(matches!(result, Err(ConsensusError::InvalidVote(_))));
}

// ========== WEEK 7: COMMIT RULE TESTS ==========

use novai_consensus_types::codec::hash_block_v1;

/// Helper to create a block with proper parent hash linkage.
fn make_block(height: u64, parent_hash: [u8; 32]) -> novai_consensus_types::Block {
    novai_consensus_types::Block {
        height,
        round: 0,
        parent_hash,
        state_root: [height as u8; 32],
        txs: vec![],
    }
}

#[test]
fn commit_rule_3_chain() {
    // Setup
    let sk = SigningKey::generate(&mut OsRng);
    let pk = sk.verifying_key();
    let addr = address_from_pubkey(&pk);

    let mut state = ConsensusState::new(addr);
    let db = MemKv::new();

    // Create properly linked chain: genesis -> block1 -> block2 -> block3
    let genesis_hash = [0u8; 32]; // Genesis parent

    let block1 = make_block(1, genesis_hash);
    let hash1 = hash_block_v1(&block1).unwrap();

    let block2 = make_block(2, hash1);
    let hash2 = hash_block_v1(&block2).unwrap();

    let block3 = make_block(3, hash2);
    let hash3 = hash_block_v1(&block3).unwrap();

    // Cache blocks
    state.cache_block(block1.clone());
    state.cache_block(block2.clone());
    state.cache_block(block3.clone());

    // QC at height 1 - no commits yet (need height >= 2 for 3-chain)
    let qc1 = novai_consensus_types::QC {
        height: 1,
        round: 0,
        block_hash: hash1,
        votes: vec![],
    };
    let to_commit = state.cache_qc_and_check_commit(qc1, &db).unwrap();
    assert!(
        to_commit.is_empty(),
        "QC at height 1 should not trigger commit"
    );

    // QC at height 2 - no commits yet (would commit height 0, but nothing there)
    let qc2 = novai_consensus_types::QC {
        height: 2,
        round: 0,
        block_hash: hash2,
        votes: vec![],
    };
    let to_commit = state.cache_qc_and_check_commit(qc2, &db).unwrap();
    assert!(
        to_commit.is_empty(),
        "QC at height 2 should not trigger commit (commit_target=0, nothing to commit)"
    );

    // QC at height 3 - commits block at height 1!
    let qc3 = novai_consensus_types::QC {
        height: 3,
        round: 0,
        block_hash: hash3,
        votes: vec![],
    };
    let to_commit = state.cache_qc_and_check_commit(qc3, &db).unwrap();
    assert_eq!(to_commit.len(), 1, "QC at height 3 should commit block 1");
    assert_eq!(to_commit[0].height, 1);

    // Apply commits
    state.apply_commits(&to_commit);
    assert_eq!(state.committed_height(), 1);

    // Create block 4 linked to block 3
    let block4 = make_block(4, hash3);
    let hash4 = hash_block_v1(&block4).unwrap();
    state.cache_block(block4);

    // QC at height 4 - commits block at height 2
    let qc4 = novai_consensus_types::QC {
        height: 4,
        round: 0,
        block_hash: hash4,
        votes: vec![],
    };
    let to_commit = state.cache_qc_and_check_commit(qc4, &db).unwrap();
    assert_eq!(to_commit.len(), 1, "QC at height 4 should commit block 2");
    assert_eq!(to_commit[0].height, 2);

    state.apply_commits(&to_commit);
    assert_eq!(state.committed_height(), 2);
}

#[test]
fn commit_rule_batch_commits() {
    // Test that if we receive QC at height 5 but committed_height is 0,
    // we commit blocks 1, 2, 3 in order (all blocks up to height 5-2=3)
    let sk = SigningKey::generate(&mut OsRng);
    let pk = sk.verifying_key();
    let addr = address_from_pubkey(&pk);

    let mut state = ConsensusState::new(addr);
    let db = MemKv::new();

    // Create properly linked chain: genesis -> 1 -> 2 -> 3 -> 4 -> 5
    let genesis_hash = [0u8; 32];

    let block1 = make_block(1, genesis_hash);
    let hash1 = hash_block_v1(&block1).unwrap();

    let block2 = make_block(2, hash1);
    let hash2 = hash_block_v1(&block2).unwrap();

    let block3 = make_block(3, hash2);
    let hash3 = hash_block_v1(&block3).unwrap();

    let block4 = make_block(4, hash3);
    let hash4 = hash_block_v1(&block4).unwrap();

    let block5 = make_block(5, hash4);
    let hash5 = hash_block_v1(&block5).unwrap();

    // Cache all blocks
    state.cache_block(block1);
    state.cache_block(block2);
    state.cache_block(block3);
    state.cache_block(block4);
    state.cache_block(block5);

    // QC at height 5 should commit blocks 1, 2, 3 (heights <= 5-2=3)
    let qc5 = novai_consensus_types::QC {
        height: 5,
        round: 0,
        block_hash: hash5,
        votes: vec![],
    };
    let to_commit = state.cache_qc_and_check_commit(qc5, &db).unwrap();

    assert_eq!(to_commit.len(), 3, "Should commit 3 blocks");
    assert_eq!(to_commit[0].height, 1);
    assert_eq!(to_commit[1].height, 2);
    assert_eq!(to_commit[2].height, 3);

    state.apply_commits(&to_commit);
    assert_eq!(state.committed_height(), 3);
}

#[test]
fn highest_qc_updated() {
    let sk = SigningKey::generate(&mut OsRng);
    let pk = sk.verifying_key();
    let addr = address_from_pubkey(&pk);

    let mut state = ConsensusState::new(addr);
    let db = MemKv::new();

    // Create blocks for QC references
    let block1 = make_block(1, [0u8; 32]);
    let hash1 = hash_block_v1(&block1).unwrap();
    state.cache_block(block1);

    let block2 = make_block(2, hash1);
    let hash2 = hash_block_v1(&block2).unwrap();
    state.cache_block(block2);

    let block3 = make_block(3, hash2);
    let hash3 = hash_block_v1(&block3).unwrap();
    state.cache_block(block3);

    // Initially no highest QC
    assert!(state.highest_qc.is_none());

    // Add QC at height 2
    let qc2 = novai_consensus_types::QC {
        height: 2,
        round: 0,
        block_hash: hash2,
        votes: vec![],
    };
    let _ = state.cache_qc_and_check_commit(qc2.clone(), &db);
    assert_eq!(state.highest_qc.as_ref().unwrap().height, 2);

    // Add QC at height 1 (lower) - should NOT update
    let qc1 = novai_consensus_types::QC {
        height: 1,
        round: 0,
        block_hash: hash1,
        votes: vec![],
    };
    let _ = state.cache_qc_and_check_commit(qc1, &db);
    assert_eq!(
        state.highest_qc.as_ref().unwrap().height,
        2,
        "Lower QC should not update highest"
    );

    // Add QC at height 3 (higher) - should update
    let qc3 = novai_consensus_types::QC {
        height: 3,
        round: 0,
        block_hash: hash3,
        votes: vec![],
    };
    let _ = state.cache_qc_and_check_commit(qc3, &db);
    assert_eq!(state.highest_qc.as_ref().unwrap().height, 3);
}

#[test]
fn commit_fails_on_missing_block() {
    // Fix D: Verify contiguous commit enforcement
    let sk = SigningKey::generate(&mut OsRng);
    let pk = sk.verifying_key();
    let addr = address_from_pubkey(&pk);

    let mut state = ConsensusState::new(addr);
    let db = MemKv::new();

    // Create chain but DON'T cache block 2 (creating a gap)
    let block1 = make_block(1, [0u8; 32]);
    let hash1 = hash_block_v1(&block1).unwrap();

    let block2 = make_block(2, hash1);
    let hash2 = hash_block_v1(&block2).unwrap();

    let block3 = make_block(3, hash2);
    let hash3 = hash_block_v1(&block3).unwrap();

    // Only cache block 1 and 3, skip block 2
    state.cache_block(block1);
    state.cache_block(block3);

    // QC at height 3 should fail because block 2 is missing from chain
    let qc3 = novai_consensus_types::QC {
        height: 3,
        round: 0,
        block_hash: hash3,
        votes: vec![],
    };

    let result = state.cache_qc_and_check_commit(qc3, &db);
    assert!(result.is_err(), "Should fail when chain has missing blocks");
}

#[test]
fn commit_fails_on_wrong_block_hash_in_qc() {
    // Verify chain linkage: QC must reference actual block hash
    let sk = SigningKey::generate(&mut OsRng);
    let pk = sk.verifying_key();
    let addr = address_from_pubkey(&pk);

    let mut state = ConsensusState::new(addr);
    let db = MemKv::new();

    let block1 = make_block(1, [0u8; 32]);
    let hash1 = hash_block_v1(&block1).unwrap();

    let block2 = make_block(2, hash1);
    let hash2 = hash_block_v1(&block2).unwrap();

    let block3 = make_block(3, hash2);
    // Don't compute hash3, use a fake one

    state.cache_block(block1);
    state.cache_block(block2);
    state.cache_block(block3);

    // QC with wrong block_hash (doesn't match any cached block)
    let qc3 = novai_consensus_types::QC {
        height: 3,
        round: 0,
        block_hash: [0xFF; 32], // Fake hash
        votes: vec![],
    };

    let result = state.cache_qc_and_check_commit(qc3, &db);
    assert!(
        result.is_err(),
        "Should fail when QC references unknown block"
    );
}

#[test]
fn persistence_roundtrip() {
    let sk = SigningKey::generate(&mut OsRng);
    let pk = sk.verifying_key();
    let addr = address_from_pubkey(&pk);

    let mut db = novai_state::MemKv::new();

    // Create a block
    let block = make_block(5, [0u8; 32]);
    let block_hash = hash_block_v1(&block).unwrap();

    // Create a QC
    let qc = novai_consensus_types::QC {
        height: 5,
        round: 0,
        block_hash,
        votes: vec![],
    };

    let state = ConsensusState::new(addr);

    // Persist block
    state.persist_block(&mut db, &block).unwrap();

    // Load block
    let loaded_block = ConsensusState::load_block(&db, 5).unwrap();
    assert!(loaded_block.is_some());
    assert_eq!(loaded_block.unwrap().height, 5);

    // Persist and load committed height
    let mut state2 = ConsensusState::new(addr);
    state2.committed_height = 5;
    state2.persist_committed_height(&mut db).unwrap();

    let loaded_height = ConsensusState::load_committed_height(&db).unwrap();
    assert_eq!(loaded_height, 5);

    // Persist and load highest QC
    state2.highest_qc = Some(qc.clone());
    state2.persist_highest_qc(&mut db).unwrap();

    let loaded_qc = ConsensusState::load_highest_qc(&db).unwrap();
    assert!(loaded_qc.is_some());
    assert_eq!(loaded_qc.unwrap().height, 5);
}

#[test]
fn recovery_after_restart() {
    let sk = SigningKey::generate(&mut OsRng);
    let pk = sk.verifying_key();
    let addr = address_from_pubkey(&pk);

    let mut db = novai_state::MemKv::new();

    // Simulate previous run: set committed_height=10, highest_qc at height 12
    let mut old_state = ConsensusState::new(addr);
    old_state.committed_height = 10;

    let block12 = make_block(12, [0u8; 32]);
    let hash12 = hash_block_v1(&block12).unwrap();

    old_state.highest_qc = Some(novai_consensus_types::QC {
        height: 12,
        round: 0,
        block_hash: hash12,
        votes: vec![],
    });

    // Persist state
    old_state.persist_committed_height(&mut db).unwrap();
    old_state.persist_highest_qc(&mut db).unwrap();

    // Recover (simulating restart)
    let recovered = ConsensusState::recover(addr, &db).unwrap();

    assert_eq!(recovered.committed_height, 10);
    assert_eq!(recovered.height, 10); // height = committed_height after recovery
    assert!(recovered.highest_qc.is_some());
    assert_eq!(recovered.highest_qc.unwrap().height, 12);
}
