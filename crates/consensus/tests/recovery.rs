//! Recovery tests for Week 8: Timeouts, round advance, and catch-up.
//!
//! D8.5 Deliverables:
//! - leader_crash: Leader crash → view change (round advances after timeout)
//! - restart_catches_up: Restart → catch up (node syncs to committed height)
//! - partition_and_rejoin: Network partition → rejoin and sync

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::{timeout_for_round, ConsensusState, BASE_TIMEOUT_MS, MAX_TIMEOUT_MS};
use novai_consensus_types::{Block, Timeout};
use novai_crypto::address_from_pubkey;
use novai_state::{Kv, MemKv, KEY_COMMITTED_HEIGHT};
use novai_types::Address;

/// Helper to set committed height in database.
fn set_committed_height(db: &mut MemKv, height: u64) {
    // Format: just u64 big-endian (8 bytes), no version byte
    db.put(KEY_COMMITTED_HEIGHT, &height.to_be_bytes()).unwrap();
}

/// Create test validators with deterministic keys.
fn make_test_validators(count: usize) -> Vec<(Address, SigningKey, VerifyingKey)> {
    (0..count)
        .map(|i| {
            // Use deterministic seed for reproducibility
            let seed = [i as u8; 32];
            let sk = SigningKey::from_bytes(&seed);
            let pk = sk.verifying_key();
            let addr = address_from_pubkey(&pk);
            (addr, sk, pk)
        })
        .collect()
}

/// Helper to build a valid chain of blocks.
fn build_block_chain(count: u64) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut prev_hash = [0u8; 32];

    for h in 1..=count {
        let block = Block {
            height: h,
            round: 0,
            parent_hash: prev_hash,
            state_root: [h as u8; 32],
            txs: vec![],
        };
        prev_hash = novai_consensus_types::codec::hash_block_v1(&block).unwrap();
        blocks.push(block);
    }

    blocks
}

// =============================================================================
// D8.5 TEST: leader_crash
// =============================================================================

/// Test that when leader crashes, validators can timeout and advance round.
///
/// Scenario:
/// 1. Leader (node 0) is supposed to propose at round 0
/// 2. Leader "crashes" (doesn't propose)
/// 3. Other validators create timeout messages
/// 4. When 2f+1 timeouts received, round advances
/// 5. New leader (node 1) can now propose at round 1
#[test]
fn leader_crash_triggers_round_advance() {
    let validators = make_test_validators(4); // n=4, f=1, quorum=3
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
    let pubkeys: Vec<(Address, VerifyingKey)> =
        validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

    // Create consensus state for node 1 (not the leader)
    let mut state = ConsensusState::new(validator_set[1]);

    // Verify initial state
    assert_eq!(state.round, 0, "Should start at round 0");

    // Leader (node 0) crashes - we don't receive a proposal
    // Other validators (0, 1, 2) timeout and broadcast timeout messages

    // Collect 3 timeouts (quorum for n=4)
    for i in 0..3 {
        let timeout = Timeout {
            height: 1, // Timeout for next height
            round: 0,
            voter: validator_set[i],
            highest_qc: None,
            signature: [0u8; 64],
        };

        // Sign the timeout
        let unsigned_bytes =
            novai_consensus_types::codec::encode_timeout_v1_unsigned(&timeout).unwrap();
        let domain_tag = b"NOVAI_TIMEOUT_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validators[i].1, &to_sign);

        let signed_timeout = Timeout {
            signature,
            ..timeout
        };

        // Add timeout to state
        state.add_timeout(signed_timeout, &pubkeys).unwrap();
    }

    // Try to advance round
    let advanced = state.try_advance_round(&validator_set);

    // Verify round advanced
    assert!(advanced, "Round should advance with 2f+1 timeouts");
    assert_eq!(state.round, 1, "Round should now be 1");

    // Verify new leader for round 1
    let new_leader =
        ConsensusState::compute_leader_for_view(state.height, state.round, &validator_set).unwrap();
    assert_eq!(
        new_leader, validator_set[1],
        "Node 1 should be leader at round 1"
    );

    println!("✅ leader_crash: Round advanced from 0 to 1 after timeout quorum");
}

