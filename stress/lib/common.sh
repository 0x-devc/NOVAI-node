#!/bin/bash
# stress/lib/common.sh
# Shared configuration, safety guards, preflight, logging, and node-query helpers
# for the NOVAI stress-testing framework. Sourced by run.sh and every scenario.
#
# Safety posture: the framework targets a LOCAL devnet only. Endpoints default to
# 127.0.0.1 with the standard NOVAI port offsets. A localhost-only guard refuses
# any non-local target. No production values live in this file.

# Guard against double-sourcing.
if [ -n "${STRESS_COMMON_SOURCED:-}" ]; then
  return 0
fi
STRESS_COMMON_SOURCED=1

# ---------------------------------------------------------------------------
# Configuration (override via environment or stress.env)
# ---------------------------------------------------------------------------

# Number of validators in the local cluster.
STRESS_NODES="${STRESS_NODES:-4}"

# Host the local cluster binds to. Localhost only by construction.
STRESS_HOST="${STRESS_HOST:-127.0.0.1}"

# Port bases, matching the repo convention (P2P 9000+i, metrics 8080+i, rpc 3030+i).
STRESS_P2P_BASE="${STRESS_P2P_BASE:-9000}"
STRESS_METRICS_BASE="${STRESS_METRICS_BASE:-8080}"
STRESS_RPC_BASE="${STRESS_RPC_BASE:-3030}"

# Runtime directories. Kept under $HOME, gitignored, never committed.
STRESS_LOG_DIR="${STRESS_LOG_DIR:-$HOME/.novai/stress-logs}"
STRESS_DATA_DIR="${STRESS_DATA_DIR:-$HOME/.novai/stress-data}"
STRESS_REPORT_DIR="${STRESS_REPORT_DIR:-$HOME/.novai/stress-reports}"

# Destructive actions (kill-node, high-rate load) are OFF unless explicitly enabled.
STRESS_ENABLE_DESTRUCTIVE="${STRESS_ENABLE_DESTRUCTIVE:-0}"

# curl connect timeout (seconds) for metric and RPC probes.
STRESS_CONNECT_TIMEOUT="${STRESS_CONNECT_TIMEOUT:-3}"

# Defaults consumed by later scenarios (documented in stress.env.example).
STRESS_ROUND_MAX="${STRESS_ROUND_MAX:-20}"
STRESS_DURATION="${STRESS_DURATION:-120}"
STRESS_INTERVAL="${STRESS_INTERVAL:-5}"

# ---------------------------------------------------------------------------
# Colors (disabled when stdout is not a TTY)
# ---------------------------------------------------------------------------
if [ -t 1 ]; then
  C_RED=$'\033[0;31m'; C_GREEN=$'\033[0;32m'; C_YELLOW=$'\033[1;33m'
  C_BLUE=$'\033[0;34m'; C_BOLD=$'\033[1m'; C_NC=$'\033[0m'
else
  C_RED=''; C_GREEN=''; C_YELLOW=''; C_BLUE=''; C_BOLD=''; C_NC=''
fi

stress_ts() { date -u '+%Y-%m-%dT%H:%M:%SZ'; }

log_info()  { printf '%s[INFO]%s %s\n'  "$C_BLUE"   "$C_NC" "$*"; }
log_ok()    { printf '%s[OK]%s %s\n'    "$C_GREEN"  "$C_NC" "$*"; }
log_warn()  { printf '%s[WARN]%s %s\n'  "$C_YELLOW" "$C_NC" "$*"; }
log_error() { printf '%s[ERROR]%s %s\n' "$C_RED"    "$C_NC" "$*" >&2; }
# Loud failure marker used by assertions and the fork check.
log_fail()  { printf '%s[FAIL]%s %s\n'  "$C_RED"    "$C_NC" "$*" >&2; }

# ---------------------------------------------------------------------------
# Localhost-only safety guard
# ---------------------------------------------------------------------------

# Extract the host portion from a URL or host:port string.
url_host() {
  local s="$1"
  s="${s#*://}"   # strip scheme
  s="${s%%/*}"    # strip path
  s="${s%%:*}"    # strip port
  printf '%s' "$s"
}

# Return 0 if the given host or URL is local, 1 otherwise.
is_localhost() {
  local h
  h="$(url_host "$1")"
  case "$h" in
    127.0.0.1|localhost|::1|0.0.0.0) return 0 ;;
    *) return 1 ;;
  esac
}

