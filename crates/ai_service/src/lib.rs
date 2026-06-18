//! AI Service: multi-provider LLM integration for NOVAI validator intelligence.
//!
//! PURPOSE: Provides off-chain LLM analysis through a configurable provider.
//! Supported providers are the Anthropic Messages API and any OpenAI-compatible
//! Chat Completions endpoint, including local or self-hosted runtimes such as
//! Ollama, vLLM, LM Studio, and the llama.cpp server. This is Rail B
//! (non-deterministic, advisory only): results NEVER influence consensus.
//!
//! INVARIANTS:
//! - All API calls are async and non-blocking
//! - Circuit breaker prevents cascading failures on provider outages
//! - Concurrency limited by semaphore (max_concurrent config)
//! - Anthropic loads its key from config or ANTHROPIC_API_KEY; a local
//!   OpenAI-compatible server may run with no key at all
//!
//! FAILURE MODES:
//! - API key missing for a provider that requires one: `AiServiceError::ApiKeyMissing`
//! - Provider unreachable: circuit breaker opens after threshold failures
//! - Rate limited: error propagated with retry-after hint
//! - Response unparseable: falls back to raw text finding

pub mod bridge;
pub mod client;
pub mod error;
pub mod prompt;
pub mod runner;
pub mod scheduler;
pub mod types;

pub use bridge::{AiTriggerCallback, AnomalyTrigger};
pub use client::{AiClient, AnthropicClient};
pub use error::AiServiceError;
pub use prompt::PromptBuilder;
pub use runner::{AiServiceRunner, FeatureFlags};
pub use scheduler::{
    InferenceCallback, InferenceScheduler, InferenceTask, LoggingInferenceCallback,
};
pub use types::{
    AiAnalysisResponse, AiProvider, AiServiceConfig, AnomalyReport, ChainSnapshot, Finding,
    InferenceType, ValidatorStat,
};
