# price-oracle

The first NOVAI sub-agent. Two Python scripts that turn the local node
into a published BTC/USD price feed using the two-key Type-10 funding
model documented in [`docs/AGENT_FUNDING_PLAYBOOK.md`](../../docs/AGENT_FUNDING_PLAYBOOK.md):

- `bootstrap.py` runs once: generates a funder ed25519 keypair and a
  separate entity ed25519 keypair, requests faucet funds for the
  funder, and submits a `RegisterEntityWithKey` (Type-10) transaction
  signed by the funder. The entity signing key is bound at registration
  with the `post_oracle_anchors` capability (bit 6).
- `oracle.py` runs forever as a systemd service: fetches BTC/USD from
  CoinGecko every 60 seconds, signs `CreditAiEntity` top-ups with the
  funder when the entity ledger drops, builds a deterministic
  `OracleAnchor` signal signed by the entity key, submits it via the
  Python SDK, and exposes Prometheus metrics on `localhost:9201`.

Future NOVAI sub-agents (the next nineteen) should copy this shape:
flat module layout, stdlib + SDK, dataclass config from env, structured
`event=key=value` logging, systemd-ready, and the two-key funding model
documented in the playbook linked above.

## Layout

```
bootstrap.py        idempotent setup
oracle.py           long-running main loop
lib/log.py          stdlib logging configured to the monitor format
lib/coingecko.py    urllib HTTP fetch + exponential backoff
lib/signal.py       deterministic data_hash for a price observation
lib/metrics.py      stdlib HTTPServer + Prometheus text registry
lib/chain.py        NOVAIClient wrapper, error -> reason map
tests/              pytest, no network
systemd/            unit file and env template
```

## Prerequisites

- Python 3.12 on the host ([redacted-host] has it; macOS via `brew install python@3.12`).
- The `novai_sdk` package importable in whatever Python the unit runs
  under. The locked deploy uses a per-agent venv at
  `/opt/novai-price-oracle/.venv` with the SDK editable-installed from
  the in-repo `sdk/novai-python-sdk/`. The SDK pulls
  `pynacl`, `blake3`, `aiohttp` transitively; nothing else.
- A reachable NOVAI node on `localhost:3030` with the public faucet
  enabled (`--faucet-key`).

## Bootstrap (one time)

On the host, after the repo is in `[redacted-server]/NOVAI-node/NOVAI-node`:

```bash
python3.12 -m venv /opt/novai-price-oracle/.venv
/opt/novai-price-oracle/.venv/bin/pip install -e \
    [redacted-server]/NOVAI-node/NOVAI-node/sdk/novai-python-sdk
install -d /opt/novai-price-oracle/lib /etc/novai
install -m 0755 [redacted-server]/NOVAI-node/NOVAI-node/agents/price-oracle/bootstrap.py \
                [redacted-server]/NOVAI-node/NOVAI-node/agents/price-oracle/oracle.py \
                /opt/novai-price-oracle/
install -m 0644 [redacted-server]/NOVAI-node/NOVAI-node/agents/price-oracle/lib/*.py \
                /opt/novai-price-oracle/lib/
install -m 0644 [redacted-server]/NOVAI-node/NOVAI-node/agents/price-oracle/systemd/novai-price-oracle.service \
                /etc/systemd/system/
install -m 0600 [redacted-server]/NOVAI-node/NOVAI-node/agents/price-oracle/systemd/oracle.env.example \
                /etc/novai/oracle.env

set -a && source /etc/novai/oracle.env && set +a
/opt/novai-price-oracle/.venv/bin/python /opt/novai-price-oracle/bootstrap.py
```

The script prints a summary on success: funder address, funder balance,
entity address, entity_id, capability byte (`0x47` for an oracle =
bits 0,1,2,6). Safe to re-run on a v2 keyfile; refuses to load a v1
(single-key) keyfile with `KeyFileVersionError` so the operator can
archive the legacy file deliberately.

## Service

```bash
systemctl daemon-reload
systemctl enable --now novai-price-oracle
systemctl status novai-price-oracle
journalctl -u novai-price-oracle -f
```

## Metrics

```bash
curl -sS http://localhost:9201/metrics
```

The exposed series, all prefixed `novai_oracle_`:

| name | type | meaning |
|---|---|---|
| `price_fetch_success_total` | counter | successful CoinGecko reads |
| `price_fetch_failure_total{reason}` | counter | reasons: `rate_limit`, `server_error`, `network_error`, `parse_error` |
| `submission_success_total` | counter | OracleAnchor signals accepted by the chain |
| `submission_failure_total{reason}` | counter | reasons include `fee_too_low`, `nonce_too_low`, `mempool_full`, `validation_failed`, `rpc_unreachable`, `entity_not_registered` |
| `last_price_usd` | gauge | most recent observed BTC/USD price |
| `last_submission_height` | gauge | chain head height at the last successful submission |
| `last_loop_completed_timestamp` | gauge | unix seconds of the last completed loop |
| `uptime_seconds` | gauge | seconds since process start |

