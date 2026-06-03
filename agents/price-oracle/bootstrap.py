#!/usr/bin/env python3
"""PURPOSE: One-shot idempotent setup for the NOVAI price-oracle.

Steps (every step skips itself if already done):

  1. Load or generate an ed25519 keypair; write /etc/novai/oracle-keys.json
     at 0600 with seed + derived public info.
  2. Fund the address via the public faucet if balance is below the
     register threshold. Faucet is per-IP-24h-cooldown; bootstrap exits
     non-zero if the cooldown blocks a needed top-up.
  3. RegisterEntity with Capabilities.oracle() (bits 0,1,2,6 = 0x47) if
     no entity with this (code_hash, creator_addr) is on-chain yet, then
     verify capability bit 6 (post_oracle_anchors) is set in the on-chain
     view. Capabilities are frozen post-register, so a mismatch is fatal
     and requires manual cleanup.
  4. Rewrite oracle-keys.json with entity_id, capabilities, registered_at.

INVARIANTS:
- Re-running on the same host with the same key file is a no-op once the
  oracle is registered.
- The key file is the only persistent secret; everything else is derived.

FAILURE MODES:
- Missing config (PRICE_ORACLE_RPC_ENDPOINT) -> exit 2.
- Faucet cooldown AND insufficient balance for register -> exit 3.
- Entity exists without bit 6 -> exit 4 (operator intervention required).
- RPC unreachable / timeout -> exit 5 (transient; systemd-free script, so
  the operator re-runs).
"""

from __future__ import annotations

import json
import logging
import os
import sys
import time
from dataclasses import dataclass
from pathlib import Path

from novai_sdk import Keypair, RateLimitedError

from lib.chain import POST_ORACLE_ANCHORS_BIT, Chain
from lib.log import configure_logging

LOG = logging.getLogger("price_oracle.bootstrap")

DEFAULT_KEY_PATH = "/etc/novai/oracle-keys.json"
DEFAULT_ENDPOINT = "http://localhost:3030"
KEYFILE_VERSION = 1
KEYFILE_MODE = 0o600

MIN_BALANCE_FOR_REGISTER = 50_000
MIN_BALANCE_TO_OPERATE = 5_000
FAUCET_POLL_TIMEOUT_SECS = 30.0
FAUCET_POLL_INTERVAL_SECS = 2.0
REGISTER_POLL_TIMEOUT_SECS = 30.0
REGISTER_POLL_INTERVAL_SECS = 2.0


@dataclass(frozen=True)
class BootstrapConfig:
    endpoint: str
    key_path: Path
    log_level: str

    @classmethod
    def from_env(cls, env: dict[str, str] | None = None) -> "BootstrapConfig":
        env = env if env is not None else dict(os.environ)
        return cls(
            endpoint=env.get("PRICE_ORACLE_RPC_ENDPOINT", DEFAULT_ENDPOINT),
            key_path=Path(env.get("PRICE_ORACLE_KEY_PATH", DEFAULT_KEY_PATH)),
            log_level=env.get("PRICE_ORACLE_LOG_LEVEL", "INFO"),
        )


@dataclass
class KeyFile:
    seed_hex: str
    pubkey_hex: str
    address_hex: str
    entity_id_hex: str | None = None
    capabilities_byte: int | None = None
    registered_at_unix: int | None = None

    def to_dict(self) -> dict[str, object]:
        d: dict[str, object] = {
            "version": KEYFILE_VERSION,
            "seed_hex": self.seed_hex,
            "pubkey_hex": self.pubkey_hex,
            "address_hex": self.address_hex,
        }
        if self.entity_id_hex is not None:
            d["entity_id_hex"] = self.entity_id_hex
        if self.capabilities_byte is not None:
            d["capabilities_byte"] = self.capabilities_byte
        if self.registered_at_unix is not None:
            d["registered_at_unix"] = self.registered_at_unix
        return d


def load_or_generate_key(path: Path) -> tuple[Keypair, KeyFile, bool]:
    """Return (keypair, keyfile, generated). ``generated`` is True if we wrote a new key."""
    if path.exists():
        kp, kf = _load_key(path)
        LOG.info("keypair_load event=reused path=%s address=%s", path, kf.address_hex)
        return kp, kf, False
    kp = Keypair.generate()
    kf = KeyFile(
        seed_hex=kp.seed.hex(),
        pubkey_hex=kp.pubkey.hex(),
        address_hex=kp.address.hex(),
    )
    _write_key_atomic(path, kf)
    LOG.info(
        "keypair_create event=generated path=%s address=%s", path, kf.address_hex
    )
    return kp, kf, True


def _load_key(path: Path) -> tuple[Keypair, KeyFile]:
    raw = path.read_text(encoding="utf-8")
    data = json.loads(raw)
    seed_hex = str(data["seed_hex"])
    if len(seed_hex) != 64:
        raise ValueError(f"seed_hex must be 64 hex chars, got {len(seed_hex)}")
    kp = Keypair.from_seed(bytes.fromhex(seed_hex))
    kf = KeyFile(
        seed_hex=seed_hex,
        pubkey_hex=str(data.get("pubkey_hex", kp.pubkey.hex())),
        address_hex=str(data.get("address_hex", kp.address.hex())),
        entity_id_hex=data.get("entity_id_hex"),
        capabilities_byte=data.get("capabilities_byte"),
        registered_at_unix=data.get("registered_at_unix"),
    )
    if kf.address_hex != kp.address.hex():
        raise ValueError(
            "address_hex in keyfile does not match seed-derived address; refusing to load"
        )
    return kp, kf


