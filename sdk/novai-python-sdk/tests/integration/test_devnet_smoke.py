"""End-to-end smoke test against a running NOVAI devnet.

Marked with the ``integration`` pytest marker so the default ``pytest`` run
skips it. To execute:

    pytest -m integration

Requires the devnet to be running on the endpoint pointed to by the
``NOVAI_ENDPOINT`` env var (defaults to ``http://localhost:3030``) with the
faucet enabled (``--dev-keys`` or ``--faucet-key <path>``).

These tests are intentionally tolerant: each one waits briefly for the
mempool to process the prior tx, and uses fresh randomness to avoid
colliding with previous runs.
"""

from __future__ import annotations

import os
import secrets
import time

import blake3
import pytest

from novai_sdk import (
    AutonomyMode,
    Capabilities,
    Keypair,
    NonceTooLowError,
    NOVAIClient,
    PaymentCondition,
    PaymentSplit,
)

pytestmark = pytest.mark.integration


ENDPOINT = os.environ.get("NOVAI_ENDPOINT", "http://localhost:3030")
SETTLE_DELAY_SECONDS = 1.5


def _wait_for_settle() -> None:
    """Give the mempool / block production a moment to advance state."""
    time.sleep(SETTLE_DELAY_SECONDS)


@pytest.fixture(scope="module")
def client() -> NOVAIClient:
    return NOVAIClient(ENDPOINT)


@pytest.fixture(scope="module")
def funded_keypair(client: NOVAIClient) -> Keypair:
    """A keypair pre-funded by the dev faucet."""
    kp = Keypair.generate()
    client.faucet(kp.address)
    _wait_for_settle()
    bal = client.get_balance(kp.address)
    assert int(bal.balance) > 0, f"faucet did not fund {kp.address_hex} (balance={bal.balance})"
    return kp


def test_faucet_and_balance(client: NOVAIClient) -> None:
    kp = Keypair.generate()
    result = client.faucet(kp.address)
    assert result.txid
    assert int(result.amount) > 0
    _wait_for_settle()
    balance = client.get_balance(kp.address)
    assert int(balance.balance) >= int(result.amount)


def test_get_latest_block(client: NOVAIClient) -> None:
    block = client.get_latest_block()
    # On a fresh devnet this could be None for a brief window; assert it is
    # eventually populated.
    if block is None:
        _wait_for_settle()
        block = client.get_latest_block()
    assert block is not None
    assert block.height >= 0
    assert len(block.block_hash) == 64


def test_transfer_between_addresses(
    client: NOVAIClient, funded_keypair: Keypair
) -> None:
    bob = Keypair.generate()
    result = client.transfer(funded_keypair, bob.address, amount=1_000)
    assert result.txid
    _wait_for_settle()
    bob_balance = client.get_balance(bob.address)
    assert int(bob_balance.balance) >= 1_000


def test_register_entity_and_query(
    client: NOVAIClient, funded_keypair: Keypair
) -> None:
    code_hash = secrets.token_bytes(32)
    reg = client.register_entity(
        keypair=funded_keypair,
        code_hash=code_hash,
        capabilities=Capabilities.oracle(),
        autonomy_mode=AutonomyMode.GATED,
        initial_balance=10_000,
    )
    assert reg.entity_id
    _wait_for_settle()
    info = client.get_ai_entity(reg.entity_id)
    assert info is not None
    assert info.code_hash == code_hash.hex()
    assert info.is_active is True
    assert info.capabilities & (1 << 6), "post_oracle_anchors bit not set"


