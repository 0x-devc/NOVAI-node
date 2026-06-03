"""PURPOSE: Thin wrapper over novai_sdk.NOVAIClient with idempotency helpers.

INVARIANTS:
- All chain interaction goes through this module. bootstrap.py and
  oracle.py never import novai_sdk directly except for Keypair (a pure
  local crypto helper, no I/O).
- ORACLE_CODE_HASH is locked at v1; bumping it is a deliberate semantic
  change that produces a NEW entity_id and forces a new RegisterEntity.

FAILURE MODES:
- See map_submit_error for the SDK-exception -> reason mapping. Unknown
  errors fall back to "rpc_error"; network-layer errors fall back to
  "rpc_unreachable".
- get_balance / get_entity_status pass through SDK exceptions; bootstrap
  catches them at top level and exits non-zero.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from typing import Optional

from novai_sdk import (
    AutonomyMode,
    Capabilities,
    FeeTooLowError,
    Keypair,
    MempoolFullError,
    NOVAIClient,
    NonceTooLowError,
    NovaiRpcError,
    RateLimitedError,
    SenderLimitExceededError,
    SubmissionResult,
    ValidationError,
    compute_entity_id,
)
from novai_sdk.crypto import blake3_hash

LOG = logging.getLogger("price_oracle.chain")

POST_ORACLE_ANCHORS_BIT = 0x40

ORACLE_CODE_HASH: bytes = blake3_hash(b"novai-price-oracle-v1")

_SUBMIT_ERROR_MAP: tuple[tuple[type[BaseException], str], ...] = (
    (FeeTooLowError, "fee_too_low"),
    (NonceTooLowError, "nonce_too_low"),
    (MempoolFullError, "mempool_full"),
    (SenderLimitExceededError, "sender_limit"),
    (ValidationError, "validation_failed"),
    (RateLimitedError, "rpc_rate_limited"),
)


def map_submit_error(exc: BaseException) -> str:
    """Map an SDK exception to a Prometheus reason label."""
    for exc_type, reason in _SUBMIT_ERROR_MAP:
        if isinstance(exc, exc_type):
            return reason
    if isinstance(exc, NovaiRpcError):
        return "rpc_error"
    return "rpc_unreachable"


@dataclass(frozen=True)
class EntityStatus:
    exists: bool
    has_post_oracle_anchors: bool
    capabilities: int
    entity_id: bytes
    entity_id_hex: str


class Chain:
    """Wrapper around NOVAIClient. All chain calls funnel through this class."""

    def __init__(self, endpoint: str, *, timeout_seconds: float = 30.0) -> None:
        self._client = NOVAIClient(endpoint, timeout_seconds=timeout_seconds)

    @property
    def endpoint(self) -> str:
        return self._client.endpoint

    def entity_id_for(self, address: bytes) -> bytes:
        return compute_entity_id(ORACLE_CODE_HASH, address)

    def get_balance(self, address: bytes) -> int:
        result = self._client.get_balance(address)
        return int(result.balance)

    def get_entity_status(self, entity_id: bytes) -> EntityStatus:
        info = self._client.get_ai_entity(entity_id)
        entity_id_hex = entity_id.hex()
        if info is None:
            return EntityStatus(False, False, 0, entity_id, entity_id_hex)
        caps = int(info.capabilities)
        return EntityStatus(
            exists=True,
            has_post_oracle_anchors=bool(caps & POST_ORACLE_ANCHORS_BIT),
            capabilities=caps,
            entity_id=entity_id,
            entity_id_hex=entity_id_hex,
        )

    def faucet(self, address: bytes):
        return self._client.faucet(address)

    def register_oracle(
        self, kp: Keypair, *, fee: int = 5_000, initial_balance: int = 0
    ) -> SubmissionResult:
        return self._client.register_entity(
            keypair=kp,
            code_hash=ORACLE_CODE_HASH,
            capabilities=Capabilities.oracle(),
            autonomy_mode=AutonomyMode.GATED,
            initial_balance=initial_balance,
            fee=fee,
        )

    def post_anchor(
        self,
        kp: Keypair,
        entity_id: bytes,
        data_hash: bytes,
        external_timestamp: int,
        data_tag: str,
        *,
        fee: int = 1_000,
    ) -> SubmissionResult:
        return self._client.post_oracle_anchor(
            keypair=kp,
            issuer_entity_id=entity_id,
            data_hash=data_hash,
            external_timestamp=external_timestamp,
            data_tag=data_tag,
            fee=fee,
        )

    def latest_block_height(self) -> Optional[int]:
        block = self._client.get_latest_block()
        if block is None:
            return None
        return int(block.height)
