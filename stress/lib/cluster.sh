#!/bin/bash
# stress/lib/cluster.sh
# Local FULL-MESH devnet lifecycle for the stress framework.
#
# Brings up an N-node local cluster by invoking the node binary directly and
# wiring a full mesh (node i dials nodes 0..i-1, exactly like devnet.sh), so
# every node connects to the other N-1 and novai_peer_count is uniform at N-1
# across the cluster. This mirrors the production topology (every node peered to
# all others), which is what the stress test should exercise.
#
# Each node gets its own P2P, RPC, and metrics ports (base + index), its own
# rocksdb data-dir, and a PID file for clean per-node control.
#
# Safety: every node binds and dials 127.0.0.1 only. The flag line is built from
# LOCAL devnet config (port offsets, local data-dir), never a production value or
# remote endpoint. A localhost guard refuses any non-local host.
#
# This module owns full-cluster start/stop/status/readiness AND per-node kill and
# restart (used by the kill-node fault scenario).

if [ -n "${STRESS_CLUSTER_SOURCED:-}" ]; then
  return 0
fi
STRESS_CLUSTER_SOURCED=1

_CLUSTER_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
. "$_CLUSTER_DIR/common.sh"

cluster_node_pidfile() { printf '%s/validator-%s.pid' "$STRESS_RUN_DIR" "$1"; }
cluster_node_logfile() { printf '%s/node-%s.log' "$STRESS_LOG_DIR" "$1"; }
cluster_node_datadir() { printf '%s/%s' "$STRESS_DATA_DIR" "$1"; }

# Populate the global array CLUSTER_ARGV with the exact `novai-node run` argv for
# node i. Built from LOCAL config only: 127.0.0.1 binds and peers, per-node local
# data-dir, port offsets. Never a remote endpoint or production value.
#
# peer_mode (optional, default "incremental"):
#   incremental : dial nodes 0..i-1 only (used at initial start; each pair is
#                 dialed once by the higher-indexed node, so peer_count == N-1).
#   full        : dial every other node (used on rejoin so any victim, not just
#                 the highest-indexed one, re-establishes all its connections;
#                 peers that did not re-dial it get a fresh inbound instead).
cluster_build_argv() {
  local i="$1" peer_mode="${2:-incremental}" j
  assert_localhost_or_die "http://$STRESS_HOST" "cluster host"
  CLUSTER_ARGV=(
    "$STRESS_NODE_BIN" run
    --port "$(( STRESS_P2P_BASE + i ))"
    --rpc-port "$(( STRESS_RPC_BASE + i ))"
    --metrics-port "$(( STRESS_METRICS_BASE + i ))"
    --dev-keys --allow-insecure-dev-keys
    --validator "$i"
    --storage rocksdb
    --data-dir "$(cluster_node_datadir "$i")"
    --base-timeout "$STRESS_BASE_TIMEOUT"
    --proposal-interval "$STRESS_PROPOSAL_INTERVAL"
  )
  for (( j = 0; j < STRESS_NODES; j++ )); do
    [ "$j" -eq "$i" ] && continue
    if [ "$peer_mode" = "incremental" ] && [ "$j" -ge "$i" ]; then continue; fi
    CLUSTER_ARGV+=( --peer "127.0.0.1:$(( STRESS_P2P_BASE + j ))" )
  done
}

# Check whether validator i is running (by its PID file).
cluster_validator_running() {
  local pf pid
  pf="$(cluster_node_pidfile "$1")"
  [ -f "$pf" ] || return 1
  pid="$(cat "$pf" 2>/dev/null || true)"
  [ -n "$pid" ] || return 1
  kill -0 "$pid" 2>/dev/null
}

# Verify the node binary exists; exit with a clear message otherwise.
cluster_build_check() {
  if [ ! -x "$STRESS_NODE_BIN" ]; then
    log_error "node binary not found or not executable: $STRESS_NODE_BIN"
    log_error "Build it with: cargo build --release -p novai-node"
    exit 5
  fi
}

# Launch a single validator (nohup, PID file). peer_mode is passed through to
# cluster_build_argv. Internal helper used by cluster_start and cluster_restart_node.
_cluster_launch_node() {
  local i="$1" peer_mode="${2:-incremental}" pid
  mkdir -p "$(cluster_node_datadir "$i")"
  cluster_build_argv "$i" "$peer_mode"
  nohup "${CLUSTER_ARGV[@]}" >> "$(cluster_node_logfile "$i")" 2>&1 &
  pid=$!
  echo "$pid" > "$(cluster_node_pidfile "$i")"
  printf '%s' "$pid"
}

# Start the full-mesh cluster. Localhost only.
cluster_start() {
  assert_localhost_or_die "http://$STRESS_HOST" "cluster host"
  cluster_build_check
  mkdir -p "$STRESS_LOG_DIR" "$STRESS_RUN_DIR" "$STRESS_DATA_DIR"
  log_info "Starting local full-mesh devnet: $STRESS_NODES node(s) on 127.0.0.1 (P2P ${STRESS_P2P_BASE}+, rpc ${STRESS_RPC_BASE}+, metrics ${STRESS_METRICS_BASE}+)"
  local i pid
  for (( i = 0; i < STRESS_NODES; i++ )); do
    if cluster_validator_running "$i"; then
      log_warn "validator $i already running (pid $(cat "$(cluster_node_pidfile "$i")")); leaving as is"
      continue
    fi
    pid="$(_cluster_launch_node "$i" incremental)"
    log_ok "validator $i started (pid $pid, port $(( STRESS_P2P_BASE + i )))"
    # Give the seed node a head start so leaves have something to dial.
    if [ "$i" -eq 0 ]; then sleep 2; else sleep 1; fi
  done
}

