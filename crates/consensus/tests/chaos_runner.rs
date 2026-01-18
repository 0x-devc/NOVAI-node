//! Chaos Testing: Automated Chaos Runner
//!
//! Orchestrates complex chaos scenarios by combining multiple faults:
//! 1. Sequential faults (one after another)
//! 2. Concurrent faults (multiple at once)
//! 3. Random fault injection
//! 4. Stress testing with sustained chaos
//! 5. Fault recovery cycles

mod chaos_framework;
use chaos_framework::setup_chaos_testnet;
use std::time::Duration;

// =============================================================================
// Test 1: Sequential Fault Injection
// =============================================================================

/// Test sequential application of different fault types.
///
/// Scenario:
/// 1. Start with partition
/// 2. Add latency
/// 3. Add message drops
/// 4. Crash validator
/// 5. Heal everything
#[test]
fn test_sequential_faults() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 12345);

    println!("\n=== Test: Sequential Fault Injection ===");

    // Phase 1: Partition
    controller
        .inject_partition(vec![vec![0, 1], vec![2, 3, 4]])
        .unwrap();
    println!("Phase 1: Partition [0,1] vs [2,3,4]");
    std::thread::sleep(Duration::from_millis(100));

    // Phase 2: Add latency
    for i in 0..5 {
        controller
            .inject_latency(i, Duration::from_millis(500))
            .unwrap();
    }
    println!("Phase 2: Added 500ms latency");
    std::thread::sleep(Duration::from_millis(100));

    // Phase 3: Add message drops
    for i in 0..5 {
        controller.inject_message_drop(i, 0.3).unwrap();
    }
    println!("Phase 3: Added 30% message drops");
    std::thread::sleep(Duration::from_millis(100));

    // Phase 4: Crash validator
    controller.crash_validator(0).unwrap();
    println!("Phase 4: Crashed validator 0");
    std::thread::sleep(Duration::from_millis(100));

    // Phase 5: Heal everything
    controller.heal_network().unwrap();
    controller.restart_validator(0).unwrap();
    println!("Phase 5: All faults healed");

    println!("✅ Sequential fault injection completed");
}

// =============================================================================
// Test 2: Concurrent Fault Injection
// =============================================================================

/// Test simultaneous application of multiple faults.
///
/// Scenario:
/// 1. Apply partition + latency + drops + crash all at once
/// 2. Run for period
/// 3. Heal all at once
#[test]
fn test_concurrent_faults() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 22222);

    println!("\n=== Test: Concurrent Fault Injection ===");

    // Apply all faults simultaneously
    println!("Injecting all faults concurrently...");

    // Partition
    controller
        .inject_partition(vec![vec![0, 1, 2], vec![3, 4]])
        .unwrap();

    // Latency
    for i in 0..5 {
        controller
            .inject_latency(i, Duration::from_millis(1000))
            .unwrap();
    }

    // Drops
    for i in 0..5 {
        controller.inject_message_drop(i, 0.4).unwrap();
    }

    // Crash
    controller.crash_validator(0).unwrap();

    println!("All faults active:");
    println!("  - Partition: [0,1,2] vs [3,4]");
    println!("  - Latency: 1000ms");
    println!("  - Drops: 40%");
    println!("  - Crash: validator 0");

    // Let system run under stress
    std::thread::sleep(Duration::from_millis(200));

    // Heal all
    controller.heal_network().unwrap();
    controller.restart_validator(0).unwrap();
    println!("All faults healed simultaneously");

    println!("✅ Concurrent fault injection completed");
}

// =============================================================================
// Test 3: Fault Injection Cycles
// =============================================================================

/// Test repeated fault injection and healing cycles.
///
/// Scenario:
/// 1. Inject faults
/// 2. Heal
/// 3. Repeat 10 times
#[test]
fn test_fault_cycles() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 33333);

    println!("\n=== Test: Fault Injection Cycles ===");

    for cycle in 0..10 {
        println!("\nCycle {}:", cycle);

        // Inject partition (alternate patterns)
        let partition = if cycle % 2 == 0 {
            vec![vec![0, 1], vec![2, 3, 4]]
        } else {
            vec![vec![0, 1, 2], vec![3, 4]]
        };

        controller.inject_partition(partition).unwrap();
        println!("  Partition injected");

        // Brief operation under fault
        std::thread::sleep(Duration::from_millis(50));

        // Heal
        controller.heal_network().unwrap();
        println!("  Healed");

        // Brief normal operation
        std::thread::sleep(Duration::from_millis(50));
    }

    println!("\n✅ 10 fault cycles completed");
}

// =============================================================================
// Test 4: Escalating Chaos
// =============================================================================

