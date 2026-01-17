//! Integration test: Block sync protocol

use ed25519_dalek::SigningKey;
use novai_consensus::ConsensusState;
use novai_consensus_types::{Block, BlockRequest, BlockResponse};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::ConsensusNode;
use novai_state::Kv;
use rand_core::OsRng;
use std::collections::HashMap;

/// Test BlockRequest/BlockResponse roundtrip through node methods.
#[test]
fn test_block_request_response_roundtrip() {
    // Create two validator nodes
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

    let node1 = ConsensusNode::new(sk1, validator_set.clone(), validator_pubkeys.clone());
    let _node2 = ConsensusNode::new(sk2, validator_set, validator_pubkeys);

    // Manually store some blocks in node1's DB
    let block1 = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32],
        state_root: [0xaa; 32],
        txs: vec![],
    };

    let block2 = Block {
        height: 2,
        round: 0,
        parent_hash: novai_consensus_types::block_hash(&block1),
        state_root: [0xbb; 32],
        txs: vec![],
    };

    // Store blocks in node1's DB
    {
        let mut db1 = node1.db.lock().unwrap();
        let key1 = novai_state::block_key(1);
        let value1 = novai_consensus_types::codec::encode_block_v1(&block1).unwrap();
        db1.put(&key1, &value1).unwrap();

        let key2 = novai_state::block_key(2);
        let value2 = novai_consensus_types::codec::encode_block_v1(&block2).unwrap();
        db1.put(&key2, &value2).unwrap();
    }

    // Node2 sends a BlockRequest to node1
    let request = BlockRequest {
        requester: addr2,
        start_height: 1,
        end_height: 2,
    };

    // Node1 handles the request (this would normally broadcast the response)
    // For this test, we'll just verify it doesn't error
    let result = node1.handle_block_request(request);
    assert!(
        result.is_ok(),
        "Block request handling failed: {:?}",
        result
    );

    println!("✅ Block request/response roundtrip succeeded");
}

/// Test that a node can sync from a peer after restarting behind.
#[test]
fn test_sync_from_peer_on_restart() {
    // Create two validator nodes
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

    let node1 = ConsensusNode::new(sk1, validator_set.clone(), validator_pubkeys.clone());
    let node2 = ConsensusNode::new(sk2, validator_set, validator_pubkeys);

    // Simulate node1 being ahead with committed blocks
    let block1 = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32],
        state_root: [0xaa; 32],
        txs: vec![],
    };

    let block2 = Block {
        height: 2,
        round: 0,
        parent_hash: novai_consensus_types::block_hash(&block1),
        state_root: [0xbb; 32],
        txs: vec![],
    };

    // Store blocks in node1
    {
        let mut db1 = node1.db.lock().unwrap();
        let key1 = novai_state::block_key(1);
        let value1 = novai_consensus_types::codec::encode_block_v1(&block1).unwrap();
        db1.put(&key1, &value1).unwrap();

        let key2 = novai_state::block_key(2);
        let value2 = novai_consensus_types::codec::encode_block_v1(&block2).unwrap();
        db1.put(&key2, &value2).unwrap();

        // Update node1's committed height
        let mut state1 = node1.state.lock().unwrap();
        state1.committed_height = 2;
    }

    // Node2 is behind (committed_height = 0)
    // Node2 requests blocks from node1
    let result = node2.request_blocks_from_peer(1, 2);
    assert!(result.is_ok(), "Block request failed: {:?}", result);

    // Simulate node1 responding with blocks
    let response = BlockResponse {
        responder: addr1,
        request_start: 1,
        request_end: 2,
        blocks: vec![block1.clone(), block2.clone()],
    };

    // Node2 handles the response
    let result = node2.handle_block_response(response);
    assert!(
        result.is_ok(),
        "Block response handling failed: {:?}",
        result
    );

    // Verify node2 has caught up
    {
        let state2 = node2.state.lock().unwrap();
        assert_eq!(
            state2.committed_height, 2,
            "Node2 should have caught up to height 2"
        );
    }

    // Verify blocks are stored in node2's DB
    {
        let db2 = node2.db.lock().unwrap();
        let loaded_block1 = ConsensusState::load_block(&*db2, 1).unwrap();
        let loaded_block2 = ConsensusState::load_block(&*db2, 2).unwrap();

        assert!(loaded_block1.is_some(), "Block 1 should be stored");
        assert!(loaded_block2.is_some(), "Block 2 should be stored");

        assert_eq!(loaded_block1.unwrap().height, 1);
        assert_eq!(loaded_block2.unwrap().height, 2);
    }

    println!("✅ Node successfully synced from peer on restart");
}
