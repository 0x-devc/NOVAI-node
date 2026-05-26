"""Shared pytest fixtures."""

from __future__ import annotations

import pytest

from novai_sdk import Keypair


@pytest.fixture
def kp_alice() -> Keypair:
    """Deterministic keypair from a seed of 0x01 * 32."""
    return Keypair.from_seed(bytes([1] * 32))


@pytest.fixture
def kp_bob() -> Keypair:
    """Deterministic keypair from a seed of 0x02 * 32."""
    return Keypair.from_seed(bytes([2] * 32))
