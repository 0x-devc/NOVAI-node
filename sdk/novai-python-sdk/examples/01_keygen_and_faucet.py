"""Example 01: Generate a key, dispense from the faucet, check the balance.

Prerequisite: a running NOVAI devnet with the faucet enabled (start with
``--dev-keys`` or pass ``--faucet-key <path>``).

    python examples/01_keygen_and_faucet.py
"""

from __future__ import annotations

import os

from novai_sdk import Keypair, NOVAIClient


def main() -> None:
    endpoint = os.environ.get("NOVAI_ENDPOINT", "http://localhost:3030")
    client = NOVAIClient(endpoint)

    kp = Keypair.generate()
    print("new keypair:")
    print(f"  address: {kp.address_hex}")
    print(f"  pubkey:  {kp.pubkey_hex}")
    kp.save("example_01.key")
    print("  saved seed to example_01.key (0o600 on POSIX)")

    print(f"\ndispensing from faucet at {endpoint} ...")
    result = client.faucet(kp.address)
    print(f"  txid:   {result.txid}")
    print(f"  amount: {result.amount}")

    print("\nchecking balance ...")
    balance = client.get_balance(kp.address)
    print(f"  balance: {balance.balance}")
    print(f"  nonce:   {balance.nonce}")


if __name__ == "__main__":
    main()
