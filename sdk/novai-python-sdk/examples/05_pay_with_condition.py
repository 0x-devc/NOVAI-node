"""Example 05: Conditional payment gated on an oracle anchor (Week 36).

The payment is only released if the referenced anchor matches the
expected data hash. If the condition fails the entire transaction reverts
with nothing charged.

    python examples/05_pay_with_condition.py
"""

from __future__ import annotations

import os
import secrets
import time

import blake3

from novai_sdk import (
    Keypair,
    NOVAIClient,
    PaymentCondition,
    compute_entity_id,
)


def main() -> None:
    endpoint = os.environ.get("NOVAI_ENDPOINT", "http://localhost:3030")
    client = NOVAIClient(endpoint)

    payer_kp = Keypair.load("example_01.key")
    payer_entity_id = compute_entity_id(bytes([0x42] * 32), payer_kp.address)

    # Step 1: post an oracle anchor we'll later condition a payment on.
    snapshot = f"ETH-USD@{time.time():.0f}=4321.50".encode()
    data_hash = blake3.blake3(snapshot).digest()
    anchor_result = client.post_oracle_anchor(
        keypair=payer_kp,
        issuer_entity_id=payer_entity_id,
        data_hash=data_hash,
        external_timestamp=int(time.time()),
        data_tag="price/ETH-USD",
    )
    assert anchor_result.signal_hash is not None
    anchor_signal_hash = bytes.fromhex(anchor_result.signal_hash)
    print(f"anchor posted: {anchor_result.signal_hash}")

    # Step 2: pay another agent, gated on that anchor's data_hash matching.
    latest = client.get_latest_block()
    deadline = (latest.height if latest else 0) + 100
    payee = bytes([0xAA] * 32)
    pay_result = client.pay(
        keypair=payer_kp,
        issuer_entity_id=payer_entity_id,
        payee=payee,
        amount=1_000,
        signal_hash=secrets.token_bytes(32),
        service_descriptor_hash=bytes([0x33] * 32),
        request_hash=secrets.token_bytes(32),
        max_block_height=deadline,
        condition=PaymentCondition.anchor_data_hash_equals(
            anchor_signal_hash=anchor_signal_hash,
            expected_data_hash=data_hash,
        ),
    )
    print(f"\npaid conditionally: txid={pay_result.txid}")
    print(f"  release gated on anchor {anchor_signal_hash.hex()}")
    print(f"  expected data hash:     {data_hash.hex()}")


if __name__ == "__main__":
    main()