# Hard guard: refuse to proceed if the target is not local. Used by every
# destructive path and by cluster.sh. Exits non-zero on a non-local target.
assert_localhost_or_die() {
  local target="$1" context="${2:-target}"
  if ! is_localhost "$target"; then
    log_error "Refusing non-local ${context}: ${target}"
    log_error "The stress framework operates on a LOCAL devnet only (127.0.0.1)."
    exit 3
  fi
}

# ---------------------------------------------------------------------------
# Preflight
# ---------------------------------------------------------------------------

# Verify a list of commands exist; exit with a clear message if any is missing.
require_cmd() {
  local missing=0 c
  for c in "$@"; do
    if ! command -v "$c" >/dev/null 2>&1; then
      log_error "Required command not found: $c"
      missing=1
    fi
  done
  if [ "$missing" -ne 0 ]; then
    log_error "Install the missing prerequisites and retry."
    exit 4
  fi
}

# Standard preflight for any scenario that queries nodes.
stress_preflight() {
  require_cmd curl jq
  mkdir -p "$STRESS_LOG_DIR" "$STRESS_REPORT_DIR" 2>/dev/null || true
}

# ---------------------------------------------------------------------------
# Node endpoint helpers
# ---------------------------------------------------------------------------

node_metrics_port() { printf '%s' "$(( STRESS_METRICS_BASE + $1 ))"; }
node_rpc_port()     { printf '%s' "$(( STRESS_RPC_BASE + $1 ))"; }

node_metrics_url() { printf 'http://%s:%s/metrics' "$STRESS_HOST" "$(node_metrics_port "$1")"; }
node_rpc_url()     { printf 'http://%s:%s' "$STRESS_HOST" "$(node_rpc_port "$1")"; }

# ---------------------------------------------------------------------------
# Metrics scrape (reuses the grep/awk idiom from testnet-status.sh)
# ---------------------------------------------------------------------------

# scrape_metric <metrics_url> <metric_name> -> prints value; returns 1 if the
# endpoint is unreachable. A reachable endpoint with no such metric prints empty.
scrape_metric() {
  local url="$1" name="$2" body val
  body="$(curl -s --connect-timeout "$STRESS_CONNECT_TIMEOUT" "$url" 2>/dev/null || true)"
  [ -z "$body" ] && return 1
  val="$(printf '%s\n' "$body" | grep "^${name} " | awk '{print $2}' | head -n1 || true)"
  printf '%s' "$val"
}

# get_node_metric <node_index> <metric_name>
get_node_metric() { scrape_metric "$(node_metrics_url "$1")" "$2"; }

# ---------------------------------------------------------------------------
# JSON-RPC helpers
# ---------------------------------------------------------------------------

# rpc_call <rpc_url> <method> <params_json> -> prints raw JSON response.
rpc_call() {
  local url="$1" method="$2" params="$3"
  curl -s --connect-timeout "$STRESS_CONNECT_TIMEOUT" \
    -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"${method}\",\"params\":${params},\"id\":1}" \
    "$url" 2>/dev/null || true
}

# rpc_latest_height <rpc_url> -> prints committed height, or empty on failure.
rpc_latest_height() {
  local resp
  resp="$(rpc_call "$1" novai_getLatestBlock '[]')"
  printf '%s' "$resp" | jq -r '.result.height // empty' 2>/dev/null || true
}

# rpc_state_root_at_height <rpc_url> <height>
#   prints the state_root hex on success.
#   returns 1 (and prints nothing) if the node has not committed that height
#   (RPC error object) or the block is absent (null result) or the node is down.
rpc_state_root_at_height() {
  local url="$1" height="$2" resp err root
  resp="$(rpc_call "$url" novai_getBlockByHeight "{\"height\":${height}}")"
  [ -z "$resp" ] && return 1
  err="$(printf '%s' "$resp" | jq -r '.error.code // empty' 2>/dev/null || true)"
  [ -n "$err" ] && return 1
  root="$(printf '%s' "$resp" | jq -r '.result.state_root // empty' 2>/dev/null || true)"
  [ -z "$root" ] && return 1
  printf '%s' "$root"
}

# is_uint <value> -> 0 if value is a non-negative integer, 1 otherwise.
is_uint() {
  case "$1" in
    ''|*[!0-9]*) return 1 ;;
    *) return 0 ;;
  esac
}
