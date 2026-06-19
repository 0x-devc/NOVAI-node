#!/bin/bash
# stress/scenarios/kill_node.sh
# Kill-node fault scenario: with quorum maintained (kill 1 of N, f=1), prove the
# chain keeps committing AND agreeing throughout the fault and recovery. This is
# the local stand-in for the fault behavior the locked-QC safety rule governs, so
# it checks the fix's property (no two nodes commit a different state_root at the
# same height) CONTINUOUSLY, at every sampled height, across the kill, the
# downtime, and the rejoin, not once at the end.
#
# Phases and assertions:
#   baseline : all N agree on state_root at sampled common heights; peer_count N-1.
#   kill     : kill the victim by its PID file; survivors detect it (peer_count
#              N-2), keep committing, and agree on state_root at each common height.
#   rejoin   : restart the victim (same --validator i and persisted data-dir, full
#              peer dial). It must rejoin (peer_count returns to N-1 on ALL nodes)
#              AND catch up to the majority state_root within the timeout.
#   post     : all N agree again at full height; peer_count N-1.
# Continuous no-fork: every sample runs the cross-validator agreement check
# (lib/state_root_check.sh); any disagreement at equal height fails loud and aborts.
#
# Safety: DESTRUCTIVE and DEFAULT OFF. Requires STRESS_ENABLE_DESTRUCTIVE=1 (or
# --enable-destructive) and a LOCAL devnet (127.0.0.1). It kills and restarts only
# its own PID-file nodes. --dry-run validates the kill/rejoin/assertion logic
# without standing up a cluster or killing anything.

set -uo pipefail

_KN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
. "$_KN_DIR/../lib/common.sh"
# shellcheck source=../lib/assert.sh
. "$_KN_DIR/../lib/assert.sh"
# shellcheck source=../lib/state_root_check.sh
. "$_KN_DIR/../lib/state_root_check.sh"
# shellcheck source=../lib/cluster.sh
. "$_KN_DIR/../lib/cluster.sh"

KN_DRYRUN=0
KN_REPORT=""

usage() {
  cat <<EOF
kill-node: DESTRUCTIVE fault scenario. Kill 1 of N (f=1), prove the chain keeps
committing and agreeing, then restart the victim and prove it rejoins and catches
up to the majority state_root. Continuous no-fork at every phase.

Usage: $0 [flags]

Flags:
  --dry-run             Validate logic, gate, and commands (no cluster, no kill).
  --enable-destructive  Required to actually run (default OFF).
  --victim <i>          Validator index to kill (default ${STRESS_VICTIM}).
  --kill-duration <s>   Seconds to run on the surviving quorum (default ${STRESS_KILL_DURATION}).
  --rejoin-timeout <s>  Seconds to wait for rejoin and catch-up (default ${STRESS_REJOIN_TIMEOUT}).
  --interval <s>        Sample interval (default ${STRESS_INTERVAL}).
  --round-max <n>       Maximum allowed consensus round (default ${STRESS_ROUND_MAX}).
  --nodes <n>           Validator count (default ${STRESS_NODES}); sets expected peers to n-1.
  --report <file>       Write a plain-text report copy to this file.
  -h, --help            Show this help.
EOF
}

# Smallest of a list of integers.
_min_of() {
  local m="$1"; shift
  local v
  for v in "$@"; do [ "$v" -lt "$m" ] && m="$v"; done
  printf '%s' "$m"
}

# True if the destructive gate is enabled.
kn_gate_ok() { [ "$STRESS_ENABLE_DESTRUCTIVE" = "1" ]; }

# Initialize per-node previous heights from whatever is currently reachable.
kn_init_prev() {
  local i h
  KN_PREV_H=()
  for (( i = 0; i < STRESS_NODES; i++ )); do
    h="$(get_node_metric "$i" novai_committed_height 2>/dev/null || true)"
    is_uint "$h" || h=0
    KN_PREV_H[$i]="$h"
  done
}

# Collect committed heights of all reachable nodes into the global KN_HEIGHTS.
kn_collect_heights() {
  local i h
  KN_HEIGHTS=()
  for (( i = 0; i < STRESS_NODES; i++ )); do
    h="$(get_node_metric "$i" novai_committed_height 2>/dev/null || true)"
    is_uint "$h" && KN_HEIGHTS[$i]="$h"
  done
}

