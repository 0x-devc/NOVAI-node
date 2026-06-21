"""Spec parsing and validation for a new oracle_anchor sub-agent.

The spec is the minimal per-agent variation block plus economics and runtime knobs.
Everything the generator needs to fill the templates comes from here. The fetch logic
and the premise do not: those are human judgment and live outside the spec.

I keep this module dependency-light (stdlib only) so the spec can be validated without
the SDK installed. The SDK is only needed later, for the registration plan.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

try:  # Python 3.11+
    import tomllib  # type: ignore
except ModuleNotFoundError:  # pragma: no cover - exercised only on older runtimes
    import tomli as tomllib  # type: ignore


class SpecError(ValueError):
    """Raised when a spec is malformed or fails a validation rule."""


# The field types the observation encoder understands. Each maps to a Python type and
# a validation style in meta.observation. Keeping the set small and explicit is the
# point: the observation is a small canonical record, not an arbitrary object.
FIELD_TYPES = ("str", "price", "int")

# Capability flag names, in canonical bit order (bit 0 first). This mirrors
# crates/ai_entities/src/lib.rs:116-129 and novai_sdk.capabilities.Capabilities.
CAPABILITY_FLAGS = (
    "read_public_chain",
    "read_memory_objects",
    "emit_proposals",
    "request_execution",
    "read_nnpx_derived",
    "submit_reputation_updates",
    "post_oracle_anchors",
)

_NAME_RE = re.compile(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$")
_ENV_RE = re.compile(r"^[A-Z][A-Z0-9]*(_[A-Z0-9]+)*$")
_IDENT_RE = re.compile(r"^[a-z][a-z0-9_]*$")
_ORACLE_ANCHOR_DATA_TAG_MAX_LEN = 32  # crates/execution/src/lib.rs:1482


@dataclass(frozen=True)
class ObservationField:
    """One field of the canonical observation that the data_hash commits to."""

    name: str
    type: str
    fmt: str = ""  # format spec for "price" fields, e.g. "4f"; ignored otherwise

    def placeholder(self) -> str:
        """The {name...} token this field occupies in the observation format string."""
        if self.type == "price" and self.fmt:
            return "{" + self.name + ":" + self.fmt + "}"
        return "{" + self.name + "}"


@dataclass(frozen=True)
class AgentSpec:
    name: str
    version: int
    description: str
    capabilities: tuple[str, ...]
    autonomy_mode: str
    initial_entity_balance: int
    register_fee: int
    anchor_fee: int
    entity_min_balance: int
    account_min_balance: int
    credit_amount: int
    source_module: str
    source_url: str
    source_id: str
    fetch_function: str
    observation_class: str
    observation_fields: tuple[ObservationField, ...]
    primary_value_field: str
    data_tag: str
    observation_format: str
    config_namespace: str
    metrics_port: int
    loop_interval_secs: int
    http_timeout_secs: int

    # Derived naming. All of it flows from name, version, and config_namespace so the
    # generated tree is internally consistent and free of reference-agent tokens.
    @property
    def name_snake(self) -> str:
        return self.name.replace("-", "_")

    @property
    def name_camel(self) -> str:
        return "".join(part.capitalize() for part in self.name.split("-"))

    @property
    def metric_prefix(self) -> str:
        return "novai_" + self.name_snake

    @property
    def code_hash_label(self) -> str:
        return f"novai-{self.name}-v{self.version}"

    @property
    def code_hash_const(self) -> str:
        return f"{self.config_namespace}_CODE_HASH"

    @property
    def logger_root(self) -> str:
        return self.name_snake

    @property
    def install_dir(self) -> str:
        return f"/opt/novai-{self.name}"

    @property
    def env_path(self) -> str:
        return f"/etc/novai/{self.name}.env"

    @property
    def key_path(self) -> str:
        return f"/etc/novai/{self.name}-keys.json"

    @property
    def unit_name(self) -> str:
        return f"novai-{self.name}"

    def capability_byte(self) -> int:
        """The single capability byte, built from the flag list by bit position."""
        byte = 0
        for flag in self.capabilities:
            byte |= 1 << CAPABILITY_FLAGS.index(flag)
        return byte


def _require(cond: bool, msg: str, errors: list[str]) -> None:
    if not cond:
        errors.append(msg)


def load_spec(path: str | Path) -> AgentSpec:
    """Parse and validate a spec TOML file. Raise SpecError on any problem."""
    raw = tomllib.loads(Path(path).read_text(encoding="utf-8"))
    return parse_spec(raw)


def parse_spec(raw: dict[str, Any]) -> AgentSpec:
    agent = raw.get("agent", {})
    identity = raw.get("identity", {})
    econ = raw.get("economics", {})
    src = raw.get("data_source", {})
    anchor = raw.get("anchor", {})
    runtime = raw.get("runtime", {})

    name = str(agent.get("name", ""))
    version = int(agent.get("version", 1))
    fields_raw = src.get("observation_fields", [])
    obs_fields = tuple(
        ObservationField(
            name=str(f.get("name", "")),
            type=str(f.get("type", "")),
            fmt=str(f.get("fmt", "")),
        )
        for f in fields_raw
    )
    default_ns = name.replace("-", "_").upper() if name else ""

    spec = AgentSpec(
        name=name,
        version=version,
        description=str(agent.get("description", "")),
        capabilities=tuple(str(c) for c in identity.get("capabilities", [])),
        autonomy_mode=str(identity.get("autonomy_mode", "gated")),
        initial_entity_balance=int(econ.get("initial_entity_balance", 50_000)),
        register_fee=int(econ.get("register_fee", 5_000)),
        anchor_fee=int(econ.get("anchor_fee", 1_000)),
        entity_min_balance=int(econ.get("entity_min_balance", 5_000)),
        account_min_balance=int(econ.get("account_min_balance", 200_000)),
        credit_amount=int(econ.get("credit_amount", 100_000)),
        source_module=str(src.get("module", "")),
        source_url=str(src.get("source_url", "")),
        source_id=str(src.get("source_id", "")),
        fetch_function=str(src.get("fetch_function", "fetch_observation")),
        observation_class=str(src.get("observation_class", "Observation")),
        observation_fields=obs_fields,
        primary_value_field=str(src.get("primary_value_field", "")),
        data_tag=str(anchor.get("data_tag", "")),
        observation_format=str(anchor.get("observation_format", "")),
        config_namespace=str(runtime.get("config_namespace", default_ns)),
        metrics_port=int(runtime.get("metrics_port", 0)),
        loop_interval_secs=int(runtime.get("loop_interval_secs", 300)),
        http_timeout_secs=int(runtime.get("http_timeout_secs", 10)),
    )
    validate(spec)
    return spec


def validate(spec: AgentSpec) -> None:
    """Raise SpecError listing every problem found. No partial acceptance."""
    errors: list[str] = []

    _require(bool(_NAME_RE.match(spec.name)), f"name must be kebab-case, got {spec.name!r}", errors)
    _require(spec.version >= 1, f"version must be >= 1, got {spec.version}", errors)
    _require(bool(spec.description), "description must be non-empty", errors)

    # Capabilities: known flags, and the two hard requirements for an emitting oracle.
    for cap in spec.capabilities:
        _require(cap in CAPABILITY_FLAGS, f"unknown capability {cap!r}", errors)
    _require(
        "emit_proposals" in spec.capabilities,
        "capabilities must include emit_proposals (bit 2 is required to dispatch any "
        "signal, crates/ai_entities/src/lib.rs:219-221)",
        errors,
    )
    _require(
        "post_oracle_anchors" in spec.capabilities,
        "oracle_anchor archetype requires the post_oracle_anchors capability (bit 6)",
        errors,
    )
    _require(
        spec.autonomy_mode == "gated",
        "autonomy_mode must be 'gated'; the chain rejects 'autonomous' at "
        "crates/execution/src/lib.rs:9344-9346",
        errors,
    )

    _require(bool(_ENV_RE.match(spec.config_namespace)), f"config_namespace must be UPPER_SNAKE, got {spec.config_namespace!r}", errors)
    _require(bool(_IDENT_RE.match(spec.source_module)), f"data_source.module must be a python identifier, got {spec.source_module!r}", errors)
    _require(bool(_IDENT_RE.match(spec.fetch_function)), f"fetch_function must be a python identifier, got {spec.fetch_function!r}", errors)
    _require(spec.observation_class.isidentifier(), f"observation_class must be an identifier, got {spec.observation_class!r}", errors)

    tag_len = len(spec.data_tag.encode("utf-8"))
    _require(1 <= tag_len <= _ORACLE_ANCHOR_DATA_TAG_MAX_LEN, f"data_tag must encode to 1..{_ORACLE_ANCHOR_DATA_TAG_MAX_LEN} bytes, got {tag_len}", errors)

    _require(9200 <= spec.metrics_port <= 9299, f"metrics_port should follow the 9200+N convention, got {spec.metrics_port}", errors)
    _require(spec.loop_interval_secs >= 1, "loop_interval_secs must be >= 1", errors)
    _require(spec.http_timeout_secs >= 1, "http_timeout_secs must be >= 1", errors)

    # Observation fields and the format string must agree.
    _require(len(spec.observation_fields) >= 1, "at least one observation_field is required", errors)
    seen: set[str] = set()
    for f in spec.observation_fields:
        _require(bool(_IDENT_RE.match(f.name)), f"observation field name must be an identifier, got {f.name!r}", errors)
        _require(f.type in FIELD_TYPES, f"observation field {f.name!r} has unknown type {f.type!r} (allowed: {FIELD_TYPES})", errors)
        _require(f.name not in seen, f"duplicate observation field {f.name!r}", errors)
        seen.add(f.name)
        if f.type == "price":
            _require(bool(f.fmt), f"price field {f.name!r} must declare a fmt (e.g. '4f')", errors)
    _require(bool(spec.observation_format), "observation_format must be non-empty", errors)
    for f in spec.observation_fields:
        ref_plain = "{" + f.name + "}"
        ref_fmt = "{" + f.name + ":"
        _require(
            ref_plain in spec.observation_format or ref_fmt in spec.observation_format,
            f"observation_format is missing a placeholder for field {f.name!r} "
            f"(expected {ref_plain} or {ref_fmt}...)",
            errors,
        )
    _require("{ts}" in spec.observation_format, "observation_format must include the {ts} placeholder", errors)
    if spec.primary_value_field:
        _require(spec.primary_value_field in seen, f"primary_value_field {spec.primary_value_field!r} is not an observation field", errors)
        ptype = next((f.type for f in spec.observation_fields if f.name == spec.primary_value_field), "")
        _require(ptype in ("price", "int"), f"primary_value_field {spec.primary_value_field!r} must be a numeric field, got type {ptype!r}", errors)

    _require(spec.initial_entity_balance >= 0, "initial_entity_balance must be >= 0", errors)
    _require(spec.register_fee >= 0, "register_fee must be >= 0", errors)
    _require(spec.anchor_fee >= 0, "anchor_fee must be >= 0", errors)

    if errors:
        raise SpecError("invalid spec:\n  - " + "\n  - ".join(errors))
