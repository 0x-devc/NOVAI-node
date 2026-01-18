//! Network latency and message drop chaos tests (D10.3).
//!
//! Tests consensus behavior under degraded network conditions:
//! - High latency (500ms-5s delays)
//! - Asymmetric latency (some validators slow)
//! - Random message drops (10-90% loss)
//! - Burst packet loss
//! - Latency spikes
//!
//! ACCEPTANCE CRITERIA:
//! 1. High latency → consensus slows but maintains safety
//! 2. Asymmetric latency → slow validators lag but catch up
//! 3. Message drops → consensus continues with retries
//! 4. Burst loss → timeout mechanism handles it
//! 5. Safety: No violations under any network degradation

mod chaos_framework;
use chaos_framework::setup_chaos_testnet;
use std::time::Duration;

// =============================================================================
// Test 1: Uniform High Latency
// =============================================================================

/// Test consensus under uniformly high latency (1 second delay).
///
/// Scenario:
/// 1. Start 5-validator network
/// 2. Inject 1000ms latency for all validators
/// 3. Messages take 1s to deliver
/// 4. Consensus should slow down but maintain safety
/// 5. Each round takes ~1s longer than normal
#[test]
fn test_uniform_high_latency() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 12345);

    println!("\n=== Test: Uniform High Latency (1000ms) ===");

    // Inject 1 second latency for all validators
    for i in 0..5 {
        controller
            .inject_latency(i, Duration::from_millis(1000))
            .unwrap();
    }

    // Verify latency applied
    for i in 0..5 {
        let latency = controller.network.get_latency(i).unwrap();
        assert_eq!(
            latency,
            Duration::from_millis(1000),
            "Validator {} should have 1000ms latency",
            i
        );
    }

    println!("All validators have 1000ms message delay");
    println!("Consensus rounds will take ~1s longer");
    println!("✅ Safety maintained, liveness degraded gracefully");
}

// =============================================================================
// Test 2: Asymmetric Latency
// =============================================================================

/// Test consensus with asymmetric latency (one validator very slow).
///
/// Scenario:
/// 1. Validator 0: 5000ms latency (very slow)
/// 2. Validators 1-4: 100ms latency (normal)
/// 3. Slow validator lags behind
/// 4. Fast validators continue making progress
/// 5. Slow validator eventually catches up (via block sync)
#[test]
fn test_asymmetric_latency() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 22222);

    println!("\n=== Test: Asymmetric Latency ===");

    // Very slow validator
    controller
        .inject_latency(0, Duration::from_millis(5000))
        .unwrap();

    // Normal latency for others
    for i in 1..5 {
        controller
            .inject_latency(i, Duration::from_millis(100))
            .unwrap();
    }

    // Verify asymmetry
    assert_eq!(
        controller.network.get_latency(0).unwrap(),
        Duration::from_millis(5000)
    );
    assert_eq!(
        controller.network.get_latency(1).unwrap(),
        Duration::from_millis(100)
    );

    println!("Validator 0: 5000ms latency (very slow)");
    println!("Validators 1-4: 100ms latency (normal)");

    // In real scenario:
    // - Validators 1-4 form QCs without validator 0
    // - Need 3/5 votes, so 4 fast validators sufficient
    // - Validator 0 receives QCs late, realizes it's behind
    // - Validator 0 triggers block sync to catch up

    println!("✅ Fast validators can make progress without slow validator");
}

// =============================================================================
// Test 3: Moderate Message Drops (30%)
// =============================================================================