/// Test that timeout values follow exponential backoff.
#[test]
fn timeout_backoff_is_exponential() {
    assert_eq!(timeout_for_round(0), BASE_TIMEOUT_MS);
    assert_eq!(timeout_for_round(0), 1000);

    assert_eq!(timeout_for_round(1), 2000); // 2^1 * 1000
    assert_eq!(timeout_for_round(2), 4000); // 2^2 * 1000
    assert_eq!(timeout_for_round(3), 8000); // 2^3 * 1000
    assert_eq!(timeout_for_round(4), 16000); // 2^4 * 1000
    assert_eq!(timeout_for_round(5), 32000); // 2^5 * 1000

    // Capped at MAX (2^6 * 1000 = 64000 > 60000)
    assert_eq!(timeout_for_round(6), MAX_TIMEOUT_MS);
    assert_eq!(timeout_for_round(100), MAX_TIMEOUT_MS);

    println!("✅ timeout_backoff: Exponential backoff verified");
}

// =============================================================================
// D8.5 TEST: restart_catches_up
// =============================================================================

/// Test that a restarted node can catch up to committed height.
///
/// Scenario:
/// 1. Build a chain of 5 blocks and persist them
/// 2. Simulate node restart with fresh state
/// 3. Use catch_up_to() to load and verify blocks
/// 4. Verify state matches committed height
#[test]
fn restart_catches_up_to_committed_height() {
    let validators = make_test_validators(4);
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

    let mut db = MemKv::new();

    // Build and persist a valid chain of 5 blocks
    let blocks = build_block_chain(5);
    let temp_state = ConsensusState::new(validator_set[0]);

    for block in &blocks {
        temp_state.persist_block(&mut db, block).unwrap();
    }

    // Persist committed height
    set_committed_height(&mut db, 5);

    // Simulate restart: create fresh state
    let mut state = ConsensusState::new(validator_set[0]);
    assert_eq!(state.committed_height, 0);
    assert_eq!(state.block_cache.len(), 0);

    // Catch up to height 5
    let count = state.catch_up_to(&db, 5).unwrap();

    assert_eq!(count, 5, "Should have loaded 5 blocks");
    assert_eq!(state.height, 5, "Height should be 5");
    assert_eq!(state.block_cache.len(), 5, "Cache should have 5 blocks");

    // Verify blocks are in cache
    for h in 1..=5 {
        assert!(
            state.block_cache.contains_key(&h),
            "Block {} should be in cache",
            h
        );
    }

    println!("✅ restart_catches_up: Node caught up to height 5 with 5 blocks cached");
}

/// Test recover_with_cache loads recent blocks into cache.
#[test]
fn recover_with_cache_populates_block_cache() {
    let validators = make_test_validators(4);
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

    let mut db = MemKv::new();

    // Build and persist a chain of 10 blocks
    let blocks = build_block_chain(10);
    let temp_state = ConsensusState::new(validator_set[0]);

    for block in &blocks {
        temp_state.persist_block(&mut db, block).unwrap();
    }

    // Persist committed height
    set_committed_height(&mut db, 10);

    // Recover with cache depth of 3
    let state = ConsensusState::recover_with_cache(validator_set[0], &db, 3).unwrap();

    assert_eq!(state.committed_height, 10);
    assert_eq!(state.height, 10);

    // Should have cached blocks 8, 9, 10 (last 3)
    assert!(
        state.block_cache.len() >= 3,
        "Should have at least 3 blocks cached"
    );

    println!(
        "✅ recover_with_cache: Recovered with {} blocks in cache",
        state.block_cache.len()
    );
}

// =============================================================================
// D8.5 TEST: partition_and_rejoin
// =============================================================================

