"""PURPOSE: Parse the COMPUTE_ORACLE_* environment into a frozen config.

Shared by oracle.py (the long-running loop) and bootstrap.py (setup). The
agent mirrors the price-oracle structure but centralizes config here so the
two entrypoints cannot drift on defaults.

INVARIANTS:
- DRY_RUN defaults to ON. The agent must build and log signals without
  submitting unless the operator explicitly opts into live submission.
- Bad numeric values fall back to the documented default with a warning,
  never crash the process at parse time.

FAILURE MODES:
- None at parse time. Validation of keys, capabilities, and on-chain state
  happens later in chain.py and the entrypoints.
"""

from __future__ import annotations

import logging
import os
from dataclasses import dataclass
from pathlib import Path

LOG = logging.getLogger("compute_oracle.config")

# The canonical metric and tag defaults. The data_tag must stay within
# ORACLE_ANCHOR_DATA_TAG_MAX_LEN (32 bytes); "compute/rtx4090-usd-hr" is 22.
DEFAULT_DATA_TAG = "compute/rtx4090-usd-hr"
DEFAULT_GPU_MODEL = "RTX 4090"
# Public GPU rental marketplace with an open bundles endpoint. The agent
# reads it; it is not a chain RPC. The exact query shape is documented in
# the README; the parser tolerates schema drift by failing closed.
DEFAULT_SOURCE_URL = "https://console.vast.ai/api/v0/bundles/"
DEFAULT_SOURCE_ID = "vast.ai/api/v0/bundles"

DEFAULTS: dict[str, str] = {
    "COMPUTE_ORACLE_RPC_ENDPOINT": "http://localhost:3030",
    "COMPUTE_ORACLE_KEY_PATH": "/etc/novai/compute-oracle-keys.json",
    "COMPUTE_ORACLE_SOURCE_URL": DEFAULT_SOURCE_URL,
    "COMPUTE_ORACLE_SOURCE_ID": DEFAULT_SOURCE_ID,
    "COMPUTE_ORACLE_GPU_MODEL": DEFAULT_GPU_MODEL,
    "COMPUTE_ORACLE_DATA_TAG": DEFAULT_DATA_TAG,
    "COMPUTE_ORACLE_METRICS_HOST": "127.0.0.1",
    "COMPUTE_ORACLE_METRICS_PORT": "9202",
    "COMPUTE_ORACLE_LOOP_INTERVAL_SECS": "300",
    "COMPUTE_ORACLE_HTTP_TIMEOUT_SECS": "10",
    "COMPUTE_ORACLE_LOG_LEVEL": "INFO",
    "COMPUTE_ORACLE_DRY_RUN": "1",
    "COMPUTE_ORACLE_DRY_RUN_NONCE": "0",
    "COMPUTE_ORACLE_RUN_ONCE": "0",
    "COMPUTE_ORACLE_EXPIRY_HEIGHT": "0",
    "COMPUTE_ORACLE_ANCHOR_FEE": "1000",
    "COMPUTE_ORACLE_REPUTATION_ENABLED": "0",
    "COMPUTE_ORACLE_REPUTATION_FEE": "1000",
    "COMPUTE_ORACLE_REPUTATION_TARGET": "",
    "COMPUTE_ORACLE_REPUTATION_EVENT_TYPE": "0",
    "COMPUTE_ORACLE_REPUTATION_POINTS_DELTA": "0",
    "COMPUTE_ORACLE_ENTITY_MIN_BALANCE": "5000",
    "COMPUTE_ORACLE_ACCOUNT_MIN_BALANCE": "200000",
    "COMPUTE_ORACLE_CREDIT_AMOUNT": "100000",
    "COMPUTE_ORACLE_CREDIT_RETRY_AFTER_SECS": "300",
    "COMPUTE_ORACLE_FAUCET_RETRY_AFTER_SECS": "3600",
}

_TRUE = {"1", "true", "yes", "on"}
_FALSE = {"0", "false", "no", "off", ""}


