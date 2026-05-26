"""Example 03: Post an oracle data anchor (signal type 22, Week 35).

Requires the issuing entity to hold the ``post_oracle_anchors`` capability
(bit 6) which the ``Capabilities.oracle()`` shortcut grants.

    python examples/03_post_oracle_anchor.py
"""

from __future__ import annotations

import os
import time

import blake3

from novai_sdk import Keypair, NOVAIClient, compute_entity_id


def main() -> None:
    endpoint = os.environ.get("NOVAI_ENDPOINT", "http://localhost:3030")
    client = NOVAIClient(endpoint)

    kp = Keypair.load("example_01.key")
    code_hash = bytes([0x42] * 32)
    entity_id = compute_entity_id(code_hash, kp.address)
    print(f"posting anchor as entity {entity_id.hex()}")

    # The data we are committing to. In production this would be a real
    # off-chain payload (price feed reading, model output, etc.); here we
    # commit to a synthetic snapshot keyed by current wall clock.
    snapshot = f"ETH-USD@{time.time():.0f}=4321.50".encode()
    data_hash = blake3.blake3(snapshot).digest()
    print(f"  data:      {snapshot!r}")
    print(f"  data_hash: {data_hash.hex()}")

    result = client.post_oracle_anchor(
        keypair=kp,
        issuer_entity_id=entity_id,
        data_hash=data_hash,
        external_timestamp=int(time.time()),
        data_tag="price/ETH-USD",
        # source_hash and expiry_height are optional.
    )
    print("\nanchor posted:")
    print(f"  txid:        {result.txid}")
    print(f"  signal_hash: {result.signal_hash}")

    print("\nresolving anchor by signal_hash ...")
    if result.signal_hash is None:
        print("  signal_hash missing in result; not expected")
        return
    anchor = client.get_oracle_anchor(result.signal_hash)
    if anchor is None:
        print("  not yet in state; mempool may still be pending")
        return
    print(f"  data_tag:    {anchor.data_tag}")
    print(f"  timestamp:   {anchor.external_timestamp}")
    print(f"  data_hash:   {anchor.data_hash}")
    print(f"  anchor_height: {anchor.anchor_height}")


if __name__ == "__main__":
    main()
