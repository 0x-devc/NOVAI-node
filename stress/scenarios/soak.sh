#!/bin/bash
# stress/scenarios/soak.sh
# Baseline soak: bring up a local full-mesh devnet, poll every interval, and
# assert safety and liveness invariants over a window, then emit a pass/fail
# report.
#
# Invariants asserted:
#   - committed height never regresses (per node, every sample)
#   - committed height makes forward progress across the window (cluster)
#   - consensus round stays within STRESS_ROUND_MAX (per node)
#   - novai_peer_count == STRESS_EXPECTED_PEERS (N-1) on every node (full mesh)
#   - cross-validator state_root agreement at a COMMON height that every node has
#     already passed (min committed height minus a margin), so a lagging node
#     does not produce a false fork
#
# Safety: local devnet only (127.0.0.1). --dry-run validates the assertion logic
# and the launcher command construction without standing up any cluster.

set -uo pipefail

_SOAK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
. "$_SOAK_DIR/../lib/common.sh"
# shellcheck source=../lib/assert.sh
. "$_SOAK_DIR/../lib/assert.sh"
# shellcheck source=../lib/state_root_check.sh
. "$_SOAK_DIR/../lib/state_root_check.sh"
# shellcheck source=../lib/cluster.sh
. "$_SOAK_DIR/../lib/cluster.sh"

SOAK_MANAGE=1     # 1 = start/stop the cluster; 0 = attach to an already-running one
SOAK_DRYRUN=0
SOAK_REPORT=""

usage() {
  cat <<EOF
soak: baseline soak scenario for a local full-mesh devnet.

Usage: $0 [flags]

Flags:
  --dry-run         Validate assertion logic and the launcher without a cluster.
  --attach          Use an already-running cluster (do not start or stop it).
  --duration <s>    Soak duration in seconds (default ${STRESS_DURATION}).
  --interval <s>    Sample interval in seconds (default ${STRESS_INTERVAL}).
  --nodes <n>       Validator count (default ${STRESS_NODES}); sets expected peers to n-1.
  --round-max <n>   Maximum allowed consensus round (default ${STRESS_ROUND_MAX}).
  --report <file>   Write a plain-text report copy to this file.
  -h, --help        Show this help.
EOF
}

# Smallest of a list of integers.
_min_of() {
  local m="$1"; shift
  local v
  for v in "$@"; do [ "$v" -lt "$m" ] && m="$v"; done
  printf '%s' "$m"
}

soak_print_plan() {
  log_info "Plan: ${STRESS_NODES}-node full-mesh local devnet on 127.0.0.1"
  log_info "  duration=${STRESS_DURATION}s interval=${STRESS_INTERVAL}s round_max=${STRESS_ROUND_MAX} expected_peers=${STRESS_EXPECTED_PEERS}"
  log_info "  ports: P2P ${STRESS_P2P_BASE}+i, rpc ${STRESS_RPC_BASE}+i, metrics ${STRESS_METRICS_BASE}+i"
  local i
  for (( i = 0; i < STRESS_NODES; i++ )); do
    cluster_build_argv "$i"
    log_info "  node$i: ${CLUSTER_ARGV[*]}"
  done
}

# Verify the launcher builds a LOCAL-ONLY full-mesh command for each node.
soak_dry_check_launcher() {
  local i bad=0 joined peers
  for (( i = 0; i < STRESS_NODES; i++ )); do
    cluster_build_argv "$i"
    joined="${CLUSTER_ARGV[*]}"
    case "$joined" in
      *"--data-dir"*) : ;;
      *) log_fail "launcher node$i: missing --data-dir"; bad=1 ;;
    esac
    # Any IPv4 literal in the command must be 127.0.0.1.
    if printf '%s\n' "${CLUSTER_ARGV[@]}" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+' \
         | grep -vq '^127\.0\.0\.1$'; then
      log_fail "launcher node$i: non-local address present: $joined"; bad=1
    fi
    # Full mesh: node i must dial exactly i peers (nodes 0..i-1).
    peers="$(printf '%s\n' "${CLUSTER_ARGV[@]}" | grep -c '^--peer$' || true)"
    if [ "$peers" -ne "$i" ]; then
      log_fail "launcher node$i: expected $i --peer flags (full mesh), got $peers"; bad=1
    fi
  done
  if [ "$bad" -eq 0 ]; then
    log_ok "launcher: every node builds a local-only full-mesh command"
    return 0
  fi
  return 1
}

soak_dry_run() {
  log_info "DRY RUN: validating soak assertion logic and launcher (no cluster stood up)."
  soak_print_plan
  echo
  local fails=0

  _expect() { # _expect <want_rc> <desc> <cmd...>
    local want="$1" desc="$2"; shift 2
    local rc=0
    "$@" >/dev/null 2>&1 || rc=$?
    if [ "$rc" -eq "$want" ]; then
      log_ok "dry-check: $desc (rc=$rc)"
    else
      log_fail "dry-check: $desc (want rc=$want, got $rc)"
      fails=$(( fails + 1 ))
    fi
  }
  _dry_sr() { printf '%s\n' "$1" | sr_verdict_from_pairs 5; }

  # Height invariants.
  _expect 0 "no-regression accepts a climb" assert_no_height_regression 10 12 dry
  _expect 1 "no-regression catches a regression" assert_no_height_regression 12 9 dry
  _expect 0 "progress accepts forward progress" assert_progress 10 40 5 dry
  _expect 1 "progress catches a stall" assert_progress 10 12 5 dry
  # Round bound.
  _expect 0 "round within bound" assert_round_bounded 3 20 dry
  _expect 1 "round over bound" assert_round_bounded 99 20 dry
  # Peer count (full mesh == N-1).
  _expect 0 "peer_count full mesh accepted" assert_peer_count_eq 3 3 dry
  _expect 1 "peer_count degraded caught" assert_peer_count_eq 1 3 dry
  # State-root agreement (reuse the fork-check verdict).
  _expect 0 "state_root agreement passes" _dry_sr "node0|aa
node1|aa
node2|aa
node3|aa"
  _expect 1 "state_root fork caught" _dry_sr "node0|aa
node1|aa
node2|bb
node3|aa"

  # Launcher builds a local-only full-mesh command.
  if ! soak_dry_check_launcher; then fails=$(( fails + 1 )); fi

  echo
  if [ "$fails" -eq 0 ]; then
    log_ok "soak dry-run: assertion logic and launcher validated"
    return 0
  fi
  log_fail "soak dry-run: $fails check(s) failed"
  return 1
}

