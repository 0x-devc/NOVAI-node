//! Anthropic Claude API client with circuit breaker and concurrency control.
//!
//! PURPOSE: Makes HTTP requests to the Anthropic Messages API, parses responses
//! into structured findings. Includes circuit breaker to prevent cascading
//! failures and semaphore to limit concurrent requests.
//!
//! INVARIANTS:
//! - API key is never logged or included in error messages
//! - Circuit breaker transitions: Closed → Open (after threshold failures) → HalfOpen → Closed
//! - Semaphore ensures max_concurrent requests at any time
//! - Rate limiting (HTTP 429) does NOT trip the circuit breaker
//!
//! FAILURE MODES:
//! - Network failure → HttpError, circuit breaker counts failure
//! - API error (4xx/5xx) → ApiError, circuit breaker counts failure
//! - Timeout → Timeout error, circuit breaker counts failure
//! - Rate limited → RateLimited error, circuit breaker NOT affected

use crate::error::AiServiceError;
use crate::prompt::PromptBuilder;
use crate::types::{AiAnalysisResponse, AiServiceConfig, ChainSnapshot, Finding, InferenceType};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// Anthropic Messages API endpoint.
const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";

/// Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

// ============================================================================
// CIRCUIT BREAKER
// ============================================================================

/// Circuit breaker state machine.
#[derive(Debug)]
enum CircuitBreakerState {
    /// Normal operation — requests are allowed.
    Closed,
    /// Rejecting all requests until reset duration elapses.
    Open { since: Instant },
    /// Allowing a single probe request to test recovery.
    HalfOpen,
}

/// Circuit breaker to prevent cascading failures when the API is down.
#[derive(Debug)]
struct CircuitBreaker {
    state: CircuitBreakerState,
    consecutive_failures: u32,
    threshold: u32,
    reset_duration: Duration,
}

impl CircuitBreaker {
    fn new(threshold: u32, reset_secs: u64) -> Self {
        Self {
            state: CircuitBreakerState::Closed,
            consecutive_failures: 0,
            threshold,
            reset_duration: Duration::from_secs(reset_secs),
        }
    }

    /// Check if a request should be allowed through.
    fn allow_request(&mut self) -> bool {
        match &self.state {
            CircuitBreakerState::Closed => true,
            CircuitBreakerState::Open { since } => {
                if since.elapsed() >= self.reset_duration {
                    self.state = CircuitBreakerState::HalfOpen;
                    tracing::info!("Circuit breaker → HALF-OPEN (probing)");
                    true
                } else {
                    false
                }
            }
            CircuitBreakerState::HalfOpen => {
                // Only one probe at a time — block additional requests
                // while probe is in flight. In practice, the semaphore
                // handles this since max_concurrent is usually small.
                true
            }
        }
    }

    /// Record a successful request — reset to Closed.
    fn record_success(&mut self) {
        if !matches!(self.state, CircuitBreakerState::Closed) {
            tracing::info!("Circuit breaker → CLOSED (recovered)");
        }
        self.consecutive_failures = 0;
        self.state = CircuitBreakerState::Closed;
    }

    /// Record a failed request — may trip to Open.
    fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= self.threshold {
            self.state = CircuitBreakerState::Open {
                since: Instant::now(),
            };
            tracing::warn!(
                failures = self.consecutive_failures,
                "Circuit breaker → OPEN"
            );
        }
    }

    /// Get the current consecutive failure count (for testing).
    #[cfg(test)]
    fn failures(&self) -> u32 {
        self.consecutive_failures
    }
}

// ============================================================================
// API REQUEST/RESPONSE TYPES
// ============================================================================

/// Anthropic Messages API request body.
#[derive(Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    temperature: f64,
    system: String,
    messages: Vec<ApiMessage>,
}

/// A single message in the API conversation.
#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: String,
}

/// Anthropic Messages API response body.
#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
}

/// A content block in the API response.
#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
}

/// Anthropic API error response envelope.
#[derive(Deserialize)]
struct ApiErrorResponse {
    error: ApiErrorDetail,
}

/// Inner error detail from the API.
#[derive(Deserialize)]
struct ApiErrorDetail {
    message: String,
}

/// Intermediate parsed structure for structured model output.
#[derive(Deserialize)]
struct ParsedAnalysis {
    findings: Option<Vec<Finding>>,
    confidence: Option<u8>,
    recommendation: Option<String>,
}

// ============================================================================
// CLIENT
// ============================================================================

