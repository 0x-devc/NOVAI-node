"""Adversarial: prove DRY_RUN constructs valid bytes and never touches the RPC.

An exploding client records and raises on ANY attribute access (any RPC
method). The dry-run write paths must construct and sign locally without ever
reaching it, and the read/funding paths must raise DryRunError before touching
it.
"""

from __future__ import annotations

import pytest
from novai_sdk import Keypair
from novai_sdk.codec import decode_tx_v1_signed
from novai_sdk.enums import AiSignalType

from lib.chain import Chain, DryRunError, DryRunResult


class _ExplodingClient:
    """Any RPC method access is recorded and then raises.

    ``accessed`` lives in __dict__, so reading it inside __getattr__ does not
    recurse (only missing attributes trigger __getattr__).
    """

    def __init__(self) -> None:
        self.accessed: list[str] = []

    def __getattr__(self, name: str):
        self.accessed.append(name)
        raise AssertionError(f"dry-run reached the RPC client: {name}")


def test_post_anchor_dry_run_builds_valid_bytes_without_touching_client():
    client = _ExplodingClient()
    chain = Chain("http://unused", dry_run=True, client=client)
    kp = Keypair.generate()
    entity_id = chain.entity_id_for(kp.address)

    result = chain.post_anchor(
        kp,
        entity_id,
        bytes([0xAB] * 32),
        1_718_000_000,
        "compute/rtx4090-usd-hr",
        source_hash=bytes([0xCD] * 32),
        expiry_height=0,
        fee=1000,
    )

    assert isinstance(result, DryRunResult)
    assert client.accessed == []  # the client was never touched
    assert result.payload[0] == 2
    assert result.payload[33] == int(AiSignalType.ORACLE_ANCHOR)
    assert len(result.signal_hash) == 32
    assert len(result.txid) == 32
    # The signed transaction decodes and round-trips the payload and signer.
    tx = decode_tx_v1_signed(result.signed_tx)
    assert tx.payload == result.payload
    assert tx.from_address == kp.address
    assert tx.sig != bytes(64)


def test_reputation_dry_run_builds_valid_bytes_without_touching_client():
    client = _ExplodingClient()
    chain = Chain("http://unused", dry_run=True, client=client)
    kp = Keypair.generate()
    issuer = chain.entity_id_for(kp.address)

    result = chain.submit_reputation_update(kp, issuer, bytes([0x09] * 32), 3, 5, fee=1000)

    assert isinstance(result, DryRunResult)
    assert client.accessed == []
    assert len(result.payload) == 101
    assert result.payload[33] == int(AiSignalType.REPUTATION_UPDATE)


@pytest.mark.parametrize(
    "call",
    [
        lambda c, kp: c.get_balance(kp.address),
        lambda c, kp: c.get_account_nonce(kp.address),
        lambda c, kp: c.get_entity_economic_balance(c.entity_id_for(kp.address)),
        lambda c, kp: c.get_entity_status(c.entity_id_for(kp.address)),
        lambda c, kp: c.latest_block_height(),
        lambda c, kp: c.faucet(kp.address),
    ],
)
def test_reads_and_funding_raise_dry_run_error_without_touching_client(call):
    client = _ExplodingClient()
    chain = Chain("http://unused", dry_run=True, client=client)
    kp = Keypair.generate()
    with pytest.raises(DryRunError):
        call(chain, kp)
    assert client.accessed == []  # DryRunError fires before any client access


def test_dry_run_constructs_no_client_when_none_injected():
    chain = Chain("http://unused", dry_run=True)
    assert chain._client is None
