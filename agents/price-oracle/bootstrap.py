#!/usr/bin/env python3
"""PURPOSE: One-shot idempotent setup for the NOVAI price-oracle.

Two-key Type-10 funding model: a funder ed25519 account that signs all
account-level operations (Type-10 registration, runtime CreditAiEntity
top-ups) and an entity ed25519 key that signs all entity-level signals
(OracleAnchor, memory CRUD). The funder address is bound nowhere in the
chain-side reverse index, so it can submit CreditAiEntity at any time
without tripping the check_ai_entity_sender deny arm at
crates/execution/src/lib.rs:9741. See
docs/gate-oracle-funding-model-diagnosis.md for the design and
docs/AGENT_FUNDING_PLAYBOOK.md for the reusable lifecycle that future
agents copy.

Steps (every step skips itself if already done):

  1. Load or generate the two ed25519 keypairs; write
     /etc/novai/oracle-keys.json at 0600 with both seeds plus derived
     public info. Refuse to load a v1 (single-key) keyfile; the
     operator must archive it and re-bootstrap. The v1 key is bound to
     the dead Type-8 entity whose creator address is reverse-index-
     locked, so silent migration would inherit a poisoned funder.
  2. Fund the FUNDER address via the public faucet if balance is below
     the register threshold. Faucet is per-IP-24h-cooldown; bootstrap
     exits non-zero if the cooldown blocks a needed top-up.
  3. RegisterEntityWithKey (Type-10) with Capabilities.oracle() (bits
     0,1,2,6 = 0x47) if no entity with (ORACLE_CODE_HASH, funder_addr)
     is on-chain yet, then verify capability bit 6
     (post_oracle_anchors) is set in the on-chain view. The entity
     pubkey is bound at the chain-side address derived from the entity
     pubkey, not from the funder. Capabilities are frozen post-register,
     so a mismatch is fatal and requires manual cleanup.
  4. Rewrite oracle-keys.json with entity_id, capabilities,
     registered_at.

INVARIANTS:
- Re-running on the same host with the same key file is a no-op once
  the oracle is registered.
- The key file is the only persistent secret; everything else is
  derived.
- The funder address must be free of any prior creator binding under
  ORACLE_CODE_HASH. A reused funder collides at EntityAlreadyExists.

FAILURE MODES:
- Missing config (PRICE_ORACLE_RPC_ENDPOINT) -> exit 2.
- Faucet cooldown AND insufficient balance for register -> exit 3.
- Entity exists without bit 6 -> exit 4 (operator intervention).
- RPC unreachable / timeout -> exit 5.
- v1 keyfile on disk in v2 mode -> KeyFileVersionError at load time.
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
KEYFILE_VERSION = 2
KEYFILE_MODE = 0o600

# Sized so a fresh funder, after one faucet drop, can cover both the
# registration fee and the seed balance for the new entity, with slack
# left over for the first runtime CreditAiEntity top-up cycle.
INITIAL_ENTITY_BALANCE = 50_000
REGISTER_FEE = 5_000
MIN_BALANCE_FOR_REGISTER = INITIAL_ENTITY_BALANCE + REGISTER_FEE + 5_000  # 60_000
MIN_BALANCE_TO_OPERATE = 5_000
FAUCET_POLL_TIMEOUT_SECS = 30.0
FAUCET_POLL_INTERVAL_SECS = 2.0
REGISTER_POLL_TIMEOUT_SECS = 30.0
REGISTER_POLL_INTERVAL_SECS = 2.0


class KeyFileVersionError(ValueError):
    """Raised when a keyfile on disk has a version this bootstrap does
    not support.

    The v1 (single-key Type-8) to v2 (two-key Type-10) transition is
    intentionally non-migratable. The v1 key is bound to the dead
    Type-8 entity whose creator address is reverse-index-locked, so
    auto-migrating would silently inherit a poisoned funder. The
    operator must archive the v1 file and let bootstrap generate a
    fresh v2 file.
    """


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
    funder_seed_hex: str
    funder_pubkey_hex: str
    funder_address_hex: str
    entity_seed_hex: str
    entity_pubkey_hex: str
    entity_address_hex: str
    entity_id_hex: str | None = None
    capabilities_byte: int | None = None
    registered_at_unix: int | None = None

    def to_dict(self) -> dict[str, object]:
        d: dict[str, object] = {
            "version": KEYFILE_VERSION,
            "funder_seed_hex": self.funder_seed_hex,
            "funder_pubkey_hex": self.funder_pubkey_hex,
            "funder_address_hex": self.funder_address_hex,
            "entity_seed_hex": self.entity_seed_hex,
            "entity_pubkey_hex": self.entity_pubkey_hex,
            "entity_address_hex": self.entity_address_hex,
        }
        if self.entity_id_hex is not None:
            d["entity_id_hex"] = self.entity_id_hex
        if self.capabilities_byte is not None:
            d["capabilities_byte"] = self.capabilities_byte
        if self.registered_at_unix is not None:
            d["registered_at_unix"] = self.registered_at_unix
        return d


def load_or_generate_key(path: Path) -> tuple[Keypair, Keypair, KeyFile, bool]:
    """Return (funder_kp, entity_kp, keyfile, generated).

    ``generated`` is True if both keypairs were freshly generated and
    persisted (first-run path), False if they were loaded from an
    existing v2 keyfile.
    """
    if path.exists():
        funder_kp, entity_kp, kf = _load_key(path)
        LOG.info(
            "keypair_load event=reused path=%s funder=%s entity=%s",
            path,
            kf.funder_address_hex,
            kf.entity_address_hex,
        )
        return funder_kp, entity_kp, kf, False
    funder_kp = Keypair.generate()
    entity_kp = Keypair.generate()
    kf = KeyFile(
        funder_seed_hex=funder_kp.seed.hex(),
        funder_pubkey_hex=funder_kp.pubkey.hex(),
        funder_address_hex=funder_kp.address.hex(),
        entity_seed_hex=entity_kp.seed.hex(),
        entity_pubkey_hex=entity_kp.pubkey.hex(),
        entity_address_hex=entity_kp.address.hex(),
    )
    _write_key_atomic(path, kf)
    LOG.info(
        "keypair_create event=generated path=%s funder=%s entity=%s",
        path,
        kf.funder_address_hex,
        kf.entity_address_hex,
    )
    return funder_kp, entity_kp, kf, True


def _load_key(path: Path) -> tuple[Keypair, Keypair, KeyFile]:
    raw = path.read_text(encoding="utf-8")
    data = json.loads(raw)
    version = data.get("version")
    if version != KEYFILE_VERSION:
        raise KeyFileVersionError(
            f"keyfile {path} has version={version!r}, this bootstrap "
            f"requires version={KEYFILE_VERSION}; archive the existing "
            f"file and re-run to generate a fresh two-key keyfile"
        )
    try:
        funder_seed_hex = str(data["funder_seed_hex"])
        entity_seed_hex = str(data["entity_seed_hex"])
    except KeyError as exc:
        raise ValueError(
            f"keyfile {path} is missing required field {exc!s} for v2"
        ) from exc
    for label, seed_hex in (("funder", funder_seed_hex), ("entity", entity_seed_hex)):
        if len(seed_hex) != 64:
            raise ValueError(
                f"{label}_seed_hex must be 64 hex chars, got {len(seed_hex)}"
            )
    funder_kp = Keypair.from_seed(bytes.fromhex(funder_seed_hex))
    entity_kp = Keypair.from_seed(bytes.fromhex(entity_seed_hex))
    kf = KeyFile(
        funder_seed_hex=funder_seed_hex,
        funder_pubkey_hex=str(data.get("funder_pubkey_hex", funder_kp.pubkey.hex())),
        funder_address_hex=str(data.get("funder_address_hex", funder_kp.address.hex())),
        entity_seed_hex=entity_seed_hex,
        entity_pubkey_hex=str(data.get("entity_pubkey_hex", entity_kp.pubkey.hex())),
        entity_address_hex=str(data.get("entity_address_hex", entity_kp.address.hex())),
        entity_id_hex=data.get("entity_id_hex"),
        capabilities_byte=data.get("capabilities_byte"),
        registered_at_unix=data.get("registered_at_unix"),
    )
    if kf.funder_address_hex != funder_kp.address.hex():
        raise ValueError(
            "funder_address_hex in keyfile does not match seed-derived "
            "address; refusing to load"
        )
    if kf.entity_address_hex != entity_kp.address.hex():
        raise ValueError(
            "entity_address_hex in keyfile does not match seed-derived "
            "address; refusing to load"
        )
    return funder_kp, entity_kp, kf


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
    """Return final balance for ``address``. Skip the faucet call if
    already funded.

    Under v2 ``address`` is the funder address; the entity has no
    account ledger of its own.
    """
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
    funder_kp: Keypair,
    entity_kp: Keypair,
    *,
    initial_balance: int = INITIAL_ENTITY_BALANCE,
    register_fee: int = REGISTER_FEE,
    sleep_fn=time.sleep,
    now_fn=time.monotonic,
) -> tuple[bytes, int]:
    """Return (entity_id, capabilities_byte).

    Skip if an entity at compute_id(ORACLE_CODE_HASH, funder_kp.address)
    already exists with the post_oracle_anchors capability (idempotent
    re-run). Exit 4 if it exists without that capability (poisoned or
    aborted prior run). Otherwise submit Type-10 RegisterAiEntityWithKey
    signed by the funder, with the entity pubkey bound at the chain-side
    reverse-index address derived from entity_kp.pubkey.
    """
    entity_id = chain.entity_id_for(funder_kp.address)
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

    result = chain.register_oracle_with_key(
        funder_kp,
        entity_kp.pubkey,
        fee=register_fee,
        initial_balance=initial_balance,
    )
    LOG.info(
        "register event=submitted txid=%s entity_id=%s funder=%s entity_pubkey=%s",
        result.txid,
        result.entity_id,
        funder_kp.address.hex(),
        entity_kp.pubkey.hex(),
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


def print_summary(kf: KeyFile, funder_balance: int) -> None:
    bit_6_set = bool((kf.capabilities_byte or 0) & POST_ORACLE_ANCHORS_BIT)
    print()
    print("price-oracle bootstrap complete")
    print(f"  funder_address:   {kf.funder_address_hex}")
    print(f"  funder_pubkey:    {kf.funder_pubkey_hex}")
    print(f"  funder_balance:   {funder_balance}")
    print(f"  entity_address:   {kf.entity_address_hex}")
    print(f"  entity_pubkey:    {kf.entity_pubkey_hex}")
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
    funder_kp, entity_kp, kf, _ = load_or_generate_key(cfg.key_path)
    funder_balance = ensure_funded(chain, funder_kp.address)
    entity_id, caps = ensure_registered(chain, funder_kp, entity_kp)
    update_keyfile(
        cfg.key_path,
        kf,
        entity_id=entity_id,
        capabilities_byte=caps,
        registered_at_unix=int(time.time()),
    )
    funder_balance = chain.get_balance(funder_kp.address)
    print_summary(kf, funder_balance)
    LOG.info("bootstrap_done event=ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