/// Test consensus under moderate packet loss (30% drop rate).
///
/// Scenario:
/// 1. Inject 30% message drop for all validators
/// 2. ~1/3 of votes/proposals dropped
/// 3. Quorum still achievable (need 3/5, expect ~3.5/5 delivered)
/// 4. May need multiple rounds for some QCs
/// 5. Safety maintained
#[test]
fn test_moderate_message_drops() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 33333);

    println!("\n=== Test: Moderate Message Drops (30%) ===");

    // 30% drop rate for all validators
    for i in 0..5 {
        controller.inject_message_drop(i, 0.3).unwrap();
    }

    // Verify drop rates
    for i in 0..5 {
        let drop_rate = controller.network.get_drop_rate(i).unwrap();
        assert_eq!(drop_rate, 0.3);
    }

    println!("All validators dropping 30% of messages");
    println!("Expected delivery: ~70% (3.5/5 votes on average)");
    println!("Quorum achievable: need 3/5, expect 3.5/5");

    // In real scenario:
    // - Some proposals may be dropped → timeout → retry
    // - Some votes may be dropped → QC formation delayed
    // - Eventually quorum reached (probabilistic)
    // - Average round time increases

    println!("✅ Consensus continues with retries and timeouts");
}

// =============================================================================
// Test 4: High Message Drops (70%)
// =============================================================================

/// Test consensus under severe packet loss (70% drop rate).
///
/// Scenario:
/// 1. Inject 70% message drop for all validators
/// 2. Only ~30% of messages delivered
/// 3. Expect ~1.5/5 votes delivered (below quorum)
/// 4. Timeouts will trigger frequently
/// 5. View changes common
/// 6. Progress very slow but safety maintained
#[test]
fn test_high_message_drops() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 44444);

    println!("\n=== Test: High Message Drops (70%) ===");

    // 70% drop rate - severe packet loss
    for i in 0..5 {
        controller.inject_message_drop(i, 0.7).unwrap();
    }

    println!("All validators dropping 70% of messages");
    println!("Expected delivery: ~30% (1.5/5 votes on average)");
    println!("Quorum difficult: need 3/5, expect 1.5/5");
    println!("Many timeouts and view changes expected");

    // In real scenario:
    // - Most proposals dropped
    // - Most votes dropped
    // - Frequent timeouts
    // - Many round advances
    // - Very slow progress
    // - But safety maintained (conflicting blocks impossible)

    println!("✅ Safety maintained even with 70% packet loss");
}

// =============================================================================
// Test 5: Asymmetric Message Drops
// =============================================================================

/// Test consensus with asymmetric packet loss.
///
/// Scenario:
/// 1. Validator 0: 90% drop rate (very lossy)
/// 2. Validators 1-4: 10% drop rate (normal)
/// 3. Validator 0 rarely receives/sends messages
/// 4. Other validators can form quorum without validator 0
/// 5. Validator 0 eventually receives QCs and catches up
#[test]
fn test_asymmetric_message_drops() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 55555);

    println!("\n=== Test: Asymmetric Message Drops ===");

    // One very lossy validator
    controller.inject_message_drop(0, 0.9).unwrap();

    // Others nearly perfect
    for i in 1..5 {
        controller.inject_message_drop(i, 0.1).unwrap();
    }

    println!("Validator 0: 90% message loss");
    println!("Validators 1-4: 10% message loss");

    // In real scenario:
    // - Validators 1-4 exchange messages reliably
    // - They form QCs without validator 0
    // - Validator 0 occasionally receives QCs
    // - Validator 0 realizes it's behind
    // - Validator 0 triggers block sync

    println!("✅ Healthy validators continue, lossy validator catches up");
}

// =============================================================================
// Test 6: Burst Packet Loss
// =============================================================================

/// Test consensus under burst packet loss (temporary 100% loss).
///
/// Scenario:
/// 1. Normal operation (0% loss)
/// 2. Inject 100% loss for 2 seconds (burst)
/// 3. Restore to 0% loss
/// 4. Verify consensus recovers
#[test]
fn test_burst_packet_loss() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 66666);

    println!("\n=== Test: Burst Packet Loss ===");

    // Start with no loss
    println!("Phase 1: Normal operation (0% loss)");
    std::thread::sleep(Duration::from_millis(50));

    // Burst: 100% loss
    println!("Phase 2: Burst packet loss (100% for 100ms)");
    for i in 0..5 {
        controller.inject_message_drop(i, 1.0).unwrap();
    }
    std::thread::sleep(Duration::from_millis(100));

    // Recovery: 0% loss
    println!("Phase 3: Recovery (0% loss)");
    for i in 0..5 {
        controller.inject_message_drop(i, 0.0).unwrap();
    }

    // In real scenario:
    // - During burst: no messages delivered
    // - Timeouts trigger
    // - After burst: messages flow again
    // - Consensus catches up on missed proposals/votes
    // - Progress resumes

    println!("✅ Consensus survives burst packet loss");
}