/// Test that a partitioned node can rejoin and sync.
///
/// Scenario:
/// 1. Network has 4 nodes, all at height 0
/// 2. Node 3 gets partitioned (misses blocks 1-5)
/// 3. Remaining nodes commit blocks 1-5
/// 4. Node 3 rejoins and catches up
/// 5. Verify node 3 has same state as others
#[test]
fn partition_and_rejoin_syncs_correctly() {
    let validators = make_test_validators(4);
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

    // Shared database (simulates what partitioned node would sync from)
    let mut shared_db = MemKv::new();

    // Build chain while node 3 is partitioned
    let blocks = build_block_chain(5);

    // Active nodes (0, 1, 2) commit blocks
    let mut active_state = ConsensusState::new(validator_set[0]);
    for block in &blocks {
        active_state.persist_block(&mut shared_db, block).unwrap();
        active_state.cache_block(block.clone()).unwrap();
    }

    // Update committed height
    set_committed_height(&mut shared_db, 5);
    active_state.apply_commits(&blocks).unwrap();

    // Partitioned node (node 3) was at height 0
    let mut partitioned_state = ConsensusState::new(validator_set[3]);
    assert_eq!(partitioned_state.committed_height, 0);
    assert_eq!(partitioned_state.height, 0);

    // Node 3 rejoins - catches up from shared DB
    let synced_count = partitioned_state.catch_up_to(&shared_db, 5).unwrap();

    // Verify sync
    assert_eq!(synced_count, 5, "Should sync 5 blocks");
    assert_eq!(
        partitioned_state.height, 5,
        "Height should match active nodes"
    );
    assert_eq!(
        partitioned_state.block_cache.len(),
        5,
        "Should have all blocks cached"
    );

    // Verify chain integrity was checked
    for h in 1..=5 {
        let cached = partitioned_state.block_cache.get(&h).unwrap();
        let original = &blocks[(h - 1) as usize];
        assert_eq!(cached.height, original.height);
        assert_eq!(cached.state_root, original.state_root);
    }

    println!("✅ partition_and_rejoin: Node 3 synced 5 blocks after rejoin");
}

/// Test that catch-up fails if chain is broken (state root mismatch detection).
#[test]
fn catch_up_detects_broken_chain() {
    let validators = make_test_validators(4);
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

    let mut db = MemKv::new();
    let temp_state = ConsensusState::new(validator_set[0]);

    // Create block 1 with correct parent
    let block1 = Block {
        height: 1,
        round: 0,
        parent_hash: [0; 32],
        state_root: [1; 32],
        txs: vec![],
    };
    temp_state.persist_block(&mut db, &block1).unwrap();

    // Create block 2 with WRONG parent hash (simulates corruption/attack)
    let block2 = Block {
        height: 2,
        round: 0,
        parent_hash: [0xFF; 32], // Wrong!
        state_root: [2; 32],
        txs: vec![],
    };
    temp_state.persist_block(&mut db, &block2).unwrap();

    // Try to catch up
    let mut state = ConsensusState::new(validator_set[0]);
    let result = state.catch_up_to(&db, 2);

    // Should fail due to broken chain
    assert!(result.is_err(), "Catch-up should fail on broken chain");

    println!("✅ catch_up_detects_broken_chain: Invalid chain rejected");
}

/// Test multiple round advances (simulates extended leader failures).
#[test]
fn multiple_round_advances() {
    let validators = make_test_validators(4);
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
    let pubkeys: Vec<(Address, VerifyingKey)> =
        validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

    let mut state = ConsensusState::new(validator_set[0]);

    // Advance through 3 rounds (simulating 3 leader failures)
    for expected_round in 1..=3 {
        // Collect timeouts for current round
        for i in 0..3 {
            let timeout = Timeout {
                height: 1,
                round: expected_round - 1,
                voter: validator_set[i],
                highest_qc: None,
                signature: [0u8; 64],
            };

            let unsigned_bytes =
                novai_consensus_types::codec::encode_timeout_v1_unsigned(&timeout).unwrap();
            let domain_tag = b"NOVAI_TIMEOUT_V1";
            let mut to_sign = Vec::new();
            to_sign.extend_from_slice(domain_tag);
            to_sign.extend_from_slice(&unsigned_bytes);
            let signature = novai_crypto::sign_bytes(&validators[i].1, &to_sign);

            let signed_timeout = Timeout {
                signature,
                ..timeout
            };
            state.add_timeout(signed_timeout, &pubkeys).unwrap();
        }

        let advanced = state.try_advance_round(&validator_set);
        assert!(advanced, "Round {} should advance", expected_round);
        assert_eq!(state.round, expected_round);
    }

    assert_eq!(state.round, 3, "Should be at round 3 after 3 advances");

    println!("✅ multiple_round_advances: Advanced through 3 rounds successfully");
}
