"""Every generated Python file compiles."""

from __future__ import annotations

import py_compile
from pathlib import Path

from meta.generate import generate
from meta.spec import load_spec

SPECS = Path(__file__).resolve().parent.parent / "specs"


def test_generated_tree_compiles(tmp_path):
    spec = load_spec(SPECS / "example-oracle.toml")
    written = generate(spec, tmp_path)
    py_files = [p for p in written if p.suffix == ".py"]
    assert py_files, "expected generated python files"
    for path in py_files:
        py_compile.compile(str(path), doraise=True)
