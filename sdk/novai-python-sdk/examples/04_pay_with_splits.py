"""Example 04: Multi-party payment using the Week 33 splits trailer.

The CLI's ``--split`` flag accepts ``2..=8`` recipients whose basis_points
must sum to exactly 10_000. The first recipient must equal the primary
``payee_entity_id``; the chain enforces this and so does the SDK
client-side.

    python examples/04_pay_with_splits.py
"""

from __future__ import annotations

import os
import secrets

from novai_sdk import Keypair, NOVAIClient, PaymentSplit, compute_entity_id


def main() -> None:
    endpoint = os.environ.get("NOVAI_ENDPOINT", "http://localhost:3030")
    client = NOVAIClient(endpoint)

    payer_kp = Keypair.load("example_01.key")
    payer_entity_id = compute_entity_id(bytes([0x42] * 32), payer_kp.address)

    # Three recipients sharing 50% / 30% / 20% of a 10_000-unit payment.
    primary = bytes([0xAA] * 32)
    operator = bytes([0xBB] * 32)
    referrer = bytes([0xCC] * 32)
    splits = [
        PaymentSplit(recipient_entity_id=primary, basis_points=5_000),
        PaymentSplit(recipient_entity_id=operator, basis_points=3_000),
        PaymentSplit(recipient_entity_id=referrer, basis_points=2_000),
    ]
    # Sanity-check the BPS sum locally:
    assert sum(s.basis_points for s in splits) == 10_000

    latest = client.get_latest_block()
    deadline = (latest.height if latest else 0) + 100

    result = client.pay(
        keypair=payer_kp,
        issuer_entity_id=payer_entity_id,
        payee=primary,
        amount=10_000,
        signal_hash=secrets.token_bytes(32),
        service_descriptor_hash=bytes([0x33] * 32),
        request_hash=secrets.token_bytes(32),
        max_block_height=deadline,
        splits=splits,
    )
    print(f"paid: txid={result.txid}")
    print("recipient distribution:")
    for s in splits:
        print(f"  {s.recipient_entity_id.hex()}: {s.basis_points} bps")


if __name__ == "__main__":
    main()