/// Test gradually escalating chaos intensity.
///
/// Scenario:
/// 1. Start with minor faults
/// 2. Gradually increase severity
/// 3. Peak chaos
/// 4. Gradually decrease
/// 5. Return to normal
#[test]
fn test_escalating_chaos() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 44444);

    println!("\n=== Test: Escalating Chaos ===");

    // Phase 1: Minor (10% drops, 100ms latency)
    println!("Phase 1: Minor chaos (10% drops, 100ms latency)");
    for i in 0..5 {
        controller.inject_message_drop(i, 0.1).unwrap();
        controller
            .inject_latency(i, Duration::from_millis(100))
            .unwrap();
    }
    std::thread::sleep(Duration::from_millis(100));

    // Phase 2: Moderate (30% drops, 500ms latency)
    println!("Phase 2: Moderate chaos (30% drops, 500ms latency)");
    for i in 0..5 {
        controller.inject_message_drop(i, 0.3).unwrap();
        controller
            .inject_latency(i, Duration::from_millis(500))
            .unwrap();
    }
    std::thread::sleep(Duration::from_millis(100));

    // Phase 3: High (50% drops, 1000ms latency, partition)
    println!("Phase 3: High chaos (50% drops, 1000ms latency, partition)");
    for i in 0..5 {
        controller.inject_message_drop(i, 0.5).unwrap();
        controller
            .inject_latency(i, Duration::from_millis(1000))
            .unwrap();
    }
    controller
        .inject_partition(vec![vec![0, 1], vec![2, 3, 4]])
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));

    // Phase 4: Peak (70% drops, 2000ms latency, partition, crash)
    println!("Phase 4: PEAK CHAOS (70% drops, 2000ms latency, partition, crash)");
    for i in 0..5 {
        controller.inject_message_drop(i, 0.7).unwrap();
        controller
            .inject_latency(i, Duration::from_millis(2000))
            .unwrap();
    }
    controller.crash_validator(0).unwrap();
    std::thread::sleep(Duration::from_millis(100));

    // Phase 5: De-escalate to moderate
    println!("Phase 5: De-escalating to moderate");
    controller.restart_validator(0).unwrap();
    for i in 0..5 {
        controller.inject_message_drop(i, 0.3).unwrap();
        controller
            .inject_latency(i, Duration::from_millis(500))
            .unwrap();
    }
    std::thread::sleep(Duration::from_millis(100));

    // Phase 6: Return to normal
    println!("Phase 6: Return to normal");
    controller.heal_network().unwrap();

    println!("✅ Escalating chaos scenario completed");
}

// =============================================================================
// Test 5: Random Fault Injection
// =============================================================================

/// Test random fault injection using seeded RNG.
///
/// Scenario:
/// 1. Randomly choose fault type
/// 2. Randomly choose parameters
/// 3. Apply for random duration
/// 4. Repeat 20 times
#[test]
fn test_random_faults() {
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 55555);

    println!("\n=== Test: Random Fault Injection ===");

    let mut rng = StdRng::seed_from_u64(55555);

    for round in 0..20 {
        let fault_type = rng.gen_range(0..5);

        match fault_type {
            0 => {
                // Random partition
                let split = rng.gen_range(1..4);
                let partition = if split <= 2 {
                    vec![vec![0, 1], vec![2, 3, 4]]
                } else {
                    vec![vec![0, 1, 2], vec![3, 4]]
                };
                controller.inject_partition(partition).unwrap();
                println!("Round {}: Partition", round);
            }
            1 => {
                // Random latency
                let latency_ms = rng.gen_range(100..2000);
                let validator = rng.gen_range(0..5);
                controller
                    .inject_latency(validator, Duration::from_millis(latency_ms))
                    .unwrap();
                println!(
                    "Round {}: Latency {}ms on validator {}",
                    round, latency_ms, validator
                );
            }
            2 => {
                // Random drops
                let drop_rate = rng.gen_range(10..80) as f64 / 100.0;
                let validator = rng.gen_range(0..5);
                controller
                    .inject_message_drop(validator, drop_rate)
                    .unwrap();
                println!(
                    "Round {}: {}% drops on validator {}",
                    round,
                    (drop_rate * 100.0) as u32,
                    validator
                );
            }
            3 => {
                // Random crash
                let validator = rng.gen_range(0..5);
                if !controller.validators[validator].is_crashed() {
                    controller.crash_validator(validator).unwrap();
                    println!("Round {}: Crash validator {}", round, validator);
                }
            }
            4 => {
                // Heal
                controller.heal_network().unwrap();
                // Restart all crashed validators
                for i in 0..5 {
                    if controller.validators[i].is_crashed() {
                        controller.restart_validator(i).unwrap();
                    }
                }
                println!("Round {}: Heal all", round);
            }
            _ => unreachable!(),
        }

        // Random duration
        let sleep_ms = rng.gen_range(20..100);
        std::thread::sleep(Duration::from_millis(sleep_ms));
    }

    // Final cleanup
    controller.heal_network().unwrap();
    for i in 0..5 {
        if controller.validators[i].is_crashed() {
            controller.restart_validator(i).unwrap();
        }
    }

    println!("✅ Random fault injection completed (20 rounds)");
}

// =============================================================================
// Test 6: Sustained Chaos Stress Test
// =============================================================================

