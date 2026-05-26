"""Signal type 13: ProofSubmission.

Stub (proof_type=0) extras layout (65 bytes)::

    [proof_type:1][code_hash:32][computation_hash:32]

V2 (proof_type in {1, 3}) extras layout (65 + 4 + vk_len + 4 + proof_len)::

    [proof_type:1][code_hash:32][computation_hash:32]
    [vk_len_be:4][vk_bytes:vk_len][proof_len_be:4][proof_bytes:proof_len]

For ``proof_type == PROOF_TYPE_GROTH16_REGISTERED (3)`` the ``vk_bytes`` field
holds the 32-byte registry handle (the memory object ID of a previously
registered VK), not the inline verifying key. The runtime enforces
``vk_len == 32`` in that case.
"""

from __future__ import annotations

from novai_sdk._hex import coerce_hash32
from novai_sdk.constants import PROOF_SUBMISSION_MAX_PROOF_BYTES, PROOF_SUBMISSION_MAX_VK_BYTES
from novai_sdk.enums import ProofType


def build_proof_submission_extras_v1_stub(
    code_hash: bytes | str,
    computation_hash: bytes | str,
) -> bytes:
    """Build the ProofSubmission v1 (stub) extras (65 bytes).

    Use for development / smoke tests. The runtime accepts stub proofs only
    when ``proof_type == 0``.
    """
    code = coerce_hash32(code_hash, field="code_hash")
    comp = coerce_hash32(computation_hash, field="computation_hash")
    return bytes([int(ProofType.STUB)]) + code + comp


def build_proof_submission_extras_groth16(
    code_hash: bytes | str,
    computation_hash: bytes | str,
    vk_bytes: bytes,
    proof_bytes: bytes,
) -> bytes:
    """Build the inline-VK Groth16 ProofSubmission extras (variable).

    Use for ``proof_type == PROOF_TYPE_GROTH16 (1)``. The verifying key is
    serialized inline; for high-traffic publishers, prefer
    ``build_proof_submission_extras_groth16_registered`` which references a
    pre-registered VK by its 32-byte handle.
    """
    if len(vk_bytes) > PROOF_SUBMISSION_MAX_VK_BYTES:
        raise ValueError(f"vk_bytes exceeds {PROOF_SUBMISSION_MAX_VK_BYTES} bytes")
    if len(proof_bytes) > PROOF_SUBMISSION_MAX_PROOF_BYTES:
        raise ValueError(f"proof_bytes exceeds {PROOF_SUBMISSION_MAX_PROOF_BYTES} bytes")
    code = coerce_hash32(code_hash, field="code_hash")
    comp = coerce_hash32(computation_hash, field="computation_hash")
    return (
        bytes([int(ProofType.GROTH16)])
        + code
        + comp
        + len(vk_bytes).to_bytes(4, "big")
        + vk_bytes
        + len(proof_bytes).to_bytes(4, "big")
        + proof_bytes
    )


def build_proof_submission_extras_groth16_registered(
    code_hash: bytes | str,
    computation_hash: bytes | str,
    vk_id: bytes | str,
    proof_bytes: bytes,
) -> bytes:
    """Build the registered-VK Groth16 ProofSubmission extras (variable).

    Use for ``proof_type == PROOF_TYPE_GROTH16_REGISTERED (3)``. The ``vk_id``
    is the 32-byte memory object ID of a previously created VkRegistration
    memory object; the runtime resolves it server-side. The ``code_hash``
    must match the one stored in the registered VK.
    """
    if len(proof_bytes) > PROOF_SUBMISSION_MAX_PROOF_BYTES:
        raise ValueError(f"proof_bytes exceeds {PROOF_SUBMISSION_MAX_PROOF_BYTES} bytes")
    code = coerce_hash32(code_hash, field="code_hash")
    comp = coerce_hash32(computation_hash, field="computation_hash")
    vk = coerce_hash32(vk_id, field="vk_id")
    return (
        bytes([int(ProofType.GROTH16_REGISTERED)])
        + code
        + comp
        + (32).to_bytes(4, "big")
        + vk
        + len(proof_bytes).to_bytes(4, "big")
        + proof_bytes
    )
