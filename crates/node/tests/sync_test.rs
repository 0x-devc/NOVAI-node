//! Integration test: Block sync protocol

use ed25519_dalek::SigningKey;
use novai_consensus::ConsensusState;
use novai_consensus_types::{Block, BlockRequest, BlockResponse, Vote, QC};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::ConsensusNode;
use novai_state::Kv;
use novai_types::Address;
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

    let node1 = ConsensusNode::new(sk1, validator_set.clone(), validator_pubkeys.clone(), 1000);
    let _node2 = ConsensusNode::new(sk2, validator_set, validator_pubkeys, 1000);

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
    assert!(result.is_ok(), "Block request handling failed: {result:?}");

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

    let sk1_for_qc = sk1.clone();
    let node1 = ConsensusNode::new(sk1, validator_set.clone(), validator_pubkeys.clone(), 1000);
    let node2 = ConsensusNode::new(sk2, validator_set, validator_pubkeys, 1000);

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
    assert!(result.is_ok(), "Block request failed: {result:?}");

    // Set node2's SMT root to match the first synced block's state_root
    // (C-01 fix: synced blocks are now verified against local state root)
    {
        let mut db2 = node2.db.lock().unwrap();
        let root_bytes = novai_state::encode_smt_root_v1(&block1.state_root);
        db2.put(novai_state::KEY_SMT_ROOT, &root_bytes).unwrap();
    }

    // Simulate node1 responding with blocks, each carrying a valid
    // certifying QC. Post Fix A2 the cursor only advances across blocks
    // that carry a valid certifying QC, so the honest catch-up path must
    // supply them.
    let qc1 = certifying_qc(&sk1_for_qc, addr1, &block1);
    let qc2 = certifying_qc(&sk1_for_qc, addr1, &block2);
    let response = BlockResponse {
        responder: addr1,
        request_start: 1,
        request_end: 2,
        blocks: vec![block1, block2],
        qcs: vec![Some(qc1), Some(qc2)],
    };

    // Node2 handles the response
    let result = node2.handle_block_response(response);
    assert!(result.is_ok(), "Block response handling failed: {result:?}");

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