def test_oracle_anchor_round_trip(
    client: NOVAIClient, funded_keypair: Keypair
) -> None:
    # Fresh entity per test to keep nonce / state isolated.
    code_hash = secrets.token_bytes(32)
    reg = client.register_entity(
        keypair=funded_keypair,
        code_hash=code_hash,
        capabilities=Capabilities.oracle(),
        autonomy_mode=AutonomyMode.GATED,
        initial_balance=10_000,
    )
    assert reg.entity_id is not None
    entity_id = bytes.fromhex(reg.entity_id)
    _wait_for_settle()

    snapshot = f"ETH-USD@{secrets.token_hex(8)}=4321.50".encode()
    data_hash = blake3.blake3(snapshot).digest()
    result = client.post_oracle_anchor(
        keypair=funded_keypair,
        issuer_entity_id=entity_id,
        data_hash=data_hash,
        external_timestamp=int(time.time()),
        data_tag="test/integration",
    )
    assert result.signal_hash is not None
    _wait_for_settle()
    anchor = client.get_oracle_anchor(result.signal_hash)
    assert anchor is not None
    assert anchor.data_hash == data_hash.hex()
    assert anchor.data_tag == "test/integration"


def test_conditional_payment_succeeds_when_anchor_matches(
    client: NOVAIClient, funded_keypair: Keypair
) -> None:
    code_hash = secrets.token_bytes(32)
    reg = client.register_entity(
        keypair=funded_keypair,
        code_hash=code_hash,
        capabilities=Capabilities.oracle(),
        autonomy_mode=AutonomyMode.GATED,
        initial_balance=100_000,
    )
    assert reg.entity_id is not None
    entity_id = bytes.fromhex(reg.entity_id)
    _wait_for_settle()

    data_hash = secrets.token_bytes(32)
    anchor_result = client.post_oracle_anchor(
        keypair=funded_keypair,
        issuer_entity_id=entity_id,
        data_hash=data_hash,
        external_timestamp=int(time.time()),
        data_tag="test/cond-payment",
    )
    assert anchor_result.signal_hash is not None
    _wait_for_settle()

    latest = client.get_latest_block()
    deadline = (latest.height if latest else 0) + 100
    pay_result = client.pay(
        keypair=funded_keypair,
        issuer_entity_id=entity_id,
        payee=bytes([0xAA] * 32),
        amount=500,
        signal_hash=secrets.token_bytes(32),
        service_descriptor_hash=secrets.token_bytes(32),
        request_hash=secrets.token_bytes(32),
        max_block_height=deadline,
        condition=PaymentCondition.anchor_data_hash_equals(
            anchor_signal_hash=anchor_result.signal_hash,
            expected_data_hash=data_hash,
        ),
    )
    assert pay_result.txid


def test_multi_party_payment_with_splits(
    client: NOVAIClient, funded_keypair: Keypair
) -> None:
    code_hash = secrets.token_bytes(32)
    reg = client.register_entity(
        keypair=funded_keypair,
        code_hash=code_hash,
        capabilities=Capabilities.gated(),
        initial_balance=100_000,
    )
    assert reg.entity_id is not None
    entity_id = bytes.fromhex(reg.entity_id)
    _wait_for_settle()

    primary = bytes([0xAA] * 32)
    splits = [
        PaymentSplit(recipient_entity_id=primary, basis_points=6_000),
        PaymentSplit(recipient_entity_id=bytes([0xBB] * 32), basis_points=4_000),
    ]
    latest = client.get_latest_block()
    deadline = (latest.height if latest else 0) + 100
    pay_result = client.pay(
        keypair=funded_keypair,
        issuer_entity_id=entity_id,
        payee=primary,
        amount=1_000,
        signal_hash=secrets.token_bytes(32),
        service_descriptor_hash=secrets.token_bytes(32),
        request_hash=secrets.token_bytes(32),
        max_block_height=deadline,
        splits=splits,
    )
    assert pay_result.txid


def test_nonce_too_low_is_specific_exception(
    client: NOVAIClient, funded_keypair: Keypair
) -> None:
    """Submitting at nonce 0 (after at least one prior tx) must raise NonceTooLowError."""
    # Drain at least one nonce on this keypair by transferring to a fresh address.
    target = Keypair.generate()
    client.transfer(funded_keypair, target.address, amount=1)
    _wait_for_settle()

    with pytest.raises(NonceTooLowError):
        client.transfer(funded_keypair, target.address, amount=1, nonce=0)
