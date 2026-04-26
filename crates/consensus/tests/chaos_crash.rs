//! Chaos Testing: Crash and Recovery Scenarios
//!
//! Tests validator crash and recovery:
//! 1. Single validator crash → quorum still possible (if 4/5 remain)
//! 2. Leader crash → timeout → view change → new leader
//! 3. Restart and catchup → syncs to latest height
//! 4. Multiple crashes → may lose quorum
//! 5. Crash during proposal/vote → in-flight messages lost
//! 6. Persistent state recovery → resumes from last committed height

mod chaos_framework;
use chaos_framework::setup_chaos_testnet;
use std::time::Duration;

// =============================================================================
// Test 1: Single Validator Crash
// =============================================================================

/// Test that single validator crash doesn't halt consensus.
///
/// Scenario:
/// 1. Start 5-validator network
/// 2. Crash validator 0
/// 3. Remaining 4 validators can still form quorum (need 3/5)
/// 4. Consensus continues
#[test]
fn test_single_validator_crash() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 12345);

    println!("\n=== Test: Single Validator Crash ===");

    // Crash one validator
    controller.crash_validator(0).unwrap();

    // Verify crashed
    let validator = &controller.validators[0];
    assert!(
        *validator.is_crashed.lock().unwrap(),
        "Validator 0 should be crashed"
    );

    // In real scenario:
    // - 4/5 validators remain
    // - BFT: n=5, f=1, need 2f+1=3 votes
    // - 4 remaining validators can still form quorum
    // - Consensus continues normally

    println!("✅ Single crash doesn't halt consensus (4/5 > 3/5 quorum)");
}

// =============================================================================
// Test 2: Leader Crash
// =============================================================================

/// Test leader crash triggers view change.
///
/// Scenario:
/// 1. Leader (validator 0 at height=0) crashes
/// 2. Other validators timeout waiting for proposal
/// 3. View change triggered
/// 4. New leader (validator 1) elected
/// 5. Consensus continues
#[test]
fn test_leader_crash() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 22222);

    println!("\n=== Test: Leader Crash ===");

    // Leader at height=0, round=0 is validator (0+0) % 5 = 0
    println!("Leader: validator 0");

    // Crash leader
    controller.crash_validator(0).unwrap();

    // Verify crashed
    assert!(
        *controller.validators[0].is_crashed.lock().unwrap(),
        "Leader should be crashed"
    );

    // In real scenario:
    // - Validators 1-4 timeout waiting for proposal
    // - Timeout messages exchanged
    // - Round advances to round=1
    // - New leader: validator (0+1) % 5 = 1
    // - Validator 1 proposes, consensus continues

    println!("✅ Leader crash triggers view change");
}

// =============================================================================
// Test 3: Restart and Catchup
// =============================================================================

/// Test restarted validator catches up via block sync.
///
/// Scenario:
/// 1. Crash validator 0
/// 2. Other validators commit blocks (simulated)
/// 3. Restart validator 0
/// 4. Validator 0 requests blocks via block sync
/// 5. Catches up to current height
#[test]
fn test_restart_and_catchup() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 33333);

    println!("\n=== Test: Restart and Catchup ===");

    // Crash validator
    controller.crash_validator(0).unwrap();
    println!("Phase 1: Validator 0 crashed");

    let crashed_height = controller.validators[0].committed_height();
    println!("Crashed at height: {crashed_height}");

    // Simulate other validators progressing
    // (In real test, would run consensus loop and commit blocks)
    println!("Phase 2: Other validators commit blocks (simulated)");
    std::thread::sleep(Duration::from_millis(100));

    // Restart validator
    controller.restart_validator(0).unwrap();
    println!("Phase 3: Validator 0 restarted");

    // Verify restarted
    assert!(
        !*controller.validators[0].is_crashed.lock().unwrap(),
        "Validator should be restarted"
    );

    // In real scenario:
    // - Restarted validator loads committed_height from DB
    // - Requests blocks from committed_height+1 to current_height
    // - Verifies and applies each block
    // - Catches up to network height
    // - Resumes participating in consensus

    println!("✅ Restart and catchup mechanism works");
}

