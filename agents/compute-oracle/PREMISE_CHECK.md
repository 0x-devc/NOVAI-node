# Compute Oracle Sub-Agent: Premise Check

Date: 2026-06-18
Branch: feature/compute-oracle-agent
Mode: build and dry-run only (no live chain, no funds, no on-chain side effects)

This is the recon artifact I wrote before any code. It states what a compute
oracle sub-agent needs, what already exists to model it on, confirms the SDK
builders this agent will use, and states the real gap between the task wording
and the codebase as it stands.

## 1. What a compute oracle sub-agent needs

A compute oracle is an autonomous agent that observes an off-chain quantity
(here, GPU rental pricing), commits a reproducible hash of that observation to
the chain as an `OracleAnchor` signal (signal type 22), and optionally adjusts
peer reputation with a `ReputationUpdate` signal (signal type 7). To do that it
needs:

- A pricing data source it can fetch and parse, with graceful handling when the
  source is down or returns malformed data.
- A canonical, reproducible encoding of the observation so the committed
  `data_hash` can be re-derived and audited by anyone.
- The SDK signal builders to construct the exact on-chain payload bytes.
- A registered entity holding the `post_oracle_anchors` capability (bit 6) to
  issue anchors, and `submit_reputation_updates` to issue reputation signals.
- A long-running loop with metrics, structured logging, and clean shutdown.
- A DRY_RUN mode that constructs and logs the signal it would submit without
  touching the chain.

## 2. What already exists to model it on

The existing agent at `agents/price-oracle/` is a Python agent, not Rust. Its
layout is the template:

- `bootstrap.py`: idempotent setup (two ed25519 keypairs, faucet, register a
  type-10 entity with the oracle capability bound to the entity pubkey).
- `oracle.py`: long-running loop on a fixed cadence (default 60s): fetch price,
  optional two-tier top-up, build the observation hash, submit an `OracleAnchor`,
  export Prometheus metrics, exit cleanly on SIGTERM.
- `lib/chain.py`: the single funnel for all RPC interaction, plus exception to
  metric-label mapping.
- `lib/coingecko.py`: urllib fetch with an injectable opener and exponential
  backoff on rate limits.
- `lib/signal.py`: deterministic `blake3` hash of a canonical ASCII observation
  string `BTC-USD@{ts}={price:.2f}`.
- `lib/metrics.py`, `lib/log.py`: Prometheus text registry and structured
  logging.
- `systemd/`, `tests/`, `README.md`.

The tests stub the network at two seams (an injectable HTTP opener and a
`FakeChain`), so the whole agent is exercised with zero live calls.

## 3. SDK confirmation: the builders this agent will use

The Python SDK at `sdk/novai-python-sdk/` (package `novai-sdk`, version 0.1.0)
ships both builders the task references. Confirmed by reading the source, not
just the changelog:

OracleAnchor (type 22), in `novai_sdk/signals/oracle.py`:

- `build_oracle_anchor_extras(data_hash, external_timestamp, source_hash,
  expiry_height, data_tag) -> bytes` produces the tail
  `[data_hash:32][ext_ts:8 BE][source_hash:32][expiry_height:8 BE][tag_len:1][tag:1..32]`.
- `derive_oracle_anchor_signal_hash(issuer_entity_id, data_hash,
  external_timestamp, source_hash, data_tag) -> bytes` returns the 32-byte
  content-addressed signal hash (domain tag `novai-oracle-anchor-v1`).

ReputationUpdate (type 7), in `novai_sdk/signals/reputation.py`:

- `build_reputation_update_extras(target_entity_id, event_type, points_delta)
  -> bytes` produces the fixed 35-byte tail `[target:32][event_type:1][delta:2 BE i16]`.

Shared and supporting, in `novai_sdk/tx/signal.py`, `codec.py`, `crypto.py`:

- `build_signal_commitment_payload(signal_hash, signal_type, issuer_entity_id,
  extras) -> bytes` produces `[0x02][signal_hash:32][signal_type:1][issuer:32][extras]`.
- `AiSignalType.ORACLE_ANCHOR == 22`, `AiSignalType.REPUTATION_UPDATE == 7`.
- `sign_tx_v1(signing_key, tx)` and `encode_tx_v1_signed(tx)` produce the signed
  wire bytes locally, with no network. The live submit path is
  `AsyncNOVAIClient.submit_tx` -> `novai_submitTransaction` RPC, which I will
  never call in dry-run.

## 4. Byte-layout source of truth and golden vector

The on-chain format is defined in `crates/execution/src/lib.rs` and the
canonical builder in `tools/novai-cli/src/commands/oracle.rs`. Sizes:

- Base commitment: 66 bytes `[version=2:1][signal_hash:32][signal_type:1][issuer:32]`.
- OracleAnchor total: 148 to 179 bytes (66 base + 81 fixed tail + 1..32 tag).
- ReputationUpdate total: exactly 101 bytes (66 base + 35 tail).

