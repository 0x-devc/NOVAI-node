//! Error types for the AI service.
//!
//! PURPOSE: Unified error enum covering all failure modes of the AI provider
//! client, circuit breaker, and configuration.

use std::fmt;

/// Errors from the AI service.
#[derive(Debug)]
pub enum AiServiceError {
    /// API key not configured (neither in config nor ANTHROPIC_API_KEY env var).
    ApiKeyMissing,

    /// HTTP transport error (network failure, TLS error, etc.).
    HttpError(String),

    /// Anthropic API returned a non-success status code.
    ApiError {
        /// HTTP status code.
        status: u16,
        /// Error message from the API.
        message: String,
    },

    /// Failed to parse the API response body.
    ParseError(String),

    /// API returned HTTP 429 (rate limited).
    RateLimited {
        /// Seconds to wait before retrying, if provided by the API.
        retry_after_secs: Option<u64>,
    },

    /// Request exceeded the configured timeout.
    Timeout,

    /// Circuit breaker is open — too many consecutive failures.
    CircuitBreakerOpen,

    /// AI service is disabled in configuration.
    Disabled,

    /// Invalid configuration (for example, an OpenAI-compatible provider
    /// selected without a `base_url`).
    Config(String),
}

impl fmt::Display for AiServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKeyMissing => {
                write!(
                    f,
                    "Anthropic API key not configured (set ANTHROPIC_API_KEY)"
                )
            }
            Self::HttpError(msg) => write!(f, "HTTP error: {msg}"),
            Self::ApiError { status, message } => {
                write!(f, "API error (HTTP {status}): {message}")
            }
            Self::ParseError(msg) => write!(f, "Response parse error: {msg}"),
            Self::RateLimited { retry_after_secs } => {
                if let Some(secs) = retry_after_secs {
                    write!(f, "Rate limited (retry after {secs}s)")
                } else {
                    write!(f, "Rate limited")
                }
            }
            Self::Timeout => write!(f, "Request timed out"),
            Self::CircuitBreakerOpen => {
                write!(f, "Circuit breaker open — too many consecutive failures")
            }
            Self::Disabled => write!(f, "AI service is disabled"),
            Self::Config(msg) => write!(f, "AI service configuration error: {msg}"),
        }
    }
}

impl std::error::Error for AiServiceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_formats_all_variants() {
        let cases: Vec<AiServiceError> = vec![
            AiServiceError::ApiKeyMissing,
            AiServiceError::HttpError("connection refused".into()),
            AiServiceError::ApiError {
                status: 500,
                message: "internal error".into(),
            },
            AiServiceError::ParseError("invalid json".into()),
            AiServiceError::RateLimited {
                retry_after_secs: Some(30),
            },
            AiServiceError::RateLimited {
                retry_after_secs: None,
            },
            AiServiceError::Timeout,
            AiServiceError::CircuitBreakerOpen,
            AiServiceError::Disabled,
            AiServiceError::Config("missing base_url".into()),
        ];

        for err in &cases {
            let msg = format!("{err}");
            assert!(!msg.is_empty(), "Display should produce non-empty string");
        }
    }

    #[test]
    fn error_trait_implemented() {
        let err: Box<dyn std::error::Error> = Box::new(AiServiceError::Timeout);
        assert!(!err.to_string().is_empty());
    }
}
