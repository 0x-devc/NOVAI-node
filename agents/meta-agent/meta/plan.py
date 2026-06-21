"""Reviewable DRY_RUN registration plan.

The plan is produced by running the generated bootstrap.py in DRY_RUN, the same path the
operator runs. It writes nothing on chain. By default it does not persist keys, so the
entity_id it shows is for a freshly generated funder and is illustrative; the real
identity is fixed when the operator runs the live bootstrap. The capability byte and the
83-byte Type-10 payload structure shown are exact.

Collision pre-flight is offline by default (the bootstrap prints the entity_id and the
command to check it). Passing check_endpoint runs a read-only existence query through the
bootstrap; that path issues only reads, never a write.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from .spec import AgentSpec


def generate_plan(
    agent_dir: str | Path,
    spec: AgentSpec,
    *,
    python: str | None = None,
    check_endpoint: str | None = None,
    timeout: int = 120,
) -> Path:
    """Run the generated bootstrap in DRY_RUN and write REGISTER_PLAN.md. Return its path."""
    agent_dir = Path(agent_dir)
    py = python or sys.executable
    ns = spec.config_namespace
    env = dict(os.environ)
    env[f"{ns}_DRY_RUN"] = "1"
    env[f"{ns}_KEY_PATH"] = str(agent_dir / "nonexistent-plan-keys.json")  # ephemeral; not persisted
    env.pop(f"{ns}_BOOTSTRAP_WRITE_KEYS", None)  # never persist a secret from the plan step
    if check_endpoint:
        env[f"{ns}_CHECK_ENDPOINT"] = check_endpoint
    else:
        env.pop(f"{ns}_CHECK_ENDPOINT", None)
    env["PYTHONPATH"] = str(agent_dir) + os.pathsep + env.get("PYTHONPATH", "")

    proc = subprocess.run(
        [py, "bootstrap.py"],
        cwd=str(agent_dir),
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )
    captured = (proc.stderr or "") + (proc.stdout or "")

    cap = spec.capability_byte()
    runbook = _runbook(spec, cap)
    body = (
        f"# Registration plan: {spec.name}\n\n"
        "This plan is produced by running the generated bootstrap.py in DRY_RUN. It writes\n"
        "nothing on chain. A human reviews it, then runs the live bootstrap by hand. That\n"
        "human flip is the trust boundary.\n\n"
        "## Capability byte\n\n"
        f"{spec.name} registers with capability 0x{cap:02x} "
        f"({', '.join(spec.capabilities)}).\n"
        "emit_proposals (bit 2) is required to dispatch any signal; post_oracle_anchors\n"
        "(bit 6) authorizes the anchor write.\n\n"
        "## Deploy runbook (human, supervised)\n\n"
        f"{runbook}\n\n"
        "## Dry-run bootstrap output (the derived plan)\n\n"
        f"Exit code: {proc.returncode}\n\n"
        "```\n"
        f"{captured.rstrip()}\n"
        "```\n"
    )
    dest = agent_dir / "REGISTER_PLAN.md"
    dest.write_text(body, encoding="utf-8")
    return dest


def _runbook(spec: AgentSpec, cap: int) -> str:
    ns = spec.config_namespace
    steps = [
        f"1. Implement the data-source fetch in lib/{spec.source_module}.py and make "
        f"tests/test_{spec.source_module}_parser.py pass with real fixtures.",
        "2. Review this plan.",
        "3. Generate the two keypairs and pre-flight the funder (see the collision check below).",
        "4. Faucet-fund the funder address.",
        "5. Run bootstrap.py live to submit the Type-10 RegisterAiEntityWithKey shown below.",
        f"6. Poll until the entity exists on chain with capability 0x{cap:02x}.",
        f"7. Persist the keyfile at 0600 ({spec.key_path}).",
        f"8. Install the tree to {spec.install_dir} and the env to {spec.env_path} (mode 0600).",
        f"9. Install the systemd unit and run systemctl enable --now {spec.unit_name}.",
        f"10. Flip {ns}_DRY_RUN=0 only after the parser test passes and this plan is reviewed.",
    ]
    return "\n".join(steps)
