#!/usr/bin/env python3
"""PURPOSE: Idempotent setup for the compute-oracle entity, DRY_RUN-safe.

Mirrors the price-oracle two-key Type-10 model: a funder keypair pays fees and
seeds the entity economic balance, and a separate entity keypair signs signals
and is bound at registration with the oracle capabilities.

DRY_RUN (default) does NO chain interaction and NO system-path write. It loads
or generates the keypairs, derives the entity_id locally, and logs the exact
registration it WOULD submit. Persisting keys and performing the on-chain
faucet + registration is the held supervised step, enabled only when DRY_RUN
is off (and, for key persistence, COMPUTE_ORACLE_BOOTSTRAP_WRITE_KEYS=1).

EXIT CODES:
- 0: success (dry-run plan logged, or live registration confirmed).
- 2: keyfile load/format error.
- 3: faucet cooldown and balance too low (live).
- 4: entity exists without the oracle capability (live).
- 5: RPC unreachable or registration not confirmed (live).
"""

from __future__ import annotations

import json
import logging
import os
import sys
import time
from pathlib import Path

from novai_sdk import Capabilities, Keypair

from lib.chain import (
    POST_ORACLE_ANCHORS_BIT,
    SUBMIT_REPUTATION_UPDATES_BIT,
    Chain,
)
from lib.config import ComputeOracleConfig
from lib.log import configure_logging
from oracle import load_keypair_from_file

LOG = logging.getLogger("compute_oracle.bootstrap")

MIN_BALANCE_FOR_REGISTER = 60_000
REGISTER_INITIAL_BALANCE = 50_000
REGISTER_FEE = 5_000
POLL_SECS = 30


def capabilities_for(cfg: ComputeOracleConfig) -> Capabilities:
    """Oracle capability set, plus reputation if the agent will emit it."""
    caps = Capabilities.oracle()
    if cfg.reputation_enabled:
        caps = caps | Capabilities(submit_reputation_updates=True)
    return caps


def load_or_generate_keys(cfg: ComputeOracleConfig) -> tuple[Keypair, Keypair, bool]:
    """Return (funder_kp, entity_kp, generated). Loads a v2 keyfile if present."""
    if cfg.key_path.exists():
        funder_kp, entity_kp, _data = load_keypair_from_file(cfg.key_path)
        LOG.info("keys event=loaded path=%s", cfg.key_path)
        return funder_kp, entity_kp, False
    funder_kp = Keypair.generate()
    entity_kp = Keypair.generate()
    LOG.info("keys event=generated funder=%s entity=%s", funder_kp.address.hex(), entity_kp.address.hex())
    return funder_kp, entity_kp, True


def keyfile_dict(
    funder_kp: Keypair,
    entity_kp: Keypair,
    *,
    entity_id_hex: str | None,
    capabilities_byte: int,
    registered_at_unix: int | None,
) -> dict[str, object]:
    out: dict[str, object] = {
        "version": 2,
        "funder_seed_hex": funder_kp.seed.hex(),
        "funder_pubkey_hex": funder_kp.pubkey.hex(),
        "funder_address_hex": funder_kp.address.hex(),
        "entity_seed_hex": entity_kp.seed.hex(),
        "entity_pubkey_hex": entity_kp.pubkey.hex(),
        "entity_address_hex": entity_kp.address.hex(),
        "capabilities_byte": capabilities_byte,
    }
    if entity_id_hex is not None:
        out["entity_id_hex"] = entity_id_hex
    if registered_at_unix is not None:
        out["registered_at_unix"] = registered_at_unix
    return out


def write_keyfile_atomic(path: Path, data: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2), encoding="utf-8")
    if sys.platform != "win32":
        os.chmod(tmp, 0o600)
    os.replace(tmp, path)


def _bootstrap_dry_run(
    cfg: ComputeOracleConfig,
    chain: Chain,
    funder_kp: Keypair,
    entity_kp: Keypair,
    generated: bool,
) -> int:
    caps = capabilities_for(cfg)
    caps_byte = caps.to_byte()
    entity_id = chain.entity_id_for(funder_kp.address)
    LOG.info(
        "bootstrap event=dry_run_plan submitted=false funder=%s entity_pubkey=%s "
        "entity_id=%s code_hash_tag=%s caps_byte=0x%02x post_anchors=%s reputation=%s "
        "initial_balance=%d fee=%d",
        funder_kp.address.hex(),
        entity_kp.pubkey.hex(),
        entity_id.hex(),
        "novai-compute-oracle-v1",
        caps_byte,
        bool(caps_byte & POST_ORACLE_ANCHORS_BIT),
        bool(caps_byte & SUBMIT_REPUTATION_UPDATES_BIT),
        REGISTER_INITIAL_BALANCE,
        REGISTER_FEE,
    )
    LOG.info(
        "bootstrap event=dry_run_notice detail=%s",
        "no faucet, no RegisterAiEntityWithKey, no on-chain write; this is the held supervised step",
    )
    write_keys = os.environ.get("COMPUTE_ORACLE_BOOTSTRAP_WRITE_KEYS", "0").strip().lower() in {
        "1",
        "true",
        "yes",
        "on",
    }
    if write_keys and generated:
        try:
            write_keyfile_atomic(
                cfg.key_path,
                keyfile_dict(
                    funder_kp,
                    entity_kp,
                    entity_id_hex=None,
                    capabilities_byte=caps_byte,
                    registered_at_unix=None,
                ),
            )
            LOG.info("keys event=persisted path=%s mode=0600 note=entity_id_set_at_live_register", cfg.key_path)
        except OSError as exc:
            LOG.warning("keys event=persist_failed path=%s error=%s", cfg.key_path, exc)
    else:
        LOG.info("keys event=not_persisted reason=dry_run_default advice=set COMPUTE_ORACLE_BOOTSTRAP_WRITE_KEYS=1_to_persist")
    return 0


