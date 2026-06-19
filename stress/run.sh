#!/bin/bash
# stress/run.sh
# Single entrypoint for the NOVAI stress-testing framework.
#
# Usage:
#   stress/run.sh self-test            Run offline self-tests (no devnet).
#   stress/run.sh soak      [flags]    Baseline soak (Phase 2).
#   stress/run.sh load      [flags]    Load under tx-generator (Phase 3).
#   stress/run.sh kill-node [flags]    Kill and rejoin fault scenario (Phase 4).
#
# Safety: scenarios target a LOCAL devnet only (127.0.0.1). Destructive scenarios
# require STRESS_ENABLE_DESTRUCTIVE=1 (or --enable-destructive) and refuse any
# non-local target.

set -euo pipefail

STRESS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
. "$STRESS_ROOT/lib/common.sh"

usage() {
  cat <<EOF
NOVAI stress-testing framework

Usage: $0 <command> [flags]

Commands:
  self-test        Run offline self-tests (state-root fork check). No devnet.
  soak             Baseline soak scenario (Phase 2: not yet built).
  load             Load scenario via tx-generator (Phase 3: not yet built).
  kill-node        Kill and rejoin fault scenario (Phase 4: not yet built, destructive).
  help             Show this help.

Environment (see stress/stress.env.example):
  STRESS_NODES=${STRESS_NODES}                 Validator count
  STRESS_HOST=${STRESS_HOST}            Cluster host (localhost only)
  STRESS_ENABLE_DESTRUCTIVE=${STRESS_ENABLE_DESTRUCTIVE}             Gate for destructive scenarios
EOF
}

cmd="${1:-help}"
if [ "$#" -gt 0 ]; then shift; fi

case "$cmd" in
  self-test)
    exec "$STRESS_ROOT/lib/state_root_check.sh" --self-test
    ;;
  soak|load|kill-node)
    scenario="$STRESS_ROOT/scenarios/${cmd//-/_}.sh"
    if [ -x "$scenario" ]; then
      exec "$scenario" "$@"
    fi
    log_warn "Scenario '$cmd' is not built yet (planned for a later phase)."
    exit 2
    ;;
  help|-h|--help)
    usage
    ;;
  *)
    log_error "Unknown command: $cmd"
    usage
    exit 2
    ;;
esac
