#!/usr/bin/env python3
"""NOVAI meta-agent scaffolder (Direction B): operator-local CLI.

Given a TOML spec for a new oracle_anchor sub-agent, this tool generates the complete
per-agent tree and a reviewable DRY_RUN registration plan. It writes nothing on chain and
starts no service. A human reviews the tree and the plan, then flips the agent to live by
hand. This is an operator tool, not a registered NOVAI agent: it needs no entity, key, or
funding because it performs no chain write (the optional collision pre-flight is a read).

Usage:
  scaffold.py validate <spec.toml>
  scaffold.py generate <spec.toml> [--out DIR] [--plan] [--check-endpoint URL]
  scaffold.py plan     <spec.toml> [--out DIR] [--check-endpoint URL]

The default output directory is .out/<name> under the meta-agent directory (gitignored),
so a casual run does not create an agent tree under agents/. Pass --out agents/<name> when
you are ready to land a real agent for review.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from meta.generate import generate  # noqa: E402
from meta.plan import generate_plan  # noqa: E402
from meta.spec import SpecError, load_spec  # noqa: E402

_DEFAULT_OUT_ROOT = Path(__file__).resolve().parent / ".out"


def _out_dir(spec_name: str, out: str | None) -> Path:
    if out:
        return Path(out)
    return _DEFAULT_OUT_ROOT / spec_name


def cmd_validate(args: argparse.Namespace) -> int:
    try:
        spec = load_spec(args.spec)
    except SpecError as exc:
        print(str(exc), file=sys.stderr)
        return 1
    print(f"spec OK: {spec.name} (code_hash label {spec.code_hash_label}, "
          f"capability 0x{spec.capability_byte():02x}, metrics port {spec.metrics_port})")
    return 0


def cmd_generate(args: argparse.Namespace) -> int:
    try:
        spec = load_spec(args.spec)
    except SpecError as exc:
        print(str(exc), file=sys.stderr)
        return 1
    out = _out_dir(spec.name, args.out)
    written = generate(spec, out)
    print(f"generated {len(written)} files under {out}")
    for path in written:
        print(f"  {path.relative_to(out)}")
    if args.plan:
        plan_path = generate_plan(out, spec, check_endpoint=args.check_endpoint)
        print(f"registration plan: {plan_path}")
    print(
        "\nNext: implement the data-source fetch, make the parser test pass with real "
        "fixtures, review REGISTER_PLAN.md, then deploy by hand. The agent ships in "
        "DRY_RUN by default."
    )
    return 0


def cmd_plan(args: argparse.Namespace) -> int:
    try:
        spec = load_spec(args.spec)
    except SpecError as exc:
        print(str(exc), file=sys.stderr)
        return 1
    out = _out_dir(spec.name, args.out)
    if not (out / "bootstrap.py").exists():
        generate(spec, out)
    plan_path = generate_plan(out, spec, check_endpoint=args.check_endpoint)
    print(f"registration plan: {plan_path}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="scaffold.py", description="NOVAI meta-agent scaffolder (Direction B)")
    sub = parser.add_subparsers(dest="command", required=True)

    p_val = sub.add_parser("validate", help="parse and validate a spec")
    p_val.add_argument("spec")
    p_val.set_defaults(func=cmd_validate)

    p_gen = sub.add_parser("generate", help="generate the agent tree")
    p_gen.add_argument("spec")
    p_gen.add_argument("--out", default=None, help="output directory (default .out/<name>)")
    p_gen.add_argument("--plan", action="store_true", help="also produce REGISTER_PLAN.md")
    p_gen.add_argument("--check-endpoint", default=None, help="opt-in read-only collision check RPC URL")
    p_gen.set_defaults(func=cmd_generate)

    p_plan = sub.add_parser("plan", help="produce the DRY_RUN registration plan")
    p_plan.add_argument("spec")
    p_plan.add_argument("--out", default=None, help="agent directory (default .out/<name>)")
    p_plan.add_argument("--check-endpoint", default=None, help="opt-in read-only collision check RPC URL")
    p_plan.set_defaults(func=cmd_plan)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return int(args.func(args))


if __name__ == "__main__":
    sys.exit(main())
