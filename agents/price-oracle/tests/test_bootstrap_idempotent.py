"""Idempotency contract for bootstrap.py.

Re-running on the same host must produce zero side effects once the
oracle is registered. Each step has its own skip-path; this file exercises
them in isolation and end-to-end.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

import pytest
from novai_sdk import Keypair, RateLimitedError

import bootstrap
from lib.chain import POST_ORACLE_ANCHORS_BIT, EntityStatus


@dataclass
class _FaucetResult:
    txid: str = "deadbeef"
    amount: str = "100000"


@dataclass
class _SubmissionResult:
    txid: str = "abc123"
    entity_id: str = ""


@dataclass
class FakeChain:
    address_to_balance: dict[bytes, int] = field(default_factory=dict)
    address_to_status: dict[bytes, EntityStatus] = field(default_factory=dict)
    faucet_calls: list[bytes] = field(default_factory=list)
    register_calls: list[bytes] = field(default_factory=list)
    faucet_raises: Optional[BaseException] = None
    faucet_credit: int = 100_000
    register_then_caps: int = POST_ORACLE_ANCHORS_BIT | 0x07  # 0x47

    endpoint: str = "http://fake"

    def entity_id_for(self, address: bytes) -> bytes:
        # The real compute_entity_id is deterministic; for the fake we just
        # blake-of-address-prefix. Tests do not compare entity_id bytes to
        # the canonical derivation, only consistency within one run.
        return b"E" * 32

    def get_balance(self, address: bytes) -> int:
        return self.address_to_balance.get(address, 0)

    def get_entity_status(self, entity_id: bytes) -> EntityStatus:
        return self.address_to_status.get(
            entity_id, EntityStatus(False, False, 0, entity_id, entity_id.hex())
        )

    def faucet(self, address: bytes) -> _FaucetResult:
        self.faucet_calls.append(address)
        if self.faucet_raises is not None:
            raise self.faucet_raises
        self.address_to_balance[address] = (
            self.address_to_balance.get(address, 0) + self.faucet_credit
        )
        return _FaucetResult()

    def register_oracle(self, kp: Keypair) -> _SubmissionResult:
        self.register_calls.append(kp.address)
        # Simulate the chain accepting the register: next get_entity_status
        # call returns an entity with the post_oracle_anchors capability.
        eid = self.entity_id_for(kp.address)
        self.address_to_status[eid] = EntityStatus(
            exists=True,
            has_post_oracle_anchors=bool(self.register_then_caps & POST_ORACLE_ANCHORS_BIT),
            capabilities=self.register_then_caps,
            entity_id=eid,
            entity_id_hex=eid.hex(),
        )
        return _SubmissionResult(entity_id=eid.hex())

    def latest_block_height(self) -> Optional[int]:
        return 100


@pytest.fixture
def tmp_key(tmp_path: Path) -> Path:
    return tmp_path / "oracle-keys.json"


def _no_sleep(_secs: float) -> None:
    pass


class _StepClock:
    def __init__(self) -> None:
        self.t = 0.0

    def __call__(self) -> float:
        self.t += 0.5
        return self.t


def test_load_or_generate_creates_key_on_first_run(tmp_key: Path):
    kp, kf, generated = bootstrap.load_or_generate_key(tmp_key)
    assert generated is True
    assert tmp_key.exists()
    data = json.loads(tmp_key.read_text())
    assert data["seed_hex"] == kp.seed.hex()
    assert data["address_hex"] == kp.address.hex()
    assert (tmp_key.stat().st_mode & 0o777) == 0o600


def test_load_or_generate_reuses_existing_key(tmp_key: Path):
    kp1, _, generated1 = bootstrap.load_or_generate_key(tmp_key)
    kp2, _, generated2 = bootstrap.load_or_generate_key(tmp_key)
    assert generated1 is True
    assert generated2 is False
    assert kp1.seed == kp2.seed
    assert kp1.address == kp2.address


def test_load_or_generate_rejects_address_mismatch(tmp_key: Path):
    kp = Keypair.generate()
    tmp_key.parent.mkdir(parents=True, exist_ok=True)
    tmp_key.write_text(
        json.dumps(
            {
                "version": 1,
                "seed_hex": kp.seed.hex(),
                "pubkey_hex": kp.pubkey.hex(),
                "address_hex": "00" * 32,
            }
        )
    )
    with pytest.raises(ValueError):
        bootstrap.load_or_generate_key(tmp_key)


def test_ensure_funded_skips_when_already_funded():
    chain = FakeChain()
    addr = b"A" * 32
    chain.address_to_balance[addr] = bootstrap.MIN_BALANCE_FOR_REGISTER
    balance = bootstrap.ensure_funded(chain, addr, sleep_fn=_no_sleep, now_fn=_StepClock())
    assert balance == bootstrap.MIN_BALANCE_FOR_REGISTER
    assert chain.faucet_calls == []


def test_ensure_funded_calls_faucet_and_polls_until_funded():
    chain = FakeChain()
    addr = b"A" * 32
    chain.address_to_balance[addr] = 0  # below threshold
    balance = bootstrap.ensure_funded(chain, addr, sleep_fn=_no_sleep, now_fn=_StepClock())
    assert balance >= bootstrap.MIN_BALANCE_FOR_REGISTER
    assert chain.faucet_calls == [addr]


def test_ensure_funded_exits_on_cooldown_and_low_balance():
    chain = FakeChain()
    addr = b"A" * 32
    chain.address_to_balance[addr] = 0
    chain.faucet_raises = RateLimitedError(-32000, "rate limit: retry in 3600s")
    with pytest.raises(SystemExit) as info:
        bootstrap.ensure_funded(chain, addr, sleep_fn=_no_sleep, now_fn=_StepClock())
    assert info.value.code == 3


def test_ensure_funded_tolerates_cooldown_when_balance_sufficient():
    chain = FakeChain()
    addr = b"A" * 32
    chain.address_to_balance[addr] = bootstrap.MIN_BALANCE_TO_OPERATE + 10
    chain.faucet_raises = RateLimitedError(-32000, "rate limit: retry in 3600s")
    balance = bootstrap.ensure_funded(chain, addr, sleep_fn=_no_sleep, now_fn=_StepClock())
    assert balance == chain.address_to_balance[addr]


def test_ensure_registered_skips_when_already_registered_with_bit_6():
    chain = FakeChain()
    kp = Keypair.generate()
    eid = chain.entity_id_for(kp.address)
    chain.address_to_status[eid] = EntityStatus(
        True, True, 0x47, eid, eid.hex()
    )
    entity_id, caps = bootstrap.ensure_registered(
        chain, kp, sleep_fn=_no_sleep, now_fn=_StepClock()
    )
    assert entity_id == eid
    assert caps == 0x47
    assert chain.register_calls == []


def test_ensure_registered_exits_when_entity_exists_without_bit_6():
    chain = FakeChain()
    kp = Keypair.generate()
    eid = chain.entity_id_for(kp.address)
    chain.address_to_status[eid] = EntityStatus(True, False, 0x07, eid, eid.hex())
    with pytest.raises(SystemExit) as info:
        bootstrap.ensure_registered(chain, kp, sleep_fn=_no_sleep, now_fn=_StepClock())
    assert info.value.code == 4


def test_ensure_registered_submits_and_verifies_when_absent():
    chain = FakeChain()
    kp = Keypair.generate()
    entity_id, caps = bootstrap.ensure_registered(
        chain, kp, sleep_fn=_no_sleep, now_fn=_StepClock()
    )
    assert chain.register_calls == [kp.address]
    assert caps & POST_ORACLE_ANCHORS_BIT


def test_ensure_registered_exits_when_register_lands_without_bit_6():
    chain = FakeChain()
    chain.register_then_caps = 0x07  # missing bit 6
    kp = Keypair.generate()
    with pytest.raises(SystemExit) as info:
        bootstrap.ensure_registered(chain, kp, sleep_fn=_no_sleep, now_fn=_StepClock())
    assert info.value.code == 4


def test_update_keyfile_writes_entity_id_and_caps(tmp_key: Path):
    kp, kf, _ = bootstrap.load_or_generate_key(tmp_key)
    bootstrap.update_keyfile(
        tmp_key,
        kf,
        entity_id=b"X" * 32,
        capabilities_byte=0x47,
        registered_at_unix=1717428000,
    )
    data = json.loads(tmp_key.read_text())
    assert data["entity_id_hex"] == ("58" * 32)
    assert data["capabilities_byte"] == 0x47
    assert data["registered_at_unix"] == 1717428000


def test_full_bootstrap_is_idempotent_end_to_end(tmp_key: Path, monkeypatch):
    chain = FakeChain()

    def fake_ctor(endpoint: str):
        return chain

    monkeypatch.setattr(bootstrap, "Chain", fake_ctor)
    monkeypatch.setenv("PRICE_ORACLE_KEY_PATH", str(tmp_key))
    monkeypatch.setenv("PRICE_ORACLE_RPC_ENDPOINT", "http://fake")

    # First run: keypair generated, faucet called, register called.
    bootstrap.main([])
    assert tmp_key.exists()
    data = json.loads(tmp_key.read_text())
    assert len(chain.faucet_calls) == 1
    assert len(chain.register_calls) == 1

    # Second run: everything skipped.
    bootstrap.main([])
    assert len(chain.faucet_calls) == 1
    assert len(chain.register_calls) == 1
    data2 = json.loads(tmp_key.read_text())
    assert data2["seed_hex"] == data["seed_hex"]
