"""Typed dataclasses for NOVAI RPC responses.

Each dataclass mirrors the JSON shape returned by a specific RPC method.
``from_json`` classmethods parse the raw dict; raw values like ``u128`` and
``u64`` are decoded into Python ``int`` (Python natively handles bignums,
unlike JavaScript) but balance / amount fields stay as strings to preserve
the chain's decimal-string return format for downstream display.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


def _opt_str(value: Any) -> str | None:
    """Coerce an optional JSON field into ``str`` or ``None``."""
    if value is None:
        return None
    return str(value)


def _opt_int(value: Any) -> int | None:
    """Coerce an optional JSON field into ``int`` or ``None``."""
    if value is None:
        return None
    return int(value)


def _req(d: dict[str, Any], key: str) -> Any:
    if key not in d:
        raise KeyError(f"missing required field '{key}' in {d!r}")
    return d[key]


# ---------------------------------------------------------------------------
# Block / tx / account
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class BlockHeader:
    """Response shape for ``novai_getBlockByHeight`` / ``novai_getBlockByHash`` /
    ``novai_getLatestBlock``."""

    height: int
    round: int
    block_hash: str
    parent_hash: str
    state_root: str
    tx_count: int

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> BlockHeader:
        return cls(
            height=int(_req(d, "height")),
            round=int(_req(d, "round")),
            block_hash=str(_req(d, "block_hash")),
            parent_hash=str(_req(d, "parent_hash")),
            state_root=str(_req(d, "state_root")),
            tx_count=int(_req(d, "tx_count")),
        )


@dataclass(frozen=True)
class TxReceipt:
    """Response shape for ``novai_getTransaction``."""

    block_height: int
    tx_index: int
    from_address: str
    nonce: int
    fee: int
    payload_len: int

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> TxReceipt:
        return cls(
            block_height=int(_req(d, "block_height")),
            tx_index=int(_req(d, "tx_index")),
            from_address=str(_req(d, "from")),
            nonce=int(_req(d, "nonce")),
            fee=int(_req(d, "fee")),
            payload_len=int(_req(d, "payload_len")),
        )


# ---------------------------------------------------------------------------
# AI entity / upgrade
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class AiEntityInfo:
    """Response shape for ``novai_getAiEntity``."""

    id: str
    code_hash: str
    creator: str
    autonomy_mode: int
    capabilities: int
    economic_balance: str
    nonce: int
    pubkey: str
    memory_root: str
    params_root: str
    registered_at: int
    last_active_at: int
    is_active: bool
    reputation_score: int
    total_transactions: int
    reputation_events_count: int
    stake_balance: str
    stake_locked_until: int
    upgrade_count: int
    last_upgrade_height: int

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> AiEntityInfo:
        return cls(
            id=str(_req(d, "id")),
            code_hash=str(_req(d, "code_hash")),
            creator=str(_req(d, "creator")),
            autonomy_mode=int(_req(d, "autonomy_mode")),
            capabilities=int(_req(d, "capabilities")),
            economic_balance=str(_req(d, "economic_balance")),
            nonce=int(_req(d, "nonce")),
            pubkey=str(_req(d, "pubkey")),
            memory_root=str(_req(d, "memory_root")),
            params_root=str(_req(d, "params_root")),
            registered_at=int(_req(d, "registered_at")),
            last_active_at=int(_req(d, "last_active_at")),
            is_active=bool(_req(d, "is_active")),
            reputation_score=int(_req(d, "reputation_score")),
            total_transactions=int(_req(d, "total_transactions")),
            reputation_events_count=int(_req(d, "reputation_events_count")),
            stake_balance=str(_req(d, "stake_balance")),
            stake_locked_until=int(_req(d, "stake_locked_until")),
            upgrade_count=int(d.get("upgrade_count", 0)),
            last_upgrade_height=int(d.get("last_upgrade_height", 0)),
        )


@dataclass(frozen=True)
class UpgradeRecord:
    """Response shape for one entry of ``novai_getUpgradeHistory``."""

    old_code_hash: str
    new_code_hash: str
    upgrade_height: int
    upgrade_count: int
    reason_hash: str

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> UpgradeRecord:
        return cls(
            old_code_hash=str(_req(d, "old_code_hash")),
            new_code_hash=str(_req(d, "new_code_hash")),
            upgrade_height=int(_req(d, "upgrade_height")),
            upgrade_count=int(_req(d, "upgrade_count")),
            reason_hash=str(_req(d, "reason_hash")),
        )


# ---------------------------------------------------------------------------
# Signals + payments
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class SignalInfo:
    """Response shape for ``novai_getSignalsBy*``."""

    commitment_hash: str
    signal_type: int
    height: int
    issuer: str

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> SignalInfo:
        return cls(
            commitment_hash=str(_req(d, "commitment_hash")),
            signal_type=int(_req(d, "signal_type")),
            height=int(_req(d, "height")),
            issuer=str(_req(d, "issuer")),
        )


@dataclass(frozen=True)
class PaymentSplitJson:
    """Response shape for one entry in ``PaymentJson.splits`` (Week 33)."""

    recipient_entity_id: str
    basis_points: int
    credited_amount: str

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> PaymentSplitJson:
        return cls(
            recipient_entity_id=str(_req(d, "recipient_entity_id")),
            basis_points=int(_req(d, "basis_points")),
            credited_amount=str(_req(d, "credited_amount")),
        )


@dataclass(frozen=True)
class PaymentConditionJson:
    """Response shape for ``PaymentJson.condition`` (Week 36)."""

    kind: str
    anchor_signal_hash: str
    expected_data_hash: str | None
    expected_tag: str | None
    expected_tag_hex: str | None

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> PaymentConditionJson:
        return cls(
            kind=str(_req(d, "kind")),
            anchor_signal_hash=str(_req(d, "anchor_signal_hash")),
            expected_data_hash=_opt_str(d.get("expected_data_hash")),
            expected_tag=_opt_str(d.get("expected_tag")),
            expected_tag_hex=_opt_str(d.get("expected_tag_hex")),
        )


@dataclass(frozen=True)
class PaymentRecord:
    """Response shape for one entry of ``novai_getPaymentsByEntity``."""

    payer: str
    payee: str
    amount: str
    service_descriptor_hash: str
    request_hash: str
    payment_height: int
    max_block_height: int
    attested_status: str | None
    attested_height: int | None
    splits: list[PaymentSplitJson] | None = None
    condition: PaymentConditionJson | None = None

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> PaymentRecord:
        splits_raw = d.get("splits")
        condition_raw = d.get("condition")
        return cls(
            payer=str(_req(d, "payer")),
            payee=str(_req(d, "payee")),
            amount=str(_req(d, "amount")),
            service_descriptor_hash=str(_req(d, "service_descriptor_hash")),
            request_hash=str(_req(d, "request_hash")),
            payment_height=int(_req(d, "payment_height")),
            max_block_height=int(_req(d, "max_block_height")),
            attested_status=_opt_str(d.get("attested_status")),
            attested_height=_opt_int(d.get("attested_height")),
            splits=(
                [PaymentSplitJson.from_json(s) for s in splits_raw]
                if isinstance(splits_raw, list)
                else None
            ),
            condition=(
                PaymentConditionJson.from_json(condition_raw)
                if isinstance(condition_raw, dict)
                else None
            ),
        )


# ---------------------------------------------------------------------------
# Service descriptors + VK registrations
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class ServiceDescriptorInfo:
    """Response shape for ``novai_getServiceDescriptorsByCategory``."""

    object_id: str
    owner_entity: str
    created_at: int
    updated_at: int
    version: int
    service_name_hash: str
    service_url_hash: str
    description_hash: str
    category: int
    category_label: str
    price_per_call: str
    subscription_rate_per_block: str
    min_reputation_score: int
    min_stake: str
    capability_tags: int
    status: int
    status_label: str

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> ServiceDescriptorInfo:
        return cls(
            object_id=str(_req(d, "object_id")),
            owner_entity=str(_req(d, "owner_entity")),
            created_at=int(_req(d, "created_at")),
            updated_at=int(_req(d, "updated_at")),
            version=int(_req(d, "version")),
            service_name_hash=str(_req(d, "service_name_hash")),
            service_url_hash=str(_req(d, "service_url_hash")),
            description_hash=str(_req(d, "description_hash")),
            category=int(_req(d, "category")),
            category_label=str(_req(d, "category_label")),
            price_per_call=str(_req(d, "price_per_call")),
            subscription_rate_per_block=str(_req(d, "subscription_rate_per_block")),
            min_reputation_score=int(_req(d, "min_reputation_score")),
            min_stake=str(_req(d, "min_stake")),
            capability_tags=int(_req(d, "capability_tags")),
            status=int(_req(d, "status")),
            status_label=str(_req(d, "status_label")),
        )

    @property
    def entity_id(self) -> str:
        """Alias of ``owner_entity`` for the agent-discovery use case."""
        return self.owner_entity


@dataclass(frozen=True)
class VkRegistrationInfo:
    """Response shape for ``novai_getVkRegistration`` / ``novai_listVkRegistrations``."""

    object_id: str
    owner_entity: str
    created_at: int
    updated_at: int
    version: int
    proof_type: int
    proof_type_label: str
    code_hash: str
    label: str
    vk_len: int
    vk_bytes_hex: str

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> VkRegistrationInfo:
        return cls(
            object_id=str(_req(d, "object_id")),
            owner_entity=str(_req(d, "owner_entity")),
            created_at=int(_req(d, "created_at")),
            updated_at=int(_req(d, "updated_at")),
            version=int(_req(d, "version")),
            proof_type=int(_req(d, "proof_type")),
            proof_type_label=str(_req(d, "proof_type_label")),
            code_hash=str(_req(d, "code_hash")),
            label=str(_req(d, "label")),
            vk_len=int(_req(d, "vk_len")),
            vk_bytes_hex=str(_req(d, "vk_bytes_hex")),
        )


# ---------------------------------------------------------------------------
# SLA + Channel
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class SlaAgreementInfo:
    """Response shape for ``novai_getSlaAgreement`` / ``novai_listSlasBy*``."""

    object_id: str
    owner_entity: str
    created_at: int
    updated_at: int
    version: int
    buyer_entity_id: str
    seller_entity_id: str
    service_descriptor_hash: str
    status: int
    status_label: str
    created_at_height: int
    accepted_at_height: int
    start_height: int
    end_height: int
    violation_count: int
    violation_threshold: int
    max_response_time_blocks: int
    min_uptime_bps: int
    min_delivery_success_bps: int
    price_per_call: str
    slash_amount: str
    terminated_at_height: int
    slashed_amount: str

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> SlaAgreementInfo:
        return cls(
            object_id=str(_req(d, "object_id")),
            owner_entity=str(_req(d, "owner_entity")),
            created_at=int(_req(d, "created_at")),
            updated_at=int(_req(d, "updated_at")),
            version=int(_req(d, "version")),
            buyer_entity_id=str(_req(d, "buyer_entity_id")),
            seller_entity_id=str(_req(d, "seller_entity_id")),
            service_descriptor_hash=str(_req(d, "service_descriptor_hash")),
            status=int(_req(d, "status")),
            status_label=str(_req(d, "status_label")),
            created_at_height=int(_req(d, "created_at_height")),
            accepted_at_height=int(_req(d, "accepted_at_height")),
            start_height=int(_req(d, "start_height")),
            end_height=int(_req(d, "end_height")),
            violation_count=int(_req(d, "violation_count")),
            violation_threshold=int(_req(d, "violation_threshold")),
            max_response_time_blocks=int(_req(d, "max_response_time_blocks")),
            min_uptime_bps=int(_req(d, "min_uptime_bps")),
            min_delivery_success_bps=int(_req(d, "min_delivery_success_bps")),
            price_per_call=str(_req(d, "price_per_call")),
            slash_amount=str(_req(d, "slash_amount")),
            terminated_at_height=int(_req(d, "terminated_at_height")),
            slashed_amount=str(_req(d, "slashed_amount")),
        )


@dataclass(frozen=True)
class PaymentChannelInfo:
    """Response shape for ``novai_getPaymentChannel`` / ``novai_listChannelsBy*``."""

    object_id: str
    owner_entity: str
    created_at: int
    updated_at: int
    version: int
    party_a_entity_id: str
    party_b_entity_id: str
    sla_object_id: str
    status: int
    status_label: str
    deposit_a: str
    deposit_b: str
    balance_a: str
    balance_b: str
    nonce: int
    proposed_at_height: int
    accepted_at_height: int
    closing_at_height: int
    dispute_deadline_height: int
    dispute_window_blocks: int

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> PaymentChannelInfo:
        return cls(
            object_id=str(_req(d, "object_id")),
            owner_entity=str(_req(d, "owner_entity")),
            created_at=int(_req(d, "created_at")),
            updated_at=int(_req(d, "updated_at")),
            version=int(_req(d, "version")),
            party_a_entity_id=str(_req(d, "party_a_entity_id")),
            party_b_entity_id=str(_req(d, "party_b_entity_id")),
            sla_object_id=str(_req(d, "sla_object_id")),
            status=int(_req(d, "status")),
            status_label=str(_req(d, "status_label")),
            deposit_a=str(_req(d, "deposit_a")),
            deposit_b=str(_req(d, "deposit_b")),
            balance_a=str(_req(d, "balance_a")),
            balance_b=str(_req(d, "balance_b")),
            nonce=int(_req(d, "nonce")),
            proposed_at_height=int(_req(d, "proposed_at_height")),
            accepted_at_height=int(_req(d, "accepted_at_height")),
            closing_at_height=int(_req(d, "closing_at_height")),
            dispute_deadline_height=int(_req(d, "dispute_deadline_height")),
            dispute_window_blocks=int(_req(d, "dispute_window_blocks")),
        )


@dataclass(frozen=True)
class ChannelDisputeStatus:
    """Response shape for ``novai_getChannelDisputeStatus``."""

    found: bool
    status: int
    status_label: str
    closing_at_height: int
    dispute_deadline_height: int
    current_height: int
    blocks_remaining: int
    finalize_ready: bool

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> ChannelDisputeStatus:
        return cls(
            found=bool(_req(d, "found")),
            status=int(_req(d, "status")),
            status_label=str(_req(d, "status_label")),
            closing_at_height=int(_req(d, "closing_at_height")),
            dispute_deadline_height=int(_req(d, "dispute_deadline_height")),
            current_height=int(_req(d, "current_height")),
            blocks_remaining=int(_req(d, "blocks_remaining")),
            finalize_ready=bool(_req(d, "finalize_ready")),
        )


# ---------------------------------------------------------------------------
# Oracle anchors
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class OracleAnchorInfo:
    """Response shape for ``novai_getOracleAnchor*``."""

    issuer_entity_id: str
    data_hash: str
    external_timestamp: int
    source_hash: str
    expiry_height: int
    anchor_height: int
    data_tag: str
    data_tag_hex: str

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> OracleAnchorInfo:
        return cls(
            issuer_entity_id=str(_req(d, "issuer_entity_id")),
            data_hash=str(_req(d, "data_hash")),
            external_timestamp=int(_req(d, "external_timestamp")),
            source_hash=str(_req(d, "source_hash")),
            expiry_height=int(_req(d, "expiry_height")),
            anchor_height=int(_req(d, "anchor_height")),
            data_tag=str(_req(d, "data_tag")),
            data_tag_hex=str(_req(d, "data_tag_hex")),
        )


# ---------------------------------------------------------------------------
# Generic memory object
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class MemoryObjectInfo:
    """Response shape for ``novai_getMemoryObjects``."""

    object_id: str
    object_type: int
    owner_entity: str
    created_at: int
    updated_at: int
    data_size: int
    data_hex: str = field(repr=False)

    @classmethod
    def from_json(cls, d: dict[str, Any]) -> MemoryObjectInfo:
        return cls(
            object_id=str(_req(d, "object_id")),
            object_type=int(_req(d, "object_type")),
            owner_entity=str(_req(d, "owner_entity")),
            created_at=int(_req(d, "created_at")),
            updated_at=int(_req(d, "updated_at")),
            data_size=int(_req(d, "data_size")),
            data_hex=str(_req(d, "data")),
        )

    @property
    def data(self) -> bytes:
        """Decoded raw data bytes."""
        return bytes.fromhex(self.data_hex)


# ---------------------------------------------------------------------------
# Submission helpers
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class SubmissionResult:
    """Convenience wrapper returned by high-level submit helpers."""

    txid: str
    entity_id: str | None = None
    signal_hash: str | None = None
    object_id: str | None = None