# Kill a single validator by its PID file (SIGTERM, then SIGKILL if needed).
# Kills only the node recorded in our PID file, never some other novai-node.
cluster_kill_node() {
  local i="$1" pf pid
  pf="$(cluster_node_pidfile "$i")"
  if [ ! -f "$pf" ]; then
    log_warn "no PID file for validator $i; nothing to kill"
    return 1
  fi
  pid="$(cat "$pf" 2>/dev/null || true)"
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
    sleep 2
    if kill -0 "$pid" 2>/dev/null; then kill -KILL "$pid" 2>/dev/null || true; fi
  fi
  rm -f "$pf"
  log_ok "validator $i killed"
}

# Restart a single validator, dialing ALL other nodes (full peer mode) so any
# victim rejoins the full mesh, reusing its persisted data-dir so it re-syncs.
cluster_restart_node() {
  local i="$1" pid
  assert_localhost_or_die "http://$STRESS_HOST" "cluster host"
  if cluster_validator_running "$i"; then
    log_warn "validator $i already running; not restarting"
    return 0
  fi
  pid="$(_cluster_launch_node "$i" full)"
  log_ok "validator $i restarted (pid $pid, dialing all peers to rejoin the mesh)"
}

# Stop the cluster. Only kills nodes recorded in our PID files (never some other
# novai-node process on the machine).
cluster_stop() {
  local i pf pid killed=0
  for (( i = 0; i < STRESS_NODES; i++ )); do
    pf="$(cluster_node_pidfile "$i")"
    [ -f "$pf" ] || continue
    pid="$(cat "$pf" 2>/dev/null || true)"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
      kill -TERM "$pid" 2>/dev/null || true
      killed=$(( killed + 1 ))
    fi
  done
  [ "$killed" -gt 0 ] && sleep 3
  for (( i = 0; i < STRESS_NODES; i++ )); do
    pf="$(cluster_node_pidfile "$i")"
    [ -f "$pf" ] || continue
    pid="$(cat "$pf" 2>/dev/null || true)"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
      kill -KILL "$pid" 2>/dev/null || true
    fi
    rm -f "$pf"
  done
  log_ok "cluster stopped"
}

# Wait until every node is reachable and has committed at least one block.
cluster_wait_ready() {
  local timeout="${1:-$STRESS_READY_TIMEOUT}"
  local start now i h ready
  start="$(date +%s)"
  log_info "Waiting up to ${timeout}s for all $STRESS_NODES node(s) to start committing..."
  while :; do
    ready=1
    for (( i = 0; i < STRESS_NODES; i++ )); do
      h="$(get_node_metric "$i" novai_committed_height 2>/dev/null || true)"
      if ! is_uint "$h" || [ "$h" -lt 1 ]; then ready=0; break; fi
    done
    if [ "$ready" -eq 1 ]; then log_ok "all nodes committing"; return 0; fi
    now="$(date +%s)"
    if [ "$(( now - start ))" -ge "$timeout" ]; then
      log_error "cluster not ready after ${timeout}s (node $i height='$h')"
      return 1
    fi
    sleep 2
  done
}

# Print a per-node status table.
cluster_status() {
  printf '%-6s %-6s %-8s %-7s %-7s\n' "node" "proc" "height" "round" "peers"
  local i proc h r p
  for (( i = 0; i < STRESS_NODES; i++ )); do
    if cluster_validator_running "$i"; then proc="UP"; else proc="DOWN"; fi
    h="$(get_node_metric "$i" novai_committed_height 2>/dev/null || true)"; [ -n "$h" ] || h="-"
    r="$(get_node_metric "$i" novai_current_round 2>/dev/null || true)";    [ -n "$r" ] || r="-"
    p="$(get_node_metric "$i" novai_peer_count 2>/dev/null || true)";       [ -n "$p" ] || p="-"
    printf '%-6s %-6s %-8s %-7s %-7s\n' "$i" "$proc" "$h" "$r" "$p"
  done
}

# --- CLI -------------------------------------------------------------------
# Runs only when executed directly, not when sourced by a scenario.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
  set -uo pipefail
  case "${1:-}" in
    plan)
      for (( _i = 0; _i < STRESS_NODES; _i++ )); do
        cluster_build_argv "$_i" "${2:-incremental}"
        printf 'node %s: ' "$_i"; printf '%s ' "${CLUSTER_ARGV[@]}"; printf '\n'
      done
      ;;
    start)   stress_preflight; cluster_start; cluster_wait_ready ;;
    stop)    cluster_stop ;;
    status)  cluster_status ;;
    ready)   cluster_wait_ready ;;
    kill)    cluster_kill_node "${2:?usage: cluster.sh kill <node-index>}" ;;
    restart) cluster_restart_node "${2:?usage: cluster.sh restart <node-index>}" ;;
    *)
      printf 'Usage: %s {plan [mode]|start|stop|status|ready|kill <i>|restart <i>}\n' "$0"
      exit 2
      ;;
  esac
fi
