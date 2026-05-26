"""NOVAI Python SDK.

Pure-Python client for the NOVAI blockchain. Wraps the JSON-RPC surface and
builds canonical signed transactions locally, with no Rust toolchain
required. Tier-1 use case is letting agent-framework code talk to a NOVAI
node from Python.

Quick start::

    from novai_sdk import NOVAIClient, Keypair, Capabilities

    client = NOVAIClient("http://localhost:3030")
    kp = Keypair.generate()
    client.faucet(kp.address)

    # Register an oracle entity and post an anchor:
    result = client.register_entity(
        keypair=kp,
        code_hash=bytes.fromhex("..."),
        capabilities=Capabilities.oracle(),
    )
    anchor = client.post_oracle_anchor(
        keypair=kp,
        issuer_entity_id=result.entity_id,
        data_hash=bytes.fromhex("..."),
        external_timestamp=1735776000,
        data_tag="price/ETH-USD",
    )
"""

from novai_sdk.capabilities import Capabilities
from novai_sdk.client import AsyncNOVAIClient, BalanceResult, FaucetResult
from novai_sdk.codec import (
    TX_V1_OVERHEAD,
    TxV1,
    encode_tx_v1_signed,
    encode_tx_v1_unsigned,
    tx_encoded_size,
    txid_v1,
)
from novai_sdk.constants import (
    BPS_DENOMINATOR,
    DOMAIN_TAG_ADDRESS_V1,
    DOMAIN_TAG_AI_ENTITY_ID_V1,
    DOMAIN_TAG_CHANNEL_STATE_V1,
    DOMAIN_TAG_TX_V1,
    MAX_PAYMENT_SPLITS,
    MAX_TX_SIZE,
    MIN_PAYMENT_SPLITS_WHEN_PRESENT,
    MIN_UPGRADE_INTERVAL_BLOCKS,
    NOVAI_CHANNEL_CHAIN_ID,
    ORACLE_ANCHOR_DATA_TAG_MAX_LEN,
    PAYMENT_CONDITION_MARKER,
)
from novai_sdk.crypto import (
    address_from_pubkey,
    compute_entity_id,
    sign_channel_state,
    sign_tx_v1,
    verify_channel_state_signature,
    verify_tx_v1,
)
from novai_sdk.enums import (
    AiSignalType,
    AutonomyMode,
    ChannelStatus,
    MemoryObjectType,
    PaymentAttestationStatus,
    PaymentConditionKind,
    ProofType,
    ServiceCategory,
    ServiceDescriptorStatus,
    SlaStatus,
    TxPayloadType,
    TxVersion,
)
from novai_sdk.errors import (
    DecodeError,
    EncodingError,
    FeeTooLowError,
    InvalidParamsError,
    MempoolFullError,
    MethodNotFoundError,
    NonceTooLowError,
    NovaiError,
    NovaiRpcError,
    RateLimitedError,
    ResponseTooLargeError,
    SenderLimitExceededError,
    ServerError,
    StateQueryError,
    ValidationError,
)
from novai_sdk.keys import Keypair
from novai_sdk.signals.payments import PaymentCondition, PaymentSplit
from novai_sdk.sync_client import NOVAIClient
from novai_sdk.types import (
    AiEntityInfo,
    BlockHeader,
    ChannelDisputeStatus,
    MemoryObjectInfo,
    OracleAnchorInfo,
    PaymentChannelInfo,
    PaymentConditionJson,
    PaymentRecord,
    PaymentSplitJson,
    ServiceDescriptorInfo,
    SignalInfo,
    SlaAgreementInfo,
    SubmissionResult,
    TxReceipt,
    UpgradeRecord,
    VkRegistrationInfo,
)

__version__ = "0.1.0"

__all__ = [
    "BPS_DENOMINATOR",
    "DOMAIN_TAG_ADDRESS_V1",
    "DOMAIN_TAG_AI_ENTITY_ID_V1",
    "DOMAIN_TAG_CHANNEL_STATE_V1",
    "DOMAIN_TAG_TX_V1",
    "MAX_PAYMENT_SPLITS",
    "MAX_TX_SIZE",
    "MIN_PAYMENT_SPLITS_WHEN_PRESENT",
    "MIN_UPGRADE_INTERVAL_BLOCKS",
    "NOVAI_CHANNEL_CHAIN_ID",
    "ORACLE_ANCHOR_DATA_TAG_MAX_LEN",
    "PAYMENT_CONDITION_MARKER",
    "TX_V1_OVERHEAD",
    "AiEntityInfo",
    "AiSignalType",
    "AsyncNOVAIClient",
    "AutonomyMode",
    "BalanceResult",
    "BlockHeader",
    "Capabilities",
    "ChannelDisputeStatus",
    "ChannelStatus",
    "DecodeError",
    "EncodingError",
    "FaucetResult",
    "FeeTooLowError",
    "InvalidParamsError",
    "Keypair",
    "MemoryObjectInfo",
    "MemoryObjectType",
    "MempoolFullError",
    "MethodNotFoundError",
    "NOVAIClient",
    "NonceTooLowError",
    "NovaiError",
    "NovaiRpcError",
    "OracleAnchorInfo",
    "PaymentAttestationStatus",
    "PaymentChannelInfo",
    "PaymentCondition",
    "PaymentConditionJson",
    "PaymentConditionKind",
    "PaymentRecord",
    "PaymentSplit",
    "PaymentSplitJson",
    "ProofType",
    "RateLimitedError",
    "ResponseTooLargeError",
    "SenderLimitExceededError",
    "ServerError",
    "ServiceCategory",
    "ServiceDescriptorInfo",
    "ServiceDescriptorStatus",
    "SignalInfo",
    "SlaAgreementInfo",
    "SlaStatus",
    "StateQueryError",
    "SubmissionResult",
    "TxPayloadType",
    "TxReceipt",
    "TxV1",
    "TxVersion",
    "UpgradeRecord",
    "ValidationError",
    "VkRegistrationInfo",
    "__version__",
    "address_from_pubkey",
    "compute_entity_id",
    "encode_tx_v1_signed",
    "encode_tx_v1_unsigned",
    "sign_channel_state",
    "sign_tx_v1",
    "tx_encoded_size",
    "txid_v1",
    "verify_channel_state_signature",
    "verify_tx_v1",
]