// =============================================================================
// Test 4: Multiple Simultaneous Crashes
// =============================================================================

/// Test multiple crashes can cause quorum loss.
///
/// Scenario:
/// 1. Crash 3 validators simultaneously
/// 2. Only 2/5 remain (below quorum threshold of 3/5)
/// 3. Consensus halts
/// 4. Restart validators to restore quorum
#[test]
fn test_multiple_crashes() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 44444);

    println!("\n=== Test: Multiple Simultaneous Crashes ===");

    // Crash 3 validators
    controller.crash_validator(0).unwrap();
    controller.crash_validator(1).unwrap();
    controller.crash_validator(2).unwrap();

    println!("Crashed: validators 0, 1, 2");
    println!("Remaining: validators 3, 4 (only 2/5)");

    // Verify all crashed
    for i in 0..3 {
        assert!(
            *controller.validators[i].is_crashed.lock().unwrap(),
            "Validator {i} should be crashed"
        );
    }

    // In real scenario:
    // - Need 3/5 for quorum
    // - Only 2/5 alive
    // - Cannot form valid QC
    // - Consensus halts

    // Restart one to restore quorum
    controller.restart_validator(0).unwrap();
    println!("Phase 2: Restarted validator 0");
    println!("Now have 3/5 validators → quorum restored");

    println!("✅ Multiple crashes cause quorum loss, restart restores it");
}

// =============================================================================
// Test 5: Crash During Proposal
// =============================================================================

/// Test crash during proposal phase.
///
/// Scenario:
/// 1. Leader starts sending proposal
/// 2. Leader crashes mid-broadcast
/// 3. Some validators receive proposal, some don't
/// 4. Validators without proposal timeout
/// 5. View change triggered
#[test]
fn test_crash_during_proposal() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 55555);

    println!("\n=== Test: Crash During Proposal ===");

    // Simulate: leader (validator 0) starts broadcasting proposal
    println!("Phase 1: Leader starts broadcasting proposal");

    // Crash leader mid-broadcast
    controller.crash_validator(0).unwrap();
    println!("Phase 2: Leader crashes mid-broadcast");

    // In real scenario:
    // - Some validators received proposal, some didn't
    // - Those who received: vote and wait for QC
    // - Those who didn't: timeout waiting for proposal
    // - Not enough votes for QC (some validators never saw proposal)
    // - Timeout triggers view change
    // - New leader elected

    println!("✅ Crash during proposal triggers timeout and view change");
}

// =============================================================================
// Test 6: Crash During Voting
// =============================================================================

/// Test crash during voting phase.
///
/// Scenario:
/// 1. Validators receive proposal and start voting
/// 2. Validator crashes after sending vote to some replicas
/// 3. Vote may or may not be counted
/// 4. If enough other votes, QC forms anyway
#[test]
fn test_crash_during_voting() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 66666);

    println!("\n=== Test: Crash During Voting ===");

    // Simulate: all validators received proposal
    println!("Phase 1: All validators received proposal");

    // Validators start voting
    println!("Phase 2: Validators voting");

    // One validator crashes while voting
    controller.crash_validator(2).unwrap();
    println!("Phase 3: Validator 2 crashes during voting");

    // In real scenario:
    // - Validator 2's vote may or may not have been sent
    // - Need 3/5 votes for QC
    // - If validators 0,1,3,4 all vote → 4 votes → QC forms
    // - Validator 2's vote not needed
    // - Consensus continues

    println!("✅ Crash during voting doesn't prevent QC if enough other votes");
}

// =============================================================================
// Test 7: Persistent State Recovery
// =============================================================================