/// Test: QC catch-up via justify_qc in proposal (race condition fix).
///
/// Scenario:
///   1. Validator 3 has processed (cached) block 1 but has NOT received QC(1).
///   2. Leader for height 2 (validator 1) forms QC(1) and immediately proposes
///      block 2 with justify_qc = QC(1).
///   3. The standalone QC(1) broadcast has not reached validator 3 yet.
///   4. Validator 3 receives the proposal for height 2.
///
/// Without the fix:
///   handle_proposal calls verify_block which checks expected_height based on
///   highest_qc (None). expected_height = 1, but block.height = 2 → REJECTED.
///
/// With the fix:
///   handle_proposal applies justify_qc first (QC catch-up), advancing
///   highest_qc to QC(1). Then verify_block sees expected_height = 2 → ACCEPTED.
#[test]
fn test_qc_catchup_via_justify_qc_in_proposal() {
    // Use deterministic keys matching main.rs (seed = index)
    let validator_keys: Vec<SigningKey> = (0..4)
        .map(|i| SigningKey::from_bytes(&[i as u8; 32]))
        .collect();

    let validator_set: Vec<novai_types::Address> = validator_keys
        .iter()
        .map(|sk| address_from_pubkey(&sk.verifying_key()))
        .collect();

    let validator_pubkeys: HashMap<novai_types::Address, ed25519_dalek::VerifyingKey> =
        validator_keys
            .iter()
            .map(|sk| {
                let pk = sk.verifying_key();
                (address_from_pubkey(&pk), pk)
            })
            .collect();

    // Node under test: validator 3 (not leader for heights 1 or 2)
    // Leader for height 1: view_height=0, round=0 → idx=(0+0)%4=0 → validator 0
    // Leader for height 2: view_height=1, round=0 → idx=(1+0)%4=1 → validator 1
    let node = ConsensusNode::new(
        validator_keys[3].clone(),
        validator_set.clone(),
        validator_pubkeys,
        1000,
    );

    // --- Step 1: Create block 1 and cache it in the node's state ---
    // (simulates: validator 3 received proposal for height 1, voted, cached it,
    //  but has NOT yet received the QC for height 1)
    let block1 = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32], // genesis parent
        state_root: [0u8; 32],  // genesis root (MemKv returns this when empty)
        txs: vec![],
    };
    let block1_hash = novai_consensus_types::block_hash(&block1);

    {
        let mut state = node.state.lock().unwrap();
        // Cache block 1 (as if we voted on it via handle_proposal)
        state.cache_block(block1).unwrap();
        // Crucially: highest_qc is still None — the QC broadcast hasn't arrived
        assert!(state.highest_qc.is_none(), "Precondition: no QC yet");
    }

    // --- Step 2: Build QC for height 1 with quorum votes ---
    // 3 votes from validators 0, 1, 2 (quorum = 2f+1 = 3 for n=4)
    let mut qc_votes = Vec::new();
    for i in 0..3 {
        let unsigned_vote = novai_consensus_types::Vote {
            height: 1,
            round: 0,
            block_hash: block1_hash,
            voter: validator_set[i],
            signature: [0u8; 64],
            ai_signal_commitment: None,
        };
        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned_vote);
        let domain_tag = b"NOVAI_VOTE_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validator_keys[i], &to_sign);

        qc_votes.push(novai_consensus_types::Vote {
            signature,
            ..unsigned_vote
        });
    }

    let justify_qc = novai_consensus_types::QC {
        height: 1,
        round: 0,
        block_hash: block1_hash,
        votes: qc_votes,
    };

    // --- Step 3: Build proposal for height 2 from validator 1 (leader) ---
    let block2 = Block {
        height: 2,
        round: 0,
        parent_hash: block1_hash, // parent is block 1
        state_root: [0u8; 32],    // same genesis root (no txs executed)
        txs: vec![],
    };

    let proposal = novai_consensus_types::Proposal {
        block: block2,
        justify_qc,
    };

    // Sign proposal with validator 1's key (the leader for height 2)
    let unsigned_bytes = novai_consensus_types::codec::encode_proposal_v1_unsigned(&proposal)
        .expect("encode proposal");
    let signature = novai_crypto::sign_bytes(&validator_keys[1], &unsigned_bytes);

    let signed_proposal = novai_consensus_types::SignedProposal {
        proposer: validator_set[1],
        proposal,
        signature,
    };

    // --- Step 4: Handle the proposal (this is where the fix matters) ---
    let result = node.handle_proposal(signed_proposal);
    assert!(
        result.is_ok(),
        "Proposal should be accepted after QC catch-up, got: {:?}",
        result.err()
    );

    // --- Step 5: Verify post-conditions ---
    {
        let state = node.state.lock().unwrap();

        // highest_qc should now be QC(1) (applied from justify_qc)
        assert!(
            state.highest_qc.is_some(),
            "highest_qc should be set after QC catch-up"
        );
        let hqc = state.highest_qc.as_ref().unwrap();
        assert_eq!(hqc.height, 1, "highest_qc should be for height 1");
        assert_eq!(
            hqc.block_hash, block1_hash,
            "highest_qc should reference block 1"
        );

        // Block 2 should be cached
        assert!(
            state.block_cache.contains_key(&2),
            "Block 2 should be cached after handle_proposal"
        );
    }

    println!("✅ QC catch-up via justify_qc in proposal works correctly");
}

