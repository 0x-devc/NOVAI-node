"""Protocol enums.

Discriminant bytes are pinned to their Rust definitions. Naming follows
Python snake_case for member names where possible, with the wire byte
preserved as the enum value.
"""

from __future__ import annotations

from enum import IntEnum


class TxVersion(IntEnum):
    """TxV1 version byte; only ``V1`` is currently valid."""

    V1 = 1


class TxPayloadType(IntEnum):
    """First byte of every tx payload; selects the decoder branch on the chain side."""

    TRANSFER = 1
    SIGNAL_COMMITMENT = 2
    CREATE_MEMORY = 3
    UPDATE_MEMORY = 4
    DELETE_MEMORY = 5
    SUBMIT_PROPOSAL = 6
    EXECUTE_PROPOSAL = 7
    REGISTER_AI_ENTITY = 8
    CREDIT_AI_ENTITY = 9
    REGISTER_AI_ENTITY_WITH_KEY = 10
    ENTITY_UPGRADE = 11


class AutonomyMode(IntEnum):
    """AI entity autonomy mode (see crates/ai_entities/src/lib.rs)."""

    ADVISORY = 0
    GATED = 1
    AUTONOMOUS = 2


class AiSignalType(IntEnum):
    """Signal type discriminant carried in every SignalCommitment payload.

    Types 0-6 are the original Week 1-6 advisory/oracle set. Types 7-22 were
    added incrementally across Weeks 25-36 for reputation, staking, proofs,
    subscriptions, payments, SLAs, channels, and oracle anchoring.
    """

    ANOMALY = 0
    OPTIMIZATION = 1
    PREDICTION = 2
    RISK_SCORE = 3
    AUDIT_REPORT = 4
    SPAM_RISK = 5
    CONGESTION_FORECAST = 6
    REPUTATION_UPDATE = 7
    SIGNAL_PURCHASE = 8
    STAKE_DEPOSIT = 9
    STAKE_WITHDRAW = 10
    STAKE_SLASH = 11
    COMPOSITION_CHECK = 12
    PROOF_SUBMISSION = 13
    SUBSCRIPTION_CREATE = 14
    SUBSCRIPTION_CANCEL = 15
    PAYMENT_REQUEST = 16
    SERVICE_ATTESTATION = 17
    SLA_ACCEPT = 18
    CHANNEL_ACCEPT = 19
    CHANNEL_CLOSE = 20
    CHANNEL_FINALIZE = 21
    ORACLE_ANCHOR = 22


class MemoryObjectType(IntEnum):
    """Memory object type discriminant (Week 21 onward)."""

    CHAIN_SUMMARY = 0
    LABEL_INDEX = 1
    EMBEDDING_COMMITMENT = 2
    ANOMALY_LOG = 3
    STATISTICS_SNAPSHOT = 4
    REPUTATION_EVENT = 5
    RATING = 6
    SIGNAL_CATALOG = 7
    COMPOSITION_GRAPH = 8
    VERIFICATION_RECORD = 9
    DELEGATION_GRANT = 10
    SUBSCRIPTION = 11
    SERVICE_DESCRIPTOR = 12
    VK_REGISTRATION = 13
    SLA_AGREEMENT = 14
    PAYMENT_CHANNEL = 15


class PaymentAttestationStatus(IntEnum):
    """Status byte of a `ServiceAttestation` signal (Week 28)."""

    DELIVERED = 0
    FAILED = 1


class PaymentConditionKind(IntEnum):
    """Discriminant of the Week 36 conditional execution body.

    The kind byte sits immediately after the ``PAYMENT_CONDITION_MARKER`` (0xC1)
    at offset 179 of the `PaymentRequest` extras. Each kind has its own operand
    layout (see ``novai_sdk.tx`` for builders).
    """

    ANCHOR_EXISTS = 1
    ANCHOR_DATA_HASH_EQUALS = 2
    ANCHOR_TAG_EQUALS = 3
    ANCHOR_NOT_EXPIRED = 4


class ProofType(IntEnum):
    """Proof type discriminant for `ProofSubmission` signals (Weeks 27, 30)."""

    STUB = 0
    GROTH16 = 1
    PLONK = 2
    GROTH16_REGISTERED = 3
    PLONK_REGISTERED = 4


class ServiceCategory(IntEnum):
    """Well-known service categories for the Week 29 discovery registry."""

    GENERIC = 0
    DATA_ORACLE = 1
    INFERENCE = 2
    COMPUTE = 3
    STORAGE = 4
    INDEXER = 5
    SIGNAL_PROVIDER = 6
    VERIFICATION = 7
    MONITORING = 8
    GATEWAY = 9


class ServiceDescriptorStatus(IntEnum):
    """Lifecycle status byte of a ServiceDescriptor memory object."""

    ACTIVE = 0
    PAUSED = 1
    DEPRECATED = 2


class SlaStatus(IntEnum):
    """Lifecycle status byte of an SlaAgreement memory object (Week 31)."""

    PROPOSED = 0
    ACTIVE = 1
    COMPLETED = 2
    VIOLATED = 3
    CANCELLED = 4


class ChannelStatus(IntEnum):
    """Lifecycle status byte of a PaymentChannel memory object (Week 32)."""

    PROPOSED = 0
    OPEN = 1
    CLOSING = 2
