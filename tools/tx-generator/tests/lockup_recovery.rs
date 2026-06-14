//! Integration test: workers recover from a mid-run MempoolFull period.
//!
//! Reproduces the original production lockup scenario in a self-contained
//! test harness. Phase 1 confirms the system is healthy. Phase 2 swaps
//! the mock to MempoolFull responses so the submitter engages the pause
//! flag and the generator stops sending. Phase 3 swaps the mock back to
//! success responses and asserts the system recovers without operator
//! intervention. Before the architectural fix, Phase 3 would never see
//! the accepted counter resume because workers were architecturally
//! incapable of being woken once parked on recv with paused stuck true.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tx_generator::generator::{Generator, GeneratorConfig, TxType};
use tx_generator::metrics::{metric_channel, MetricsCollector};
use tx_generator::sender::SenderPool;
use tx_generator::submitter::{Submitter, SubmitterConfig};

const OK_BODY: &str = r#"{"jsonrpc":"2.0","result":{"txid":"0000000000000000000000000000000000000000000000000000000000000000"},"id":1}"#;
const MEMPOOL_FULL_BODY: &str =
    r#"{"jsonrpc":"2.0","error":{"code":-32001,"message":"MempoolFull"},"id":1}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workers_recover_after_mempool_full_period() {
    let mut server = mockito::Server::new_async().await;

    // ----- Phase 1: mock returns success -----
    let mock_ok_1 = server
        .mock("POST", "/")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(OK_BODY)
        .expect_at_least(1)
        .create_async()
        .await;

    let endpoint = server.url();
    let paused = Arc::new(AtomicBool::new(false));
    let pool = Arc::new(SenderPool::new(10));

    let (metric_tx, metric_rx) = metric_channel();
    let metrics_collector = MetricsCollector::new(false);
    let metrics_handle = metrics_collector.start(metric_rx);
    let metrics_state = metrics_handle.clone_state();

    let (tx_sender, tx_receiver) = mpsc::channel(200);

    let submitter_config = SubmitterConfig {
        endpoint: endpoint.clone(),
        worker_count: 4,
        ..Default::default()
    };
    let submitter = Submitter::new(submitter_config, Arc::clone(&pool), Arc::clone(&paused));
    let submitter_handle = submitter.start(tx_receiver, metric_tx);

    let generator_config = GeneratorConfig {
        target_tps: 100,
        tx_type: TxType::Transfer,
        fee: 1,
        max_duration: None,
    };
    let generator = Generator::new(generator_config, Arc::clone(&pool), Arc::clone(&paused));
    let generator_handle = generator.start(tx_sender);

    // Allow Phase 1 to run for 4 seconds so the system is fully warm.
    tokio::time::sleep(Duration::from_secs(4)).await;

    let phase1_accepted = metrics_state.read().await.accepted_count();
    assert!(
        phase1_accepted > 0,
        "phase 1: accepted counter should have grown, got {phase1_accepted}"
    );
    assert!(
        !paused.load(Ordering::Relaxed),
        "phase 1: paused must be false during normal operation"
    );

    // ----- Phase 2: swap mock to MempoolFull -----
    drop(mock_ok_1);
    let mock_mempool_full = server
        .mock("POST", "/")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(MEMPOOL_FULL_BODY)
        .expect_at_least(1)
        .create_async()
        .await;

    // Wait long enough for the submitter to see at least one MempoolFull
    // response and engage the pause flag. The first MempoolFull error
    // sets paused; subsequent retries sleep 2 s between attempts.
    tokio::time::sleep(Duration::from_secs(8)).await;

    let phase2_accepted = metrics_state.read().await.accepted_count();
    assert!(
        paused.load(Ordering::Relaxed),
        "phase 2: paused must be set after sustained MempoolFull responses"
    );
    // The counter is allowed to grow by the few in-flight submissions
    // that were already past the channel boundary when Phase 2 began.
    // It should not have grown by the Phase 1 magnitude.
    let phase2_delta = phase2_accepted.saturating_sub(phase1_accepted);
    assert!(
        phase2_delta < phase1_accepted,
        "phase 2: accepted should plateau, but grew by {phase2_delta} from {phase1_accepted}"
    );

    // ----- Phase 3: swap mock back to success and assert recovery -----
    drop(mock_mempool_full);
    let _mock_ok_2 = server
        .mock("POST", "/")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(OK_BODY)
        .expect_at_least(1)
        .create_async()
        .await;

    // Recovery budget: worker's MempoolFull retry sleeps up to 2 s
    // between attempts; on the next attempt against the OK mock it
    // succeeds, clears paused, the generator resumes within 100 ms,
    // and downstream submissions land. 8 seconds is a safe margin.
    tokio::time::sleep(Duration::from_secs(8)).await;

    let phase3_accepted = metrics_state.read().await.accepted_count();
    let phase3_delta = phase3_accepted.saturating_sub(phase2_accepted);
    assert!(
        !paused.load(Ordering::Relaxed),
        "phase 3: paused must clear after the first successful submission"
    );
    assert!(
        phase3_delta > 10,
        "phase 3: accepted must resume growing after recovery, grew by only {phase3_delta} from {phase2_accepted}"
    );

    // Clean shutdown.
    generator_handle.shutdown();
    submitter_handle.shutdown();
    let _ = submitter_handle.wait().await;
}
