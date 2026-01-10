//! Integration test: 4-node deterministic consensus simulation.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::ConsensusState;
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

/// Simulated node with all consensus state.
struct SimNode {
    id: usize,
    #[allow(dead_code)]
    address: Address,
    signing_key: SigningKey,
    state: ConsensusState,
    mempool: mempool::TxMempool,
    db: MemKv,
}

impl SimNode {
    fn new(id: usize, address: Address, signing_key: SigningKey) -> Self {
        Self {
            id,
            address,
            signing_key,
            state: ConsensusState::new(address),
            mempool: mempool::TxMempool::new(1, 100),
            db: MemKv::new(),
        }
    }
}

#[test]
fn four_node_consensus_simulation() {
    // Setup 4 nodes
    let mut nodes = Vec::new();
    let mut validator_addresses = Vec::new();
    let mut validator_pubkeys: Vec<(Address, VerifyingKey)> = Vec::new();

    for id in 0..4 {
        let sk = SigningKey::generate(&mut OsRng);
        let pk = sk.verifying_key();
        let addr = address_from_pubkey(&pk);

        validator_addresses.push(addr);
        validator_pubkeys.push((addr, pk));
        nodes.push(SimNode::new(id, addr, sk));
    }

    // Node 0 is leader at height=0, round=0
    let nonce_provider = TestNonceProvider;

    // Step 1: Leader proposes
    let block = {
        let leader = &mut nodes[0];
        leader
            .state
            .propose_block(
                &mut leader.mempool,
                &nonce_provider,
                &leader.db,
                &validator_addresses,
            )
            .unwrap()
    };

    println!(
        "Node 0 proposed block at height={} round={}",
        block.height, block.round
    );

    // Step 2: All nodes verify block
    for node in &nodes {
        node.state.verify_block(&block, &node.db).unwrap();
    }

    // Step 3: All nodes vote
    let mut votes = Vec::new();
    for node in &nodes {
        let vote = node.state.create_vote(&block, &node.signing_key).unwrap();
        votes.push(vote);
    }

    // Step 4: Broadcast votes to all nodes (deterministic order)
    for vote in votes {
        for node in &mut nodes {
            node.state
                .add_vote(vote.clone(), &validator_pubkeys)
                .unwrap();
        }
    }

    // Step 5: All nodes form QC
    let block_hash = novai_consensus_types::codec::hash_block_v1(&block).unwrap();

    let mut qcs = Vec::new();
    for node in &mut nodes {
        let qc = node
            .state
            .try_form_qc(&block_hash, &validator_addresses)
            .unwrap();
        assert!(qc.is_some(), "Node {} failed to form QC", node.id);
        qcs.push(qc.unwrap());
    }

    // Step 6: Verify all nodes have same QC
    for i in 1..qcs.len() {
        assert_eq!(qcs[i].height, qcs[0].height);
        assert_eq!(qcs[i].round, qcs[0].round);
        assert_eq!(qcs[i].block_hash, qcs[0].block_hash);
        assert_eq!(qcs[i].votes.len(), qcs[0].votes.len());
    }

    println!(
        "✅ All 4 nodes formed identical QC with {} votes",
        qcs[0].votes.len()
    );

    // Step 7: Verify consensus state consistency
    for node in &nodes {
        assert_eq!(node.state.height, 0);
        assert_eq!(node.state.round, 0);
    }

    println!(
        "✅ All nodes in same (height={}, round={}) state",
        nodes[0].state.height, nodes[0].state.round
    );
}
