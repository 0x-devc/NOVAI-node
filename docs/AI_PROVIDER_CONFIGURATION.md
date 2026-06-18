# AI provider configuration

The `crates/ai_service` crate runs off-chain LLM analysis for validator
intelligence. It is Rail B: non-deterministic, advisory only, and it never feeds
back into consensus. The provider is configurable, so a validator is not locked
to a single hosted vendor and can run inference on a local or self-hosted model.

This matters for decentralization. A network whose intelligence layer depends on
one external API inherits that vendor as a point of control and failure. Running
a local model removes that dependency entirely.

## Supported providers

| Provider value | Wire protocol | Endpoint | Auth | Covers |
|----------------|---------------|----------|------|--------|
| `Anthropic` | Anthropic Messages API | `https://api.anthropic.com/v1/messages` (default) | `x-api-key` header | Anthropic Claude models |
| `OpenAiCompatible` | OpenAI Chat Completions | your `base_url` | `Authorization: Bearer` when a key is set, none otherwise | OpenAI, plus local servers: Ollama, vLLM, LM Studio, llama.cpp |

The `OpenAiCompatible` provider is the path to local models. Ollama, vLLM, LM
Studio, and the llama.cpp server all expose the OpenAI Chat Completions API, so
one provider value reaches all of them by pointing `base_url` at the local host.

## Configuration fields

Configuration is the `AiServiceConfig` struct in `crates/ai_service/src/types.rs`:

| Field | Meaning |
|-------|---------|
| `provider` | `AiProvider::Anthropic` (default) or `AiProvider::OpenAiCompatible` |
| `base_url` | Endpoint override. Optional for Anthropic, required for OpenAI-compatible |
| `api_key` | Optional. Falls back to the provider env var. Optional entirely for a local server |
| `model` | Model id, for example `claude-sonnet-4-20250514` or `llama3.1` |
| `enabled` | Off by default. Set to `true` to build a client |
| `max_tokens`, `temperature`, `max_concurrent`, `timeout_secs` | Request shaping and concurrency |
| `circuit_breaker_threshold`, `circuit_breaker_reset_secs` | Circuit breaker tuning |

`AiProvider` parses from a string (case-insensitive, whitespace-trimmed):

- `anthropic` or `claude` parse to `Anthropic`.
- `openai`, `openai-compatible`, `openai_compatible`, or `local` parse to `OpenAiCompatible`.

### API key resolution

1. `config.api_key`, if set and non-empty.
2. Otherwise the provider env var: `ANTHROPIC_API_KEY` for Anthropic,
   `OPENAI_API_KEY` for OpenAI-compatible.

Anthropic requires a key and returns `ApiKeyMissing` if none is found. The
OpenAI-compatible provider treats the key as optional, because a local server
usually has no authentication.

### Endpoint resolution

For Anthropic, `base_url` overrides the default Messages endpoint, or the default
is used.

For OpenAI-compatible, `base_url` is required and is normalized to the chat
completions path:

| `base_url` you set | Endpoint used |
|--------------------|---------------|
| `http://localhost:11434` | `http://localhost:11434/v1/chat/completions` |
| `http://localhost:11434/v1` | `http://localhost:11434/v1/chat/completions` |
| `http://localhost:8000/v1/chat/completions` | used unchanged |

A missing `base_url` on the OpenAI-compatible provider is a configuration error.

## Example: Anthropic (hosted)

```rust
use novai_ai_service::{AiClient, AiProvider, AiServiceConfig};

let config = AiServiceConfig {
    enabled: true,
    provider: AiProvider::Anthropic,
    api_key: None, // read from ANTHROPIC_API_KEY
    model: "claude-sonnet-4-20250514".to_string(),
    ..AiServiceConfig::default()
};
let client = AiClient::new(config)?;
```

```bash
export ANTHROPIC_API_KEY=sk-ant-...   # your key
```

## Example: local model with Ollama (no external provider, no key)

Start a local server and pull a model:

```bash
ollama serve
ollama pull llama3.1
```

Configure the client for the local endpoint. No API key is needed:

```rust
use novai_ai_service::{AiClient, AiProvider, AiServiceConfig};

let config = AiServiceConfig {
    enabled: true,
    provider: AiProvider::OpenAiCompatible,
    base_url: Some("http://localhost:11434".to_string()),
    api_key: None, // a local server needs no auth
    model: "llama3.1".to_string(),
    ..AiServiceConfig::default()
};
let client = AiClient::new(config)?;
```

A runnable version of this is checked in as an example. Run it from the
repository root:

```bash
cargo run -p novai-ai-service --example local_inference
```

Override the endpoint or model with environment variables:

```bash
NOVAI_AI_BASE_URL=http://localhost:8000 NOVAI_AI_MODEL=qwen2.5 \
  cargo run -p novai-ai-service --example local_inference
```

## Other local runtimes

vLLM, LM Studio, and the llama.cpp server all speak the OpenAI Chat Completions
API. Use `AiProvider::OpenAiCompatible` and point `base_url` at the server:

```text
vLLM        http://localhost:8000        (serves /v1/chat/completions)
LM Studio   http://localhost:1234
llama.cpp   http://localhost:8080
```

If the server enforces a token, set `api_key` or `OPENAI_API_KEY`; otherwise
leave it unset.

## Using a local model from the node binary

The library API and the example above support any configured provider today. The
node binary (`crates/node/src/main.rs`) currently enables the AI service only
when `NOVAI_AI_API_KEY` is set and builds it with the Anthropic default. Wiring
the node to select the provider, base URL, and model from environment variables
is a small follow-up in the node crate and is intentionally left out of this
change, which is scoped to the AI provider abstraction, the example, and the
docs.

## Determinism note

None of this touches consensus. AI output is advisory (Rail B) and never
influences block production, voting, or state. Switching providers or models
changes only the advisory analysis, never a consensus outcome.
