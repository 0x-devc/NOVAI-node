//! Core types for the AI service.
//!
//! PURPOSE: Data structures for configuration, inference requests/responses,
//! and chain state snapshots. All types are Rail B (advisory only).

use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// AI provider wire protocol.
///
/// `Anthropic` targets the Anthropic Messages API. `OpenAiCompatible` targets
/// any endpoint speaking the OpenAI Chat Completions API. That covers the
/// hosted OpenAI API and, more importantly for a decentralized network, local
/// or self-hosted runtimes such as Ollama, vLLM, LM Studio, and the llama.cpp
/// server. Selecting `OpenAiCompatible` with a loopback `base_url` lets a
/// validator run inference with no external provider at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiProvider {
    /// Anthropic Messages API (`x-api-key` auth, `/v1/messages`).
    Anthropic,
    /// OpenAI Chat Completions API and compatible local servers
    /// (`Authorization: Bearer` auth when a key is set, `/v1/chat/completions`).
    OpenAiCompatible,
}

impl AiProvider {
    /// Stable lowercase identifier for logging, metrics, and config files.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAiCompatible => "openai_compatible",
        }
    }
}

impl Default for AiProvider {
    fn default() -> Self {
        Self::Anthropic
    }
}

impl FromStr for AiProvider {
    type Err = String;

    /// Parse a provider from a config string. Spellings are case-insensitive
    /// and surrounding whitespace is ignored.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "anthropic" | "claude" => Ok(Self::Anthropic),
            "openai" | "openai-compatible" | "openai_compatible" | "local" => {
                Ok(Self::OpenAiCompatible)
            }
            other => Err(format!("unknown AI provider: {other}")),
        }
    }
}

/// Configuration for the AI service.
#[derive(Debug, Clone)]
pub struct AiServiceConfig {
    /// Which provider wire protocol to use. Defaults to `Anthropic`.
    pub provider: AiProvider,

    /// Endpoint override. When `None`, the Anthropic provider uses its public
    /// Messages endpoint. The `OpenAiCompatible` provider requires this (for
    /// example `http://localhost:11434` for Ollama). A bare host, or a host
    /// ending in `/v1`, is expanded to the chat completions path.
    pub base_url: Option<String>,

    /// API key. When `None`, the Anthropic provider reads `ANTHROPIC_API_KEY`
    /// and the OpenAI-compatible provider reads `OPENAI_API_KEY`. A local
    /// OpenAI-compatible server usually needs no key, so `None` is valid there.
    pub api_key: Option<String>,

    /// Model identifier. Provider-specific (for example
    /// `"claude-sonnet-4-20250514"` for Anthropic, or `"llama3.1"` for a local
    /// Ollama model).
    pub model: String,

    /// Maximum tokens in API response.
    pub max_tokens: u32,

    /// Sampling temperature (0.0–1.0). Lower = more deterministic.
    /// NOTE: This is Rail B — floats are acceptable here (not consensus path).
    pub temperature: f64,

    /// Maximum concurrent API requests (semaphore permits).
    pub max_concurrent: usize,

    /// Request timeout in seconds.
    pub timeout_secs: u64,

    /// Consecutive failures before circuit breaker opens.
    pub circuit_breaker_threshold: u32,

    /// Seconds before circuit breaker resets from open to half-open.
    pub circuit_breaker_reset_secs: u64,

    /// Path to context file for enriched system prompts.
    pub context_file_path: Option<String>,

    /// Whether the AI service is enabled. Disabled by default (opt-in).
    pub enabled: bool,
}

impl Default for AiServiceConfig {
    fn default() -> Self {
        Self {
            provider: AiProvider::Anthropic,
            base_url: None,
            api_key: None,
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 2048,
            temperature: 0.3,
            max_concurrent: 2,
            timeout_secs: 30,
            circuit_breaker_threshold: 5,
            circuit_breaker_reset_secs: 60,
            context_file_path: None,
            enabled: false,
        }
    }
}

/// Type of inference to request from the AI service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InferenceType {
    /// Analyze detected anomalies for root cause and severity.
    AnomalyAnalysis,

    /// Review governance proposals for safety and impact.
    GovernanceReview,

    /// Forecast network congestion trends.
    CongestionForecast,

    /// Audit an AI entity's behavior patterns.
    EntityAudit,

    /// General chain health analysis.
    GeneralAnalysis,
}

impl InferenceType {
    /// Human-readable name for logging and metrics.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::AnomalyAnalysis => "anomaly_analysis",
            Self::GovernanceReview => "governance_review",
            Self::CongestionForecast => "congestion_forecast",
            Self::EntityAudit => "entity_audit",
            Self::GeneralAnalysis => "general_analysis",
        }
    }
}

/// Snapshot of chain state provided to the AI for analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainSnapshot {
    /// Current committed block height.
    pub height: u64,

    /// Current consensus round.
    pub round: u64,

    /// Number of connected peers.
    pub peer_count: u64,

    /// Number of transactions in mempool.
    pub mempool_size: u64,

    /// Total view changes (timeouts) since node start.
    pub view_changes: u64,

    /// Number of validators in the active set.
    pub validator_count: u32,

    /// Recent anomaly descriptions from the copilot detector.
    pub recent_anomalies: Vec<String>,
}