The CLI test `anchor_payload_layout_is_correct` pins an exact 160-byte vector
(signal_hash `0x10*32`, issuer `0x01*32`, data_hash `0xAB*32`, ext_ts
`0x0102030405060708`, source_hash `0xCD*32`, expiry 5000, tag `price/ETH-USD`).
The SDK builders should reproduce this byte for byte. I will assert that
equality as the strongest adversarial check: it proves the Python construction
path matches the protocol source of truth, not just itself.

## 5. The real gap between the task wording and the codebase

The task says "mirror the price-oracle's structure" and "in DRY_RUN mode log
exactly what it would submit without submitting." The honest gaps:

1. price-oracle has no dry-run mode. Every submit in `lib/chain.py` is live.
   The central new mechanism I must build is a DRY_RUN path that constructs and
   signs the payload locally and logs it, with no reference to the RPC client.
   This is the main deliverable, not a copy of an existing flag.

2. "Real GPU pricing" is not a single scalar like BTC/USD. GPU price is per
   model, per provider, per hour. I must define a precise, reproducible
   observation: a named GPU model, an aggregation (median on-demand USD per
   hour across current public offers), and fixed decimals. The source is a real
   public marketplace API (Vast.ai bundles). When the source returns no offers
   for the model, the agent must skip the cycle, never fabricate a price.

3. The OracleAnchor `data_hash` commits to off-chain bytes. The agent must
   define the canonical observation encoding so the commitment is reproducible
   and auditable, mirroring price-oracle's `BTC-USD@{ts}={price:.2f}`. I will use
   `GPU-<MODEL>-USD-HR@{ts}={price:.4f}`.

4. ReputationUpdate has no high-level convenience method in the SDK (only the
   extras builder plus `publish_signal`). This is a minor ergonomic gap I will
   flag; it does not block the build.

5. `source_hash` and `expiry_height` are OracleAnchor fields the BTC flow does
   not exercise. I will set `source_hash = blake3(source descriptor)` for honest
   provenance and `expiry_height = 0` (no expiry) by default, both configurable.

6. Capabilities differ by signal: anchors need bit 6 (`post_oracle_anchors`,
   `0x40`); reputation needs `submit_reputation_updates` (a different bit). The
   compute oracle is a distinct entity (`code_hash = blake3("novai-compute-oracle-v1")`).
   On-chain registration is out of scope here, so bootstrap mirrors structure
   but must be dry-run-safe.

## 6. Never-lie application (per field)

- price: only commit a price I actually computed from real offers. No offers for
  the model means skip the cycle, not post a stale or invented number.
- timestamp: the real observation time, validated positive and in range.
- source_hash: hash of the source descriptor I actually queried, not a
  placeholder claiming a source I did not use.
- data_tag: names the exact metric (model and unit), so a reader knows what the
  hash commits to.

## 7. Plan

ONE. lib modules: `config` (env to dataclass), `log`, `metrics`, `gpu_source`
(fetch and parse real public GPU pricing through an injectable opener), `signal`
(canonical observation, `data_hash`, and OracleAnchor + ReputationUpdate
construction via the SDK builders), `chain` (wrapper with `dry_run` gating that
builds, signs, and logs bytes locally and never references the RPC client).

TWO. `oracle.py` main loop running a full cycle in DRY_RUN by default, and
`bootstrap.py` mirroring price-oracle but dry-run-safe.

THREE. `systemd/novai-compute-oracle.service`, `systemd/compute-oracle.env.example`
(DRY_RUN default on), `README.md`. Generic provenance, no PII, no org handles.

FOUR. Tests: parser (success, API down, malformed), signal construction
including the golden-vector byte match against the Rust CLI, dry-run makes no
network call (inject a client that raises on any RPC method), and the loop with
error paths.

FIVE. Run the suite and a full dry-run cycle; capture the constructed signal
(payload hex, signal_hash, decoded fields, wire length). Attempt one best-effort
read-only live pricing fetch and fall back to a bundled sample on any failure.

SIX. Adversarial verification, scope and PII and style scan, final report, and
an optional local commit (no AI attribution, noreply identity, no push).

## 8. What I will NOT verify (held supervised steps)

- Live on-chain acceptance of the anchor or reputation signals.
- Live RPC submission, live entity registration, capability grant, faucet, fees,
  mempool behavior.
- Live pricing-API schema stability, if the optional read-only probe is
  unavailable in this environment.

These are the supervised steps to run later, deliberately out of scope here.

## 9. Guardrails I am operating under

- All writes confined to `agents/compute-oracle/`.
- No edits to `crates/consensus/`, `crates/consensus_types/`, or
  `crates/node/src/consensus_node.rs` (a supervised session owns consensus).
- No live chain, RPC, or funds. No push. No AI attribution. No PII.
- First-person singular, no em or en dashes.
