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

    assert_eq!(block.height, 0);
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
    assert_eq!(qc.height, 0);
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