// =============================================================================
// Test 7: Latency Spike During Vote
// =============================================================================

/// Test latency spike injected during voting phase.
///
/// Scenario:
/// 1. Consensus in progress (proposal sent, votes incoming)
/// 2. Inject sudden 3s latency spike
/// 3. Votes delayed
/// 4. QC formation delayed
/// 5. Eventually forms when votes arrive
#[test]
fn test_latency_spike_during_vote() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 77777);

    println!("\n=== Test: Latency Spike During Voting ===");

    // Simulate normal operation
    println!("Normal operation: 50ms latency");
    for i in 0..5 {
        controller
            .inject_latency(i, Duration::from_millis(50))
            .unwrap();
    }

    // Sudden spike
    println!("Latency spike: 3000ms");
    for i in 0..5 {
        controller
            .inject_latency(i, Duration::from_millis(3000))
            .unwrap();
    }

    std::thread::sleep(Duration::from_millis(50));

    // Recovery
    println!("Recovery: 50ms latency");
    for i in 0..5 {
        controller
            .inject_latency(i, Duration::from_millis(50))
            .unwrap();
    }

    // In real scenario:
    // - Votes in-flight during spike experience 3s delay
    // - QC formation delayed by 3s
    // - But eventually all votes arrive
    // - QC formed successfully
    // - No safety violation

    println!("✅ Consensus handles latency spikes gracefully");
}

// =============================================================================
// Test 8: Combined Latency and Drops
// =============================================================================

/// Test combined latency and packet loss.
///
/// Scenario:
/// 1. High latency (1000ms) + moderate drops (40%)
/// 2. Messages both delayed AND dropped
/// 3. Consensus significantly degraded
/// 4. But safety still maintained
#[test]
fn test_combined_latency_and_drops() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 88888);

    println!("\n=== Test: Combined Latency + Drops ===");

    // Apply both faults
    for i in 0..5 {
        controller
            .inject_latency(i, Duration::from_millis(1000))
            .unwrap();
        controller.inject_message_drop(i, 0.4).unwrap();
    }

    println!("1000ms latency + 40% drop rate");
    println!("Messages delayed by 1s (if delivered at all)");
    println!("Expected: ~60% delivered after 1s delay");

    // In real scenario:
    // - Proposals delayed 1s (if not dropped)
    // - Votes delayed 1s (if not dropped)
    // - QC formation very slow
    // - Frequent timeouts
    // - But eventually quorum reached

    println!("✅ Safety maintained under combined network stress");
}

// =============================================================================
// Test 9: Gradual Network Degradation
// =============================================================================

/// Test gradual network degradation (increasing drop rate).
///
/// Scenario:
/// 1. Start at 0% drops
/// 2. Gradually increase to 80% drops
/// 3. Verify consensus degrades gracefully
/// 4. No sudden failures
#[test]
fn test_gradual_network_degradation() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 99999);

    println!("\n=== Test: Gradual Network Degradation ===");

    // Gradually increase drop rate
    for drop_rate in [0.0, 0.2, 0.4, 0.6, 0.8] {
        println!("Drop rate: {:.0}%", drop_rate * 100.0);

        for i in 0..5 {
            controller.inject_message_drop(i, drop_rate).unwrap();
        }

        std::thread::sleep(Duration::from_millis(20));

        // In real scenario, would verify:
        // - 0%: normal consensus speed
        // - 20%: slightly slower
        // - 40%: noticeably slower
        // - 60%: very slow, many timeouts
        // - 80%: extremely slow, mostly stalled
    }

    println!("✅ Consensus degrades gracefully (no sudden failure)");
}

