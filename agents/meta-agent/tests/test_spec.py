"""Spec parsing and validation."""

from __future__ import annotations

import copy
from pathlib import Path

import pytest

from meta.spec import SpecError, load_spec, parse_spec

SPECS = Path(__file__).resolve().parent.parent / "specs"


def _valid_raw() -> dict:
    return {
        "agent": {"name": "demo-oracle", "version": 1, "description": "demo"},
        "identity": {
            "capabilities": [
                "read_public_chain",
                "read_memory_objects",
                "emit_proposals",
                "post_oracle_anchors",
            ],
            "autonomy_mode": "gated",
        },
        "economics": {},
        "data_source": {
            "module": "demo_source",
            "source_url": "https://example.org/api",
            "source_id": "example.org/api",
            "fetch_function": "fetch_demo",
            "observation_class": "DemoObservation",
            "primary_value_field": "value",
            "observation_fields": [{"name": "value", "type": "price", "fmt": "2f"}],
        },
        "anchor": {"data_tag": "demo/value", "observation_format": "DEMO@{ts}={value:.2f}"},
        "runtime": {"config_namespace": "DEMO_ORACLE", "metrics_port": 9211},
    }


def test_shipped_specs_load():
    for name in ("compute-oracle.toml", "example-oracle.toml"):
        spec = load_spec(SPECS / name)
        assert spec.capability_byte() == 0x47


def test_valid_raw_parses():
    spec = parse_spec(_valid_raw())
    assert spec.name == "demo-oracle"
    assert spec.config_namespace == "DEMO_ORACLE"
    assert spec.metric_prefix == "novai_demo_oracle"
    assert spec.code_hash_label == "novai-demo-oracle-v1"


@pytest.mark.parametrize(
    "mutate,needle",
    [
        (lambda r: r["identity"]["capabilities"].remove("emit_proposals"), "emit_proposals"),
        (lambda r: r["identity"]["capabilities"].remove("post_oracle_anchors"), "post_oracle_anchors"),
        (lambda r: r["identity"].__setitem__("autonomy_mode", "autonomous"), "autonomy_mode"),
        (lambda r: r["agent"].__setitem__("name", "Demo_Oracle"), "kebab-case"),
        (lambda r: r["anchor"].__setitem__("data_tag", "x" * 40), "data_tag"),
        (lambda r: r["runtime"].__setitem__("metrics_port", 8080), "metrics_port"),
        (lambda r: r["identity"]["capabilities"].append("teleport"), "unknown capability"),
        (lambda r: r["data_source"].__setitem__("observation_fields", []), "observation_field"),
        (lambda r: r["anchor"].__setitem__("observation_format", "NOFIELD@{ts}"), "placeholder"),
    ],
)
def test_invalid_specs_rejected(mutate, needle):
    raw = copy.deepcopy(_valid_raw())
    mutate(raw)
    with pytest.raises(SpecError) as exc:
        parse_spec(raw)
    assert needle in str(exc.value)
