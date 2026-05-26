"""Synchronous facade over :class:`AsyncNOVAIClient`.

Each method blocks on a fresh asyncio event loop, runs the matching async
method, then closes the underlying session. This mirrors the full async
surface; async iterators (``iter_*``) materialize into lists since sync
callers cannot ``async for``.

Use this client when you don't want to write ``async def``. If you are
already inside a running event loop (e.g. Jupyter or a web framework), use
:class:`AsyncNOVAIClient` directly instead.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable, Sequence
from typing import Any, TypeVar

from novai_sdk.capabilities import Capabilities
from novai_sdk.client import (
    AsyncNOVAIClient,
    BalanceResult,
    FaucetResult,
)
from novai_sdk.codec import TxV1
from novai_sdk.enums import (
    AiSignalType,
    AutonomyMode,
    MemoryObjectType,
    PaymentAttestationStatus,
    ServiceCategory,
)
from novai_sdk.keys import Keypair
from novai_sdk.signals.payments import PaymentCondition, PaymentSplit
from novai_sdk.types import (
    AiEntityInfo,
    BlockHeader,
    ChannelDisputeStatus,
    MemoryObjectInfo,
    OracleAnchorInfo,
    PaymentChannelInfo,
    PaymentRecord,
    ServiceDescriptorInfo,
    SignalInfo,
    SlaAgreementInfo,
    SubmissionResult,
    TxReceipt,
    UpgradeRecord,
    VkRegistrationInfo,
)

T = TypeVar("T")


class NOVAIClient:
    """Synchronous facade over :class:`AsyncNOVAIClient`.

    Every method spins up a private event loop, dispatches one async call,
    then tears the loop down. Use as a drop-in for callers that do not want
    to engage with asyncio.
    """

    def __init__(
        self, endpoint: str = "http://localhost:3030", *, timeout_seconds: float = 30.0
    ) -> None:
        self._endpoint = endpoint
        self._timeout_seconds = timeout_seconds

    @property
    def endpoint(self) -> str:
        """The HTTP endpoint URL."""
        return self._endpoint

    # ------------------------------------------------------------------
    # Low-level dispatch
    # ------------------------------------------------------------------

    def call(self, method: str, params: dict[str, Any] | list[Any] | None = None) -> Any:
        """Synchronous JSON-RPC dispatch. See :meth:`AsyncNOVAIClient.call`."""
        return self._run(lambda c: c.call(method, params))

    # ------------------------------------------------------------------
    # Submission / account / faucet
    # ------------------------------------------------------------------

    def submit_tx(self, tx: TxV1) -> str:
        return self._run(lambda c: c.submit_tx(tx))

    def submit_raw_tx(self, tx_bytes: bytes) -> str:
        return self._run(lambda c: c.submit_raw_tx(tx_bytes))

    def get_nonce(self, address: bytes | str) -> int:
        return self._run(lambda c: c.get_nonce(address))

    def get_balance(self, address: bytes | str) -> BalanceResult:
        return self._run(lambda c: c.get_balance(address))

    def faucet(self, address: bytes | str) -> FaucetResult:
        return self._run(lambda c: c.faucet(address))

    # ------------------------------------------------------------------
    # Block / tx queries
    # ------------------------------------------------------------------

    def get_transaction(self, txid: bytes | str) -> TxReceipt | None:
        return self._run(lambda c: c.get_transaction(txid))

    def get_block_by_height(self, height: int) -> BlockHeader | None:
        return self._run(lambda c: c.get_block_by_height(height))

    def get_block_by_hash(self, block_hash: bytes | str) -> BlockHeader | None:
        return self._run(lambda c: c.get_block_by_hash(block_hash))

    def get_latest_block(self) -> BlockHeader | None:
        return self._run(lambda c: c.get_latest_block())

    # ------------------------------------------------------------------
    # AI entity
    # ------------------------------------------------------------------

    def get_ai_entity(self, entity_id: bytes | str) -> AiEntityInfo | None:
        return self._run(lambda c: c.get_ai_entity(entity_id))

    def get_upgrade_history(
        self, entity_id: bytes | str, start_height: int, end_height: int
    ) -> list[UpgradeRecord]:
        return self._run(lambda c: c.get_upgrade_history(entity_id, start_height, end_height))

    # ------------------------------------------------------------------
    # Memory objects
    # ------------------------------------------------------------------

    def get_memory_objects(self, entity_id: bytes | str) -> list[MemoryObjectInfo]:
        return self._run(lambda c: c.get_memory_objects(entity_id))

    # ------------------------------------------------------------------
    # Signals
    # ------------------------------------------------------------------

    def get_signals_by_height(self, height: int) -> list[SignalInfo]:
        return self._run(lambda c: c.get_signals_by_height(height))

    def get_signals_by_issuer(
        self, issuer: bytes | str, start_height: int, end_height: int
    ) -> list[SignalInfo]:
        return self._run(lambda c: c.get_signals_by_issuer(issuer, start_height, end_height))

    def get_signals_by_type(
        self,
        signal_type: AiSignalType | int,
        start_height: int,
        end_height: int,
    ) -> list[SignalInfo]:
        return self._run(lambda c: c.get_signals_by_type(signal_type, start_height, end_height))

    # ------------------------------------------------------------------
    # Payments
    # ------------------------------------------------------------------

    def get_payments_by_entity(
        self,
        entity_id: bytes | str,
        role: str,
        start_height: int,
        end_height: int,
    ) -> list[PaymentRecord]:
        return self._run(
            lambda c: c.get_payments_by_entity(entity_id, role, start_height, end_height)
        )

    # ------------------------------------------------------------------
    # Service descriptors / VK / SLA / channel / oracle
    # ------------------------------------------------------------------

    def get_service_descriptors_by_category(
        self, category: ServiceCategory | int | str
    ) -> list[ServiceDescriptorInfo]:
        return self._run(lambda c: c.get_service_descriptors_by_category(category))

    def discover_services(
        self, category: ServiceCategory | int | str
    ) -> list[ServiceDescriptorInfo]:
        """Alias of :meth:`get_service_descriptors_by_category`."""
        return self.get_service_descriptors_by_category(category)

    def get_vk_registration(self, vk_id: bytes | str) -> VkRegistrationInfo | None:
        return self._run(lambda c: c.get_vk_registration(vk_id))

    def list_vk_registrations(self, entity_id: bytes | str) -> list[VkRegistrationInfo]:
        return self._run(lambda c: c.list_vk_registrations(entity_id))

    def get_sla_agreement(
        self, owner: bytes | str, object_id: bytes | str
    ) -> SlaAgreementInfo | None:
        return self._run(lambda c: c.get_sla_agreement(owner, object_id))

    def get_active_sla(
        self, buyer: bytes | str, seller: bytes | str
    ) -> SlaAgreementInfo | None:
        return self._run(lambda c: c.get_active_sla(buyer, seller))

    def list_slas_by_buyer(
        self, entity_id: bytes | str, start_height: int, end_height: int
    ) -> list[SlaAgreementInfo]:
        return self._run(lambda c: c.list_slas_by_buyer(entity_id, start_height, end_height))

    def list_slas_by_seller(
        self, entity_id: bytes | str, start_height: int, end_height: int
    ) -> list[SlaAgreementInfo]:
        return self._run(lambda c: c.list_slas_by_seller(entity_id, start_height, end_height))

    def get_payment_channel(
        self, owner: bytes | str, object_id: bytes | str
    ) -> PaymentChannelInfo | None:
        return self._run(lambda c: c.get_payment_channel(owner, object_id))

    def list_channels_by_party_a(
        self, entity_id: bytes | str, start_height: int, end_height: int
    ) -> list[PaymentChannelInfo]:
        return self._run(lambda c: c.list_channels_by_party_a(entity_id, start_height, end_height))

    def list_channels_by_party_b(
        self, entity_id: bytes | str, start_height: int, end_height: int
    ) -> list[PaymentChannelInfo]:
        return self._run(lambda c: c.list_channels_by_party_b(entity_id, start_height, end_height))

    def get_channel_dispute_status(
        self, owner: bytes | str, object_id: bytes | str
    ) -> ChannelDisputeStatus:
        return self._run(lambda c: c.get_channel_dispute_status(owner, object_id))

    def get_oracle_anchors_by_entity(
        self,
        entity_id: bytes | str,
        start_height: int,
        end_height: int,
        *,
        ts_min: int | None = None,
        ts_max: int | None = None,
    ) -> list[OracleAnchorInfo]:
        return self._run(
            lambda c: c.get_oracle_anchors_by_entity(
                entity_id, start_height, end_height, ts_min=ts_min, ts_max=ts_max
            )
        )

    def get_oracle_anchors_by_tag(
        self,
        data_tag: str,
        start_height: int,
        end_height: int,
        *,
        ts_min: int | None = None,
        ts_max: int | None = None,
    ) -> list[OracleAnchorInfo]:
        return self._run(
            lambda c: c.get_oracle_anchors_by_tag(
                data_tag, start_height, end_height, ts_min=ts_min, ts_max=ts_max
            )
        )

    def get_oracle_anchor(self, signal_hash: bytes | str) -> OracleAnchorInfo | None:
        return self._run(lambda c: c.get_oracle_anchor(signal_hash))

    # ------------------------------------------------------------------
    # Convenience write helpers (sync mirror of the async surface)
    # ------------------------------------------------------------------

    def transfer(
        self,
        keypair: Keypair,
        to: bytes | str,
        amount: int,
        *,
        fee: int = 100,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(lambda c: c.transfer(keypair, to, amount, fee=fee, nonce=nonce))

    def register_entity(
        self,
        keypair: Keypair,
        code_hash: bytes | str,
        capabilities: Capabilities,
        autonomy_mode: AutonomyMode = AutonomyMode.GATED,
        initial_balance: int = 0,
        *,
        fee: int = 5000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.register_entity(
                keypair,
                code_hash,
                capabilities,
                autonomy_mode,
                initial_balance,
                fee=fee,
                nonce=nonce,
            )
        )

    def register_entity_with_key(
        self,
        keypair: Keypair,
        code_hash: bytes | str,
        entity_pubkey: bytes | str,
        capabilities: Capabilities,
        autonomy_mode: AutonomyMode = AutonomyMode.GATED,
        initial_balance: int = 0,
        *,
        fee: int = 5000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.register_entity_with_key(
                keypair,
                code_hash,
                entity_pubkey,
                capabilities,
                autonomy_mode,
                initial_balance,
                fee=fee,
                nonce=nonce,
            )
        )

    def credit_entity(
        self,
        keypair: Keypair,
        entity_id: bytes | str,
        amount: int,
        *,
        fee: int = 100,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.credit_entity(keypair, entity_id, amount, fee=fee, nonce=nonce)
        )

    def upgrade_entity(
        self,
        keypair: Keypair,
        entity_id: bytes | str,
        new_code_hash: bytes | str,
        reason_hash: bytes | str | None = None,
        *,
        fee: int = 5000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.upgrade_entity(
                keypair, entity_id, new_code_hash, reason_hash, fee=fee, nonce=nonce
            )
        )

    def create_memory_object(
        self,
        keypair: Keypair,
        object_type: MemoryObjectType,
        data: bytes,
        *,
        fee: int = 500,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.create_memory_object(keypair, object_type, data, fee=fee, nonce=nonce)
        )

    def update_memory_object(
        self,
        keypair: Keypair,
        object_id: bytes | str,
        new_data: bytes,
        *,
        fee: int = 500,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.update_memory_object(keypair, object_id, new_data, fee=fee, nonce=nonce)
        )

    def delete_memory_object(
        self,
        keypair: Keypair,
        object_id: bytes | str,
        *,
        fee: int = 500,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.delete_memory_object(keypair, object_id, fee=fee, nonce=nonce)
        )

    def publish_signal(
        self,
        keypair: Keypair,
        signal_hash: bytes | str,
        signal_type: AiSignalType,
        issuer_entity_id: bytes | str,
        extras: bytes = b"",
        *,
        fee: int = 1000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.publish_signal(
                keypair,
                signal_hash,
                signal_type,
                issuer_entity_id,
                extras,
                fee=fee,
                nonce=nonce,
            )
        )

    def pay(
        self,
        keypair: Keypair,
        issuer_entity_id: bytes | str,
        payee: bytes | str,
        amount: int,
        signal_hash: bytes | str,
        service_descriptor_hash: bytes | str,
        request_hash: bytes | str,
        max_block_height: int,
        *,
        splits: Sequence[PaymentSplit] | None = None,
        condition: PaymentCondition | None = None,
        fee: int = 1000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.pay(
                keypair,
                issuer_entity_id,
                payee,
                amount,
                signal_hash,
                service_descriptor_hash,
                request_hash,
                max_block_height,
                splits=splits,
                condition=condition,
                fee=fee,
                nonce=nonce,
            )
        )

    def attest_payment(
        self,
        keypair: Keypair,
        issuer_entity_id: bytes | str,
        payment_signal_hash: bytes | str,
        payee: bytes | str,
        status: PaymentAttestationStatus,
        signal_hash: bytes | str,
        *,
        fee: int = 1000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.attest_payment(
                keypair,
                issuer_entity_id,
                payment_signal_hash,
                payee,
                status,
                signal_hash,
                fee=fee,
                nonce=nonce,
            )
        )

    def post_oracle_anchor(
        self,
        keypair: Keypair,
        issuer_entity_id: bytes | str,
        data_hash: bytes | str,
        external_timestamp: int,
        data_tag: bytes | str,
        *,
        source_hash: bytes | str | None = None,
        expiry_height: int = 0,
        fee: int = 1000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.post_oracle_anchor(
                keypair,
                issuer_entity_id,
                data_hash,
                external_timestamp,
                data_tag,
                source_hash=source_hash,
                expiry_height=expiry_height,
                fee=fee,
                nonce=nonce,
            )
        )

    def accept_sla(
        self,
        keypair: Keypair,
        seller_entity_id: bytes | str,
        sla_object_id: bytes | str,
        buyer_entity_id: bytes | str,
        *,
        fee: int = 1000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.accept_sla(
                keypair,
                seller_entity_id,
                sla_object_id,
                buyer_entity_id,
                fee=fee,
                nonce=nonce,
            )
        )

    def accept_channel(
        self,
        keypair: Keypair,
        party_b_entity_id: bytes | str,
        channel_object_id: bytes | str,
        party_a_entity_id: bytes | str,
        signal_hash: bytes | str,
        *,
        fee: int = 1000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.accept_channel(
                keypair,
                party_b_entity_id,
                channel_object_id,
                party_a_entity_id,
                signal_hash,
                fee=fee,
                nonce=nonce,
            )
        )

    def close_channel(
        self,
        keypair: Keypair,
        issuer_entity_id: bytes | str,
        channel_object_id: bytes | str,
        party_a_entity_id: bytes | str,
        channel_nonce: int,
        balance_a: int,
        balance_b: int,
        is_final: bool,
        sig_a: bytes | str,
        sig_b: bytes | str,
        signal_hash: bytes | str,
        *,
        fee: int = 1000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.close_channel(
                keypair,
                issuer_entity_id,
                channel_object_id,
                party_a_entity_id,
                channel_nonce,
                balance_a,
                balance_b,
                is_final,
                sig_a,
                sig_b,
                signal_hash,
                fee=fee,
                nonce=nonce,
            )
        )

    def finalize_channel(
        self,
        keypair: Keypair,
        issuer_entity_id: bytes | str,
        channel_object_id: bytes | str,
        party_a_entity_id: bytes | str,
        signal_hash: bytes | str,
        *,
        fee: int = 1000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        return self._run(
            lambda c: c.finalize_channel(
                keypair,
                issuer_entity_id,
                channel_object_id,
                party_a_entity_id,
                signal_hash,
                fee=fee,
                nonce=nonce,
            )
        )

    # ------------------------------------------------------------------
    # Internal runner
    # ------------------------------------------------------------------

    def _run(self, fn: Callable[[AsyncNOVAIClient], Awaitable[T]]) -> T:
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            running = False
        else:
            running = True
        if running:
            raise RuntimeError(
                "NOVAIClient is the sync wrapper; you are inside a running event loop. "
                "Use AsyncNOVAIClient(...) directly instead."
            )

        async def _runner() -> T:
            async with AsyncNOVAIClient(
                self._endpoint, timeout_seconds=self._timeout_seconds
            ) as client:
                return await fn(client)

        return asyncio.run(_runner())
