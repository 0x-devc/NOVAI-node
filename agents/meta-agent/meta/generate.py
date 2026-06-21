"""Template-extract-and-fill generation of a per-agent tree.

The generator fills @@TOKEN@@ placeholders in the template set (extracted from the
compute-oracle framework, normalized to a single naming convention) and injects the
generated observation encoding into lib/signal.py. It writes a clean generic
oracle_anchor agent. It does not copy-mutate any reference agent, and it never writes
real fetch logic: the data-source module is emitted as a contract stub.

Determinism: filling fixed templates with a fixed spec is a pure string operation, so
regenerating the same spec yields a byte-identical tree.
"""

from __future__ import annotations

from pathlib import Path

from .observation import render_observation_section
from .spec import AgentSpec

TEMPLATES_DIR = Path(__file__).resolve().parent.parent / "templates"

_PY_TYPE = {"str": "str", "price": "float", "int": "int"}
_SAMPLE = {"str": '"sample"', "price": "1.0", "int": "1"}


def _observation_fields_block(spec: AgentSpec) -> str:
    return "\n".join(f"    {f.name}: {_PY_TYPE[f.type]}" for f in spec.observation_fields)


def _sample_obs_construct(spec: AgentSpec) -> str:
    args = ", ".join(f"{f.name}={_SAMPLE[f.type]}" for f in spec.observation_fields)
    return f"{spec.observation_class}({args})"


def _primary_value_gauge_stmt(spec: AgentSpec) -> str:
    if spec.primary_value_field:
        return (
            f'self.registry.set_gauge("{spec.metric_prefix}_last_observation_value", '
            f"float(obs.{spec.primary_value_field}))"
        )
    return "pass  # no primary numeric field declared in the spec"


def token_map(spec: AgentSpec) -> dict[str, str]:
    """Every @@TOKEN@@ the templates use, mapped to its concrete value."""
    return {
        "@@NAME@@": spec.name,
        "@@NAME_SNAKE@@": spec.name_snake,
        "@@NAME_CAMEL@@": spec.name_camel,
        "@@NS@@": spec.config_namespace,
        "@@METRIC_PREFIX@@": spec.metric_prefix,
        "@@CODE_HASH_LABEL@@": spec.code_hash_label,
        "@@CODE_HASH_CONST@@": spec.code_hash_const,
        "@@DESCRIPTION@@": spec.description,
        "@@DATA_TAG@@": spec.data_tag,
        "@@SOURCE_MODULE@@": spec.source_module,
        "@@SOURCE_URL@@": spec.source_url,
        "@@SOURCE_ID@@": spec.source_id,
        "@@FETCH_FN@@": spec.fetch_function,
        "@@OBS_CLASS@@": spec.observation_class,
        "@@METRICS_PORT@@": str(spec.metrics_port),
        "@@LOOP_INTERVAL@@": str(spec.loop_interval_secs),
        "@@HTTP_TIMEOUT@@": str(spec.http_timeout_secs),
        "@@INITIAL_ENTITY_BALANCE@@": str(spec.initial_entity_balance),
        "@@REGISTER_FEE@@": str(spec.register_fee),
        "@@ANCHOR_FEE@@": str(spec.anchor_fee),
        "@@ENTITY_MIN_BALANCE@@": str(spec.entity_min_balance),
        "@@ACCOUNT_MIN_BALANCE@@": str(spec.account_min_balance),
        "@@CREDIT_AMOUNT@@": str(spec.credit_amount),
        "@@INSTALL_DIR@@": spec.install_dir,
        "@@ENV_PATH@@": spec.env_path,
        "@@KEY_PATH@@": spec.key_path,
        "@@UNIT_NAME@@": spec.unit_name,
        "@@OBSERVATION_SECTION@@": render_observation_section(spec),
        "@@OBSERVATION_FIELDS@@": _observation_fields_block(spec),
        "@@PRIMARY_VALUE_GAUGE_STMT@@": _primary_value_gauge_stmt(spec),
        "@@SAMPLE_OBS_CONSTRUCT@@": _sample_obs_construct(spec),
    }


def render(text: str, tokens: dict[str, str]) -> str:
    for key, value in tokens.items():
        text = text.replace(key, value)
    return text


def _plan_file_map(spec: AgentSpec) -> list[tuple[str, str]]:
    """(template path under templates/, output path under the agent dir)."""
    return [
        ("oracle.py.tmpl", "oracle.py"),
        ("bootstrap.py.tmpl", "bootstrap.py"),
        ("lib/__init__.py.tmpl", "lib/__init__.py"),
        ("lib/log.py.tmpl", "lib/log.py"),
        ("lib/config.py.tmpl", "lib/config.py"),
        ("lib/metrics.py.tmpl", "lib/metrics.py"),
        ("lib/chain.py.tmpl", "lib/chain.py"),
        ("lib/signal.py.tmpl", "lib/signal.py"),
        ("lib/source.py.tmpl", f"lib/{spec.source_module}.py"),
        ("systemd/unit.service.tmpl", f"systemd/{spec.unit_name}.service"),
        ("systemd/env.example.tmpl", f"systemd/{spec.name}.env.example"),
        ("tests/__init__.py.tmpl", "tests/__init__.py"),
        ("tests/conftest.py.tmpl", "tests/conftest.py"),
        ("tests/test_dry_run_no_network.py.tmpl", "tests/test_dry_run_no_network.py"),
        ("tests/test_signal_encoding.py.tmpl", "tests/test_signal_encoding.py"),
        ("tests/test_parser_placeholder.py.tmpl", f"tests/test_{spec.source_module}_parser.py"),
        ("README.md.tmpl", "README.md"),
        ("PREMISE_CHECK.md.tmpl", "PREMISE_CHECK.md"),
        ("agent_gitignore.tmpl", ".gitignore"),
    ]


def generate(spec: AgentSpec, out_dir: str | Path) -> list[Path]:
    """Write the full agent tree under out_dir. Return the list of written paths."""
    out = Path(out_dir)
    tokens = token_map(spec)
    written: list[Path] = []
    for tmpl_rel, out_rel in _plan_file_map(spec):
        text = (TEMPLATES_DIR / tmpl_rel).read_text(encoding="utf-8")
        rendered = render(text, tokens)
        dest = out / out_rel
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(rendered, encoding="utf-8")
        written.append(dest)
    return written