/// Client for the Anthropic Messages API.
///
/// Thread-safe: can be shared via `Arc` across async tasks.
pub struct AnthropicClient {
    api_key: String,
    config: AiServiceConfig,
    http: Client,
    circuit_breaker: Mutex<CircuitBreaker>,
    semaphore: Semaphore,
    prompt_builder: PromptBuilder,
}

impl AnthropicClient {
    /// Create a new Anthropic client.
    ///
    /// API key resolution order:
    /// 1. `config.api_key` field (if `Some`)
    /// 2. `ANTHROPIC_API_KEY` environment variable
    /// 3. Returns `AiServiceError::ApiKeyMissing`
    ///
    /// # Errors
    ///
    /// Returns `AiServiceError::Disabled` if `config.enabled` is false.
    /// Returns `AiServiceError::ApiKeyMissing` if no API key is available.
    /// Returns `AiServiceError::HttpError` if the HTTP client fails to build.
    pub fn new(config: AiServiceConfig) -> Result<Self, AiServiceError> {
        if !config.enabled {
            return Err(AiServiceError::Disabled);
        }

        let api_key = config
            .api_key
            .clone()
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
            .ok_or(AiServiceError::ApiKeyMissing)?;

        let http = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| AiServiceError::HttpError(e.to_string()))?;

        let circuit_breaker = CircuitBreaker::new(
            config.circuit_breaker_threshold,
            config.circuit_breaker_reset_secs,
        );

        let semaphore = Semaphore::new(config.max_concurrent);

        let prompt_builder = if let Some(ref path) = config.context_file_path {
            PromptBuilder::load_context(path).unwrap_or_else(|e| {
                tracing::warn!(%e, "Failed to load AI context file, using defaults");
                PromptBuilder::new()
            })
        } else {
            PromptBuilder::new()
        };

        Ok(Self {
            api_key,
            config,
            http,
            circuit_breaker: Mutex::new(circuit_breaker),
            semaphore,
            prompt_builder,
        })
    }

    /// Perform an AI analysis on the given chain snapshot.
    ///
    /// # Errors
    ///
    /// Returns `AiServiceError::CircuitBreakerOpen` if too many recent failures.
    /// Returns various `AiServiceError` variants for API/network failures.
    pub async fn analyze(
        &self,
        inference_type: InferenceType,
        snapshot: &ChainSnapshot,
    ) -> Result<AiAnalysisResponse, AiServiceError> {
        // Check circuit breaker
        {
            let mut cb = self.circuit_breaker.lock().map_err(|_| {
                AiServiceError::HttpError("circuit breaker lock poisoned".to_string())
            })?;
            if !cb.allow_request() {
                return Err(AiServiceError::CircuitBreakerOpen);
            }
        }

        // Acquire semaphore permit (limits concurrent requests)
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| AiServiceError::HttpError("semaphore closed".to_string()))?;

        tracing::debug!(
            inference_type = inference_type.name(),
            height = snapshot.height,
            "Sending inference request"
        );

        let result = self.call_api(inference_type, snapshot).await;

        // Update circuit breaker based on result
        {
            let mut cb = self.circuit_breaker.lock().map_err(|_| {
                AiServiceError::HttpError("circuit breaker lock poisoned".to_string())
            })?;
            match &result {
                Ok(_) => cb.record_success(),
                Err(AiServiceError::RateLimited { .. }) => {
                    // Rate limiting is expected — don't trip circuit breaker
                }
                Err(_) => cb.record_failure(),
            }
        }

        result
    }

    /// Make the actual HTTP request to the Anthropic API.
    async fn call_api(
        &self,
        inference_type: InferenceType,
        snapshot: &ChainSnapshot,
    ) -> Result<AiAnalysisResponse, AiServiceError> {
        let system_prompt = self.prompt_builder.system_prompt(&inference_type);
        let user_message = self.prompt_builder.user_message(snapshot);

        let request = ApiRequest {
            model: self.config.model.clone(),
            max_tokens: self.config.max_tokens,
            temperature: self.config.temperature,
            system: system_prompt,
            messages: vec![ApiMessage {
                role: "user".to_string(),
                content: user_message,
            }],
        };

        let response = self
            .http
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    AiServiceError::Timeout
                } else {
                    AiServiceError::HttpError(e.to_string())
                }
            })?;

        let status = response.status().as_u16();

        // Handle rate limiting specially (don't trip circuit breaker)
        if status == 429 {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            return Err(AiServiceError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        // Handle other error status codes
        if status >= 400 {
            let body = response.text().await.unwrap_or_default();
            let message = serde_json::from_str::<ApiErrorResponse>(&body)
                .map_or_else(|_| body.clone(), |e| e.error.message);
            return Err(AiServiceError::ApiError { status, message });
        }

        // Parse successful response
        let api_response: ApiResponse = response
            .json()
            .await
            .map_err(|e| AiServiceError::ParseError(e.to_string()))?;

        let raw_response = api_response
            .content
            .iter()
            .filter(|b| b.content_type == "text")
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");

        if raw_response.is_empty() {
            return Err(AiServiceError::ParseError(
                "No text content in response".to_string(),
            ));
        }

        tracing::debug!(
            inference_type = inference_type.name(),
            response_len = raw_response.len(),
            "Inference response received"
        );

        Ok(Self::parse_response(inference_type, raw_response))
    }

    /// Parse the raw model response into structured findings.
    ///
    /// Attempts to extract JSON from the response. If the model wrapped
    /// the JSON in markdown code fences, extracts the inner content.
    /// Falls back to treating the entire response as a single finding.
    fn parse_response(inference_type: InferenceType, raw_response: String) -> AiAnalysisResponse {
        // Try to extract JSON from markdown code fences
        let json_str = extract_json_block(&raw_response).unwrap_or(&raw_response);

        if let Ok(parsed) = serde_json::from_str::<ParsedAnalysis>(json_str) {
            AiAnalysisResponse {
                inference_type,
                findings: parsed.findings.unwrap_or_default(),
                confidence: parsed.confidence.unwrap_or(50),
                recommendation: parsed
                    .recommendation
                    .unwrap_or_else(|| "No recommendation provided".to_string()),
                raw_response,
            }
        } else {
            // Fallback: treat entire response as a single finding
            AiAnalysisResponse {
                inference_type,
                findings: vec![Finding {
                    category: "general".to_string(),
                    severity: "low".to_string(),
                    description: raw_response.clone(),
                    evidence: vec![],
                }],
                confidence: 50,
                recommendation: "See raw response for details".to_string(),
                raw_response,
            }
        }
    }
}