def _write_key_atomic(path: Path, kf: KeyFile) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(kf.to_dict(), indent=2, sort_keys=True), encoding="utf-8")
    os.chmod(tmp, KEYFILE_MODE)
    os.replace(tmp, path)
    if hasattr(os, "chmod"):
        os.chmod(path, KEYFILE_MODE)


def ensure_funded(
    chain: Chain,
    address: bytes,
    *,
    sleep_fn=time.sleep,
    now_fn=time.monotonic,
) -> int:
    """Return final balance. Skip the faucet call if already funded."""
    balance = chain.get_balance(address)
    if balance >= MIN_BALANCE_FOR_REGISTER:
        LOG.info(
            "faucet event=skipped reason=already_funded balance=%d threshold=%d",
            balance,
            MIN_BALANCE_FOR_REGISTER,
        )
        return balance

    try:
        result = chain.faucet(address)
        LOG.info("faucet event=requested txid=%s amount=%s", result.txid, result.amount)
    except RateLimitedError as exc:
        LOG.warning("faucet event=cooldown_active error=%s balance=%d", exc, balance)
        if balance < MIN_BALANCE_TO_OPERATE:
            LOG.error(
                "faucet event=fatal reason=cooldown_and_insufficient_balance balance=%d min=%d",
                balance,
                MIN_BALANCE_TO_OPERATE,
            )
            sys.exit(3)
        return balance

    deadline = now_fn() + FAUCET_POLL_TIMEOUT_SECS
    while now_fn() < deadline:
        sleep_fn(FAUCET_POLL_INTERVAL_SECS)
        balance = chain.get_balance(address)
        if balance >= MIN_BALANCE_FOR_REGISTER:
            LOG.info("faucet event=funded balance=%d", balance)
            return balance
    LOG.error(
        "faucet event=poll_timeout balance=%d threshold=%d", balance, MIN_BALANCE_FOR_REGISTER
    )
    if balance < MIN_BALANCE_TO_OPERATE:
        sys.exit(3)
    return balance


def ensure_registered(
    chain: Chain,
    kp: Keypair,
    *,
    sleep_fn=time.sleep,
    now_fn=time.monotonic,
) -> tuple[bytes, int]:
    """Return (entity_id, capabilities_byte). Skip if already registered with bit 6."""
    entity_id = chain.entity_id_for(kp.address)
    status = chain.get_entity_status(entity_id)
    if status.exists and status.has_post_oracle_anchors:
        LOG.info(
            "register event=skipped reason=already_registered entity_id=%s caps=0x%02x",
            status.entity_id_hex,
            status.capabilities,
        )
        return entity_id, status.capabilities
    if status.exists:
        LOG.error(
            "register event=conflict reason=entity_exists_without_bit_6 entity_id=%s caps=0x%02x",
            status.entity_id_hex,
            status.capabilities,
        )
        sys.exit(4)

    result = chain.register_oracle(kp)
    LOG.info(
        "register event=submitted txid=%s entity_id=%s",
        result.txid,
        result.entity_id,
    )

    deadline = now_fn() + REGISTER_POLL_TIMEOUT_SECS
    while now_fn() < deadline:
        sleep_fn(REGISTER_POLL_INTERVAL_SECS)
        status = chain.get_entity_status(entity_id)
        if status.exists:
            if not status.has_post_oracle_anchors:
                LOG.error(
                    "register event=verify_failed reason=bit_6_not_set caps=0x%02x",
                    status.capabilities,
                )
                sys.exit(4)
            LOG.info(
                "register event=verified entity_id=%s caps=0x%02x",
                status.entity_id_hex,
                status.capabilities,
            )
            return entity_id, status.capabilities
    LOG.error("register event=verify_timeout entity_id=%s", status.entity_id_hex)
    sys.exit(5)


def update_keyfile(
    path: Path,
    kf: KeyFile,
    *,
    entity_id: bytes,
    capabilities_byte: int,
    registered_at_unix: int,
) -> None:
    kf.entity_id_hex = entity_id.hex()
    kf.capabilities_byte = int(capabilities_byte)
    kf.registered_at_unix = int(registered_at_unix)
    _write_key_atomic(path, kf)
    LOG.info(
        "keyfile event=updated entity_id=%s caps=0x%02x",
        kf.entity_id_hex,
        kf.capabilities_byte,
    )


def print_summary(kf: KeyFile, balance: int) -> None:
    bit_6_set = bool((kf.capabilities_byte or 0) & POST_ORACLE_ANCHORS_BIT)
    print()
    print("price-oracle bootstrap complete")
    print(f"  address:          {kf.address_hex}")
    print(f"  pubkey:           {kf.pubkey_hex}")
    print(f"  balance:          {balance}")
    print(f"  entity_id:        {kf.entity_id_hex}")
    print(f"  capabilities:     0x{(kf.capabilities_byte or 0):02x}")
    print(f"  post_oracle_anchors (bit 6): {bit_6_set}")
    print(f"  registered_at:    {kf.registered_at_unix}")
    print()


def main(argv: list[str] | None = None) -> int:
    cfg = BootstrapConfig.from_env()
    configure_logging(cfg.log_level)
    LOG.info(
        "bootstrap_start event=running endpoint=%s key_path=%s",
        cfg.endpoint,
        cfg.key_path,
    )

    chain = Chain(cfg.endpoint)
    kp, kf, _ = load_or_generate_key(cfg.key_path)
    balance = ensure_funded(chain, kp.address)
    entity_id, caps = ensure_registered(chain, kp)
    update_keyfile(
        cfg.key_path,
        kf,
        entity_id=entity_id,
        capabilities_byte=caps,
        registered_at_unix=int(time.time()),
    )
    balance = chain.get_balance(kp.address)
    print_summary(kf, balance)
    LOG.info("bootstrap_done event=ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
