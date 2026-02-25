//! AI service runner that orchestrates inference triggered by copilot anomalies.
//!
//! PURPOSE: Main loop that receives anomaly triggers from the copilot bridge
//! and dispatches AI inference tasks. Runs in its own tokio async runtime
//! (Thread 3) to avoid blocking consensus.
//!
//! INVARIANTS:
//! - Never blocks the consensus thread
//! - Respects per-feature enable/disable flags
//! - Continues on any error (logs, doesn't crash)
//! - Exits cleanly on shutdown signal
//!
//! FAILURE MODES:
//! - API error → logged, runner continues
//! - Channel closed → runner exits main loop
//! - Shutdown flag → runner exits main loop

use crate::bridge::AnomalyTrigger;
use crate::client::AnthropicClient;
use crate::error::AiServiceError;
use crate::types::{AiAnalysisResponse, ChainSnapshot, InferenceType};
use novai_ai_entities::AiSignalType;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Per-feature enable/disable flags for the AI service.
#[derive(Debug, Clone)]
pub struct FeatureFlags {
    /// Analyze detected anomalies via AI.
    pub anomaly_analysis_enabled: bool,

    /// Analyze congestion patterns via AI.
    pub congestion_analysis_enabled: bool,

    /// Monitor validator behavior via AI.
    pub validator_monitoring_enabled: bool,

    /// Optimize mempool operations via AI (future).
    pub mempool_optimization_enabled: bool,

    /// Security scanning via AI (future).
    pub security_scanning_enabled: bool,

    /// Periodic health reports via AI.
    pub health_reports_enabled: bool,
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            anomaly_analysis_enabled: true,
            congestion_analysis_enabled: true,
            validator_monitoring_enabled: true,
            mempool_optimization_enabled: false, // Future
            security_scanning_enabled: false,    // Future
            health_reports_enabled: true,
        }
    }
}

/// Orchestrates AI inference in response to copilot triggers.
pub struct AiServiceRunner {
    client: Arc<AnthropicClient>,
    trigger_rx: mpsc::Receiver<AnomalyTrigger>,
    shutdown: Arc<AtomicBool>,
    features: FeatureFlags,
    current_height: Arc<AtomicU64>,
    recent_anomalies: Vec<String>,
}

impl AiServiceRunner {
    /// Create a new runner.
    ///
    /// # Arguments
    /// - `client`: Anthropic API client
    /// - `trigger_rx`: Receiver for anomaly triggers from the copilot bridge
    /// - `shutdown`: Shared shutdown flag (set by signal handler)
    /// - `features`: Per-feature enable/disable flags
    #[must_use]
    pub fn new(
        client: Arc<AnthropicClient>,
        trigger_rx: mpsc::Receiver<AnomalyTrigger>,
        shutdown: Arc<AtomicBool>,
        features: FeatureFlags,
    ) -> Self {
        Self {
            client,
            trigger_rx,
            shutdown,
            features,
            current_height: Arc::new(AtomicU64::new(0)),
            recent_anomalies: Vec::new(),
        }
    }