# Minimum committed height among the survivors (excludes the victim).
kn_survivor_min() {
  local victim="$1" i h vals=()
  for (( i = 0; i < STRESS_NODES; i++ )); do
    [ "$i" -eq "$victim" ] && continue
    h="$(get_node_metric "$i" novai_committed_height 2>/dev/null || true)"
    is_uint "$h" && vals+=("$h")
  done
  if [ "${#vals[@]}" -ge 1 ]; then _min_of "${vals[@]}"; else printf '0'; fi
}

# Continuous cross-validator agreement over all reachable nodes, at a common
# height (min reachable height minus margin). Uses the current KN_HEIGHTS.
# Returns 2 if a fork is detected, 0 otherwise.
kn_fork_check() {
  if [ "${#KN_HEIGHTS[@]}" -lt 2 ]; then return 0; fi
  local min_h target sr_rc
  min_h="$(_min_of "${KN_HEIGHTS[@]}")"
  target=$(( min_h - STRESS_SAMPLE_MARGIN ))
  [ "$target" -ge 1 ] || return 0
  sr_rc=0
  state_root_agreement "$target" || sr_rc=$?
  if [ "$sr_rc" -eq 0 ]; then
    record_pass "state_root_agreement@$target" "reachable nodes agree"
    return 0
  fi
  record_fail "state_root_agreement@$target" "divergence or unverifiable (rc=$sr_rc)"
  [ "$sr_rc" -eq 1 ] && return 2
  return 0
}

# One sampling pass. expected_peers applies to every present node; skip = a node
# index to exclude from per-node asserts (the killed victim), or -1 for none.
# Runs the continuous agreement check. Returns 2 on fork.
kn_sample() {
  local expected_peers="$1" skip="$2" i h r p
  KN_HEIGHTS=()
  for (( i = 0; i < STRESS_NODES; i++ )); do
    [ "$i" -eq "$skip" ] && continue
    h="$(get_node_metric "$i" novai_committed_height 2>/dev/null || true)"
    r="$(get_node_metric "$i" novai_current_round 2>/dev/null || true)"
    p="$(get_node_metric "$i" novai_peer_count 2>/dev/null || true)"
    if ! is_uint "$h"; then record_fail "node_reachable[node$i]" "metrics unreadable"; continue; fi
    assert_no_height_regression "${KN_PREV_H[$i]}" "$h" "node$i"
    assert_round_bounded "$r" "$STRESS_ROUND_MAX" "node$i"
    assert_peer_count_eq "$p" "$expected_peers" "node$i"
    KN_PREV_H[$i]="$h"
    KN_HEIGHTS[$i]="$h"
  done
  kn_fork_check
}

# Run <count> sampling passes at the given expected_peers/skip. Returns 2 on fork.
kn_sample_loop() {
  local count="$1" expected_peers="$2" skip="$3" n rc
  for (( n = 0; n < count; n++ )); do
    kn_sample "$expected_peers" "$skip"; rc=$?
    [ "$rc" -eq 2 ] && return 2
    sleep "$STRESS_INTERVAL"
  done
  return 0
}

# Wait for survivors to detect the kill (peer_count == N-2), fork-checking each
# poll. Returns 0 detected, 1 timeout, 2 fork.
kn_wait_kill_detected() {
  local victim="$1" survivor_peers=$(( STRESS_EXPECTED_PEERS - 1 ))
  local start now i p all_detected rc
  start="$(date +%s)"
  log_info "Waiting up to ${STRESS_KILL_DETECT_TIMEOUT}s for survivors to detect the kill (peer_count ${survivor_peers})..."
  while :; do
    kn_collect_heights; kn_fork_check; rc=$?; [ "$rc" -eq 2 ] && return 2
    all_detected=1
    for (( i = 0; i < STRESS_NODES; i++ )); do
      [ "$i" -eq "$victim" ] && continue
      p="$(get_node_metric "$i" novai_peer_count 2>/dev/null || true)"
      if ! is_uint "$p" || [ "$p" -ne "$survivor_peers" ]; then all_detected=0; break; fi
    done
    if [ "$all_detected" -eq 1 ]; then
      record_pass "kill_detected" "survivors at peer_count ${survivor_peers}"
      return 0
    fi
    now="$(date +%s)"
    if [ "$(( now - start ))" -ge "$STRESS_KILL_DETECT_TIMEOUT" ]; then
      record_fail "kill_detected" "survivors did not settle to peer_count ${survivor_peers} within ${STRESS_KILL_DETECT_TIMEOUT}s"
      return 1
    fi
    sleep "$STRESS_INTERVAL"
  done
}