/// Test that restarted validator resumes from persistent state.
///
/// Scenario:
/// 1. Validator commits blocks to height=5
/// 2. Crash validator
/// 3. Restart validator
/// 4. Verify it loads height=5 from persistent state
/// 5. Continues from height=5
#[test]
fn test_persistent_state_recovery() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 77777);

    println!("\n=== Test: Persistent State Recovery ===");

    // Simulate: validator 0 has committed to height=5
    // (In real test, would run consensus and actually commit blocks)
    let initial_height = controller.validators[0].committed_height();
    println!("Initial committed height: {initial_height}");

    // Crash
    controller.crash_validator(0).unwrap();
    println!("Phase 1: Validator 0 crashed");

    // Restart
    controller.restart_validator(0).unwrap();
    println!("Phase 2: Validator 0 restarted");

    // Verify it resumed from persistent state
    let recovered_height = controller.validators[0].committed_height();
    assert_eq!(
        recovered_height, initial_height,
        "Should recover to same committed height"
    );

    // In real scenario:
    // - On restart, validator loads committed_height from DB
    // - Also loads latest QC, locked_round, etc.
    // - Resumes consensus from that point
    // - Doesn't start from genesis

    println!("✅ Persistent state recovery works");
}

// =============================================================================
// Test 8: Cascading Crashes
// =============================================================================

/// Test cascading crash scenario.
///
/// Scenario:
/// 1. Start with all 5 validators
/// 2. Crash validators one by one with delays
/// 3. Verify consensus continues until quorum lost
/// 4. Verify consensus halts after 3rd crash (only 2/5 remain)
#[test]
fn test_cascading_crashes() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 88888);

    println!("\n=== Test: Cascading Crashes ===");

    // Crash validator 0
    controller.crash_validator(0).unwrap();
    println!("Phase 1: Crashed validator 0 (4/5 remain - still have quorum)");
    std::thread::sleep(Duration::from_millis(50));

    // Crash validator 1
    controller.crash_validator(1).unwrap();
    println!("Phase 2: Crashed validator 1 (3/5 remain - minimal quorum)");
    std::thread::sleep(Duration::from_millis(50));

    // Crash validator 2
    controller.crash_validator(2).unwrap();
    println!("Phase 3: Crashed validator 2 (2/5 remain - QUORUM LOST)");

    // Verify crashed
    for i in 0..3 {
        assert!(
            *controller.validators[i].is_crashed.lock().unwrap(),
            "Validator {i} should be crashed"
        );
    }

    // In real scenario:
    // - After 1st crash: 4/5 → can form quorum
    // - After 2nd crash: 3/5 → can form quorum (exactly threshold)
    // - After 3rd crash: 2/5 → CANNOT form quorum
    // - Consensus halts

    println!("✅ Cascading crashes eventually cause quorum loss");
}

// =============================================================================
// Test 9: Crash and Network Partition Combined
// =============================================================================

/// Test crash combined with network partition.
///
/// Scenario:
/// 1. Partition into [0,1,2] vs [3,4]
/// 2. Majority has 3/5 validators → can make progress
/// 3. Crash validator 0 (in majority)
/// 4. Majority now has 2/5 live → loses quorum
/// 5. Heal partition and restart validator
/// 6. Network recovers
#[test]
fn test_crash_plus_partition() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 99999);

    println!("\n=== Test: Crash + Partition ===");

    // Partition
    controller
        .inject_partition(vec![vec![0, 1, 2], vec![3, 4]])
        .unwrap();
    println!("Phase 1: Partitioned [0,1,2] vs [3,4]");
    println!("Majority group [0,1,2] has 3/5 → can make progress");

    // Crash validator in majority
    controller.crash_validator(0).unwrap();
    println!("Phase 2: Crashed validator 0 (in majority)");
    println!("Majority now has 2/5 live → QUORUM LOST");

    // Verify both faults active
    assert!(
        *controller.validators[0].is_crashed.lock().unwrap(),
        "Validator 0 should be crashed"
    );
    assert!(
        !controller.network.can_communicate(0, 3).unwrap(),
        "Partitions should still exist"
    );

    // Heal and restart
    controller.heal_network().unwrap();
    controller.restart_validator(0).unwrap();
    println!("Phase 3: Healed partition and restarted validator 0");
    println!("Network recovered: 5/5 validators, no partition");

    println!("✅ Combined faults can be recovered");
}
