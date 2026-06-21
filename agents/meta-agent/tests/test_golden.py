"""Golden test for the generator.

This does not assert source byte-exactness against compute-oracle: compute-oracle weaves
price and GPU semantics into its metric names, config defaults, and oracle.py field
references, so a clean generic template cannot source-reproduce it. Instead it proves the
properties that matter for a generic scaffolder:

  1. on-chain-byte equivalence: for a compute-oracle-shaped spec, the generated encoding
     produces byte-identical data_hash and OracleAnchor payload bytes to the deployed
     compute-oracle (the bytes that actually land on chain),
  2. idempotency: regenerating the same spec yields a byte-identical tree,
  3. no-leak: a different agent's framework files contain no reference-agent tokens, which
     proves the template is genuinely generic and not copy-mutated.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

from meta.generate import generate
from meta.spec import load_spec

HERE = Path(__file__).resolve()
META_DIR = HERE.parent.parent
SPECS = META_DIR / "specs"
REPO = META_DIR.parent.parent


def _load_module(path: Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    assert spec and spec.loader
    # Register before exec so @dataclass module lookups (cls.__module__) resolve.
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


def test_observation_onchain_bytes_match_compute_oracle(tmp_path):
    spec = load_spec(SPECS / "compute-oracle.toml")
    generate(spec, tmp_path)
    gen = _load_module(tmp_path / "lib" / "signal.py", "gen_signal")
    ref = _load_module(REPO / "agents" / "compute-oracle" / "lib" / "signal.py", "ref_signal")

    ts = 1_718_000_000
    assert gen.build_data_hash("RTX4090", 0.34, ts) == ref.build_data_hash("RTX4090", 0.34, ts)

    issuer = bytes(32)
    data_hash = gen.build_data_hash("RTX4090", 0.34, ts)
    source_hash = gen.source_hash_for("vast.ai/api/v0/bundles")
    g = gen.build_oracle_anchor(issuer, data_hash, ts, source_hash, 0, "compute/rtx4090-usd-hr")
    r = ref.build_oracle_anchor(issuer, data_hash, ts, source_hash, 0, "compute/rtx4090-usd-hr")
    assert g.payload == r.payload
    assert g.signal_hash == r.signal_hash


def test_generation_is_idempotent(tmp_path):
    spec = load_spec(SPECS / "example-oracle.toml")
    a = tmp_path / "a"
    b = tmp_path / "b"
    generate(spec, a)
    generate(spec, b)
    files_a = sorted(p.relative_to(a) for p in a.rglob("*") if p.is_file())
    files_b = sorted(p.relative_to(b) for p in b.rglob("*") if p.is_file())
    assert files_a == files_b
    for rel in files_a:
        assert (a / rel).read_bytes() == (b / rel).read_bytes()


_LEAK_TOKENS = [
    "compute-oracle",
    "compute_oracle",
    "novai_compute_oracle",
    "gpu",
    "rtx",
    "vast",
    "9202",
    "price_usd",
]
_FRAMEWORK = [
    "oracle.py",
    "bootstrap.py",
    "lib/config.py",
    "lib/metrics.py",
    "lib/chain.py",
    "lib/signal.py",
    "lib/log.py",
]


def test_no_reference_tokens_leak_into_framework(tmp_path):
    spec = load_spec(SPECS / "example-oracle.toml")
    generate(spec, tmp_path)
    for rel in _FRAMEWORK:
        text = (tmp_path / rel).read_text(encoding="utf-8").lower()
        for token in _LEAK_TOKENS:
            assert token not in text, f"{rel} leaks reference token {token!r}"
