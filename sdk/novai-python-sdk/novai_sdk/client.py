"""Async JSON-RPC client.

Wraps every documented NOVAI RPC method (Phase 3 surface) plus a layer of
convenience methods that combine build + sign + submit for common write
flows (transfer, register entity, pay, post oracle anchor, open channel,
etc.). Underlying transport is ``aiohttp``; sync access goes through
:class:`novai_sdk.sync_client.NOVAIClient`.
"""

from __future__ import annotations

import itertools
from collections.abc import AsyncIterator, Sequence
from dataclasses import dataclass
from types import TracebackType
from typing import Any

import aiohttp

from novai_sdk._hex import bytes_to_hex, coerce_address, coerce_hash32
from novai_sdk.capabilities import Capabilities
from novai_sdk.codec import TxV1, encode_tx_v1_signed
from novai_sdk.crypto import compute_entity_id, sign_tx_v1
from novai_sdk.enums import (
    AiSignalType,
    AutonomyMode,
    MemoryObjectType,
    PaymentAttestationStatus,
    ServiceCategory,
)
from novai_sdk.errors import DecodeError, NovaiError, rpc_error_from
from novai_sdk.keys import Keypair
from novai_sdk.paginate import iter_height_chunks
from novai_sdk.signals.channels import (
    build_channel_accept_extras,
    build_channel_close_extras,
    build_channel_finalize_extras,
)
from novai_sdk.signals.oracle import (
    build_oracle_anchor_extras,
    derive_oracle_anchor_signal_hash,
)
from novai_sdk.signals.payments import (
    PaymentCondition,
    PaymentSplit,
    build_payment_request_extras,
    build_service_attestation_extras,
)
from novai_sdk.signals.sla import build_sla_accept_extras, derive_sla_accept_signal_hash
from novai_sdk.tx.entities import (
    build_credit_entity_payload,
    build_entity_upgrade_payload,
    build_register_entity_payload,
    build_register_with_key_payload,
)
from novai_sdk.tx.memory import (
    build_create_memory_payload,
    build_delete_memory_payload,
    build_update_memory_payload,
)
from novai_sdk.tx.signal import build_signal_commitment_payload
from novai_sdk.tx.transfer import build_transfer_payload
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


@dataclass(frozen=True)
class FaucetResult:
    """Response shape returned by ``novai_faucet``."""

    txid: str
    amount: str


@dataclass(frozen=True)
class BalanceResult:
    """Response shape returned by ``novai_getBalance``."""

    balance: str
    nonce: int