/// Follower-side mempool eviction regression test.
///
/// Reproduces the production lockup observed on [redacted-host] 2026-06-04 14:35 to
/// 18:40 UTC where a price-oracle agent (entity 0a110df8) submitting a
/// single-sender continuous workload to a non-leader RPC accumulated the
/// per-sender pending count without ever evicting the committed txs. After
/// 16 admits the receiving node returned `SenderLimitExceeded` for that
/// sender for the rest of the run (81 submissions over 4 hours: 26 admitted,
/// 55 rejected, on-chain nonce stuck at 1).
///
/// Root cause: `TxMempool::remove` in `crates/mempool/src/lib.rs:245-250`
/// did not decrement `by_sender_count`. The propose-loop deferred-removal
/// drain at `crates/node/src/main.rs:1675-1683` calls `remove` on every
/// node every tick, but on followers (which never call `drain_ready`) the
/// missing decrement caused the per-sender counter to rise monotonically
/// to `MAX_PENDING_PER_SENDER = 16`.
///
/// Scope of this test: drives the exact deferred-removal pattern that runs
/// on every node every tick (a `pending_removals` queue is appended on
/// commit and drained into `mempool.remove`). It uses only the public
/// `TxMempool` API, mirroring the production drain at `main.rs:1675-1683`.
/// The end-to-end wiring through `ExecutionCommitCallback` (private to the
/// novai-node binary) is covered by the existing unit test
/// `on_commit_queues_committed_txs_and_drain_removes_them` at
/// `crates/node/src/main.rs:1901-2002`; that test is now also strengthened
/// indirectly by this fix because the same `mempool.remove` it exercises
/// now decrements `by_sender_count`.
///
/// Pre-fix expectation: the insert at cycle 16 panics with
/// `SenderLimitExceeded`. Post-fix expectation: all cycles succeed and the
/// mempool is empty at the end.
#[test]
fn follower_evicts_committed_txs_under_single_sender_load() {
    use ed25519_dalek::{SigningKey, VerifyingKey};
    use mempool::{NonceProvider, TxMempool, MAX_PENDING_PER_SENDER};
    use novai_crypto::sign_tx_v1;
    use novai_types::{Address, TxId, TxV1, TxVersion};
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct ProgressingNonces {
        expected: Mutex<HashMap<Address, u64>>,
    }
    impl NonceProvider for ProgressingNonces {
        fn expected_nonce(&self, addr: &Address) -> u64 {
            self.expected
                .lock()
                .unwrap()
                .get(addr)
                .copied()
                .unwrap_or(0)
        }
    }

    fn build_signed_tx(sk: &SigningKey, vk: &VerifyingKey, from: Address, nonce: u64) -> TxV1 {
        let mut tx = TxV1 {
            version: TxVersion::V1,
            from,
            pubkey: vk.to_bytes(),
            nonce,
            fee: 1,
            payload: b"oracle-anchor".to_vec(),
            sig: [0u8; 64],
        };
        sign_tx_v1(sk, &mut tx).expect("sign_tx_v1");
        tx
    }

    let sk = SigningKey::from_bytes(&[0xAAu8; 32]);
    let vk = sk.verifying_key();
    let from = address_from_pubkey(&vk);

    let np = ProgressingNonces {
        expected: Mutex::new(HashMap::new()),
    };
    let mut mp = TxMempool::new(1, 1024);
    let mut pending_removals: Vec<TxId> = Vec::new();

    // Run more than MAX_PENDING_PER_SENDER cycles. Each cycle mirrors what
    // happens on a follower node per block:
    //   1. RPC admits a fresh tx with the next monotone nonce.
    //   2. The tx is gossiped to the leader, included in a block, and the
    //      block propagates back here.
    //   3. ExecutionCommitCallback.on_commit appends the committed txid to
    //      pending_mempool_removals (modeled here as a local Vec<TxId>).
    //   4. The propose loop drain runs on every node every tick and calls
    //      mempool.remove for each queued txid (modeled here verbatim).
    // Pre-fix step 4 silently leaks by_sender_count; insert at cycle 16
    // would fail.
    let total_cycles: u64 = (MAX_PENDING_PER_SENDER as u64) * 2 + 5;
    for cycle in 0..total_cycles {
        np.expected.lock().unwrap().insert(from, cycle);
        let tx = build_signed_tx(&sk, &vk, from, cycle);
        let id = mp
            .insert(tx, &np)
            .unwrap_or_else(|e| panic!("insert at cycle {cycle} failed: {e:?}"));
        pending_removals.push(id);

        // The exact shape of the propose-loop drain at
        // crates/node/src/main.rs:1675-1683.
        for txid in pending_removals.drain(..) {
            mp.remove(&txid);
        }
    }

    assert_eq!(
        mp.len(),
        0,
        "mempool should be empty after all deferred-removal drains"
    );

    println!(
        "✅ Follower mempool evicted {total_cycles} single-sender committed txs without hitting SenderLimitExceeded"
    );
}