    /// Get a handle to update the current height from another thread.
    #[must_use]
    pub fn height_handle(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.current_height)
    }

    /// Main loop — runs until shutdown or channel closes.
    pub async fn run(&mut self) {
        tracing::info!("AI service runner started");

        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                tracing::info!("AI service runner: shutdown signal received");
                break;
            }

            // Wait for a trigger or timeout (poll shutdown periodically)
            tokio::select! {
                trigger = self.trigger_rx.recv() => {
                    match trigger {
                        Some(t) => self.handle_trigger(t).await,
                        None => {
                            tracing::info!("AI service runner: trigger channel closed");
                            break;
                        }
                    }
                }
                () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                    // Periodic check for shutdown — no action needed
                }
            }
        }

        tracing::info!("AI service runner stopped");
    }

    /// Handle a single anomaly trigger.
    async fn handle_trigger(&mut self, trigger: AnomalyTrigger) {
        let inference_type = match trigger.signal_type {
            AiSignalType::Anomaly | AiSignalType::RiskScore => InferenceType::AnomalyAnalysis,
            AiSignalType::CongestionForecast => InferenceType::CongestionForecast,
            AiSignalType::SpamRisk => InferenceType::AnomalyAnalysis,
            AiSignalType::AuditReport => InferenceType::EntityAudit,
            _ => InferenceType::GeneralAnalysis,
        };

        // Check per-feature flag
        if !self.is_feature_enabled(inference_type) {
            tracing::debug!(
                inference_type = inference_type.name(),
                "Skipping inference — disabled in config"
            );
            return;
        }

        // Track recent anomalies for context
        self.recent_anomalies.push(trigger.details.clone());
        // Keep only the last 10
        if self.recent_anomalies.len() > 10 {
            self.recent_anomalies.remove(0);
        }

        tracing::info!(
            signal_type = ?trigger.signal_type,
            confidence = trigger.confidence,
            height = trigger.height,
            inference_type = inference_type.name(),
            "AI inference triggered by anomaly"
        );

        match self.run_inference(inference_type).await {
            Ok(response) => {
                tracing::info!(
                    inference_type = response.inference_type.name(),
                    findings = response.findings.len(),
                    confidence = response.confidence,
                    "AI analysis complete"
                );
            }
            Err(e) => {
                tracing::warn!(
                    inference_type = inference_type.name(),
                    %e,
                    "AI inference failed"
                );
            }
        }
    }

    /// Run a single inference request.
    ///
    /// # Errors
    ///
    /// Returns `AiServiceError` on API or network failures.
    async fn run_inference(
        &self,
        inference_type: InferenceType,
    ) -> Result<AiAnalysisResponse, AiServiceError> {
        let snapshot = self.build_snapshot();
        self.client.analyze(inference_type, &snapshot).await
    }

    /// Build a chain snapshot from current state.
    fn build_snapshot(&self) -> ChainSnapshot {
        ChainSnapshot {
            height: self.current_height.load(Ordering::Relaxed),
            round: 0,           // Enriched later via ObservableState
            peer_count: 0,      // Enriched later
            mempool_size: 0,    // Enriched later
            view_changes: 0,    // Enriched later
            validator_count: 0, // Enriched later
            recent_anomalies: self.recent_anomalies.clone(),
        }
    }

    /// Check if a given inference type is enabled.
    fn is_feature_enabled(&self, inference_type: InferenceType) -> bool {
        match inference_type {
            InferenceType::AnomalyAnalysis => self.features.anomaly_analysis_enabled,
            InferenceType::CongestionForecast => self.features.congestion_analysis_enabled,
            InferenceType::EntityAudit => self.features.validator_monitoring_enabled,
            InferenceType::GovernanceReview => true, // Always enabled when AI service is on
            InferenceType::GeneralAnalysis => self.features.health_reports_enabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AiServiceConfig;

    #[test]
    fn feature_flags_default() {
        let flags = FeatureFlags::default();
        assert!(flags.anomaly_analysis_enabled);
        assert!(flags.congestion_analysis_enabled);
        assert!(flags.validator_monitoring_enabled);
        assert!(!flags.mempool_optimization_enabled);
        assert!(!flags.security_scanning_enabled);
        assert!(flags.health_reports_enabled);
    }

    #[test]
    fn runner_feature_check() {
        let (_, rx) = mpsc::channel(1);
        let config = AiServiceConfig {
            enabled: true,
            api_key: Some("test-key".into()),
            ..AiServiceConfig::default()
        };
        let client = Arc::new(AnthropicClient::new(config).expect("client"));
        let shutdown = Arc::new(AtomicBool::new(false));

        let features = FeatureFlags {
            anomaly_analysis_enabled: false,
            ..FeatureFlags::default()
        };

        let runner = AiServiceRunner::new(client, rx, shutdown, features);

        // Anomaly analysis disabled
        assert!(!runner.is_feature_enabled(InferenceType::AnomalyAnalysis));
        // Others still enabled
        assert!(runner.is_feature_enabled(InferenceType::CongestionForecast));
        assert!(runner.is_feature_enabled(InferenceType::GeneralAnalysis));
        assert!(runner.is_feature_enabled(InferenceType::GovernanceReview));
    }

    #[test]
    fn runner_build_snapshot() {
        let (_, rx) = mpsc::channel(1);
        let config = AiServiceConfig {
            enabled: true,
            api_key: Some("test-key".into()),
            ..AiServiceConfig::default()
        };
        let client = Arc::new(AnthropicClient::new(config).expect("client"));
        let shutdown = Arc::new(AtomicBool::new(false));

        let runner = AiServiceRunner::new(client, rx, shutdown, FeatureFlags::default());

        // Set height via handle
        runner.height_handle().store(500, Ordering::Relaxed);

        let snapshot = runner.build_snapshot();
        assert_eq!(snapshot.height, 500);
        assert!(snapshot.recent_anomalies.is_empty());
    }

    #[tokio::test]
    async fn runner_stops_on_shutdown() {
        let (tx, rx) = mpsc::channel(32);
        let config = AiServiceConfig {
            enabled: true,
            api_key: Some("test-key".into()),
            ..AiServiceConfig::default()
        };
        let client = Arc::new(AnthropicClient::new(config).expect("client"));
        let shutdown = Arc::new(AtomicBool::new(false));

        let mut runner =
            AiServiceRunner::new(client, rx, Arc::clone(&shutdown), FeatureFlags::default());

        // Signal shutdown immediately
        shutdown.store(true, Ordering::Relaxed);

        // Drop sender so channel also closes
        drop(tx);

        // Runner should exit quickly
        tokio::time::timeout(std::time::Duration::from_secs(5), runner.run())
            .await
            .expect("runner should stop within 5 seconds");
    }

    #[tokio::test]
    async fn runner_stops_on_channel_close() {
        let (tx, rx) = mpsc::channel(32);
        let config = AiServiceConfig {
            enabled: true,
            api_key: Some("test-key".into()),
            ..AiServiceConfig::default()
        };
        let client = Arc::new(AnthropicClient::new(config).expect("client"));
        let shutdown = Arc::new(AtomicBool::new(false));

        let mut runner = AiServiceRunner::new(client, rx, shutdown, FeatureFlags::default());

        // Drop sender — channel closes
        drop(tx);

        // Runner should exit
        tokio::time::timeout(std::time::Duration::from_secs(5), runner.run())
            .await
            .expect("runner should stop when channel closes");
    }

    #[test]
    fn runner_disabled_feature_skips() {
        let (_, rx) = mpsc::channel(1);
        let config = AiServiceConfig {
            enabled: true,
            api_key: Some("test-key".into()),
            ..AiServiceConfig::default()
        };
        let client = Arc::new(AnthropicClient::new(config).expect("client"));
        let shutdown = Arc::new(AtomicBool::new(false));

        let features = FeatureFlags {
            anomaly_analysis_enabled: false,
            congestion_analysis_enabled: false,
            validator_monitoring_enabled: false,
            mempool_optimization_enabled: false,
            security_scanning_enabled: false,
            health_reports_enabled: false,
        };

        let runner = AiServiceRunner::new(client, rx, shutdown, features);

        // All types disabled
        assert!(!runner.is_feature_enabled(InferenceType::AnomalyAnalysis));
        assert!(!runner.is_feature_enabled(InferenceType::CongestionForecast));
        assert!(!runner.is_feature_enabled(InferenceType::EntityAudit));
        assert!(!runner.is_feature_enabled(InferenceType::GeneralAnalysis));
        // Governance always enabled
        assert!(runner.is_feature_enabled(InferenceType::GovernanceReview));
    }
}