def _bootstrap_live(
    cfg: ComputeOracleConfig,
    chain: Chain,
    funder_kp: Keypair,
    entity_kp: Keypair,
) -> int:
    """Live faucet + register + persist. Held supervised step; not exercised here."""
    caps = capabilities_for(cfg)
    entity_id = chain.entity_id_for(funder_kp.address)

    existing = chain.get_entity_status(entity_id)
    if existing.exists and existing.has_post_oracle_anchors:
        LOG.info("bootstrap event=already_registered entity_id=%s caps=0x%02x", entity_id.hex(), existing.capabilities)
        _persist(cfg, funder_kp, entity_kp, entity_id, caps.to_byte())
        return 0
    if existing.exists and not existing.has_post_oracle_anchors:
        LOG.error("bootstrap event=entity_poisoned entity_id=%s caps=0x%02x", entity_id.hex(), existing.capabilities)
        return 4

    if chain.get_balance(funder_kp.address) < MIN_BALANCE_FOR_REGISTER:
        chain.faucet(funder_kp.address)
        deadline = time.monotonic() + POLL_SECS
        while time.monotonic() < deadline:
            if chain.get_balance(funder_kp.address) >= MIN_BALANCE_FOR_REGISTER:
                break
            time.sleep(2.0)
        if chain.get_balance(funder_kp.address) < MIN_BALANCE_FOR_REGISTER:
            LOG.error("bootstrap event=funding_failed balance_below=%d", MIN_BALANCE_FOR_REGISTER)
            return 3

    result = chain.register_oracle_with_key(
        funder_kp,
        entity_kp.pubkey,
        capabilities=caps,
        fee=REGISTER_FEE,
        initial_balance=REGISTER_INITIAL_BALANCE,
    )
    LOG.info("bootstrap event=register_submitted txid=%s entity_id=%s", result.txid, entity_id.hex())

    deadline = time.monotonic() + POLL_SECS
    while time.monotonic() < deadline:
        status = chain.get_entity_status(entity_id)
        if status.exists and status.has_post_oracle_anchors:
            _persist(cfg, funder_kp, entity_kp, entity_id, caps.to_byte())
            LOG.info("bootstrap event=registered entity_id=%s caps=0x%02x", entity_id.hex(), status.capabilities)
            return 0
        if status.exists and not status.has_post_oracle_anchors:
            LOG.error("bootstrap event=registered_without_capability caps=0x%02x", status.capabilities)
            return 4
        time.sleep(2.0)
    LOG.error("bootstrap event=register_not_confirmed entity_id=%s", entity_id.hex())
    return 5


def _persist(
    cfg: ComputeOracleConfig,
    funder_kp: Keypair,
    entity_kp: Keypair,
    entity_id: bytes,
    caps_byte: int,
) -> None:
    write_keyfile_atomic(
        cfg.key_path,
        keyfile_dict(
            funder_kp,
            entity_kp,
            entity_id_hex=entity_id.hex(),
            capabilities_byte=caps_byte,
            registered_at_unix=int(time.time()),
        ),
    )
    LOG.info("keys event=persisted path=%s mode=0600", cfg.key_path)


def main(argv: list[str] | None = None) -> int:
    cfg = ComputeOracleConfig.from_env()
    configure_logging(cfg.log_level)
    LOG.info(
        "bootstrap_init event=loading mode=%s endpoint=%s key_path=%s",
        "dry_run" if cfg.dry_run else "live",
        cfg.endpoint,
        cfg.key_path,
    )
    chain = Chain(cfg.endpoint, dry_run=cfg.dry_run, dry_run_nonce=cfg.dry_run_nonce)
    try:
        funder_kp, entity_kp, generated = load_or_generate_keys(cfg)
    except (OSError, json.JSONDecodeError, ValueError) as exc:
        LOG.error("bootstrap_init event=keyfile_error error=%s path=%s", exc, cfg.key_path)
        return 2

    if cfg.dry_run:
        return _bootstrap_dry_run(cfg, chain, funder_kp, entity_kp, generated)
    return _bootstrap_live(cfg, chain, funder_kp, entity_kp)


if __name__ == "__main__":
    sys.exit(main())
