"""Capability byte construction, cross-checked against the SDK."""

from __future__ import annotations

from pathlib import Path

from novai_sdk import Capabilities

from meta.spec import load_spec

SPECS = Path(__file__).resolve().parent.parent / "specs"


def test_oracle_capability_byte_is_0x47():
    spec = load_spec(SPECS / "compute-oracle.toml")
    assert spec.capability_byte() == 0x47


def test_capability_byte_matches_sdk_oracle_preset():
    spec = load_spec(SPECS / "compute-oracle.toml")
    assert spec.capability_byte() == Capabilities.oracle().to_byte()


def test_capability_bits_decode_to_the_named_flags():
    spec = load_spec(SPECS / "compute-oracle.toml")
    caps = Capabilities.from_byte(spec.capability_byte())
    assert caps.read_public_chain  # bit 0
    assert caps.read_memory_objects  # bit 1
    assert caps.emit_proposals  # bit 2, required to dispatch any signal
    assert caps.post_oracle_anchors  # bit 6
    assert not caps.submit_reputation_updates  # bit 5 not set for this archetype