# Wait for the victim to rejoin (peer_count N-1 on ALL nodes) AND catch up to a
# committed height >= catchup, fork-checking each poll. Returns 0 healed, 1
# timeout, 2 fork.
kn_wait_rejoin() {
  local victim="$1" catchup="$2" start now i p h ok_peers ok_catch rc
  start="$(date +%s)"
  log_info "Waiting up to ${STRESS_REJOIN_TIMEOUT}s for node${victim} to rejoin (peer_count ${STRESS_EXPECTED_PEERS} on all) and catch up to height >= ${catchup}..."
  while :; do
    kn_collect_heights; kn_fork_check; rc=$?; [ "$rc" -eq 2 ] && return 2
    ok_peers=1
    for (( i = 0; i < STRESS_NODES; i++ )); do
      p="$(get_node_metric "$i" novai_peer_count 2>/dev/null || true)"
      if ! is_uint "$p" || [ "$p" -ne "$STRESS_EXPECTED_PEERS" ]; then ok_peers=0; break; fi
    done
    h="$(get_node_metric "$victim" novai_committed_height 2>/dev/null || true)"
    ok_catch=0
    if is_uint "$h" && [ "$h" -ge "$catchup" ]; then ok_catch=1; fi
    if [ "$ok_peers" -eq 1 ] && [ "$ok_catch" -eq 1 ]; then
      record_pass "rejoin_mesh_healed" "peer_count ${STRESS_EXPECTED_PEERS} on all nodes"
      record_pass "rejoin_catch_up" "node${victim} reached height ${h} (>= ${catchup}) with matching state_root"
      return 0
    fi
    now="$(date +%s)"
    if [ "$(( now - start ))" -ge "$STRESS_REJOIN_TIMEOUT" ]; then
      record_fail "rejoin" "node${victim} failed to rejoin and catch up within ${STRESS_REJOIN_TIMEOUT}s (peers_ok=${ok_peers} catch_ok=${ok_catch} victim_height='${h}' target=${catchup})"
      return 1
    fi
    sleep "$STRESS_INTERVAL"
  done
}

