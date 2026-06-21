"""NOVAI meta-agent: an operator-local scaffolder for oracle_anchor sub-agents.

This package generates a complete per-agent tree from a TOML spec and a reviewable
DRY_RUN registration plan. It writes nothing on chain and starts no service. A human
reviews the generated tree and the plan, then flips the agent to live by hand.

The generator is template-extract-and-fill against the compute-oracle reference, with
normalization to a single naming convention. It deliberately produces a generic
oracle_anchor agent, not a byte copy of compute-oracle (which embeds price and GPU
semantics in its names). The one thing it never writes is real data-fetch logic: the
data-source module ships as a contract stub that raises NoDataError until a human
implements it, and the parser test ships as a failing placeholder so an unimplemented
source is a red test.
"""

from __future__ import annotations

__all__ = ["spec", "observation", "generate", "plan"]