/// Extract a JSON block from markdown-wrapped response text.
///
/// Looks for content between ` ```json\n` and `\n``` ` markers.
fn extract_json_block(text: &str) -> Option<&str> {
    let start = text.find("```json\n").map(|i| i + 8)?;
    let end = text[start..].find("\n```").map(|i| i + start)?;
    Some(&text[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Circuit Breaker Tests ──────────────────────────────────────────

    #[test]
    fn circuit_breaker_starts_closed() {
        let mut cb = CircuitBreaker::new(3, 60);
        assert!(cb.allow_request());
        assert_eq!(cb.failures(), 0);
    }

    #[test]
    fn circuit_breaker_opens_after_threshold() {
        let mut cb = CircuitBreaker::new(3, 60);

        cb.record_failure();
        assert!(cb.allow_request()); // Still closed
        cb.record_failure();
        assert!(cb.allow_request()); // Still closed
        cb.record_failure(); // Hits threshold
        assert!(!cb.allow_request()); // Now open
    }

    #[test]
    fn circuit_breaker_resets_on_success() {
        let mut cb = CircuitBreaker::new(3, 60);

        cb.record_failure();
        cb.record_failure();
        cb.record_success(); // Reset
        assert_eq!(cb.failures(), 0);
        assert!(cb.allow_request());

        // Need 3 more failures to trip again
        cb.record_failure();
        cb.record_failure();
        assert!(cb.allow_request()); // Not yet
    }

    #[test]
    fn circuit_breaker_transitions_to_half_open() {
        let mut cb = CircuitBreaker::new(2, 0); // 0 second reset for testing

        cb.record_failure();
        cb.record_failure(); // Trip to open

        // With 0-second reset, next check transitions to half-open
        assert!(cb.allow_request()); // Transitions to HalfOpen
        assert!(matches!(cb.state, CircuitBreakerState::HalfOpen));
    }

    #[test]
    fn circuit_breaker_half_open_success_closes() {
        let mut cb = CircuitBreaker::new(2, 0);

        cb.record_failure();
        cb.record_failure(); // Open
        cb.allow_request(); // → HalfOpen
        cb.record_success(); // → Closed

        assert!(matches!(cb.state, CircuitBreakerState::Closed));
        assert_eq!(cb.failures(), 0);
    }

    #[test]
    fn circuit_breaker_half_open_failure_reopens() {
        let mut cb = CircuitBreaker::new(2, 0);

        cb.record_failure();
        cb.record_failure(); // Open
        cb.allow_request(); // → HalfOpen
        cb.record_failure(); // Back to Open (consecutive_failures was already 2, now 3)

        assert!(matches!(cb.state, CircuitBreakerState::Open { .. }));
    }

    // ── JSON Extraction Tests ──────────────────────────────────────────

    #[test]
    fn extract_json_from_markdown() {
        let text = "Here is the analysis:\n```json\n{\"confidence\": 80}\n```\nDone.";
        let extracted = extract_json_block(text);
        assert_eq!(extracted, Some("{\"confidence\": 80}"));
    }

    #[test]
    fn extract_json_returns_none_without_markers() {
        let text = "Just plain text without JSON";
        assert!(extract_json_block(text).is_none());
    }

    #[test]
    fn extract_json_handles_multiline() {
        let text = "```json\n{\n  \"findings\": [],\n  \"confidence\": 50\n}\n```";
        let extracted = extract_json_block(text).expect("should extract");
        assert!(extracted.contains("\"findings\""));
        assert!(extracted.contains("\"confidence\""));
    }

    // ── Response Parsing Tests ─────────────────────────────────────────

    #[test]
    fn parse_structured_json_response() {
        let raw = "```json\n{\"findings\": [{\"category\": \"network\", \"severity\": \"high\", \
                   \"description\": \"Peer drop\", \"evidence\": [\"was 4\"]}], \
                   \"confidence\": 85, \"recommendation\": \"Monitor peers\"}\n```"
            .to_string();

        let response = AnthropicClient::parse_response(InferenceType::AnomalyAnalysis, raw);

        assert_eq!(response.confidence, 85);
        assert_eq!(response.findings.len(), 1);
        assert_eq!(response.findings[0].category, "network");
        assert_eq!(response.findings[0].severity, "high");
        assert_eq!(response.recommendation, "Monitor peers");
    }

    #[test]
    fn parse_plain_json_response() {
        let raw = "{\"findings\": [], \"confidence\": 60, \
                   \"recommendation\": \"All clear\"}"
            .to_string();

        let response = AnthropicClient::parse_response(InferenceType::GeneralAnalysis, raw);

        assert_eq!(response.confidence, 60);
        assert!(response.findings.is_empty());
        assert_eq!(response.recommendation, "All clear");
    }

    #[test]
    fn parse_fallback_on_non_json() {
        let raw = "I couldn't parse the data properly.".to_string();

        let response = AnthropicClient::parse_response(InferenceType::AnomalyAnalysis, raw.clone());

        assert_eq!(response.confidence, 50); // Default fallback
        assert_eq!(response.findings.len(), 1);
        assert_eq!(response.findings[0].category, "general");
        assert_eq!(response.findings[0].description, raw);
    }

    #[test]
    fn parse_partial_json_uses_defaults() {
        let raw = "```json\n{\"confidence\": 90}\n```".to_string();

        let response = AnthropicClient::parse_response(InferenceType::EntityAudit, raw);

        assert_eq!(response.confidence, 90);
        assert!(response.findings.is_empty()); // None provided → default empty
        assert_eq!(response.recommendation, "No recommendation provided");
    }

    // ── Client Construction Tests ──────────────────────────────────────

    #[test]
    fn client_disabled_returns_error() {
        let config = AiServiceConfig {
            enabled: false,
            ..AiServiceConfig::default()
        };

        let result = AnthropicClient::new(config);
        assert!(matches!(result, Err(AiServiceError::Disabled)));
    }

    #[test]
    fn client_missing_api_key_returns_error() {
        // Remove env var to ensure it's not set
        std::env::remove_var("ANTHROPIC_API_KEY");

        let config = AiServiceConfig {
            enabled: true,
            api_key: None,
            ..AiServiceConfig::default()
        };

        let result = AnthropicClient::new(config);
        assert!(matches!(result, Err(AiServiceError::ApiKeyMissing)));
    }

    #[test]
    fn client_accepts_config_api_key() {
        let config = AiServiceConfig {
            enabled: true,
            api_key: Some("test-key-12345".into()),
            ..AiServiceConfig::default()
        };

        let client = AnthropicClient::new(config).expect("should create client");
        assert_eq!(client.api_key, "test-key-12345");
    }
}
