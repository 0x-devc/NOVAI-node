# NOVAI AI Context

This file provides protocol context to the AI service for enriched inference.
Place this file at a path accessible by the validator node and configure
`context_file_path` in `AiServiceConfig` to enable it.

## Protocol Overview

NOVAI is an AI-native blockchain with two execution rails:

- **Rail A (Consensus)**: Deterministic BFT consensus using HotStuff-like protocol.
  All state transitions are canonical and reproducible.
- **Rail B (AI Advisory)**: Non-deterministic AI inference that provides advisory
  signals. Results never directly influence consensus.

## Validator Set

- 4 validators in the current testnet configuration
- Leader rotation: `(height + round) % validator_count`
- BFT threshold: 2f+1 (tolerates 1 Byzantine validator out of 4)

## Transaction Types

| Payload Version | Type | Min Fee |
|----------------|------|---------|
| 1 | Transfer | 100 |
| 2 | Signal Commitment | 1000 |
| 3 | Memory Object (Create) | 500 |
| 4 | Memory Object (Update) | 500 |
| 5 | Memory Object (Delete) | 500 |
| 6 | Governance Submit | 2000 |
| 7 | Governance Execute | 500 |
| 8 | Register AI Entity | 5000 |
| 9 | Credit AI Entity | 100 |

## AI Entity Types

- **EphemeralAdvisor**: Short-lived, advisory only
- **PersistentOracle**: Long-lived, provides data feeds
- **SignalPublisher**: Publishes signals to the chain

## Autonomy Modes

- **Advisory** (0): Entity provides suggestions, human approves
- **Gated** (1): Entity acts within predefined rules, human can override
- **Autonomous** (2): Reserved, not yet supported

## Anomaly Detection

The copilot detector monitors:
- **Missed blocks**: Validators missing their proposal slot
- **Vote delay**: Consensus round taking longer than expected
- **Peer churn**: Sudden changes in connected peer count
- **Mempool congestion**: Transaction backlog exceeding baseline

## Fee Distribution

- Base transactions: 100% to fee pool
- AI-related transactions (signal, memory, register, credit):
  80% to fee pool, 20% to AI treasury

## Key Metrics to Watch

- `committed_height`: Should increase steadily
- `current_round`: Should stay near 0 (high round = timeouts)
- `peer_count`: Should match validator count minus 1
- `mempool_size`: Spikes indicate congestion
- `view_changes_total`: Cumulative timeouts since start