/// Stage 1 (gate-equivocation-535004): build_block_response pairs each
/// served block positionally with its certifying QC. Absence is a
/// faithful None, and qc_cache covers live-tail QCs whose rows are not
/// yet on disk.
#[test]
fn test_block_response_carries_qcs_positionally() {
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

    let node1 = ConsensusNode::new(sk1, validator_set.clone(), validator_pubkeys.clone(), 1000);
    let _node2 = ConsensusNode::new(sk2, validator_set, validator_pubkeys, 1000);

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

    let qc1 = QC {
        height: 1,
        round: 0,
        block_hash: novai_consensus_types::block_hash(&block1),
        votes: vec![],
    };

    // Blocks 1 and 2 on disk; a QC row for height 1 only.
    {
        let mut db1 = node1.db.lock().unwrap();
        db1.put(
            &novai_state::block_key(1),
            &novai_consensus_types::codec::encode_block_v1(&block1).unwrap(),
        )
        .unwrap();
        db1.put(
            &novai_state::block_key(2),
            &novai_consensus_types::codec::encode_block_v1(&block2).unwrap(),
        )
        .unwrap();
        db1.put(
            &novai_state::qc_key(1),
            &novai_consensus_types::codec::encode_qc_v1(&qc1).unwrap(),
        )
        .unwrap();
    }

    let request = BlockRequest {
        requester: addr2,
        start_height: 1,
        end_height: 2,
    };

    let response = node1.build_block_response(&request);
    assert_eq!(response.blocks.len(), 2);
    assert_eq!(response.qcs.len(), 2, "one qcs entry per served block");
    assert_eq!(response.qcs[0], Some(qc1.clone()));
    assert_eq!(
        response.qcs[1], None,
        "missing QC must surface as an explicit None, never be skipped"
    );

    // Live-tail fallback: height 2's QC exists only in qc_cache.
    let qc2 = QC {
        height: 2,
        round: 0,
        block_hash: novai_consensus_types::block_hash(&block2),
        votes: vec![],
    };
    node1.state.lock().unwrap().qc_cache.insert(2, qc2.clone());

    let response = node1.build_block_response(&request);
    assert_eq!(response.qcs[0], Some(qc1));
    assert_eq!(response.qcs[1], Some(qc2));

    println!("✅ build_block_response pairs blocks and QCs positionally");
}

// ===== Stage 2 Fix A2 (gate-equivocation-535004): certify before advancing =====

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
        votes: vec![signed_vote(
            signer,
            voter,
            block.height,
            block.round,
            block_hash,
        )],
    }
}

/// A receiver (node2) behind at committed_height 0, plus the addr1 signing
/// key for certifying QCs and a two-block chain to sync.
fn a2_receiver_fixture() -> (ConsensusNode, SigningKey, Address, Block, Block) {
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
    // node2 is the receiver under test; sk1 is retained to sign certifying
    // QCs as addr1.
    let node2 = ConsensusNode::new(sk2, validator_set, validator_pubkeys, 1000);

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

    // The receiver's SMT root must match the first synced block's state_root
    // so the existing C-01 state-root check passes and execution reaches the
    // Fix A2 certification logic.
    {
        let mut db2 = node2.db.lock().unwrap();
        let root_bytes = novai_state::encode_smt_root_v1(&block1.state_root);
        db2.put(novai_state::KEY_SMT_ROOT, &root_bytes).unwrap();
    }
    (node2, sk1, addr1, block1, block2)
}

#[test]
fn sync_rejects_uncertified_block() {
    // THE 535004 regression: a block carrying no certifying QC must NOT
    // advance committed_height via the lenient sync path. This is the exact
    // mechanism that let the uncertified 535003 become committed and wedged
    // the chain.
    let (node2, _sk1, addr1, block1, block2) = a2_receiver_fixture();
    let response = BlockResponse {
        responder: addr1,
        request_start: 1,
        request_end: 2,
        blocks: vec![block1, block2],
        qcs: vec![None, None],
    };
    node2.handle_block_response(response).unwrap();
    assert_eq!(
        node2.state.lock().unwrap().committed_height,
        0,
        "uncertified blocks must not advance the cursor"
    );
}

#[test]
fn sync_advances_only_certified_prefix() {
    // block1 carries a valid QC, block2 does not: the cursor advances to 1
    // and stops, never reaching the uncertified block2.
    let (node2, sk1, addr1, block1, block2) = a2_receiver_fixture();
    let qc1 = certifying_qc(&sk1, addr1, &block1);
    let response = BlockResponse {
        responder: addr1,
        request_start: 1,
        request_end: 2,
        blocks: vec![block1, block2],
        qcs: vec![Some(qc1), None],
    };
    node2.handle_block_response(response).unwrap();
    assert_eq!(
        node2.state.lock().unwrap().committed_height,
        1,
        "the cursor must stop at the first uncertified block"
    );
}

#[test]
fn sync_rejects_wrong_height_qc() {
    // A QC validly certifying block1's hash but claiming the wrong height
    // must not certify block1.
    let (node2, sk1, addr1, block1, block2) = a2_receiver_fixture();
    let mut qc = certifying_qc(&sk1, addr1, &block1);
    qc.height = 9;
    let response = BlockResponse {
        responder: addr1,
        request_start: 1,
        request_end: 2,
        blocks: vec![block1, block2],
        qcs: vec![Some(qc), None],
    };
    node2.handle_block_response(response).unwrap();
    assert_eq!(
        node2.state.lock().unwrap().committed_height,
        0,
        "a wrong-height QC must not certify the block"
    );
}

