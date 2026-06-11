"""Idempotency contract for bootstrap.py under the two-key Type-10 model.

Re-running on the same host must produce zero side effects once the
oracle is registered. Each step has its own skip-path; this file
exercises them in isolation and end-to-end.

Under v2 the keyfile holds two keypairs (funder + entity). The funder
signs account-level operations; the entity signs entity-level signals.
See docs/AGENT_FUNDING_PLAYBOOK.md for the reusable lifecycle.
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
    register_calls: list[tuple[bytes, bytes, int, int]] = field(default_factory=list)
    faucet_raises: Optional[BaseException] = None
    faucet_credit: int = 100_000
    register_then_caps: int = POST_ORACLE_ANCHORS_BIT | 0x07  # 0x47

    endpoint: str = "http://fake"

    def entity_id_for(self, address: bytes) -> bytes:
        # The real compute_entity_id is deterministic; for the fake I use
        # a constant per-test placeholder. Tests do not compare entity_id
        # bytes to the canonical derivation, only consistency within one
        # run.
        return b"E" * 32

    def get_balance(self, address: bytes) -> int:
        return self.address_to_balance.get(address, 0)

    def get_entity_status(self, entity_id: bytes) -> EntityStatus:
        return self.address_to_status.get(
            entity_id, EntityStatus(False, False, 0, entity_id, entity_id.hex())
        )

    def funder_is_unbound(self, address: bytes) -> bool:
        return not self.get_entity_status(self.entity_id_for(address)).exists

    def faucet(self, address: bytes) -> _FaucetResult:
        self.faucet_calls.append(address)
        if self.faucet_raises is not None:
            raise self.faucet_raises
        self.address_to_balance[address] = (
            self.address_to_balance.get(address, 0) + self.faucet_credit
        )
        return _FaucetResult()

    def register_oracle_with_key(
        self,
        funder_kp: Keypair,
        entity_pubkey: bytes,
        *,
        fee: int = 5_000,
        initial_balance: int = 0,
    ) -> _SubmissionResult:
        self.register_calls.append(
            (funder_kp.address, entity_pubkey, fee, initial_balance)
        )
        # Simulate the chain accepting the register: next get_entity_status
        # call returns an entity with the post_oracle_anchors capability.
        eid = self.entity_id_for(funder_kp.address)
        self.address_to_status[eid] = EntityStatus(
            exists=True,
            has_post_oracle_anchors=bool(
                self.register_then_caps & POST_ORACLE_ANCHORS_BIT
            ),
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


# -- load_or_generate_key ----------------------------------------------------


def test_load_or_generate_creates_two_keys_on_first_run(tmp_key: Path):
    funder_kp, entity_kp, kf, generated = bootstrap.load_or_generate_key(tmp_key)
    assert generated is True
    assert funder_kp.seed != entity_kp.seed
    assert funder_kp.address != entity_kp.address
    assert tmp_key.exists()
    data = json.loads(tmp_key.read_text())
    assert data["version"] == bootstrap.KEYFILE_VERSION == 2
    assert data["funder_seed_hex"] == funder_kp.seed.hex()
    assert data["funder_pubkey_hex"] == funder_kp.pubkey.hex()
    assert data["funder_address_hex"] == funder_kp.address.hex()
    assert data["entity_seed_hex"] == entity_kp.seed.hex()
    assert data["entity_pubkey_hex"] == entity_kp.pubkey.hex()
    assert data["entity_address_hex"] == entity_kp.address.hex()
    assert (tmp_key.stat().st_mode & 0o777) == 0o600


def test_load_or_generate_reuses_existing_keys(tmp_key: Path):
    f1, e1, _, gen1 = bootstrap.load_or_generate_key(tmp_key)
    f2, e2, _, gen2 = bootstrap.load_or_generate_key(tmp_key)
    assert gen1 is True
    assert gen2 is False
    assert f1.seed == f2.seed and f1.address == f2.address
    assert e1.seed == e2.seed and e1.address == e2.address


def test_load_or_generate_rejects_v1_keyfile(tmp_key: Path):
    kp = Keypair.generate()
    tmp_key.parent.mkdir(parents=True, exist_ok=True)
    tmp_key.write_text(
        json.dumps(
            {
                "version": 1,
                "seed_hex": kp.seed.hex(),
                "pubkey_hex": kp.pubkey.hex(),
                "address_hex": kp.address.hex(),
            }
        )
    )
    with pytest.raises(bootstrap.KeyFileVersionError):
        bootstrap.load_or_generate_key(tmp_key)


def test_load_or_generate_rejects_funder_address_mismatch(tmp_key: Path):
    funder = Keypair.generate()
    entity = Keypair.generate()
    tmp_key.parent.mkdir(parents=True, exist_ok=True)
    tmp_key.write_text(
        json.dumps(
            {
                "version": 2,
                "funder_seed_hex": funder.seed.hex(),
                "funder_pubkey_hex": funder.pubkey.hex(),
                "funder_address_hex": "00" * 32,
                "entity_seed_hex": entity.seed.hex(),
                "entity_pubkey_hex": entity.pubkey.hex(),
                "entity_address_hex": entity.address.hex(),
            }
        )
    )
    with pytest.raises(ValueError):
        bootstrap.load_or_generate_key(tmp_key)


def test_load_or_generate_rejects_entity_address_mismatch(tmp_key: Path):
    funder = Keypair.generate()
    entity = Keypair.generate()
    tmp_key.parent.mkdir(parents=True, exist_ok=True)
    tmp_key.write_text(
        json.dumps(
            {
                "version": 2,
                "funder_seed_hex": funder.seed.hex(),
                "funder_pubkey_hex": funder.pubkey.hex(),
                "funder_address_hex": funder.address.hex(),
                "entity_seed_hex": entity.seed.hex(),
                "entity_pubkey_hex": entity.pubkey.hex(),
                "entity_address_hex": "00" * 32,
            }
        )
    )
    with pytest.raises(ValueError):
        bootstrap.load_or_generate_key(tmp_key)


# -- ensure_funded -----------------------------------------------------------


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
    chain.address_to_balance[addr] = 0
    chain.faucet_credit = bootstrap.MIN_BALANCE_FOR_REGISTER + 10
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


# -- ensure_registered -------------------------------------------------------


def test_ensure_registered_skips_when_already_registered_with_bit_6():
    chain = FakeChain()
    funder = Keypair.generate()
    entity = Keypair.generate()
    eid = chain.entity_id_for(funder.address)
    chain.address_to_status[eid] = EntityStatus(
        True, True, 0x47, eid, eid.hex()
    )
    entity_id, caps = bootstrap.ensure_registered(
        chain, funder, entity, sleep_fn=_no_sleep, now_fn=_StepClock()
    )
    assert entity_id == eid
    assert caps == 0x47
    assert chain.register_calls == []


def test_ensure_registered_exits_when_entity_exists_without_bit_6():
    chain = FakeChain()
    funder = Keypair.generate()
    entity = Keypair.generate()
    eid = chain.entity_id_for(funder.address)
    chain.address_to_status[eid] = EntityStatus(True, False, 0x07, eid, eid.hex())
    with pytest.raises(SystemExit) as info:
        bootstrap.ensure_registered(
            chain, funder, entity, sleep_fn=_no_sleep, now_fn=_StepClock()
        )
    assert info.value.code == 4


def test_ensure_registered_submits_with_funder_and_entity_pubkey():
    chain = FakeChain()
    funder = Keypair.generate()
    entity = Keypair.generate()
    entity_id, caps = bootstrap.ensure_registered(
        chain, funder, entity, sleep_fn=_no_sleep, now_fn=_StepClock()
    )
    assert len(chain.register_calls) == 1
    funder_addr_used, entity_pubkey_used, fee_used, initial_used = chain.register_calls[0]
    assert funder_addr_used == funder.address
    assert entity_pubkey_used == entity.pubkey
    assert fee_used == bootstrap.REGISTER_FEE
    assert initial_used == bootstrap.INITIAL_ENTITY_BALANCE
    assert caps & POST_ORACLE_ANCHORS_BIT


def test_ensure_registered_exits_when_register_lands_without_bit_6():
    chain = FakeChain()
    chain.register_then_caps = 0x07
    funder = Keypair.generate()
    entity = Keypair.generate()
    with pytest.raises(SystemExit) as info:
        bootstrap.ensure_registered(
            chain, funder, entity, sleep_fn=_no_sleep, now_fn=_StepClock()
        )
    assert info.value.code == 4


# -- update_keyfile ----------------------------------------------------------


def test_update_keyfile_preserves_both_seeds(tmp_key: Path):
    funder_kp, entity_kp, kf, _ = bootstrap.load_or_generate_key(tmp_key)
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
    assert data["funder_seed_hex"] == funder_kp.seed.hex()
    assert data["entity_seed_hex"] == entity_kp.seed.hex()


# -- end-to-end --------------------------------------------------------------


def test_full_bootstrap_is_idempotent_end_to_end(tmp_key: Path, monkeypatch):
    chain = FakeChain()
    chain.faucet_credit = bootstrap.MIN_BALANCE_FOR_REGISTER + 100_000

    def fake_ctor(endpoint: str):
        return chain

    monkeypatch.setattr(bootstrap, "Chain", fake_ctor)
    monkeypatch.setenv("PRICE_ORACLE_KEY_PATH", str(tmp_key))
    monkeypatch.setenv("PRICE_ORACLE_RPC_ENDPOINT", "http://fake")

    bootstrap.main([])
    assert tmp_key.exists()
    data = json.loads(tmp_key.read_text())
    assert "funder_seed_hex" in data
    assert "entity_seed_hex" in data
    assert len(chain.faucet_calls) == 1
    assert len(chain.register_calls) == 1
    funder_addr_used, entity_pubkey_used, _fee, _initial = chain.register_calls[0]
    assert funder_addr_used == bytes.fromhex(data["funder_address_hex"])
    assert entity_pubkey_used == bytes.fromhex(data["entity_pubkey_hex"])
    # The faucet targets the FUNDER address, not the entity address.
    assert chain.faucet_calls == [bytes.fromhex(data["funder_address_hex"])]

    bootstrap.main([])
    assert len(chain.faucet_calls) == 1
    assert len(chain.register_calls) == 1
    data2 = json.loads(tmp_key.read_text())
    assert data2["funder_seed_hex"] == data["funder_seed_hex"]
    assert data2["entity_seed_hex"] == data["entity_seed_hex"]