class AsyncNOVAIClient:
    """Async client for the NOVAI JSON-RPC endpoint.

    The client owns an ``aiohttp.ClientSession`` lazily. Prefer the async
    context-manager form for deterministic resource cleanup::

        async with AsyncNOVAIClient("http://localhost:3030") as client:
            nonce = await client.get_nonce(address)
    """

    def __init__(
        self,
        endpoint: str = "http://localhost:3030",
        *,
        timeout_seconds: float = 30.0,
        session: aiohttp.ClientSession | None = None,
    ) -> None:
        self._endpoint = endpoint.rstrip("/")
        self._timeout = aiohttp.ClientTimeout(total=timeout_seconds)
        self._session = session
        self._owned_session = session is None
        self._id_counter = itertools.count(1)

    @property
    def endpoint(self) -> str:
        """The HTTP endpoint URL (no trailing slash)."""
        return self._endpoint

    async def __aenter__(self) -> AsyncNOVAIClient:
        await self._ensure_session()
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        await self.close()

    async def close(self) -> None:
        """Close the underlying HTTP session if we own it."""
        if self._owned_session and self._session is not None:
            await self._session.close()
            self._session = None

    async def _ensure_session(self) -> aiohttp.ClientSession:
        if self._session is None:
            self._session = aiohttp.ClientSession(timeout=self._timeout)
            self._owned_session = True
        return self._session

    # ------------------------------------------------------------------
    # Low-level dispatch
    # ------------------------------------------------------------------

    async def call(self, method: str, params: dict[str, Any] | list[Any] | None = None) -> Any:
        """Dispatch a raw JSON-RPC 2.0 call and return the ``result`` field.

        Use this as an escape hatch for RPCs the SDK does not yet wrap or
        for advanced testing where you need full control over the params.

        Raises:
            NovaiRpcError: If the node returns a JSON-RPC error envelope.
                The concrete class depends on the ``code`` (see
                :mod:`novai_sdk.errors`).
            NovaiError: On transport failure or malformed response.
        """
        session = await self._ensure_session()
        request_id = next(self._id_counter)
        body = {
            "jsonrpc": "2.0",
            "method": method,
            "params": params if params is not None else {},
            "id": request_id,
        }
        try:
            async with session.post(self._endpoint, json=body) as resp:
                text = await resp.text()
                if resp.status != 200:
                    raise NovaiError(f"HTTP {resp.status} from {self._endpoint}: {text}")
                try:
                    envelope = await resp.json(content_type=None)
                except aiohttp.ContentTypeError as exc:
                    raise NovaiError(
                        f"non-JSON response from {self._endpoint}: {text[:200]}"
                    ) from exc
        except aiohttp.ClientError as exc:
            raise NovaiError(f"transport error contacting {self._endpoint}: {exc}") from exc

        if not isinstance(envelope, dict):
            raise NovaiError(f"unexpected RPC envelope shape: {envelope!r}")
        if "error" in envelope and envelope["error"] is not None:
            err = envelope["error"]
            code = int(err.get("code", -32000))
            message = str(err.get("message", "unknown error"))
            raise rpc_error_from(code, message, err.get("data"))
        if "result" not in envelope:
            raise NovaiError(f"RPC envelope missing 'result' field: {envelope!r}")
        return envelope["result"]

    # ------------------------------------------------------------------
    # Transactions / account
    # ------------------------------------------------------------------

    async def submit_tx(self, tx: TxV1) -> str:
        """Submit a signed TxV1 to the mempool and return its txid (hex)."""
        if tx.sig == bytes(64):
            raise NovaiError("refusing to submit an unsigned tx (sig is all zeros)")
        tx_hex = bytes_to_hex(encode_tx_v1_signed(tx))
        result = await self.call("novai_submitTransaction", {"tx": tx_hex})
        return _expect_str_field(result, "txid")

    async def submit_raw_tx(self, tx_bytes: bytes) -> str:
        """Submit a pre-encoded signed tx (escape hatch for offline builders)."""
        result = await self.call("novai_submitTransaction", {"tx": bytes_to_hex(tx_bytes)})
        return _expect_str_field(result, "txid")

    async def get_nonce(self, address: bytes | str) -> int:
        """Return the next expected nonce for ``address``."""
        addr_hex = bytes_to_hex(coerce_address(address))
        result = await self.call("novai_getNonce", {"address": addr_hex})
        return _expect_int_field(result, "nonce")

    async def get_balance(self, address: bytes | str) -> BalanceResult:
        """Return the current balance (decimal string) and nonce for ``address``."""
        addr_hex = bytes_to_hex(coerce_address(address))
        result = await self.call("novai_getBalance", {"address": addr_hex})
        if not isinstance(result, dict):
            raise DecodeError(f"novai_getBalance: expected object, got {result!r}")
        try:
            return BalanceResult(
                balance=str(result["balance"]),
                nonce=int(result["nonce"]),
            )
        except (KeyError, TypeError, ValueError) as exc:
            raise DecodeError(f"novai_getBalance: bad response shape {result!r}") from exc

    async def faucet(self, address: bytes | str) -> FaucetResult:
        """Request a faucet dispense (dev / testnet only)."""
        addr_hex = bytes_to_hex(coerce_address(address))
        result = await self.call("novai_faucet", {"address": addr_hex})
        if not isinstance(result, dict):
            raise DecodeError(f"novai_faucet: expected object, got {result!r}")
        try:
            return FaucetResult(txid=str(result["txid"]), amount=str(result["amount"]))
        except (KeyError, TypeError) as exc:
            raise DecodeError(f"novai_faucet: bad response shape {result!r}") from exc

    # ------------------------------------------------------------------
    # Block / tx queries
    # ------------------------------------------------------------------

    async def get_transaction(self, txid: bytes | str) -> TxReceipt | None:
        """Fetch a tx receipt (block height + index) by transaction id."""
        result = await self.call(
            "novai_getTransaction", {"txid": bytes_to_hex(coerce_hash32(txid, field="txid"))}
        )
        if result is None:
            return None
        return TxReceipt.from_json(result)

    async def get_block_by_height(self, height: int) -> BlockHeader | None:
        """Fetch a committed block header by height."""
        result = await self.call("novai_getBlockByHeight", {"height": int(height)})
        if result is None:
            return None
        return BlockHeader.from_json(result)

    async def get_block_by_hash(self, block_hash: bytes | str) -> BlockHeader | None:
        """Fetch a committed block header by hash."""
        result = await self.call(
            "novai_getBlockByHash",
            {"hash": bytes_to_hex(coerce_hash32(block_hash, field="block_hash"))},
        )
        if result is None:
            return None
        return BlockHeader.from_json(result)

    async def get_latest_block(self) -> BlockHeader | None:
        """Fetch the most recently committed block header."""
        result = await self.call("novai_getLatestBlock", {})
        if result is None:
            return None
        return BlockHeader.from_json(result)

    # ------------------------------------------------------------------
    # AI entity
    # ------------------------------------------------------------------

    async def get_ai_entity(self, entity_id: bytes | str) -> AiEntityInfo | None:
        """Fetch full AI entity state and reputation."""
        result = await self.call(
            "novai_getAiEntity",
            {"entity_id": bytes_to_hex(coerce_hash32(entity_id, field="entity_id"))},
        )
        entity = result.get("entity") if isinstance(result, dict) else None
        if entity is None:
            return None
        return AiEntityInfo.from_json(entity)

    async def get_upgrade_history(
        self, entity_id: bytes | str, start_height: int, end_height: int
    ) -> list[UpgradeRecord]:
        """Fetch entity upgrade history within ``[start_height, end_height]``."""
        eid_hex = bytes_to_hex(coerce_hash32(entity_id, field="entity_id"))
        result = await self.call(
            "novai_getUpgradeHistory",
            {"entity_id": eid_hex, "start_height": start_height, "end_height": end_height},
        )
        return _decode_list(result, "upgrades", UpgradeRecord.from_json)

    # ------------------------------------------------------------------
    # Memory objects
    # ------------------------------------------------------------------

    async def get_memory_objects(self, entity_id: bytes | str) -> list[MemoryObjectInfo]:
        """Fetch all memory objects owned by ``entity_id``."""
        eid_hex = bytes_to_hex(coerce_hash32(entity_id, field="entity_id"))
        result = await self.call("novai_getMemoryObjects", {"entity_id": eid_hex})
        return _decode_list(result, "objects", MemoryObjectInfo.from_json)

    # ------------------------------------------------------------------
    # Signals
    # ------------------------------------------------------------------

    async def get_signals_by_height(self, height: int) -> list[SignalInfo]:
        """Fetch all signals committed at exactly ``height``."""
        result = await self.call("novai_getSignalsByHeight", {"height": int(height)})
        return _decode_list(result, "signals", SignalInfo.from_json)

    async def get_signals_by_issuer(
        self, issuer: bytes | str, start_height: int, end_height: int
    ) -> list[SignalInfo]:
        """Fetch signals issued by ``issuer`` within an inclusive height range."""
        issuer_hex = bytes_to_hex(coerce_hash32(issuer, field="issuer"))
        result = await self.call(
            "novai_getSignalsByIssuer",
            {"issuer": issuer_hex, "start_height": start_height, "end_height": end_height},
        )
        return _decode_list(result, "signals", SignalInfo.from_json)

    async def iter_signals_by_issuer(
        self, issuer: bytes | str, start_height: int, end_height: int
    ) -> AsyncIterator[SignalInfo]:
        """Auto-paginate :meth:`get_signals_by_issuer` past the 10K-block cap."""
        async def fetch(s: int, e: int) -> list[SignalInfo]:
            return await self.get_signals_by_issuer(issuer, s, e)

        async for row in iter_height_chunks(fetch, start_height, end_height):
            yield row

    async def get_signals_by_type(
        self,
        signal_type: AiSignalType | int,
        start_height: int,
        end_height: int,
    ) -> list[SignalInfo]:
        """Fetch signals of a specific type within an inclusive height range."""
        result = await self.call(
            "novai_getSignalsByType",
            {
                "signal_type": int(signal_type),
                "start_height": start_height,
                "end_height": end_height,
            },
        )
        return _decode_list(result, "signals", SignalInfo.from_json)

    # ------------------------------------------------------------------
    # Payments
    # ------------------------------------------------------------------

    async def get_payments_by_entity(
        self,
        entity_id: bytes | str,
        role: str,
        start_height: int,
        end_height: int,
    ) -> list[PaymentRecord]:
        """Fetch payment records where ``entity_id`` is ``"payer"`` or ``"payee"``."""
        if role not in ("payer", "payee"):
            raise ValueError(f"role must be 'payer' or 'payee', got {role!r}")
        eid_hex = bytes_to_hex(coerce_hash32(entity_id, field="entity_id"))
        result = await self.call(
            "novai_getPaymentsByEntity",
            {
                "entity_id": eid_hex,
                "role": role,
                "start_height": start_height,
                "end_height": end_height,
            },
        )
        return _decode_list(result, "payments", PaymentRecord.from_json)

    async def iter_payments_by_entity(
        self,
        entity_id: bytes | str,
        role: str,
        start_height: int,
        end_height: int,
    ) -> AsyncIterator[PaymentRecord]:
        """Auto-paginate :meth:`get_payments_by_entity`."""
        async def fetch(s: int, e: int) -> list[PaymentRecord]:
            return await self.get_payments_by_entity(entity_id, role, s, e)

        async for row in iter_height_chunks(fetch, start_height, end_height):
            yield row

    # ------------------------------------------------------------------
    # Service descriptors (Week 29)
    # ------------------------------------------------------------------

    async def get_service_descriptors_by_category(
        self, category: ServiceCategory | int | str
    ) -> list[ServiceDescriptorInfo]:
        """Discover services by category.

        Accepts a :class:`ServiceCategory`, an integer discriminant, or a
        human-readable name (``"inference"``, ``"data-oracle"``, etc.) which
        is mapped against the enum's lower-case member names.
        """
        cat_byte = _coerce_category(category)
        result = await self.call(
            "novai_getServiceDescriptorsByCategory", {"category": cat_byte}
        )
        return _decode_list(result, "descriptors", ServiceDescriptorInfo.from_json)

    # Convenience alias for the headline example in the README.
    async def discover_services(
        self, category: ServiceCategory | int | str
    ) -> list[ServiceDescriptorInfo]:
        """Alias of :meth:`get_service_descriptors_by_category`."""
        return await self.get_service_descriptors_by_category(category)

    # ------------------------------------------------------------------
    # VK registrations (Week 30)
    # ------------------------------------------------------------------

    async def get_vk_registration(self, vk_id: bytes | str) -> VkRegistrationInfo | None:
        """Fetch a single VK registration by memory object ID."""
        result = await self.call(
            "novai_getVkRegistration",
            {"id": bytes_to_hex(coerce_hash32(vk_id, field="vk_id"))},
        )
        reg = result.get("registration") if isinstance(result, dict) else None
        if reg is None:
            return None
        return VkRegistrationInfo.from_json(reg)

    async def list_vk_registrations(
        self, entity_id: bytes | str
    ) -> list[VkRegistrationInfo]:
        """List all VK registrations owned by ``entity_id``."""
        eid_hex = bytes_to_hex(coerce_hash32(entity_id, field="entity_id"))
        result = await self.call("novai_listVkRegistrations", {"entity_id": eid_hex})
        return _decode_list(result, "registrations", VkRegistrationInfo.from_json)

    # ------------------------------------------------------------------
    # SLAs (Week 31)
    # ------------------------------------------------------------------

    async def get_sla_agreement(
        self, owner: bytes | str, object_id: bytes | str
    ) -> SlaAgreementInfo | None:
        """Fetch a single SLA by ``(owner, object_id)`` pair."""
        result = await self.call(
            "novai_getSlaAgreement",
            {
                "owner": bytes_to_hex(coerce_hash32(owner, field="owner")),
                "object_id": bytes_to_hex(coerce_hash32(object_id, field="object_id")),
            },
        )
        agreement = result.get("agreement") if isinstance(result, dict) else None
        if agreement is None:
            return None
        return SlaAgreementInfo.from_json(agreement)

    async def get_active_sla(
        self, buyer: bytes | str, seller: bytes | str
    ) -> SlaAgreementInfo | None:
        """Resolve the currently-open SLA between ``buyer`` and ``seller``."""
        result = await self.call(
            "novai_getActiveSla",
            {
                "buyer": bytes_to_hex(coerce_hash32(buyer, field="buyer")),
                "seller": bytes_to_hex(coerce_hash32(seller, field="seller")),
            },
        )
        agreement = result.get("agreement") if isinstance(result, dict) else None
        if agreement is None:
            return None
        return SlaAgreementInfo.from_json(agreement)

    async def list_slas_by_buyer(
        self, entity_id: bytes | str, start_height: int, end_height: int
    ) -> list[SlaAgreementInfo]:
        """List SLAs where ``entity_id`` is the buyer, created in the given height range."""
        return await self._list_slas("novai_listSlasByBuyer", entity_id, start_height, end_height)

    async def list_slas_by_seller(
        self, entity_id: bytes | str, start_height: int, end_height: int
    ) -> list[SlaAgreementInfo]:
        """List SLAs where ``entity_id`` is the seller, created in the given height range."""
        return await self._list_slas("novai_listSlasBySeller", entity_id, start_height, end_height)

    async def _list_slas(
        self,
        method: str,
        entity_id: bytes | str,
        start_height: int,
        end_height: int,
    ) -> list[SlaAgreementInfo]:
        eid_hex = bytes_to_hex(coerce_hash32(entity_id, field="entity_id"))
        result = await self.call(
            method,
            {"entity_id": eid_hex, "start_height": start_height, "end_height": end_height},
        )
        return _decode_list(result, "agreements", SlaAgreementInfo.from_json)

    # ------------------------------------------------------------------
    # Payment channels (Week 32)
    # ------------------------------------------------------------------

    async def get_payment_channel(
        self, owner: bytes | str, object_id: bytes | str
    ) -> PaymentChannelInfo | None:
        """Fetch a single payment channel by ``(owner, object_id)``."""
        result = await self.call(
            "novai_getPaymentChannel",
            {
                "owner": bytes_to_hex(coerce_hash32(owner, field="owner")),
                "object_id": bytes_to_hex(coerce_hash32(object_id, field="object_id")),
            },
        )
        channel = result.get("channel") if isinstance(result, dict) else None
        if channel is None:
            return None
        return PaymentChannelInfo.from_json(channel)

    async def list_channels_by_party_a(
        self, entity_id: bytes | str, start_height: int, end_height: int
    ) -> list[PaymentChannelInfo]:
        """List channels where ``entity_id`` is party A."""
        return await self._list_channels(
            "novai_listChannelsByPartyA", entity_id, start_height, end_height
        )

    async def list_channels_by_party_b(
        self, entity_id: bytes | str, start_height: int, end_height: int
    ) -> list[PaymentChannelInfo]:
        """List channels where ``entity_id`` is party B."""
        return await self._list_channels(
            "novai_listChannelsByPartyB", entity_id, start_height, end_height
        )

    async def _list_channels(
        self,
        method: str,
        entity_id: bytes | str,
        start_height: int,
        end_height: int,
    ) -> list[PaymentChannelInfo]:
        eid_hex = bytes_to_hex(coerce_hash32(entity_id, field="entity_id"))
        result = await self.call(
            method,
            {"entity_id": eid_hex, "start_height": start_height, "end_height": end_height},
        )
        return _decode_list(result, "channels", PaymentChannelInfo.from_json)

    async def get_channel_dispute_status(
        self, owner: bytes | str, object_id: bytes | str
    ) -> ChannelDisputeStatus:
        """Fetch dispute-window fields for a channel."""
        result = await self.call(
            "novai_getChannelDisputeStatus",
            {
                "owner": bytes_to_hex(coerce_hash32(owner, field="owner")),
                "object_id": bytes_to_hex(coerce_hash32(object_id, field="object_id")),
            },
        )
        if not isinstance(result, dict):
            raise DecodeError(f"novai_getChannelDisputeStatus: expected object, got {result!r}")
        return ChannelDisputeStatus.from_json(result)

    # ------------------------------------------------------------------
    # Oracle anchors (Week 35)
    # ------------------------------------------------------------------

    async def get_oracle_anchors_by_entity(
        self,
        entity_id: bytes | str,
        start_height: int,
        end_height: int,
        *,
        ts_min: int | None = None,
        ts_max: int | None = None,
    ) -> list[OracleAnchorInfo]:
        """Query oracle anchors posted by ``entity_id`` within a height range."""
        params: dict[str, Any] = {
            "entity_id": bytes_to_hex(coerce_hash32(entity_id, field="entity_id")),
            "start_height": start_height,
            "end_height": end_height,
        }
        if ts_min is not None:
            params["ts_min"] = ts_min
        if ts_max is not None:
            params["ts_max"] = ts_max
        result = await self.call("novai_getOracleAnchorsByEntity", params)
        return _decode_list(result, "anchors", OracleAnchorInfo.from_json)

    async def get_oracle_anchors_by_tag(
        self,
        data_tag: str,
        start_height: int,
        end_height: int,
        *,
        ts_min: int | None = None,
        ts_max: int | None = None,
    ) -> list[OracleAnchorInfo]:
        """Query oracle anchors by ``data_tag`` within a height range."""
        params: dict[str, Any] = {
            "data_tag": data_tag,
            "start_height": start_height,
            "end_height": end_height,
        }
        if ts_min is not None:
            params["ts_min"] = ts_min
        if ts_max is not None:
            params["ts_max"] = ts_max
        result = await self.call("novai_getOracleAnchorsByTag", params)
        return _decode_list(result, "anchors", OracleAnchorInfo.from_json)

    async def get_oracle_anchor(self, signal_hash: bytes | str) -> OracleAnchorInfo | None:
        """Point lookup for a single oracle anchor by content-addressed signal hash."""
        result = await self.call(
            "novai_getOracleAnchor",
            {"signal_hash": bytes_to_hex(coerce_hash32(signal_hash, field="signal_hash"))},
        )
        anchor = result.get("anchor") if isinstance(result, dict) else None
        if anchor is None:
            return None
        return OracleAnchorInfo.from_json(anchor)

    # ------------------------------------------------------------------
    # Convenience write helpers
    # ------------------------------------------------------------------

    async def _build_and_submit(
        self,
        keypair: Keypair,
        payload: bytes,
        fee: int,
        *,
        nonce: int | None = None,
    ) -> str:
        """Build, sign, and submit a TxV1 carrying ``payload``."""
        n = nonce if nonce is not None else await self.get_nonce(keypair.address)
        tx = TxV1(
            from_address=keypair.address,
            pubkey=keypair.pubkey,
            nonce=n,
            fee=fee,
            payload=payload,
        )
        tx.sig = sign_tx_v1(keypair.signing_key, tx)
        return await self.submit_tx(tx)

    async def transfer(
        self,
        keypair: Keypair,
        to: bytes | str,
        amount: int,
        *,
        fee: int = 100,
        nonce: int | None = None,
    ) -> SubmissionResult:
        """Build + sign + submit a Transfer tx."""
        payload = build_transfer_payload(to, amount)
        txid = await self._build_and_submit(keypair, payload, fee, nonce=nonce)
        return SubmissionResult(txid=txid)

    async def register_entity(
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
        """Register a new AI entity (tx type 8). The new entity ID is derived
        deterministically from ``code_hash`` and ``keypair.address``."""
        ch = coerce_hash32(code_hash, field="code_hash")
        payload = build_register_entity_payload(
            code_hash=ch,
            autonomy_mode=autonomy_mode,
            capabilities=capabilities,
            initial_balance=initial_balance,
        )
        txid = await self._build_and_submit(keypair, payload, fee, nonce=nonce)
        entity_id = compute_entity_id(ch, keypair.address)
        return SubmissionResult(txid=txid, entity_id=bytes_to_hex(entity_id))

    async def register_entity_with_key(
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
        """Register a new AI entity with its own signing key (tx type 10)."""
        ch = coerce_hash32(code_hash, field="code_hash")
        payload = build_register_with_key_payload(
            code_hash=ch,
            entity_pubkey=entity_pubkey,
            autonomy_mode=autonomy_mode,
            capabilities=capabilities,
            initial_balance=initial_balance,
        )
        txid = await self._build_and_submit(keypair, payload, fee, nonce=nonce)
        entity_id = compute_entity_id(ch, keypair.address)
        return SubmissionResult(txid=txid, entity_id=bytes_to_hex(entity_id))

    async def credit_entity(
        self,
        keypair: Keypair,
        entity_id: bytes | str,
        amount: int,
        *,
        fee: int = 100,
        nonce: int | None = None,
    ) -> SubmissionResult:
        """Credit an existing AI entity's economic balance (tx type 9)."""
        payload = build_credit_entity_payload(entity_id, amount)
        txid = await self._build_and_submit(keypair, payload, fee, nonce=nonce)
        return SubmissionResult(txid=txid)

    async def upgrade_entity(
        self,
        keypair: Keypair,
        entity_id: bytes | str,
        new_code_hash: bytes | str,
        reason_hash: bytes | str | None = None,
        *,
        fee: int = 5000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        """Issue an EntityUpgrade tx (type 11, Week 34). Creator-only.

        Subject to ``MIN_UPGRADE_INTERVAL_BLOCKS = 1000`` cooldown per entity
        and rejected when ``new_code_hash`` equals the current code hash.
        """
        payload = build_entity_upgrade_payload(entity_id, new_code_hash, reason_hash)
        txid = await self._build_and_submit(keypair, payload, fee, nonce=nonce)
        return SubmissionResult(txid=txid)

    async def create_memory_object(
        self,
        keypair: Keypair,
        object_type: MemoryObjectType,
        data: bytes,
        *,
        fee: int = 500,
        nonce: int | None = None,
    ) -> SubmissionResult:
        """Create a memory object (tx type 3)."""
        payload = build_create_memory_payload(object_type, data)
        txid = await self._build_and_submit(keypair, payload, fee, nonce=nonce)
        return SubmissionResult(txid=txid)

    async def update_memory_object(
        self,
        keypair: Keypair,
        object_id: bytes | str,
        new_data: bytes,
        *,
        fee: int = 500,
        nonce: int | None = None,
    ) -> SubmissionResult:
        """Update an existing memory object (tx type 4)."""
        payload = build_update_memory_payload(object_id, new_data)
        txid = await self._build_and_submit(keypair, payload, fee, nonce=nonce)
        return SubmissionResult(txid=txid)

    async def delete_memory_object(
        self,
        keypair: Keypair,
        object_id: bytes | str,
        *,
        fee: int = 500,
        nonce: int | None = None,
    ) -> SubmissionResult:
        """Delete a memory object (tx type 5)."""
        payload = build_delete_memory_payload(object_id)
        txid = await self._build_and_submit(keypair, payload, fee, nonce=nonce)
        return SubmissionResult(txid=txid)

    async def publish_signal(
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
        """Publish a signal commitment (tx type 2). Low-level helper; for
        specific signal types use :meth:`pay`, :meth:`post_oracle_anchor`,
        etc."""
        payload = build_signal_commitment_payload(
            signal_hash, signal_type, issuer_entity_id, extras=extras
        )
        txid = await self._build_and_submit(keypair, payload, fee, nonce=nonce)
        return SubmissionResult(
            txid=txid, signal_hash=bytes_to_hex(coerce_hash32(signal_hash, field="signal_hash"))
        )

    async def pay(
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
        """Issue a PaymentRequest signal (type 16).

        The Week 33 splits and Week 36 condition trailers are optional; pass
        either, both, or neither. ``signal_hash`` is the caller-supplied
        unique identifier for the request (any 32 bytes; downstream
        attestations reference it).
        """
        extras = build_payment_request_extras(
            payee_entity_id=payee,
            amount=amount,
            service_descriptor_hash=service_descriptor_hash,
            request_hash=request_hash,
            max_block_height=max_block_height,
            splits=splits,
            condition=condition,
        )
        return await self.publish_signal(
            keypair,
            signal_hash,
            AiSignalType.PAYMENT_REQUEST,
            issuer_entity_id,
            extras=extras,
            fee=fee,
            nonce=nonce,
        )

    async def attest_payment(
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
        """Issue a ServiceAttestation signal (type 17). The issuer must be the
        original payer (the chain enforces this)."""
        extras = build_service_attestation_extras(payment_signal_hash, payee, status)
        return await self.publish_signal(
            keypair,
            signal_hash,
            AiSignalType.SERVICE_ATTESTATION,
            issuer_entity_id,
            extras=extras,
            fee=fee,
            nonce=nonce,
        )

    async def post_oracle_anchor(
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
        """Post an OracleAnchor signal (type 22, Week 35).

        The signal hash is derived locally from the inputs; identical
        anchor content collides and the chain rejects duplicates as
        replays. The issuing entity must hold the ``post_oracle_anchors``
        capability (bit 6).
        """
        issuer = coerce_address(issuer_entity_id, field="issuer_entity_id")
        sig_hash = derive_oracle_anchor_signal_hash(
            issuer_entity_id=issuer,
            data_hash=data_hash,
            external_timestamp=external_timestamp,
            source_hash=source_hash,
            data_tag=data_tag,
        )
        extras = build_oracle_anchor_extras(
            data_hash=data_hash,
            external_timestamp=external_timestamp,
            source_hash=source_hash,
            expiry_height=expiry_height,
            data_tag=data_tag,
        )
        return await self.publish_signal(
            keypair,
            sig_hash,
            AiSignalType.ORACLE_ANCHOR,
            issuer,
            extras=extras,
            fee=fee,
            nonce=nonce,
        )

    async def accept_sla(
        self,
        keypair: Keypair,
        seller_entity_id: bytes | str,
        sla_object_id: bytes | str,
        buyer_entity_id: bytes | str,
        *,
        fee: int = 1000,
        nonce: int | None = None,
    ) -> SubmissionResult:
        """Issue an SlaAccept signal (type 18, Week 31). The issuer is the
        seller; the signal hash is derived locally."""
        sig_hash = derive_sla_accept_signal_hash(sla_object_id, buyer_entity_id)
        extras = build_sla_accept_extras(sla_object_id, buyer_entity_id)
        return await self.publish_signal(
            keypair,
            sig_hash,
            AiSignalType.SLA_ACCEPT,
            seller_entity_id,
            extras=extras,
            fee=fee,
            nonce=nonce,
        )

    async def accept_channel(
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
        """Issue a ChannelAccept signal (type 19, Week 32). The issuer is party B."""
        extras = build_channel_accept_extras(channel_object_id, party_a_entity_id)
        return await self.publish_signal(
            keypair,
            signal_hash,
            AiSignalType.CHANNEL_ACCEPT,
            party_b_entity_id,
            extras=extras,
            fee=fee,
            nonce=nonce,
        )

    async def close_channel(
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
        """Issue a ChannelClose signal (type 20, Week 32).

        ``sig_a`` and ``sig_b`` must each be a valid ed25519 signature over
        the canonical channel state bytes for ``(nonce, balance_a,
        balance_b, is_final)``; see
        :func:`novai_sdk.crypto.sign_channel_state`.
        """
        extras = build_channel_close_extras(
            channel_object_id,
            party_a_entity_id,
            nonce=channel_nonce,
            balance_a=balance_a,
            balance_b=balance_b,
            is_final=is_final,
            sig_a=sig_a,
            sig_b=sig_b,
        )
        return await self.publish_signal(
            keypair,
            signal_hash,
            AiSignalType.CHANNEL_CLOSE,
            issuer_entity_id,
            extras=extras,
            fee=fee,
            nonce=nonce,
        )

    async def finalize_channel(
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
        """Issue a ChannelFinalize signal (type 21, Week 32). Permissionless
        after the dispute deadline expires."""
        extras = build_channel_finalize_extras(channel_object_id, party_a_entity_id)
        return await self.publish_signal(
            keypair,
            signal_hash,
            AiSignalType.CHANNEL_FINALIZE,
            issuer_entity_id,
            extras=extras,
            fee=fee,
            nonce=nonce,
        )


# ---------------------------------------------------------------------------
# Module-private helpers
# ---------------------------------------------------------------------------


def _expect_str_field(result: Any, field: str) -> str:
    if isinstance(result, dict) and field in result:
        return str(result[field])
    raise DecodeError(f"expected object with '{field}' field, got {result!r}")


def _expect_int_field(result: Any, field: str) -> int:
    if isinstance(result, dict) and field in result:
        try:
            return int(result[field])
        except (TypeError, ValueError) as exc:
            raise DecodeError(f"field '{field}' is not an int: {result[field]!r}") from exc
    raise DecodeError(f"expected object with '{field}' field, got {result!r}")


def _decode_list(result: Any, key: str, parser: Any) -> list[Any]:
    if not isinstance(result, dict):
        raise DecodeError(f"expected object with '{key}' list, got {result!r}")
    raw = result.get(key, [])
    if not isinstance(raw, list):
        raise DecodeError(f"'{key}' must be a list, got {raw!r}")
    return [parser(row) for row in raw]


def _coerce_category(category: ServiceCategory | int | str) -> int:
    if isinstance(category, ServiceCategory):
        return int(category)
    if isinstance(category, int):
        return category
    if isinstance(category, str):
        normalized = category.upper().replace("-", "_")
        try:
            return int(ServiceCategory[normalized])
        except KeyError as exc:
            raise ValueError(
                f"unknown service category {category!r}; expected one of "
                f"{[c.name.lower().replace('_', '-') for c in ServiceCategory]} "
                f"or an integer in [0, 255]"
            ) from exc
    raise TypeError(f"category must be ServiceCategory, int, or str, got {type(category).__name__}")


__all__ = [
    "AsyncNOVAIClient",
    "BalanceResult",
    "FaucetResult",
]