#[test]
fn sync_rejects_qc_for_different_block() {
    // A QC at block1's height, validly signed, but bound to block2's hash
    // must not certify block1.
    let (node2, sk1, addr1, block1, block2) = a2_receiver_fixture();
    let wrong_hash = novai_consensus_types::block_hash(&block2);
    let qc = QC {
        height: 1,
        round: 0,
        block_hash: wrong_hash,
        votes: vec![signed_vote(&sk1, addr1, 1, 0, wrong_hash)],
    };
    let response = BlockResponse {
        responder: addr1,
        request_start: 1,
        request_end: 2,
        blocks: vec![block1, block2],
        qcs: vec![Some(qc), None],
    };
    node2.handle_block_response(response).unwrap();
    assert_eq!(
        node2.state.lock().unwrap().committed_height,
        0,
        "a QC bound to a different block must not certify block1"
    );
}

#[test]
fn sync_persists_certified_qc_rows() {
    // The Stage 1 to Stage 2 delta: a certified sync writes the QC row at
    // qc_key(height), so the synced node can in turn serve and certify it to
    // the next lagging peer.
    let (node2, sk1, addr1, block1, block2) = a2_receiver_fixture();
    let qc1 = certifying_qc(&sk1, addr1, &block1);
    let qc2 = certifying_qc(&sk1, addr1, &block2);
    let response = BlockResponse {
        responder: addr1,
        request_start: 1,
        request_end: 2,
        blocks: vec![block1, block2],
        qcs: vec![Some(qc1.clone()), Some(qc2.clone())],
    };
    node2.handle_block_response(response).unwrap();
    assert_eq!(node2.state.lock().unwrap().committed_height, 2);
    let db2 = node2.db.lock().unwrap();
    assert_eq!(
        ConsensusState::load_qc_at_height(&*db2, 1).unwrap(),
        Some(qc1)
    );
    assert_eq!(
        ConsensusState::load_qc_at_height(&*db2, 2).unwrap(),
        Some(qc2)
    );
}

// ===== Stage 2 Fix D (gate-equivocation-535004): check_timeout backoff =====

/// The check_timeout failure path must record the attempt so the existing
/// rebroadcast throttle backs off repeated create_timeout failures. Before
/// Fix D the error branch returned None without setting last_timeout_time,
/// so the throttle never engaged and the loop spun at roughly 195/sec.
#[test]
fn check_timeout_backs_off_on_repeated_failure() {
    use std::time::{Duration, Instant};

    let sk = SigningKey::generate(&mut OsRng);
    let pk = sk.verifying_key();
    let addr = address_from_pubkey(&pk);
    let mut validator_pubkeys = HashMap::new();
    validator_pubkeys.insert(addr, pk);
    // base_timeout_ms 1000 gives a 1s throttle window, far larger than the
    // microseconds between the two check_timeout calls below.
    let node = ConsensusNode::new(sk, vec![addr], validator_pubkeys, 1000);

    // Make the round timer appear long-elapsed so check_timeout reaches
    // create_timeout without sleeping.
    *node.round_start_time.lock().unwrap() = Instant::now()
        .checked_sub(Duration::from_secs(3600))
        .expect("test host uptime should exceed one hour");

    // Poison highest_qc with a duplicate-voter QC so create_timeout fails to
    // encode it (the node2 shape from the incident).
    {
        let dup_vote = Vote {
            height: 1,
            round: 0,
            block_hash: [0x11; 32],
            voter: addr,
            signature: [0u8; 64],
            ai_signal_commitment: None,
        };
        let mut state = node.state.lock().unwrap();
        state.highest_qc = Some(QC {
            height: 1,
            round: 0,
            block_hash: [0x11; 32],
            votes: vec![dup_vote.clone(), dup_vote],
        });
    }

    // First call: create_timeout fails. Fix D records the attempt time.
    assert!(
        node.check_timeout().is_none(),
        "a failed create_timeout yields no timeout"
    );
    assert!(
        node.last_timeout_time.lock().unwrap().is_some(),
        "Fix D must record the failed attempt so the throttle can engage"
    );

    // Repair highest_qc so create_timeout WOULD now succeed.
    node.state.lock().unwrap().highest_qc = None;

    // Second call, microseconds later: the throttle must suppress it even
    // though create_timeout could now succeed. Without Fix D the first call
    // left last_timeout_time None, this call would reach create_timeout and
    // return Some, and this assertion would fail.
    assert!(
        node.check_timeout().is_none(),
        "the rebroadcast throttle must back off the immediate retry"
    );
}