## Configuration

All knobs are environment variables. See `systemd/oracle.env.example`.

| variable | default | notes |
|---|---|---|
| `PRICE_ORACLE_RPC_ENDPOINT` | `http://localhost:3030` | local NOVAI node |
| `PRICE_ORACLE_KEY_PATH` | `/etc/novai/oracle-keys.json` | funder + entity seeds plus derived public info, 0600, v2 schema |
| `PRICE_ORACLE_COINGECKO_URL` | CoinGecko BTC/USD endpoint | swap for a paid tier if needed |
| `PRICE_ORACLE_METRICS_HOST` | `127.0.0.1` | bind localhost only |
| `PRICE_ORACLE_METRICS_PORT` | `9201` | agent #1 reserves 9201; pattern is 9200+N |
| `PRICE_ORACLE_LOOP_INTERVAL_SECS` | `60` | one submission per minute under free-tier limit |
| `PRICE_ORACLE_HTTP_TIMEOUT_SECS` | `10` | hard timeout on every outbound HTTP call |
| `PRICE_ORACLE_DATA_TAG` | `price/BTC-USD` | anchor tag, indexed by the chain |
| `PRICE_ORACLE_LOG_LEVEL` | `INFO` | one of DEBUG / INFO / WARNING / ERROR |

## Tests

From a checkout with the SDK venv on the path:

```bash
cd agents/price-oracle
PYTHONPATH=. ../../sdk/novai-python-sdk/.venv/bin/python -m pytest tests/ -v
```

There are no network-touching tests; every external call is stubbed.

## Troubleshooting

- **`bootstrap.py` exits 3** (`cooldown_and_insufficient_balance`). The
  public faucet is per-IP per 24h. If the host has already drawn within
  the last day and the address has no balance, you must wait for the
  cooldown to expire or top up from another funded address with a
  manual `transfer` tx.
- **`bootstrap.py` or `oracle.py` exits 2** with
  `KeyFileVersionError`. The keyfile on disk is v1 (single-key Type-8)
  but the agent is v2 (two-key Type-10). The v1 key is bound to the
  dead Type-8 entity and cannot be migrated; archive
  `/etc/novai/oracle-keys.json` and re-run `bootstrap.py` to generate
  a fresh v2 keyfile.
- **`bootstrap.py` exits 4** (`entity_exists_without_bit_6`). The
  `(code_hash, funder_addr)` pair already maps to an entity, but it
  was registered without `post_oracle_anchors`. Capabilities are frozen
  post-register; you cannot upgrade them. Rotate the funder (archive
  `oracle-keys.json`, re-run bootstrap to generate a fresh two-key
  pair) or bump `ORACLE_CODE_HASH` in `lib/chain.py`.
- **`oracle.py` exits 4** (`metrics bind failed`). Port 9201 is in use.
  `ss -tlnp | grep 9201` finds the holder.
- **`oracle.py` exits 3** (`entity_not_ready`). Bootstrap has not been
  run (or it landed but has not yet been committed on chain). Re-run
  `bootstrap.py` and wait a block.
- **`oracle.py` exits 6** (`entity_id_drift_fatal` or
  `keyfile_missing_entity_id`). The keyfile's stored `entity_id_hex`
  disagrees with what `chain.entity_id_for(funder_address)` derives,
  or it is missing entirely. Silently funding the wrong entity is the
  bug class this check exists to prevent. Archive the keyfile and
  re-run `bootstrap.py`.
- **Submission failures with `reason=fee_too_low`**. The chain bumped
  `MIN_FEE_SIGNAL_COMMITMENT`. Raise the `fee=` default in
  `lib/chain.py:post_anchor`.
- **Submission failures with `reason=nonce_too_low`**. The SDK refetches
  nonce per submit; transient races resolve on the next tick.

## What is intentionally NOT in this ship

- Multi-asset support. BTC/USD only. Multi-asset adds an off-tick
  scheduler and a second `data_tag`; defer to v2.
- Anomaly detection (cross-oracle, cross-source). Defer.
- Self-repair on faucet exhaustion. The oracle stops submitting once the
  balance drops below the per-signal minimum fee; an operator alert
  catches it.
- Replay of failed submissions. Each tick is fresh; no queue.
- Per-key user, AppArmor, syscall sandbox. v0 runs as root. Hardening is
  an open loop.
