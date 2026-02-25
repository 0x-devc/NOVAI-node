//! Inference task scheduler for the AI service.
//!
//! PURPOSE: Manages a queue of inference tasks and processes them through the
//! Anthropic client. Supports both triggered (on-demand) and periodic scheduling.
//!
//! INVARIANTS:
//! - Tasks are processed in FIFO order (priority is metadata only)
//! - Each task is processed in its own spawned tokio task
//! - Queue capacity is bounded to prevent unbounded memory growth
//!
//! FAILURE MODES:
//! - Queue full → `try_submit` returns error (non-blocking)
//! - Client error → callback receives error, task is not retried

use crate::client::AnthropicClient;
use crate::error::AiServiceError;
use crate::types::{AiAnalysisResponse, ChainSnapshot, InferenceType};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};

/// A task to be processed by the inference scheduler.
#[derive(Debug, Clone)]
pub struct InferenceTask {
    /// Type of inference to run.
    pub inference_type: InferenceType,

    /// Chain snapshot to analyze.
    pub snapshot: ChainSnapshot,

    /// Priority hint (0–255, higher = more important).
    /// Currently for logging only — tasks are processed FIFO.
    pub priority: u8,
}

/// Callback for inference results.
///
/// Implementations receive both successful and failed inference outcomes.
pub trait InferenceCallback: Send + Sync {
    /// Called when an inference completes successfully.
    fn on_result(&self, response: AiAnalysisResponse);

    /// Called when an inference fails.
    fn on_error(&self, inference_type: InferenceType, error: AiServiceError);
}

/// Logging callback that traces inference results.
pub struct LoggingInferenceCallback;

impl InferenceCallback for LoggingInferenceCallback {
    fn on_result(&self, response: AiAnalysisResponse) {
        tracing::info!(
            inference_type = response.inference_type.name(),
            findings = response.findings.len(),
            confidence = response.confidence,
            "AI inference complete"
        );
    }

    fn on_error(&self, inference_type: InferenceType, error: AiServiceError) {
        tracing::warn!(
            inference_type = inference_type.name(),
            %error,
            "AI inference failed"
        );
    }
}

/// Schedules and manages AI inference tasks.
///
/// Create with `new`, submit tasks via `submit` or `try_submit`,
/// and spawn the processing loop with `run`.
pub struct InferenceScheduler {
    client: Arc<AnthropicClient>,
    tx: mpsc::Sender<InferenceTask>,
}

impl InferenceScheduler {
    /// Create a new scheduler.
    ///
    /// Returns the scheduler (for submitting tasks) and the receiver end
    /// (for the processing loop). Pass the receiver to `run` to start
    /// processing tasks.
    #[must_use]
    pub fn new(
        client: Arc<AnthropicClient>,
        queue_capacity: usize,
    ) -> (Self, mpsc::Receiver<InferenceTask>) {
        let (tx, rx) = mpsc::channel(queue_capacity);
        (Self { client, tx }, rx)
    }

    /// Submit a task for inference (async, waits if queue is full).
    ///
    /// # Errors
    ///
    /// Returns error if the receiver has been dropped (scheduler stopped).
    pub async fn submit(&self, task: InferenceTask) -> Result<(), AiServiceError> {
        self.tx
            .send(task)
            .await
            .map_err(|_| AiServiceError::HttpError("scheduler queue closed".to_string()))
    }

    /// Submit a task for inference (non-blocking, fails if queue is full).
    ///
    /// # Errors
    ///
    /// Returns error if the queue is full or the receiver has been dropped.
    pub fn try_submit(&self, task: InferenceTask) -> Result<(), AiServiceError> {
        self.tx
            .try_send(task)
            .map_err(|_| AiServiceError::HttpError("scheduler queue full or closed".to_string()))
    }

    /// Get a reference to the underlying client.
    #[must_use]
    pub fn client(&self) -> &Arc<AnthropicClient> {
        &self.client
    }

