//! System prompt builder for AI inference requests.
//!
//! PURPOSE: Generates tailored system prompts and user messages for each
//! inference type. Optionally loads external context from a file to enrich
//! the prompts with protocol-specific knowledge.
//!
//! INVARIANTS:
//! - Prompts always instruct the model to return structured JSON
//! - Context file is loaded once at construction (not per-request)
//! - All string formatting is deterministic (no randomness)
//!
//! FAILURE MODES:
//! - Context file unreadable → error on construction, caller can fall back

use crate::error::AiServiceError;
use crate::types::{ChainSnapshot, InferenceType};

/// Builds system prompts and user messages for different inference types.
pub struct PromptBuilder {
    /// Optional context loaded from an external file.
    context: Option<String>,
}

impl PromptBuilder {
    /// Create a new prompt builder without external context.
    #[must_use]
    pub fn new() -> Self {
        Self { context: None }
    }

    /// Create a prompt builder with context loaded from a file.
    ///
    /// # Errors
    ///
    /// Returns `AiServiceError::HttpError` if the file cannot be read.
    pub fn load_context(path: &str) -> Result<Self, AiServiceError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| AiServiceError::HttpError(format!("Failed to read context file: {e}")))?;
        Ok(Self {
            context: Some(content),
        })
    }

    /// Whether external context is loaded.
    #[must_use]
    pub fn has_context(&self) -> bool {
        self.context.is_some()
    }

    /// Generate the system prompt for a given inference type.
    #[must_use]
    pub fn system_prompt(&self, inference_type: &InferenceType) -> String {
        let base = Self::base_system_prompt();
        let type_specific = Self::type_specific_prompt(inference_type);
        let context_section = self.context.as_deref().map_or_else(String::new, |ctx| {
            format!("\n\n## Protocol Context\n\n{ctx}")
        });

        format!(
            "{base}\n\n{type_specific}{context_section}\n\n\
             ## Response Format\n\n\
             Respond with a JSON object containing:\n\
             - \"findings\": array of {{\"category\": string, \"severity\": \"low\"|\"medium\"|\"high\"|\"critical\", \
             \"description\": string, \"evidence\": [string]}}\n\
             - \"confidence\": integer 0-100\n\
             - \"recommendation\": string\n\n\
             Wrap the JSON in ```json``` code fences."
        )
    }

    /// Generate the user message from a chain snapshot.
    #[must_use]
    pub fn user_message(&self, snapshot: &ChainSnapshot) -> String {
        let anomalies = if snapshot.recent_anomalies.is_empty() {
            "None detected".to_string()
        } else {
            snapshot.recent_anomalies.join("; ")
        };

        format!(
            "Analyze the following chain state:\n\n\
             - Block height: {}\n\
             - Consensus round: {}\n\
             - Connected peers: {}\n\
             - Mempool size: {} txs\n\
             - View changes: {}\n\
             - Validator count: {}\n\
             - Recent anomalies: {}",
            snapshot.height,
            snapshot.round,
            snapshot.peer_count,
            snapshot.mempool_size,
            snapshot.view_changes,
            snapshot.validator_count,
            anomalies,
        )
    }

    fn base_system_prompt() -> &'static str {
        "You are an AI validator co-pilot for the NOVAI blockchain protocol. \
         Your role is to analyze chain state and provide advisory signals. \
         Your analysis is non-binding (Rail B) and never directly affects consensus. \
         Be concise, precise, and always ground findings in the provided data."
    }

    fn type_specific_prompt(inference_type: &InferenceType) -> &'static str {
        match inference_type {
            InferenceType::AnomalyAnalysis => {
                "## Task: Anomaly Analysis\n\n\
                 Analyze the detected anomalies. For each:\n\
                 1. Identify likely root cause\n\
                 2. Assess severity (low/medium/high/critical)\n\
                 3. Recommend mitigation if needed\n\
                 4. Note if this could indicate a Byzantine actor"
            }
            InferenceType::GovernanceReview => {
                "## Task: Governance Proposal Review\n\n\
                 Review the governance proposal context. Assess:\n\
                 1. Safety implications for the network\n\
                 2. Economic impact on validators and entities\n\
                 3. Technical feasibility and risks\n\
                 4. Whether the proposal aligns with protocol goals"
            }
            InferenceType::CongestionForecast => {
                "## Task: Congestion Forecast\n\n\
                 Based on current chain state, predict:\n\
                 1. Congestion severity in the next 10 blocks\n\
                 2. Whether fee increases are warranted\n\
                 3. Mempool growth trajectory\n\
                 4. Recommended actions for validators"
            }
            InferenceType::EntityAudit => {
                "## Task: AI Entity Audit\n\n\
                 Analyze the AI entity's behavior patterns:\n\
                 1. Is the entity operating within expected parameters?\n\
                 2. Are there signs of anomalous behavior?\n\
                 3. Should the entity's capabilities be reviewed?\n\
                 4. Economic activity assessment"
            }
            InferenceType::GeneralAnalysis => {
                "## Task: General Chain Health\n\n\
                 Provide an overall health assessment:\n\
                 1. Network stability (peer count, view changes)\n\
                 2. Transaction processing health (mempool, throughput)\n\
                 3. Validator performance summary\n\
                 4. Any concerns or recommendations"
            }
        }
    }
}

