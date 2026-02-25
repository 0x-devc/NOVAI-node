//! AI Service: Anthropic Claude API integration for NOVAI validator intelligence.
//!
//! PURPOSE: Provides real LLM analysis via the Anthropic Messages API. This is
//! Rail B (non-deterministic, advisory only) — results NEVER influence consensus.
//!
//! INVARIANTS:
//! - All API calls are async and non-blocking
//! - Circuit breaker prevents cascading failures on API outages
//! - Concurrency limited by semaphore (max_concurrent config)
//! - API key loaded from environment variable (never hardcoded)
//!
//! FAILURE MODES:
//! - API key missing → `AiServiceError::ApiKeyMissing`
//! - API unreachable → circuit breaker opens after threshold failures
//! - Rate limited → error propagated with retry-after hint
//! - Response unparseable → falls back to raw text finding

pub mod client;
pub mod error;
pub mod prompt;
pub mod scheduler;
pub mod types;

pub use client::AnthropicClient;
pub use error::AiServiceError;
pub use prompt::PromptBuilder;
pub use scheduler::{
    InferenceCallback, InferenceScheduler, InferenceTask, LoggingInferenceCallback,
};
pub use types::{
    AiAnalysisResponse, AiServiceConfig, AnomalyReport, ChainSnapshot, Finding, InferenceType,
    ValidatorStat,
};