kn_live() {
  if ! kn_gate_ok; then
    log_error "kill-node is DESTRUCTIVE and OFF by default."
    log_error "Re-run with --enable-destructive (local devnet only) to enable it."
    exit 7
  fi
  stress_preflight
  cluster_build_check

  if ! is_uint "$STRESS_VICTIM" || [ "$STRESS_VICTIM" -ge "$STRESS_NODES" ]; then
    log_error "invalid --victim ${STRESS_VICTIM} (must be 0..$(( STRESS_NODES - 1 )))"; exit 2
  fi

  local i
  for (( i = 0; i < STRESS_NODES; i++ )); do
    assert_localhost_or_die "$(node_rpc_url "$i")" "rpc endpoint"
    assert_localhost_or_die "$(node_metrics_url "$i")" "metrics endpoint"
  done

  cluster_start
  trap 'cluster_stop' EXIT INT TERM
  if ! cluster_wait_ready; then
    log_fail "cluster failed to become ready; aborting"
    return 1
  fi

  assert_init
  local report="$KN_REPORT"
  if [ -z "$report" ]; then
    mkdir -p "$STRESS_REPORT_DIR" 2>/dev/null || true
    report="$STRESS_REPORT_DIR/kill-node-$(date -u +%Y%m%dT%H%M%SZ).txt"
  fi

  local victim="$STRESS_VICTIM"
  local survivor_peers=$(( STRESS_EXPECTED_PEERS - 1 ))
  local rc

  # Phase baseline: all N healthy and agreeing.
  log_info "Phase baseline: all ${STRESS_NODES} nodes, expect peer_count ${STRESS_EXPECTED_PEERS} and agreement"
  kn_init_prev
  kn_sample_loop "$STRESS_BASELINE_SAMPLES" "$STRESS_EXPECTED_PEERS" -1; rc=$?
  if [ "$rc" -eq 2 ]; then log_fail "FORK in baseline"; assert_report "$report"; return 1; fi

  # Phase kill.
  log_warn "Phase kill: killing node${victim} (quorum $(( STRESS_NODES - 1 )) of ${STRESS_NODES} maintained, f=1)"
  cluster_kill_node "$victim"
  kn_init_prev
  local kill_start_min; kill_start_min="$(kn_survivor_min "$victim")"
  kn_wait_kill_detected "$victim"; rc=$?
  if [ "$rc" -eq 2 ]; then log_fail "FORK among survivors right after the kill"; assert_report "$report"; return 1; fi

  log_info "Phase kill: survivors expect peer_count ${survivor_peers} and continuous agreement for ${STRESS_KILL_DURATION}s"
  local start_ts now elapsed
  start_ts="$(date +%s)"
  while :; do
    kn_sample "$survivor_peers" "$victim"; rc=$?
    if [ "$rc" -eq 2 ]; then log_fail "FORK among survivors during the kill"; assert_report "$report"; return 1; fi
    now="$(date +%s)"; elapsed=$(( now - start_ts ))
    [ "$elapsed" -ge "$STRESS_KILL_DURATION" ] && break
    sleep "$STRESS_INTERVAL"
  done
  local kill_end_min; kill_end_min="$(kn_survivor_min "$victim")"
  assert_progress "$kill_start_min" "$kill_end_min" 1 "survivors_during_kill"

  # Phase rejoin: restart the victim and require rejoin + catch-up to the majority.
  log_info "Phase rejoin: restarting node${victim} (full peer dial, same persisted data-dir)"
  cluster_restart_node "$victim"
  kn_init_prev
  kn_wait_rejoin "$victim" "$kill_end_min"; rc=$?
  if [ "$rc" -eq 2 ]; then log_fail "FORK during rejoin (victim re-synced a conflicting state_root)"; assert_report "$report"; return 1; fi

  # Phase post: all N agree again at full height.
  log_info "Phase post: all ${STRESS_NODES} nodes, expect peer_count ${STRESS_EXPECTED_PEERS} and agreement"
  kn_init_prev
  kn_sample_loop "$STRESS_POST_SAMPLES" "$STRESS_EXPECTED_PEERS" -1; rc=$?
  if [ "$rc" -eq 2 ]; then log_fail "FORK in post"; assert_report "$report"; return 1; fi

  assert_report "$report"
}

# --- Dry-run ---------------------------------------------------------------

# Verify cluster_build_argv in full mode dials all N-1 other nodes, localhost only.
kn_dry_check_rejoin_dial() {
  cluster_build_argv "$STRESS_VICTIM" full
  local peers
  peers="$(printf '%s\n' "${CLUSTER_ARGV[@]}" | grep -c '^--peer$' || true)"
  if [ "$peers" -ne "$(( STRESS_NODES - 1 ))" ]; then
    log_fail "rejoin dial: node${STRESS_VICTIM} should dial $(( STRESS_NODES - 1 )) peers, got ${peers}"; return 1
  fi
  if printf '%s\n' "${CLUSTER_ARGV[@]}" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+' | grep -vq '^127\.0\.0\.1$'; then
    log_fail "rejoin dial: non-local address present"; return 1
  fi
  log_ok "rejoin dial: node${STRESS_VICTIM} dials all $(( STRESS_NODES - 1 )) peers, localhost-only"
  return 0
}

# Verify cluster_build_argv in incremental mode is unchanged (node i dials i).
kn_dry_check_incremental() {
  local i peers bad=0
  for (( i = 0; i < STRESS_NODES; i++ )); do
    cluster_build_argv "$i"
    peers="$(printf '%s\n' "${CLUSTER_ARGV[@]}" | grep -c '^--peer$' || true)"
    [ "$peers" -ne "$i" ] && { log_fail "incremental node$i: expected $i peers, got $peers"; bad=1; }
  done
  [ "$bad" -eq 0 ] && { log_ok "incremental dial unchanged (node i dials i peers)"; return 0; }
  return 1
}

