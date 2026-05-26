"""Memory object data block encoders.

Each function returns the inner ``data`` payload for a CreateMemoryObject /
UpdateMemoryObject tx. The outer envelope is built via
:func:`novai_sdk.tx.build_create_memory_payload`.

Phase 2 ships encoders for the four agent-facing fixed-format types
introduced in Weeks 29-32: ServiceDescriptor (12), VkRegistration (13),
SlaAgreement (14), and PaymentChannel (15). The earlier types (0-11) are
typically constructed internally by AI modules from raw protobuf-like
serializations; users who need them can pass raw bytes directly to
``build_create_memory_payload``.
"""

from novai_sdk.memory_objects.payment_channel import (
    PAYMENT_CHANNEL_SIZE,
    PAYMENT_CHANNEL_VERSION,
    encode_payment_channel,
)
from novai_sdk.memory_objects.service_descriptor import (
    SERVICE_DESCRIPTOR_SIZE,
    SERVICE_DESCRIPTOR_VERSION,
    encode_service_descriptor,
)
from novai_sdk.memory_objects.sla_agreement import (
    SLA_AGREEMENT_SIZE,
    SLA_AGREEMENT_VERSION,
    encode_sla_agreement,
)
from novai_sdk.memory_objects.vk_registration import (
    VK_REGISTRATION_HEADER_SIZE,
    VK_REGISTRATION_LABEL_MAX,
    VK_REGISTRATION_VERSION,
    encode_vk_registration,
)

__all__ = [
    "PAYMENT_CHANNEL_SIZE",
    "PAYMENT_CHANNEL_VERSION",
    "SERVICE_DESCRIPTOR_SIZE",
    "SERVICE_DESCRIPTOR_VERSION",
    "SLA_AGREEMENT_SIZE",
    "SLA_AGREEMENT_VERSION",
    "VK_REGISTRATION_HEADER_SIZE",
    "VK_REGISTRATION_LABEL_MAX",
    "VK_REGISTRATION_VERSION",
    "encode_payment_channel",
    "encode_service_descriptor",
    "encode_sla_agreement",
    "encode_vk_registration",
]