soak_live() {
  stress_preflight
  local i
  for (( i = 0; i < STRESS_NODES; i++ )); do
    assert_localhost_or_die "$(node_rpc_url "$i")" "rpc endpoint"
    assert_localhost_or_die "$(node_metrics_url "$i")" "metrics endpoint"
  done

  if [ "$SOAK_MANAGE" -eq 1 ]; then
    cluster_build_check
    cluster_start
    trap 'cluster_stop' EXIT INT TERM
    if ! cluster_wait_ready; then
      log_fail "cluster failed to become ready; aborting soak"
      return 1
    fi
  else
    log_info "attach mode: using an already-running cluster (no start or stop)"
  fi

  assert_init
  local report="$SOAK_REPORT"
  if [ -z "$report" ]; then
    mkdir -p "$STRESS_REPORT_DIR" 2>/dev/null || true
    report="$STRESS_REPORT_DIR/soak-$(date -u +%Y%m%dT%H%M%SZ).txt"
  fi

  # Initial per-node heights.
  local prev_h=() start_h=() h r p
  for (( i = 0; i < STRESS_NODES; i++ )); do
    h="$(get_node_metric "$i" novai_committed_height 2>/dev/null || true)"
    is_uint "$h" || h=0
    prev_h[$i]="$h"; start_h[$i]="$h"
  done

  local start_ts now elapsed min_h target sr_rc
  start_ts="$(date +%s)"
  log_info "Soaking for ${STRESS_DURATION}s, sampling every ${STRESS_INTERVAL}s..."
  while :; do
    local heights=()
    for (( i = 0; i < STRESS_NODES; i++ )); do
      h="$(get_node_metric "$i" novai_committed_height 2>/dev/null || true)"
      r="$(get_node_metric "$i" novai_current_round 2>/dev/null || true)"
      p="$(get_node_metric "$i" novai_peer_count 2>/dev/null || true)"
      if ! is_uint "$h"; then
        record_fail "node_reachable[node$i]" "metrics unreadable (committed_height='$h')"
        continue
      fi
      assert_no_height_regression "${prev_h[$i]}" "$h" "node$i"
      assert_round_bounded "$r" "$STRESS_ROUND_MAX" "node$i"
      assert_peer_count_eq "$p" "$STRESS_EXPECTED_PEERS" "node$i"
      prev_h[$i]="$h"
      heights[$i]="$h"
    done

    # Cross-validator state_root agreement at a height every node has passed.
    if [ "${#heights[@]}" -eq "$STRESS_NODES" ]; then
      min_h="$(_min_of "${heights[@]}")"
      target=$(( min_h - STRESS_SAMPLE_MARGIN ))
      if [ "$target" -ge 1 ]; then
        sr_rc=0
        state_root_agreement "$target" || sr_rc=$?
        if [ "$sr_rc" -eq 0 ]; then
          record_pass "state_root_agreement@$target" "all nodes agree"
        else
          record_fail "state_root_agreement@$target" "divergence or unverifiable (rc=$sr_rc)"
          if [ "$sr_rc" -eq 1 ]; then
            log_fail "FORK observed during soak; aborting sampling immediately"
            break
          fi
        fi
      fi
    fi

    now="$(date +%s)"; elapsed=$(( now - start_ts ))
    [ "$elapsed" -ge "$STRESS_DURATION" ] && break
    sleep "$STRESS_INTERVAL"
  done

  # Forward progress across the window (use the slowest node at each end).
  local end_h=() smin emin
  for (( i = 0; i < STRESS_NODES; i++ )); do
    h="$(get_node_metric "$i" novai_committed_height 2>/dev/null || true)"
    is_uint "$h" || h="${prev_h[$i]}"
    end_h[$i]="$h"
  done
  smin="$(_min_of "${start_h[@]}")"
  emin="$(_min_of "${end_h[@]}")"
  assert_progress "$smin" "$emin" "$STRESS_MIN_PROGRESS" "cluster"

  # Teardown runs via the EXIT trap (managed mode only).
  assert_report "$report"
}

# --- Flag parsing ----------------------------------------------------------
while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run)  SOAK_DRYRUN=1 ;;
    --attach)   SOAK_MANAGE=0 ;;
    --duration) shift; STRESS_DURATION="${1:-}" ;;
    --interval) shift; STRESS_INTERVAL="${1:-}" ;;
    --nodes)    shift; STRESS_NODES="${1:-}"; STRESS_EXPECTED_PEERS=$(( STRESS_NODES - 1 )) ;;
    --round-max) shift; STRESS_ROUND_MAX="${1:-}" ;;
    --report)   shift; SOAK_REPORT="${1:-}" ;;
    -h|--help)  usage; exit 0 ;;
    *) log_error "unknown flag: $1"; usage; exit 2 ;;
  esac
  shift
done

if [ "$SOAK_DRYRUN" -eq 1 ]; then
  soak_dry_run
  exit $?
fi
soak_live
exit $?
