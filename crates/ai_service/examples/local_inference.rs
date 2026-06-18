//! Example: run validator-intelligence inference against a local model.
//!
//! This shows the multi-provider AI client talking to a self-hosted,
//! OpenAI-compatible server (for example Ollama) with no external provider and
//! no API key. It is the "lead by example on local models" path: a validator
//! can run advisory inference without depending on any hosted provider.
//!
//! Point it at any OpenAI-compatible endpoint with `NOVAI_AI_BASE_URL` and pick
//! the model with `NOVAI_AI_MODEL`. Defaults target a local Ollama install.
//!
//! Run a local server first, for example with Ollama:
//!     ollama serve
//!     ollama pull llama3.1
//!
//! Then run this example from the repository root:
//!     cargo run -p novai-ai-service --example local_inference
//!
//! Defaults: base URL http://localhost:11434, model llama3.1.

use novai_ai_service::{AiClient, AiProvider, AiServiceConfig, ChainSnapshot, InferenceType};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let base_url = std::env::var("NOVAI_AI_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model = std::env::var("NOVAI_AI_MODEL").unwrap_or_else(|_| "llama3.1".to_string());

    // OpenAI-compatible provider with no api_key: a local server needs no auth.
    let config = AiServiceConfig {
        enabled: true,
        provider: AiProvider::OpenAiCompatible,
        base_url: Some(base_url.clone()),
        api_key: None,
        model: model.clone(),
        ..AiServiceConfig::default()
    };

    let client = match AiClient::new(config) {
        Ok(client) => client,
        Err(e) => {
            eprintln!("failed to build AI client: {e}");
            return;
        }
    };

    println!(
        "provider={}, model={model}, base_url={base_url}",
        AiProvider::OpenAiCompatible.name()
    );

    let snapshot = ChainSnapshot {
        height: 1000,
        round: 5,
        peer_count: 4,
        mempool_size: 150,
        view_changes: 3,
        validator_count: 4,
        recent_anomalies: vec!["mempool_congestion: size 150 over baseline 50".to_string()],
    };

    match client.analyze(InferenceType::GeneralAnalysis, &snapshot).await {
        Ok(response) => {
            println!("confidence: {}", response.confidence);
            println!("recommendation: {}", response.recommendation);
            for finding in &response.findings {
                println!(
                    "- [{}] {}: {}",
                    finding.severity, finding.category, finding.description
                );
            }
        }
        Err(e) => {
            eprintln!("inference failed (is the local server running?): {e}");
        }
    }
}