    /// Process tasks from the queue until the sender is dropped.
    ///
    /// Each task is spawned as a separate tokio task for concurrent processing.
    /// Call this from a `tokio::spawn` to run in the background.
    pub async fn run(
        mut rx: mpsc::Receiver<InferenceTask>,
        client: Arc<AnthropicClient>,
        callback: Arc<dyn InferenceCallback>,
    ) {
        tracing::info!("Inference scheduler started");

        while let Some(task) = rx.recv().await {
            let client = Arc::clone(&client);
            let callback = Arc::clone(&callback);

            tracing::debug!(
                inference_type = task.inference_type.name(),
                priority = task.priority,
                height = task.snapshot.height,
                "Processing inference task"
            );

            tokio::spawn(async move {
                match client.analyze(task.inference_type, &task.snapshot).await {
                    Ok(response) => callback.on_result(response),
                    Err(e) => callback.on_error(task.inference_type, e),
                }
            });
        }

        tracing::info!("Inference scheduler stopped (sender dropped)");
    }

    /// Run periodic inference at a fixed interval.
    ///
    /// Calls `snapshot_fn` to get the current chain state, then submits
    /// an inference request. Runs indefinitely until the future is dropped.
    pub async fn run_periodic<F>(
        client: Arc<AnthropicClient>,
        inference_type: InferenceType,
        period: Duration,
        snapshot_fn: F,
        callback: Arc<dyn InferenceCallback>,
    ) where
        F: Fn() -> ChainSnapshot + Send + 'static,
    {
        let mut ticker = interval(period);

        loop {
            ticker.tick().await;

            let snapshot = snapshot_fn();
            let client = Arc::clone(&client);
            let callback = Arc::clone(&callback);

            tokio::spawn(async move {
                match client.analyze(inference_type, &snapshot).await {
                    Ok(response) => callback.on_result(response),
                    Err(e) => callback.on_error(inference_type, e),
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AiServiceConfig;

    fn test_snapshot() -> ChainSnapshot {
        ChainSnapshot {
            height: 100,
            round: 5,
            peer_count: 4,
            mempool_size: 50,
            view_changes: 2,
            validator_count: 4,
            recent_anomalies: vec![],
        }
    }

    #[test]
    fn inference_task_fields() {
        let task = InferenceTask {
            inference_type: InferenceType::AnomalyAnalysis,
            snapshot: test_snapshot(),
            priority: 128,
        };

        assert_eq!(task.inference_type, InferenceType::AnomalyAnalysis);
        assert_eq!(task.priority, 128);
        assert_eq!(task.snapshot.height, 100);
    }

    #[test]
    fn logging_callback_does_not_panic() {
        let cb = LoggingInferenceCallback;

        let response = AiAnalysisResponse {
            inference_type: InferenceType::GeneralAnalysis,
            findings: vec![],
            confidence: 50,
            recommendation: "test".into(),
            raw_response: "raw".into(),
        };
        cb.on_result(response);

        cb.on_error(InferenceType::AnomalyAnalysis, AiServiceError::Timeout);
    }

    #[tokio::test]
    async fn scheduler_submit_and_receive() {
        // Create a client with a test key (won't actually make API calls)
        let config = AiServiceConfig {
            enabled: true,
            api_key: Some("test-key".into()),
            ..AiServiceConfig::default()
        };
        let client = Arc::new(AnthropicClient::new(config).expect("create client"));

        let (scheduler, mut rx) = InferenceScheduler::new(client, 10);

        let task = InferenceTask {
            inference_type: InferenceType::GeneralAnalysis,
            snapshot: test_snapshot(),
            priority: 100,
        };

        scheduler.submit(task).await.expect("submit");

        let received = rx.recv().await.expect("receive task");
        assert_eq!(received.inference_type, InferenceType::GeneralAnalysis);
        assert_eq!(received.snapshot.height, 100);
    }

    #[tokio::test]
    async fn scheduler_try_submit_fails_when_full() {
        let config = AiServiceConfig {
            enabled: true,
            api_key: Some("test-key".into()),
            ..AiServiceConfig::default()
        };
        let client = Arc::new(AnthropicClient::new(config).expect("create client"));

        // Queue capacity of 1
        let (scheduler, _rx) = InferenceScheduler::new(client, 1);

        let task = InferenceTask {
            inference_type: InferenceType::AnomalyAnalysis,
            snapshot: test_snapshot(),
            priority: 50,
        };

        // First submit should succeed
        scheduler.try_submit(task.clone()).expect("first submit");

        // Second submit should fail (queue full)
        let result = scheduler.try_submit(task);
        assert!(result.is_err());
    }
}
