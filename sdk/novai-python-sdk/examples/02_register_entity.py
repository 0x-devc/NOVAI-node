"""Example 02: Register an AI entity with oracle capabilities.

Uses a deterministic dummy code hash so the example is reproducible. In
production replace ``code_hash`` with the blake3 of the module's code +
weights, computed by your build pipeline.

    python examples/02_register_entity.py
"""

from __future__ import annotations

import os

from novai_sdk import AutonomyMode, Capabilities, Keypair, NOVAIClient


def main() -> None:
    endpoint = os.environ.get("NOVAI_ENDPOINT", "http://localhost:3030")
    client = NOVAIClient(endpoint)

    kp = Keypair.load("example_01.key")
    print(f"creator address: {kp.address_hex}")

    code_hash = bytes([0x42] * 32)
    print(f"code_hash:       {code_hash.hex()}")

    result = client.register_entity(
        keypair=kp,
        code_hash=code_hash,
        capabilities=Capabilities.oracle(),  # bits 0,1,2,6
        autonomy_mode=AutonomyMode.GATED,
        initial_balance=1_000_000,
    )
    print("\nregistered:")
    print(f"  txid:      {result.txid}")
    print(f"  entity_id: {result.entity_id}")

    print("\nverifying on chain ...")
    info = client.get_ai_entity(result.entity_id)
    if info is None:
        print("  entity not yet in state; mempool may still be pending")
        return
    print(f"  code_hash:        {info.code_hash}")
    print(f"  autonomy_mode:    {info.autonomy_mode}")
    print(f"  capabilities:    0b{info.capabilities:08b}")
    print(f"  economic_balance: {info.economic_balance}")
    print(f"  reputation_score: {info.reputation_score}")
    print(f"  is_active:        {info.is_active}")


if __name__ == "__main__":
    main()
