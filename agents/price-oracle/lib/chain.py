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
    """Map an SDK exception to a Prometheus reason label.

    The "insufficient_funds" branch fires when the chain returns an
    InsufficientFunds execution rejection via the submit RPC. Today the
    fee/balance check at crates/execution/src/lib.rs:7319 runs in
    on_commit (post-consensus) so the submitter does not see it; the
    mapping is in place so that any future mempool-admission balance
    check, or a tx-receipt poll, surfaces drain honestly in the
    novai_oracle_submission_failure_total{reason="insufficient_funds"}
    counter instead of the rpc_unreachable catch-all.
    """
    for exc_type, reason in _SUBMIT_ERROR_MAP:
        if isinstance(exc, exc_type):
            return reason
    if isinstance(exc, NovaiRpcError):
        if "InsufficientFunds" in (exc.message or ""):
            return "insufficient_funds"
        return "rpc_error"
    return "rpc_unreachable"


def map_faucet_error(exc: BaseException) -> str:
    """Map a faucet-call exception to a Prometheus reason label.

    The dev-faucet RPC at crates/node/src/rpc.rs:3129 raises:
    - RateLimitedError: per-address cooldown still active (the SDK
      promotes -32000 with "rate" in the message to this class).
    - ServerError -32000 with "Faucet disabled" message: the node was
      not started with --dev-keys or --faucet-key.
    - ServerError -32000 with "global cooldown": the 10s global window
      is still active.
    - MempoolFullError: dispense tx could not enter the mempool.
    - NovaiError or transport error: RPC unreachable.
    """
    if isinstance(exc, RateLimitedError):
        return "rate_limited"
    if isinstance(exc, MempoolFullError):
        return "mempool_full"
    if isinstance(exc, NovaiRpcError):
        msg = (exc.message or "").lower()
        if "faucet disabled" in msg:
            return "disabled"
        if "global cooldown" in msg:
            return "rate_limited"
        return "rpc_error"
    return "rpc_unreachable"


def map_credit_error(exc: BaseException) -> str:
    """Map a CreditAiEntity submission exception to a Prometheus reason label.

    The handler at crates/execution/src/lib.rs:9206 can reject with:
    - NonceMismatch (account-signed exact-equality check at :9226)
    - InsufficientFunds (sender account does not cover amount + fee)
    The mempool can also reject at admission with NonceTooLow when the
    mempool-cache expected_nonce(addr) has drifted ahead of the on-chain
    account.nonce; see docs/gate-oracle-balance-diagnosis-v2.md for the
    asymmetry between account-signed (equality) and entity-signed (range)
    nonce checks.
    """
    if isinstance(exc, NonceTooLowError):
        return "nonce_mismatch"
    if isinstance(exc, NovaiRpcError):
        msg = exc.message or ""
        if "NonceMismatch" in msg:
            return "nonce_mismatch"
        if "InsufficientFunds" in msg:
            return "insufficient_funds"
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

    def get_account_nonce(self, address: bytes) -> int:
        """Return the on-chain account.nonce via novai_getBalance.

        This is the correct nonce source for account-signed txs (Transfer,
        RegisterEntity, CreditAiEntity, RegisterEntityWithKey, EntityUpgrade),
        whose handlers do an exact-equality nonce check at the chain layer.
        Callers must NOT use novai_getNonce here, because that RPC returns
        the mempool's expected_nonce, which advances on every committed tx
        (success or fail) and drifts away from account.nonce when entity-
        signed txs are interleaved. See docs/gate-oracle-balance-diagnosis-v2.md.
        """
        result = self._client.get_balance(address)
        return int(result.nonce)

    def get_entity_economic_balance(self, entity_id: bytes) -> int:
        """Return entity.economic_balance via novai_getAiEntity.

        This is the ledger debited by signal-commitment fees at
        crates/execution/src/lib.rs:7319/7327. It is distinct from the
        account ledger that novai_getBalance reads.
        """
        info = self._client.get_ai_entity(entity_id)
        if info is None:
            raise ValueError(f"entity not registered: {entity_id.hex()}")
        return int(info.economic_balance)

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

    def credit_entity(
        self,
        kp: Keypair,
        entity_id: bytes,
        amount: int,
        *,
        nonce: int,
        fee: int = 100,
    ) -> SubmissionResult:
        """Submit a CreditAiEntity (tx type 9) from the kp's account to entity.

        The nonce is REQUIRED (no default) because the SDK's default nonce
        path calls novai_getNonce, which returns the mempool's expected
        value. For account-signed txs the chain checks exact equality
        against account.nonce; the mempool value will silently be wrong
        once the address has any committed entity-signed txs. Callers must
        pass the value from get_account_nonce(address).
        """
        return self._client.credit_entity(
            keypair=kp,
            entity_id=entity_id,
            amount=amount,
            fee=fee,
            nonce=nonce,
        )

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