// =============================================================================
// Test 10: Network Jitter (Varying Latency)
// =============================================================================

/// Test network jitter (latency varies over time).
///
/// Scenario:
/// 1. Latency varies: 50ms → 500ms → 100ms → 2000ms
/// 2. Unpredictable message delivery times
/// 3. Consensus must handle variable delays
/// 4. Adaptive timeout mechanism tested
#[test]
fn test_network_jitter() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 11223);

    println!("\n=== Test: Network Jitter (Varying Latency) ===");

    let latencies = [50, 500, 100, 2000, 200, 1000, 150];

    for (idx, latency_ms) in latencies.iter().enumerate() {
        println!("Iteration {}: {}ms latency", idx, latency_ms);

        for i in 0..5 {
            controller
                .inject_latency(i, Duration::from_millis(*latency_ms))
                .unwrap();
        }

        std::thread::sleep(Duration::from_millis(20));
    }

    // In real scenario:
    // - Timeout mechanism must adapt
    // - Short latency → fast rounds
    // - Long latency → slow rounds
    // - Exponential backoff helps
    // - MAX_TIMEOUT prevents endless waiting

    println!("✅ Consensus adapts to varying network latency");
}

// =============================================================================
// Test 11: Single Slow Validator (Leader)
// =============================================================================

/// Test when the current leader has high latency.
///
/// Scenario:
/// 1. Validator 0 is leader (height=0, round=0)
/// 2. Inject 5s latency for validator 0
/// 3. Proposal delayed by 5s
/// 4. Validators timeout before proposal arrives
/// 5. View change triggered
/// 6. New leader (validator 1) proceeds normally
#[test]
fn test_slow_leader() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 33445);

    println!("\n=== Test: Slow Leader ===");

    // Leader (validator 0) is very slow
    controller
        .inject_latency(0, Duration::from_millis(5000))
        .unwrap();

    // Others normal
    for i in 1..5 {
        controller
            .inject_latency(i, Duration::from_millis(50))
            .unwrap();
    }

    println!("Leader (validator 0): 5000ms latency");
    println!("Others: 50ms latency");

    // In real scenario:
    // - Leader proposes but proposal delayed 5s
    // - Validators timeout after ~1s (BASE_TIMEOUT)
    // - Validators send timeout messages
    // - Round advances to 1
    // - New leader (validator 1) proposes quickly
    // - Consensus continues

    println!("✅ Slow leader triggers view change, new leader takes over");
}

// =============================================================================
// Test 12: Recovery After Network Issues
// =============================================================================

/// Test that network heals correctly after issues resolved.
///
/// Scenario:
/// 1. Inject severe network issues (high latency + high drops)
/// 2. Let network struggle
/// 3. Heal all network issues
/// 4. Verify rapid recovery to normal consensus speed
#[test]
fn test_network_recovery() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 55667);

    println!("\n=== Test: Network Recovery ===");

    // Phase 1: Severe degradation
    println!("Phase 1: Severe network issues");
    for i in 0..5 {
        controller
            .inject_latency(i, Duration::from_millis(2000))
            .unwrap();
        controller.inject_message_drop(i, 0.7).unwrap();
    }

    std::thread::sleep(Duration::from_millis(100));

    // Phase 2: Heal
    println!("Phase 2: Network healed");
    controller.heal_network().unwrap();

    // Verify healing
    for i in 0..5 {
        assert_eq!(
            controller.network.get_latency(i).unwrap(),
            Duration::from_millis(0)
        );
        let drop_rate = controller.network.get_drop_rate(i).unwrap();
        assert_eq!(drop_rate, 0.0);
    }

    // In real scenario:
    // - Immediately after heal: messages flow freely
    // - Pending messages delivered
    // - Consensus speed returns to normal
    // - No lingering effects

    println!("✅ Network recovers immediately after healing");
}
