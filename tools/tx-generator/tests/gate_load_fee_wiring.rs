//! Gate LOAD: the adaptive fee must reach the wire.
//!
//! `gate_load_fee.rs` pins the policy. A policy that learns perfectly and is
//! never consulted fixes nothing, and the unit tests cannot tell the two
//! apart. These drive a real worker pool against a node that refuses on fee,
//! and assert that the refusal taught the shared policy and that the policy
//! is what decides the fee on the transaction.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tx_generator::fee::FeePolicy;
use tx_generator::generator::{Generator, GeneratorConfig, TxType};
use tx_generator::metrics::{metric_channel, MetricsCollector};
use tx_generator::sender::SenderPool;
use tx_generator::submitter::{fee_for_submission, Submitter, SubmitterConfig};

/// Exactly what `crates/node/src/rpc.rs` returns for a fee refusal.
const FEE_TOO_LOW_BODY: &str = r#"{"jsonrpc":"2.0","error":{"code":-32011,"message":"FeeTooLow: minimum 5000, got 1000"},"id":1}"#;

/// The stamping decision itself. With a policy attached, the policy decides;
/// without one, nothing changes.
#[test]
fn the_policy_is_what_decides_the_fee_on_the_transaction() {
    let policy = FeePolicy::new(1000);
    policy.observe_rejection("RPC error -32011: FeeTooLow: minimum 5000, got 1000");
    let learned = policy.current();
    assert!(learned > 5000);

    assert_eq!(
        fee_for_submission(Some(&policy), 1000),
        learned,
        "the transaction must carry what the policy learned, not the flag"
    );
    assert_eq!(
        fee_for_submission(None, 1000),
        1000,
        "with no policy attached the template fee is untouched"
    );
}

/// THE WIRING PIN. A live worker pool, a node that refuses everything on fee,
/// and the shared policy every worker stamps from. If the worker does not
/// feed refusals to the policy, the generator keeps offering 1000 forever and
/// burns a nonce on every one of the 13,669 refusals that ended the
/// 2026-08-28 run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_refused_on_fee_teaches_the_shared_policy() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("POST", "/")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(FEE_TOO_LOW_BODY)
        .expect_at_least(1)
        .create_async()
        .await;

    let paused = Arc::new(AtomicBool::new(false));
    let pool = Arc::new(SenderPool::new(4));
    let (metric_tx, metric_rx) = metric_channel();
    let _metrics_handle = MetricsCollector::new(false).start(metric_rx);
    let (tx_sender, tx_receiver) = mpsc::channel(200);

    let policy = Arc::new(FeePolicy::new(1000));
    assert_eq!(policy.current(), 1000, "starts at the configured fee");

    let submitter = Submitter::new(
        SubmitterConfig {
            endpoint: server.url(),
            worker_count: 4,
            ..Default::default()
        },
        Arc::clone(&pool),
        Arc::clone(&paused),
    )
    .with_fee_policy(Arc::clone(&policy));
    let submitter_handle = submitter.start(tx_receiver, metric_tx);

    let generator = Generator::new(
        GeneratorConfig {
            target_tps: 50,
            tx_type: TxType::Transfer,
            fee: 1000,
            max_duration: None,
        },
        Arc::clone(&pool),
        Arc::clone(&paused),
    );
    let generator_handle = generator.start(tx_sender);

    tokio::time::sleep(Duration::from_secs(3)).await;

    let learned = policy.current();
    assert!(
        learned > 5000,
        "the worker must have taught the policy the floor it was refused \
         against; the policy still offers {learned}"
    );
    assert_eq!(
        fee_for_submission(Some(&policy), 1000),
        learned,
        "and every later transaction must carry it"
    );

    generator_handle.shutdown();
    submitter_handle.shutdown();
    let _ = submitter_handle.wait().await;
}
