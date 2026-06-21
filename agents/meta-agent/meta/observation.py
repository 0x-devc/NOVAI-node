"""Observation encoding codegen.

This is the one place the generator emits structural code rather than filling tokens,
because the canonical observation differs in shape per agent (the number and type of
fields). The emitted code commits the observation to a data_hash through the exact
ASCII format string the spec declares, then hashes it with the SDK blake3 helper, so
the on-chain bytes match the protocol source of truth.

The emitted code carries the never-lie discipline by construction: it validates every
field before formatting, and it never invents a value. What it does NOT do is fetch or
parse anything; that is the human-authored data-source module.
"""

from __future__ import annotations

from .spec import AgentSpec, ObservationField

_PY_TYPE = {"str": "str", "price": "float", "int": "int"}


def _param_list(spec: AgentSpec) -> str:
    parts = [f"{f.name}: {_PY_TYPE[f.type]}" for f in spec.observation_fields]
    parts.append("timestamp: int")
    return ", ".join(parts)


def _call_args(spec: AgentSpec) -> str:
    parts = [f.name for f in spec.observation_fields]
    parts.append("timestamp")
    return ", ".join(parts)


def _obs_args(spec: AgentSpec) -> str:
    parts = [f"obs.{f.name}" for f in spec.observation_fields]
    parts.append("timestamp")
    return ", ".join(parts)


def _format_kwargs(spec: AgentSpec) -> str:
    parts = []
    for f in spec.observation_fields:
        if f.type == "price":
            parts.append(f"{f.name}=float({f.name})")
        elif f.type == "int":
            parts.append(f"{f.name}=int({f.name})")
        else:
            parts.append(f"{f.name}={f.name}")
    parts.append("ts=int(timestamp)")
    return ", ".join(parts)


def _validation(field: ObservationField) -> list[str]:
    n = field.name
    if field.type == "str":
        return [
            f"    if not isinstance({n}, str) or not {n}:",
            f'        raise ValueError("{n} must be a non-empty string")',
        ]
    if field.type == "price":
        return [
            f"    if not math.isfinite({n}):",
            f'        raise ValueError(f"{n} must be finite, got {{{n}}}")',
            f"    if {n} <= 0:",
            f'        raise ValueError(f"{n} must be positive, got {{{n}}}")',
        ]
    # int
    return [
        f"    if not 0 < {n} < 2**63:",
        f'        raise ValueError(f"{n} must be positive and fit in i63, got {{{n}}}")',
    ]


def render_observation_section(spec: AgentSpec) -> str:
    """Return the observation block of the generated lib/signal.py.

    The block defines OBSERVATION_FORMAT plus three functions:
    canonical_observation_bytes, build_data_hash, and data_hash_for_observation.
    """
    params = _param_list(spec)
    lines: list[str] = []
    lines.append(f'OBSERVATION_FORMAT = "{spec.observation_format}"')
    lines.append("")
    lines.append("")
    lines.append(f"def canonical_observation_bytes({params}) -> bytes:")
    lines.append('    """Return the exact ASCII bytes hashed for an observation."""')
    for f in spec.observation_fields:
        lines.extend(_validation(f))
    lines.append("    if not 0 < timestamp < 2**63:")
    lines.append('        raise ValueError(f"timestamp must be positive and fit in i63, got {timestamp}")')
    lines.append(f"    return OBSERVATION_FORMAT.format({_format_kwargs(spec)}).encode(\"ascii\")")
    lines.append("")
    lines.append("")
    lines.append(f"def build_data_hash({params}) -> bytes:")
    lines.append('    """Compute the 32-byte blake3 of the canonical observation bytes."""')
    lines.append(f"    return blake3_hash(canonical_observation_bytes({_call_args(spec)}))")
    lines.append("")
    lines.append("")
    lines.append('def data_hash_for_observation(obs, timestamp: int) -> bytes:')
    lines.append('    """Adapter from a fetched Observation to the committed data_hash.')
    lines.append("")
    lines.append("    This keeps oracle.py free of the per-field shape: the loop hands the whole")
    lines.append("    Observation here and the encoding lives in one place.")
    lines.append('    """')
    lines.append(f"    return build_data_hash({_obs_args(spec)})")
    return "\n".join(lines)
