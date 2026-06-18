# NOVAI compute oracle

An autonomous agent that observes GPU rental pricing from a public marketplace
and commits a reproducible hash of that observation to the chain as an
`OracleAnchor` signal (signal type 22). It can optionally emit a
`ReputationUpdate` (signal type 7). It is the second NOVAI oracle agent and
mirrors the structure of `agents/price-oracle/`.

## Safety: DRY_RUN by default

`COMPUTE_ORACLE_DRY_RUN=1` is the default. In dry-run the agent fetches
pricing, constructs the exact signal it would submit, signs the transaction
locally, and logs every byte, without contacting the chain RPC. No
transaction is submitted, no funds are spent, and no entity is registered.

The dry-run code path never constructs or references the RPC client. Reads and
funding writes raise `DryRunError` if reached in dry-run, so an accidental
network call fails loudly rather than going live silently.

Enabling live submission (`COMPUTE_ORACLE_DRY_RUN=0`) is a deliberate
supervised step. So is on-chain registration (`bootstrap.py` in live mode).

## What it observes (never-lie)

The data source is a public GPU rental marketplace bundles API (Vast.ai by
default). This is a read-only HTTP GET against a public pricing source; it is
not a chain RPC.

The observation is the median per-GPU on-demand USD-per-hour for the configured
model across currently-listed offers. For each offer the per-GPU price is
`dph_total / num_gpus`. The canonical, reproducible observation string is:

    GPU-<MODEL>-USD-HR@<unix_ts>=<price formatted to 4 decimals>

for example `GPU-RTX4090-USD-HR@1718000000=0.3400`. The committed `data_hash`
is `blake3` of those ASCII bytes, so anyone can re-derive and audit it.

If the source returns no usable offer for the model, the agent skips the cycle.
It never posts a fabricated or stale price.

## Signal layout

The OracleAnchor commitment payload is built with the `novai_sdk` builders, so
the bytes match the protocol source of truth:

    [0x02][signal_hash:32][22][issuer:32]
    [data_hash:32][external_timestamp:8 BE][source_hash:32]
    [expiry_height:8 BE][data_tag_len:1][data_tag:1..=32]

`source_hash` is `blake3` of the source identifier the agent queried. The
optional ReputationUpdate tail is `[target:32][event_type:1][points_delta:2 BE i16]`.

Known SDK gap: `novai_sdk` ships `build_reputation_update_extras` but no
canonical signal-hash derivation and no high-level helper for type 7. The agent
uses a clearly-labeled local content id for the dry-run reputation envelope; the
chain's canonical derivation must be confirmed before any live reputation
submission.

## Layout

    bootstrap.py   one-time setup (DRY_RUN-safe; live faucet + register is held)
    oracle.py      long-running loop (fetch, build, log or submit, metrics)
    lib/config.py  COMPUTE_ORACLE_* environment to a frozen config
    lib/gpu_source.py  public pricing fetch + parse, injectable opener, backoff
    lib/signal.py  canonical observation + OracleAnchor/Reputation construction
    lib/chain.py   chain funnel with the DRY_RUN construct-and-log path
    lib/metrics.py Prometheus text registry + HTTP server
    lib/log.py     structured logging
    systemd/       unit file and environment template
    tests/         parser, signal construction, dry-run, loop tests

## Running the dry-run demonstration

The agent depends on `novai_sdk` (the repo's Python SDK). Install it into a
virtualenv, then run a single dry-run cycle:

    python -m venv .venv
    .venv/bin/python -m pip install -e <repo>/sdk/novai-python-sdk
    COMPUTE_ORACLE_DRY_RUN=1 COMPUTE_ORACLE_RUN_ONCE=1 \
      .venv/bin/python agents/compute-oracle/oracle.py

With no keyfile present, dry-run generates ephemeral keys and derives the
entity id locally. The constructed signal (payload hex, signal hash, txid, wire
length) is logged with `submitted=false`.

## Tests

From the repo root, with `novai_sdk` installed in the active environment:

    python -m pytest agents/compute-oracle/tests -q

The suite stubs the network at the HTTP layer and never touches a live chain.

## Metrics

In service mode the agent serves Prometheus text at
`http://127.0.0.1:9202/metrics`.

## Held supervised steps (out of scope for this build)

- Live on-chain submission of the anchor and reputation signals.
- Live entity registration, capability grant, faucet, and funding.
- Confirming live acceptance and the canonical reputation signal-hash.
