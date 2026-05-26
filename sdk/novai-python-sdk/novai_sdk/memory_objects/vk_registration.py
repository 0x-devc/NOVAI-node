"""VkRegistration memory object (type 13, Week 30).

Variable wire layout::

    version:1
    proof_type:1
    code_hash:32
    label_len:1                  (<= 32)
    vk_len_be:4
    label:label_len
    vk_bytes:vk_len

Header overhead is 39 bytes; total = 39 + label_len + vk_len. The label is
human-readable text (UTF-8 recommended; the chain stores arbitrary bytes).
"""

from __future__ import annotations

from novai_sdk._hex import coerce_hash32
from novai_sdk.constants import PROOF_SUBMISSION_MAX_VK_BYTES
from novai_sdk.enums import ProofType

VK_REGISTRATION_VERSION: int = 1
VK_REGISTRATION_HEADER_SIZE: int = 39
VK_REGISTRATION_LABEL_MAX: int = 32


def encode_vk_registration(
    *,
    proof_type: ProofType | int,
    code_hash: bytes | str,
    label: bytes | str,
    vk_bytes: bytes,
) -> bytes:
    """Encode a VkRegistration data block (variable length).

    The result is the inner ``data`` payload for a CreateMemoryObject tx of
    object_type ``VK_REGISTRATION (13)``. At create time the runtime requires
    ``proof_type == GROTH16 (1)``; ``proof_type == 3 (GROTH16_REGISTERED)`` is
    only used at signal-submission time.
    """
    pt = int(proof_type)
    if not 0 <= pt <= 0xFF:
        raise ValueError("proof_type must fit in u8")
    label_bytes = label.encode("utf-8") if isinstance(label, str) else label
    if len(label_bytes) > VK_REGISTRATION_LABEL_MAX:
        raise ValueError(
            f"label must be <= {VK_REGISTRATION_LABEL_MAX} bytes, got {len(label_bytes)}"
        )
    if not vk_bytes:
        raise ValueError("vk_bytes must be non-empty")
    if len(vk_bytes) > PROOF_SUBMISSION_MAX_VK_BYTES:
        raise ValueError(f"vk_bytes must be <= {PROOF_SUBMISSION_MAX_VK_BYTES} bytes")
    code = coerce_hash32(code_hash, field="code_hash")
    out = bytearray()
    out.append(VK_REGISTRATION_VERSION)
    out.append(pt)
    out.extend(code)
    out.append(len(label_bytes))
    out.extend(len(vk_bytes).to_bytes(4, "big"))
    out.extend(label_bytes)
    out.extend(vk_bytes)
    return bytes(out)