kn_dry_run() {
  log_info "DRY RUN: validating kill-node logic, gate, and commands (no cluster, no kill)."
  log_info "Plan: kill node${STRESS_VICTIM} of ${STRESS_NODES} (f=1); survivors expect peer_count $(( STRESS_EXPECTED_PEERS - 1 )); rejoin restores ${STRESS_EXPECTED_PEERS} and catches up to the majority state_root."
  log_info "  phases: baseline -> kill (${STRESS_KILL_DURATION}s) -> rejoin (timeout ${STRESS_REJOIN_TIMEOUT}s) -> post; continuous no-fork at every sample."
  echo
  local fails=0

  _expect() { # _expect <want_rc> <desc> <cmd...>
    local want="$1" desc="$2"; shift 2
    local rc=0
    "$@" >/dev/null 2>&1 || rc=$?
    if [ "$rc" -eq "$want" ]; then log_ok "dry-check: $desc (rc=$rc)"
    else log_fail "dry-check: $desc (want rc=$want, got $rc)"; fails=$(( fails + 1 )); fi
  }
  _dry_sr() { printf '%s\n' "$1" | sr_verdict_from_pairs 5; }
  _dry_gate() { STRESS_ENABLE_DESTRUCTIVE="$1" kn_gate_ok; }

  # Invariant primitives across the phases.
  _expect 0 "no-regression accepts a climb" assert_no_height_regression 10 12 dry
  _expect 1 "no-regression catches a regression" assert_no_height_regression 12 9 dry
  _expect 0 "round within bound" assert_round_bounded 3 20 dry
  _expect 1 "round over bound" assert_round_bounded 99 20 dry
  _expect 0 "baseline/post peer_count N-1" assert_peer_count_eq 3 3 dry
  _expect 0 "kill-phase survivor peer_count N-2" assert_peer_count_eq 2 2 dry
  _expect 1 "wrong peer_count caught" assert_peer_count_eq 3 2 dry

  # Continuous agreement (the locked-QC property).
  _expect 0 "survivors agree (3 reachable)" _dry_sr "node0|aa
node2|aa
node3|aa"
  _expect 1 "survivor fork caught" _dry_sr "node0|aa
node2|bb
node3|aa"
  _expect 1 "rejoined-node conflicting root caught" _dry_sr "node0|aa
node1|cc
node2|aa
node3|aa"

  # Destructive gate.
  _expect 1 "gate OFF refuses to run" _dry_gate 0
  _expect 0 "gate ON allows run" _dry_gate 1

  # Command construction (local-only).
  if ! kn_dry_check_rejoin_dial; then fails=$(( fails + 1 )); fi
  if ! kn_dry_check_incremental; then fails=$(( fails + 1 )); fi

  echo
  if [ "$fails" -eq 0 ]; then
    log_ok "kill-node dry-run: logic, gate, and commands validated"
    return 0
  fi
  log_fail "kill-node dry-run: $fails check(s) failed"
  return 1
}

# --- Flag parsing ----------------------------------------------------------
while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run)            KN_DRYRUN=1 ;;
    --enable-destructive) STRESS_ENABLE_DESTRUCTIVE=1 ;;
    --victim)             shift; STRESS_VICTIM="${1:-}" ;;
    --kill-duration)      shift; STRESS_KILL_DURATION="${1:-}" ;;
    --rejoin-timeout)     shift; STRESS_REJOIN_TIMEOUT="${1:-}" ;;
    --interval)           shift; STRESS_INTERVAL="${1:-}" ;;
    --round-max)          shift; STRESS_ROUND_MAX="${1:-}" ;;
    --nodes)              shift; STRESS_NODES="${1:-}"; STRESS_EXPECTED_PEERS=$(( STRESS_NODES - 1 )) ;;
    --report)             shift; KN_REPORT="${1:-}" ;;
    -h|--help)            usage; exit 0 ;;
    *) log_error "unknown flag: $1"; usage; exit 2 ;;
  esac
  shift
done

if [ "$KN_DRYRUN" -eq 1 ]; then
  kn_dry_run
  exit $?
fi
kn_live
exit $?
