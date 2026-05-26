"""Example 06: Full agent lifecycle.

Demonstrates the end-to-end flow an agent framework would run:

1. Generate a key and fund it.
2. Register an AI entity with oracle capabilities.
3. Post an oracle anchor.
4. Pay another agent with a Week 36 condition referencing that anchor.
5. Query the payment back.

Idempotency: each run generates fresh request hashes via ``secrets`` so
reruns don't collide with prior submissions.

    python examples/06_full_lifecycle.py
"""

from __future__ import annotations

import os
import secrets
import time

import blake3

from novai_sdk import (
    AutonomyMode,
    Capabilities,
    Keypair,
    NOVAIClient,
    PaymentCondition,
)


def main() -> None:
    endpoint = os.environ.get("NOVAI_ENDPOINT", "http://localhost:3030")
    client = NOVAIClient(endpoint)

    # --------------------------------------------------------------
    # 1. Key + faucet
    # --------------------------------------------------------------
    kp = Keypair.generate()
    print(f"creator address: {kp.address_hex}")
    faucet = client.faucet(kp.address)
    print(f"faucet: amount={faucet.amount}")

    # --------------------------------------------------------------
    # 2. Register entity
    # --------------------------------------------------------------
    code_hash = bytes([0x42] * 32)
    reg = client.register_entity(
        keypair=kp,
        code_hash=code_hash,
        capabilities=Capabilities.oracle(),
        autonomy_mode=AutonomyMode.GATED,
        initial_balance=1_000_000,
    )
    print(f"entity registered: {reg.entity_id}")
    assert reg.entity_id is not None
    entity_id = bytes.fromhex(reg.entity_id)

    # --------------------------------------------------------------
    # 3. Post oracle anchor
    # --------------------------------------------------------------
    snapshot = f"ETH-USD@{time.time():.0f}=4321.50".encode()
    data_hash = blake3.blake3(snapshot).digest()
    anchor = client.post_oracle_anchor(
        keypair=kp,
        issuer_entity_id=entity_id,
        data_hash=data_hash,
        external_timestamp=int(time.time()),
        data_tag="price/ETH-USD",
    )
    assert anchor.signal_hash is not None
    print(f"anchor posted: {anchor.signal_hash}")
    anchor_id = bytes.fromhex(anchor.signal_hash)

    # --------------------------------------------------------------
    # 4. Conditional payment
    # --------------------------------------------------------------
    latest = client.get_latest_block()
    deadline = (latest.height if latest else 0) + 100
    payee = bytes([0xAA] * 32)
    pay = client.pay(
        keypair=kp,
        issuer_entity_id=entity_id,
        payee=payee,
        amount=500,
        signal_hash=secrets.token_bytes(32),
        service_descriptor_hash=bytes([0x33] * 32),
        request_hash=secrets.token_bytes(32),
        max_block_height=deadline,
        condition=PaymentCondition.anchor_data_hash_equals(
            anchor_signal_hash=anchor_id,
            expected_data_hash=data_hash,
        ),
    )
    print(f"payment submitted: {pay.txid}")

    # --------------------------------------------------------------
    # 5. Query the payment back
    # --------------------------------------------------------------
    payments = client.get_payments_by_entity(
        entity_id, role="payer", start_height=0, end_height=deadline
    )
    print(f"payer history: {len(payments)} payment(s)")
    for p in payments:
        condition_kind = p.condition.kind if p.condition else "none"
        print(f"  height={p.payment_height} amount={p.amount} condition={condition_kind}")


if __name__ == "__main__":
    main()