@dataclass(frozen=True)
class ComputeOracleConfig:
    endpoint: str
    key_path: Path
    source_url: str
    source_id: str
    gpu_model: str
    data_tag: str
    metrics_host: str
    metrics_port: int
    loop_interval_secs: float
    http_timeout_secs: float
    log_level: str
    dry_run: bool
    dry_run_nonce: int
    run_once: bool
    expiry_height: int
    anchor_fee: int
    reputation_enabled: bool
    reputation_fee: int
    reputation_target_hex: str
    reputation_event_type: int
    reputation_points_delta: int
    entity_min_balance: int
    account_min_balance: int
    credit_amount: int
    credit_retry_after_secs: float
    faucet_retry_after_secs: float

    @classmethod
    def from_env(cls, env: dict[str, str] | None = None) -> "ComputeOracleConfig":
        env = env if env is not None else dict(os.environ)

        def get(key: str) -> str:
            return env.get(key, DEFAULTS[key])

        return cls(
            endpoint=get("COMPUTE_ORACLE_RPC_ENDPOINT"),
            key_path=Path(get("COMPUTE_ORACLE_KEY_PATH")),
            source_url=get("COMPUTE_ORACLE_SOURCE_URL"),
            source_id=get("COMPUTE_ORACLE_SOURCE_ID"),
            gpu_model=get("COMPUTE_ORACLE_GPU_MODEL"),
            data_tag=get("COMPUTE_ORACLE_DATA_TAG"),
            metrics_host=get("COMPUTE_ORACLE_METRICS_HOST"),
            metrics_port=_env_int("COMPUTE_ORACLE_METRICS_PORT", env),
            loop_interval_secs=_env_float("COMPUTE_ORACLE_LOOP_INTERVAL_SECS", env),
            http_timeout_secs=_env_float("COMPUTE_ORACLE_HTTP_TIMEOUT_SECS", env),
            log_level=get("COMPUTE_ORACLE_LOG_LEVEL"),
            dry_run=_env_bool("COMPUTE_ORACLE_DRY_RUN", env),
            dry_run_nonce=_env_int("COMPUTE_ORACLE_DRY_RUN_NONCE", env),
            run_once=_env_bool("COMPUTE_ORACLE_RUN_ONCE", env),
            expiry_height=_env_int("COMPUTE_ORACLE_EXPIRY_HEIGHT", env),
            anchor_fee=_env_int("COMPUTE_ORACLE_ANCHOR_FEE", env),
            reputation_enabled=_env_bool("COMPUTE_ORACLE_REPUTATION_ENABLED", env),
            reputation_fee=_env_int("COMPUTE_ORACLE_REPUTATION_FEE", env),
            reputation_target_hex=get("COMPUTE_ORACLE_REPUTATION_TARGET"),
            reputation_event_type=_env_int("COMPUTE_ORACLE_REPUTATION_EVENT_TYPE", env),
            reputation_points_delta=_env_int(
                "COMPUTE_ORACLE_REPUTATION_POINTS_DELTA", env
            ),
            entity_min_balance=_env_int("COMPUTE_ORACLE_ENTITY_MIN_BALANCE", env),
            account_min_balance=_env_int("COMPUTE_ORACLE_ACCOUNT_MIN_BALANCE", env),
            credit_amount=_env_int("COMPUTE_ORACLE_CREDIT_AMOUNT", env),
            credit_retry_after_secs=_env_float(
                "COMPUTE_ORACLE_CREDIT_RETRY_AFTER_SECS", env
            ),
            faucet_retry_after_secs=_env_float(
                "COMPUTE_ORACLE_FAUCET_RETRY_AFTER_SECS", env
            ),
        )


def _env_int(key: str, env: dict[str, str]) -> int:
    raw = env.get(key, DEFAULTS[key])
    try:
        return int(raw)
    except (TypeError, ValueError):
        LOG.warning(
            "config event=bad_int key=%s value=%s falling_back=%s",
            key,
            raw,
            DEFAULTS[key],
        )
        return int(DEFAULTS[key])


def _env_float(key: str, env: dict[str, str]) -> float:
    raw = env.get(key, DEFAULTS[key])
    try:
        return float(raw)
    except (TypeError, ValueError):
        LOG.warning(
            "config event=bad_float key=%s value=%s falling_back=%s",
            key,
            raw,
            DEFAULTS[key],
        )
        return float(DEFAULTS[key])


def _env_bool(key: str, env: dict[str, str]) -> bool:
    raw = env.get(key, DEFAULTS[key]).strip().lower()
    if raw in _TRUE:
        return True
    if raw in _FALSE:
        return False
    LOG.warning(
        "config event=bad_bool key=%s value=%s falling_back=%s",
        key,
        raw,
        DEFAULTS[key],
    )
    return DEFAULTS[key].strip().lower() in _TRUE