/// Structured response from AI analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAnalysisResponse {
    /// Type of inference that produced this response.
    pub inference_type: InferenceType,

    /// Structured findings extracted from the model response.
    pub findings: Vec<Finding>,

    /// Overall confidence score (0–100).
    pub confidence: u8,

    /// Human-readable recommendation summary.
    pub recommendation: String,

    /// Raw model response text (for debugging and audit).
    pub raw_response: String,
}

/// A single finding from AI analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Category of the finding (e.g., "network", "consensus", "economic").
    pub category: String,

    /// Severity: `"low"`, `"medium"`, `"high"`, or `"critical"`.
    pub severity: String,

    /// Description of the finding.
    pub description: String,

    /// Supporting evidence (data points, observations).
    pub evidence: Vec<String>,
}

/// Anomaly report for AI analysis input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyReport {
    /// Type of anomaly detected (from copilot detector).
    pub anomaly_type: String,

    /// Severity level.
    pub severity: String,

    /// Description of the anomaly.
    pub description: String,

    /// Affected validator addresses (hex-encoded).
    pub affected_validators: Vec<String>,
}

/// Validator statistics for AI analysis input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorStat {
    /// Validator address (hex-encoded).
    pub address: String,

    /// Number of blocks missed.
    pub missed_blocks: u64,

    /// Total proposals made.
    pub total_proposals: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_disabled() {
        let config = AiServiceConfig::default();
        assert!(!config.enabled);
        assert!(config.api_key.is_none());
        assert_eq!(config.max_concurrent, 2);
        assert_eq!(config.circuit_breaker_threshold, 5);
    }

    #[test]
    fn default_provider_is_anthropic() {
        let config = AiServiceConfig::default();
        assert_eq!(config.provider, AiProvider::Anthropic);
        assert!(config.base_url.is_none());
    }

    #[test]
    fn provider_parses_known_spellings() {
        assert_eq!("anthropic".parse::<AiProvider>(), Ok(AiProvider::Anthropic));
        assert_eq!("Claude".parse::<AiProvider>(), Ok(AiProvider::Anthropic));
        assert_eq!("openai".parse::<AiProvider>(), Ok(AiProvider::OpenAiCompatible));
        assert_eq!(
            "openai-compatible".parse::<AiProvider>(),
            Ok(AiProvider::OpenAiCompatible)
        );
        assert_eq!(
            "  LOCAL ".parse::<AiProvider>(),
            Ok(AiProvider::OpenAiCompatible)
        );
    }

    #[test]
    fn provider_rejects_unknown() {
        assert!("gemini".parse::<AiProvider>().is_err());
    }

    #[test]
    fn provider_name_is_stable() {
        assert_eq!(AiProvider::Anthropic.name(), "anthropic");
        assert_eq!(AiProvider::OpenAiCompatible.name(), "openai_compatible");
    }

    #[test]
    fn inference_type_names() {
        assert_eq!(InferenceType::AnomalyAnalysis.name(), "anomaly_analysis");
        assert_eq!(InferenceType::GovernanceReview.name(), "governance_review");
        assert_eq!(
            InferenceType::CongestionForecast.name(),
            "congestion_forecast"
        );
        assert_eq!(InferenceType::EntityAudit.name(), "entity_audit");
        assert_eq!(InferenceType::GeneralAnalysis.name(), "general_analysis");
    }

    #[test]
    fn chain_snapshot_serializes() {
        let snapshot = ChainSnapshot {
            height: 100,
            round: 5,
            peer_count: 4,
            mempool_size: 50,
            view_changes: 2,
            validator_count: 4,
            recent_anomalies: vec!["test anomaly".into()],
        };

        let json = serde_json::to_string(&snapshot).expect("serialize");
        let decoded: ChainSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.height, 100);
        assert_eq!(decoded.validator_count, 4);
        assert_eq!(decoded.recent_anomalies.len(), 1);
    }

    #[test]
    fn finding_serializes() {
        let finding = Finding {
            category: "network".into(),
            severity: "high".into(),
            description: "Peer count dropped".into(),
            evidence: vec!["was 4, now 1".into()],
        };

        let json = serde_json::to_string(&finding).expect("serialize");
        let decoded: Finding = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.category, "network");
        assert_eq!(decoded.severity, "high");
    }

    #[test]
    fn analysis_response_serializes() {
        let response = AiAnalysisResponse {
            inference_type: InferenceType::AnomalyAnalysis,
            findings: vec![],
            confidence: 75,
            recommendation: "Monitor closely".into(),
            raw_response: "raw text".into(),
        };

        let json = serde_json::to_string(&response).expect("serialize");
        let decoded: AiAnalysisResponse = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.confidence, 75);
        assert_eq!(decoded.inference_type, InferenceType::AnomalyAnalysis);
    }
}
