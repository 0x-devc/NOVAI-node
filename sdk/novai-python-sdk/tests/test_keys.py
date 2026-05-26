"""Tests for novai_sdk.keys (Keypair)."""

from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

from novai_sdk import Keypair, address_from_pubkey


class TestGenerate:
    def test_returns_valid_keypair(self) -> None:
        kp = Keypair.generate()
        assert len(kp.seed) == 32
        assert len(kp.pubkey) == 32
        assert len(kp.address) == 32

    def test_each_generate_is_unique(self) -> None:
        kp1 = Keypair.generate()
        kp2 = Keypair.generate()
        assert kp1.seed != kp2.seed
        assert kp1.address != kp2.address


class TestFromSeed:
    def test_deterministic(self) -> None:
        seed = bytes([7] * 32)
        kp1 = Keypair.from_seed(seed)
        kp2 = Keypair.from_seed(seed)
        assert kp1.seed == kp2.seed
        assert kp1.pubkey == kp2.pubkey
        assert kp1.address == kp2.address

    def test_seed_is_preserved(self) -> None:
        seed = bytes(range(32))
        kp = Keypair.from_seed(seed)
        assert kp.seed == seed

    def test_address_matches_address_from_pubkey(self) -> None:
        kp = Keypair.from_seed(bytes([1] * 32))
        assert kp.address == address_from_pubkey(kp.pubkey)

    @pytest.mark.parametrize("bad_len", [0, 16, 31, 33, 64])
    def test_rejects_wrong_seed_length(self, bad_len: int) -> None:
        with pytest.raises(ValueError, match="seed must be exactly 32 bytes"):
            Keypair.from_seed(bytes(bad_len))


class TestSaveLoad:
    def test_roundtrip(self, tmp_path: Path) -> None:
        kp = Keypair.from_seed(bytes([42] * 32))
        key_file = tmp_path / "test.key"
        kp.save(key_file)
        loaded = Keypair.load(key_file)
        assert loaded.seed == kp.seed
        assert loaded.address == kp.address

    def test_save_writes_raw_32_bytes(self, tmp_path: Path) -> None:
        kp = Keypair.from_seed(bytes([1] * 32))
        key_file = tmp_path / "raw.key"
        kp.save(key_file)
        assert key_file.read_bytes() == bytes([1] * 32)

    @pytest.mark.skipif(sys.platform == "win32", reason="POSIX permissions only")
    def test_save_sets_permissions_0o600(self, tmp_path: Path) -> None:
        kp = Keypair.from_seed(bytes([1] * 32))
        key_file = tmp_path / "secured.key"
        kp.save(key_file)
        mode = os.stat(key_file).st_mode & 0o777
        assert mode == 0o600

    def test_load_rejects_short_file(self, tmp_path: Path) -> None:
        bad = tmp_path / "short.key"
        bad.write_bytes(b"\x00" * 16)
        with pytest.raises(ValueError, match="must be exactly 32 bytes"):
            Keypair.load(bad)


class TestSign:
    def test_sign_returns_64_bytes(self) -> None:
        kp = Keypair.from_seed(bytes([1] * 32))
        sig = kp.sign(b"hello")
        assert len(sig) == 64

    def test_sign_is_deterministic(self) -> None:
        """ed25519 signatures are deterministic for a given seed and message."""
        kp = Keypair.from_seed(bytes([1] * 32))
        sig1 = kp.sign(b"payload")
        sig2 = kp.sign(b"payload")
        assert sig1 == sig2

    def test_different_messages_different_signatures(self) -> None:
        kp = Keypair.from_seed(bytes([1] * 32))
        assert kp.sign(b"a") != kp.sign(b"b")


class TestRepr:
    def test_repr_does_not_leak_seed(self) -> None:
        kp = Keypair.from_seed(bytes([1] * 32))
        rep = repr(kp)
        assert kp.seed.hex() not in rep
        assert kp.address_hex in rep
