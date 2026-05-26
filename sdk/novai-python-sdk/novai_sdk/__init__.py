"""NOVAI Python SDK.

Pure-Python client for the NOVAI blockchain. Wraps the JSON-RPC surface and
builds canonical signed transactions locally, with no Rust toolchain required.

Quick start:

    from novai_sdk import NOVAIClient, Keypair

    client = NOVAIClient("http://localhost:3030")
    kp = Keypair.generate()
    txid, amount = client.faucet(kp.address)
"""

from novai_sdk.capabilities import Capabilities
from novai_sdk.client import AsyncNOVAIClient
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
from novai_sdk.sync_client import NOVAIClient

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
    "AiSignalType",
    "AsyncNOVAIClient",
    "AutonomyMode",
    "Capabilities",
    "ChannelStatus",
    "DecodeError",
    "EncodingError",
    "FeeTooLowError",
    "InvalidParamsError",
    "Keypair",
    "MemoryObjectType",
    "MempoolFullError",
    "MethodNotFoundError",
    "NOVAIClient",
    "NonceTooLowError",
    "NovaiError",
    "NovaiRpcError",
    "PaymentAttestationStatus",
    "PaymentConditionKind",
    "ProofType",
    "RateLimitedError",
    "ResponseTooLargeError",
    "SenderLimitExceededError",
    "ServerError",
    "ServiceCategory",
    "ServiceDescriptorStatus",
    "SlaStatus",
    "StateQueryError",
    "TxPayloadType",
    "TxV1",
    "TxVersion",
    "ValidationError",
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
