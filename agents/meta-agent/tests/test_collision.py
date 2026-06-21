"""Collision pre-flight: entity_id derivation matches the SDK and the chain formula."""

from __future__ import annotations

from pathlib import Path

from novai_sdk import compute_entity_id
from novai_sdk.crypto import blake3_hash

from meta.generate import generate
from meta.spec import load_spec

SPECS = Path(__file__).resolve().parent.parent / "specs"


def test_entity_id_derivation_matches_sdk_and_is_collision_basis():
    spec = load_spec(SPECS / "compute-oracle.toml")
    code_hash = blake3_hash(spec.code_hash_label.encode("utf-8"))
    funder = bytes(range(32))
    eid = compute_entity_id(code_hash, funder)
    assert len(eid) == 32
    # Deterministic: the same (code_hash, funder) maps to the same entity_id. This is
    # exactly why reusing a funder under one code_hash collides at EntityAlreadyExists.
    assert compute_entity_id(code_hash, funder) == eid
    # A fresh funder yields a different entity_id (the operational fix for collisions).
    assert compute_entity_id(code_hash, bytes(range(1, 33))) != eid


def test_generated_chain_embeds_correct_derivation_and_preflight(tmp_path):
    spec = load_spec(SPECS / "compute-oracle.toml")
    generate(spec, tmp_path)
    chain_src = (tmp_path / "lib" / "chain.py").read_text(encoding="utf-8")
    assert 'COMPUTE_ORACLE_CODE_HASH: bytes = blake3_hash(b"novai-compute-oracle-v1")' in chain_src
    assert "compute_entity_id(COMPUTE_ORACLE_CODE_HASH, address)" in chain_src
    assert "def funder_is_unbound" in chain_src
