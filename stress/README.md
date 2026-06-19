# NOVAI Stress-Testing Framework

Additive tooling that exercises a running LOCAL NOVAI devnet and asserts consensus
safety and liveness invariants under load and fault conditions. It does not modify
any chain code; it drives the node binary, the tx-generator, and the JSON-RPC and
Prometheus surfaces that already exist.

## Safety posture (read first)

- Local devnet only. Every scenario targets 127.0.0.1 with the standard port
  offsets. A localhost-only guard refuses any non-local target.
- Safe by default. Destructive scenarios (kill-node, high-rate load) are OFF
  unless explicitly enabled with `STRESS_ENABLE_DESTRUCTIVE=1` or `--enable-destructive`.
- Dry run available. Each scenario supports a dry mode that validates its logic
  without bringing up or killing any process.
- Nothing here pushes, deploys, or touches a remote or production endpoint.

## Prerequisites

- `curl` and `jq` on PATH (preflight fails fast if either is missing).
- A release build of the node and the load tool:
  - `cargo build --release -p novai-node`
  - `cargo build --release -p tx-generator`

## Commands

```
stress/run.sh self-test         Offline proof of the fork-detection logic (no devnet).
stress/run.sh soak      [flags] Baseline soak (Phase 2).
stress/run.sh load      [flags] Load under tx-generator (Phase 3).
stress/run.sh kill-node [flags] Kill and rejoin fault scenario (Phase 4, destructive).
```

## What the framework asserts

The signature invariant is cross-validator state-root agreement: at a common
committed height, every validator must report the identical `state_root`. The check
groups the reported roots by value and takes the majority as the reference, so the
true dissenter is identified regardless of node order. It never compares every node
to a single fixed reference node. Any disagreement at equal height is a fork and
fails loud.

Additional invariants (per scenario):

- Committed height never regresses, and makes forward progress over the window.
- Consensus round stays within a configured bound.
- Peer count returns to the full mesh value, and stays at or above quorum during a
  fault.

## Self-test

```
stress/run.sh self-test
```

Feeds synthetic node-to-root mappings into the agreement check and proves it:

- passes when all nodes agree;
- fails and names the dissenter when one node diverges (including when the
  divergent node is node 0, which proves majority grouping rather than
  compare-to-first);
- fails on a tied split with no clear majority.

No devnet is required; this runs anywhere and is suitable for CI.

## Configuration

See `stress/stress.env.example` for the full list of knobs. Defaults are local and
safe. Runtime logs, data, and reports are written under `$HOME/.novai` and are never
committed.

## Module layout

```
stress/
  run.sh                  Entrypoint and dispatcher.
  lib/common.sh           Config, localhost guard, preflight, logging, node queries.
  lib/assert.sh           Invariant primitives, pass/fail accounting, report.
  lib/state_root_check.sh Fork-detection (majority-grouped) and self-test.
  lib/cluster.sh          Per-node kill and restart for the fault scenario (Phase 4).
  scenarios/soak.sh       Baseline soak (Phase 2).
  scenarios/load.sh       Load scenario (Phase 3).
  scenarios/kill_node.sh  Fault scenario (Phase 4).
```

## Phase status

- Phase 1: lib core, dispatcher, README, self-test. Done.
- Phase 2: full-mesh launcher and baseline soak scenario. Done.
- Phase 3: load scenario (tx-generator under the mesh, concurrent invariant sampling). Done.
- Phase 4: per-node kill and restart, and the kill-node fault scenario. Done.

## Kill-node scenario (destructive, default off)

`stress/run.sh kill-node --enable-destructive` kills one validator (f=1, quorum
kept), proves the survivors keep committing and agreeing, then restarts the victim
and proves it rejoins (peer_count back to N-1 on every node) and catches up to the
majority `state_root`. It checks the no-fork property continuously, at every
sampled height, across the kill, the downtime, and the rejoin, so it is the local
stand-in for the fault behavior the locked-QC safety rule governs. It is OFF by
default and refuses to run without `--enable-destructive` on a localhost cluster.

## Load funding precondition

The tx-generator has no faucet; it derives sender keys deterministically and
expects them pre-funded. The dev-keys genesis funds the first 100 of those exact
addresses (indices 0..99). So the load scenario keeps `--senders` at or below 100
(default 10) and refuses more. High-rate load (tps above the safe cap) requires
`--enable-destructive`.