impl Default for PromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChainSnapshot;

    fn test_snapshot() -> ChainSnapshot {
        ChainSnapshot {
            height: 1000,
            round: 5,
            peer_count: 4,
            mempool_size: 150,
            view_changes: 3,
            validator_count: 4,
            recent_anomalies: vec!["mempool_congestion: size 150 > baseline 50".into()],
        }
    }

    #[test]
    fn system_prompt_contains_base_instructions() {
        let builder = PromptBuilder::new();
        let prompt = builder.system_prompt(&InferenceType::AnomalyAnalysis);

        assert!(prompt.contains("NOVAI blockchain"));
        assert!(prompt.contains("Rail B"));
        assert!(prompt.contains("Response Format"));
        assert!(prompt.contains("findings"));
    }

    #[test]
    fn system_prompt_varies_by_type() {
        let builder = PromptBuilder::new();

        let anomaly = builder.system_prompt(&InferenceType::AnomalyAnalysis);
        let governance = builder.system_prompt(&InferenceType::GovernanceReview);
        let congestion = builder.system_prompt(&InferenceType::CongestionForecast);
        let audit = builder.system_prompt(&InferenceType::EntityAudit);
        let general = builder.system_prompt(&InferenceType::GeneralAnalysis);

        assert!(anomaly.contains("Anomaly Analysis"));
        assert!(governance.contains("Governance Proposal Review"));
        assert!(congestion.contains("Congestion Forecast"));
        assert!(audit.contains("AI Entity Audit"));
        assert!(general.contains("General Chain Health"));
    }

    #[test]
    fn system_prompt_includes_context_when_loaded() {
        let mut builder = PromptBuilder::new();
        builder.context = Some("NOVAI uses HotStuff-like BFT consensus.".into());

        let prompt = builder.system_prompt(&InferenceType::GeneralAnalysis);
        assert!(prompt.contains("Protocol Context"));
        assert!(prompt.contains("HotStuff-like BFT"));
    }

    #[test]
    fn system_prompt_excludes_context_section_when_none() {
        let builder = PromptBuilder::new();
        assert!(!builder.has_context());

        let prompt = builder.system_prompt(&InferenceType::GeneralAnalysis);
        assert!(!prompt.contains("Protocol Context"));
    }

    #[test]
    fn user_message_formats_snapshot() {
        let builder = PromptBuilder::new();
        let snapshot = test_snapshot();
        let message = builder.user_message(&snapshot);

        assert!(message.contains("Block height: 1000"));
        assert!(message.contains("Consensus round: 5"));
        assert!(message.contains("Connected peers: 4"));
        assert!(message.contains("Mempool size: 150 txs"));
        assert!(message.contains("View changes: 3"));
        assert!(message.contains("Validator count: 4"));
        assert!(message.contains("mempool_congestion"));
    }

    #[test]
    fn user_message_handles_no_anomalies() {
        let builder = PromptBuilder::new();
        let snapshot = ChainSnapshot {
            height: 50,
            round: 0,
            peer_count: 4,
            mempool_size: 10,
            view_changes: 0,
            validator_count: 4,
            recent_anomalies: vec![],
        };

        let message = builder.user_message(&snapshot);
        assert!(message.contains("None detected"));
    }

    #[test]
    fn load_context_fails_on_missing_file() {
        let result = PromptBuilder::load_context("/nonexistent/path/context.md");
        assert!(result.is_err());
    }

    #[test]
    fn load_context_reads_file() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("context.md");
        std::fs::write(&path, "Test context content").expect("write");

        let builder =
            PromptBuilder::load_context(path.to_str().expect("path")).expect("load context");
        assert!(builder.has_context());

        let prompt = builder.system_prompt(&InferenceType::GeneralAnalysis);
        assert!(prompt.contains("Test context content"));
    }
}
