"""Governance tx payload builders (tx types 6, 7)."""

from __future__ import annotations

from novai_sdk._hex import coerce_hash32
from novai_sdk.enums import TxPayloadType


def build_submit_proposal_payload(
    proposal_type: int,
    gate_id: bytes | str,
    proposal_data: bytes,
) -> bytes:
    """Build the tx-type-6 SubmitProposal payload.

    Layout: ``[0x06][proposal_type:1][gate_id:32][data_len_be:4][data:N]``.
    Total length 38 + N.
    """
    if not 0 <= proposal_type <= 0xFF:
        raise ValueError(f"proposal_type must fit in u8, got {proposal_type}")
    if not 0 <= len(proposal_data) < 2**32:
        raise ValueError("proposal_data length does not fit in u32")
    gate = coerce_hash32(gate_id, field="gate_id")
    return (
        bytes([int(TxPayloadType.SUBMIT_PROPOSAL)])
        + bytes([proposal_type])
        + gate
        + len(proposal_data).to_bytes(4, "big")
        + proposal_data
    )


def build_execute_proposal_payload(proposal_id: bytes | str) -> bytes:
    """Build the tx-type-7 ExecuteProposal payload (33 bytes fixed).

    Layout: ``[0x07][proposal_id:32]``.
    """
    pid = coerce_hash32(proposal_id, field="proposal_id")
    return bytes([int(TxPayloadType.EXECUTE_PROPOSAL)]) + pid