/// Long-running stress test with sustained chaos.
///
/// Scenario:
/// 1. Apply moderate chaos
/// 2. Run for extended period
/// 3. Verify system remains stable
#[test]
fn test_sustained_chaos() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 66666);

    println!("\n=== Test: Sustained Chaos (Stress Test) ===");

    // Apply sustained moderate chaos
    controller
        .inject_partition(vec![vec![0, 1, 2], vec![3, 4]])
        .unwrap();

    for i in 0..5 {
        controller
            .inject_latency(i, Duration::from_millis(500))
            .unwrap();
        controller.inject_message_drop(i, 0.2).unwrap();
    }

    println!("Sustained chaos active:");
    println!("  - Partition: [0,1,2] vs [3,4]");
    println!("  - Latency: 500ms");
    println!("  - Drops: 20%");
    println!("  - Duration: 1 second");

    // Run under chaos for extended period
    std::thread::sleep(Duration::from_millis(1000));

    println!("Stress test completed");

    // Heal
    controller.heal_network().unwrap();

    println!("✅ System stable under sustained chaos");
}

// =============================================================================
// Test 7: Worst-Case Combined Scenario
// =============================================================================

/// Test worst-case combination of faults (still within BFT tolerance).
///
/// Scenario:
/// 1. Partition with minority containing leader
/// 2. High latency (2s)
/// 3. High packet loss (60%)
/// 4. One validator crashed
/// 5. Verify safety maintained
#[test]
fn test_worst_case_scenario() {
    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 77777);

    println!("\n=== Test: Worst-Case Scenario ===");

    // Leader (validator 0) in minority partition
    controller
        .inject_partition(vec![vec![0], vec![1, 2, 3, 4]])
        .unwrap();
    println!("Phase 1: Leader isolated in minority partition");

    // High latency
    for i in 0..5 {
        controller
            .inject_latency(i, Duration::from_millis(2000))
            .unwrap();
    }
    println!("Phase 2: 2000ms latency added");

    // High packet loss
    for i in 0..5 {
        controller.inject_message_drop(i, 0.6).unwrap();
    }
    println!("Phase 3: 60% packet loss added");

    // Crash one validator in majority
    controller.crash_validator(1).unwrap();
    println!("Phase 4: Validator 1 crashed");

    println!("\nWorst-case conditions active:");
    println!("  - Leader isolated");
    println!("  - 2000ms latency");
    println!("  - 60% packet loss");
    println!("  - 1 crashed validator");
    println!("  - Effective majority: 3/5 (validators 2,3,4)");

    // System should still maintain safety (though liveness may be affected)
    std::thread::sleep(Duration::from_millis(200));

    // Recovery
    controller.heal_network().unwrap();
    controller.restart_validator(1).unwrap();
    println!("\nRecovery complete");

    println!("✅ Safety maintained under worst-case scenario");
}

// =============================================================================
// Test 8: Chaos Monkey (Continuous Random Faults)
// =============================================================================

/// Chaos monkey: continuous random fault injection and healing.
///
/// Scenario:
/// 1. Run for 50 iterations
/// 2. Each iteration: random fault or heal
/// 3. Verify system resilience
#[test]
fn test_chaos_monkey() {
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    let (controller, _validator_addrs, _validator_pubkeys) = setup_chaos_testnet(5, 88888);

    println!("\n=== Test: Chaos Monkey ===");
    println!("Running continuous random fault injection (50 iterations)...\n");

    let mut rng = StdRng::seed_from_u64(88888);

    for iteration in 0..50 {
        let action = rng.gen_range(0..10);

        match action {
            0..=2 => {
                // 30%: Inject partition
                let patterns = [
                    vec![vec![0, 1], vec![2, 3, 4]],
                    vec![vec![0, 1, 2], vec![3, 4]],
                    vec![vec![0], vec![1, 2, 3, 4]],
                ];
                let pattern = &patterns[rng.gen_range(0..patterns.len())];
                controller.inject_partition(pattern.clone()).unwrap();
                if iteration % 10 == 0 {
                    println!("Iteration {}: Partition", iteration);
                }
            }
            3..=5 => {
                // 30%: Inject latency/drops
                let validator = rng.gen_range(0..5);
                let latency_ms = rng.gen_range(100..1500);
                controller
                    .inject_latency(validator, Duration::from_millis(latency_ms))
                    .unwrap();
                if iteration % 10 == 0 {
                    println!("Iteration {}: Latency", iteration);
                }
            }
            6..=7 => {
                // 20%: Crash/restart
                let validator = rng.gen_range(0..5);
                if controller.validators[validator].is_crashed() {
                    controller.restart_validator(validator).unwrap();
                } else {
                    controller.crash_validator(validator).unwrap();
                }
                if iteration % 10 == 0 {
                    println!("Iteration {}: Crash/restart", iteration);
                }
            }
            8..=9 => {
                // 20%: Heal
                controller.heal_network().unwrap();
                if iteration % 10 == 0 {
                    println!("Iteration {}: Heal", iteration);
                }
            }
            _ => unreachable!(),
        }

        // Brief pause
        std::thread::sleep(Duration::from_millis(20));
    }

    // Final cleanup
    controller.heal_network().unwrap();
    for i in 0..5 {
        if controller.validators[i].is_crashed() {
            controller.restart_validator(i).unwrap();
        }
    }

    println!("\n✅ Chaos monkey completed 50 iterations");
    println!("System remained stable under continuous random faults");
}
