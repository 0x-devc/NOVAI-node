"""Transaction payload builders for the 11 NOVAI tx types.

Each builder returns the raw payload bytes (the inside of a TxV1 envelope's
``payload`` field). To produce a signed tx, wrap the payload in a ``TxV1``
and call ``sign_tx_v1``::

    from novai_sdk import TxV1, sign_tx_v1
    from novai_sdk.tx import build_transfer_payload

    payload = build_transfer_payload(to_addr, amount=5000)
    tx = TxV1(from_address=kp.address, pubkey=kp.pubkey, nonce=n, fee=100, payload=payload)
    tx.sig = sign_tx_v1(kp.signing_key, tx)
"""

from novai_sdk.tx.entities import (
    build_credit_entity_payload,
    build_entity_upgrade_payload,
    build_register_entity_payload,
    build_register_with_key_payload,
)
from novai_sdk.tx.governance import (
    build_execute_proposal_payload,
    build_submit_proposal_payload,
)
from novai_sdk.tx.memory import (
    build_create_memory_payload,
    build_delete_memory_payload,
    build_update_memory_payload,
)
from novai_sdk.tx.signal import build_signal_commitment_payload
from novai_sdk.tx.transfer import build_transfer_payload

__all__ = [
    "build_create_memory_payload",
    "build_credit_entity_payload",
    "build_delete_memory_payload",
    "build_entity_upgrade_payload",
    "build_execute_proposal_payload",
    "build_register_entity_payload",
    "build_register_with_key_payload",
    "build_signal_commitment_payload",
    "build_submit_proposal_payload",
    "build_transfer_payload",
    "build_update_memory_payload",
]
