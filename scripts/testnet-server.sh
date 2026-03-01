#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════════
# NOVAI 4-Validator Testnet on a Single Server
# ═══════════════════════════════════════════════════════════════════════════════
# PURPOSE: Launch, stop, and monitor a 4-validator testnet using nohup
#          (no tmux required — designed for headless VPS deployment)
#
# USAGE:
#   ./scripts/testnet-server.sh start   # Launch all 4 validators
#   ./scripts/testnet-server.sh stop    # Graceful shutdown
#   ./scripts/testnet-server.sh status  # Show heights + process info
#   ./scripts/testnet-server.sh logs    # Tail all 4 log files
#
# ENVIRONMENT VARIABLES (tune without editing script):
#   BASE_TIMEOUT         Consensus timeout in ms (default: 1000)
#   PROPOSAL_INTERVAL    Proposal interval in ms (default: 5, min 5)
#   BIN                  Path to novai-node binary
#   LOG_DIR              Path to log directory
#   DATA_DIR             Path to data directory
#
# NETWORK TOPOLOGY:
#   Validator 0 (seed):  Port 9000, Metrics 8080
#   Validator 1:         Port 9001, Metrics 8081 (peers to 0)
#   Validator 2:         Port 9002, Metrics 8082 (peers to 0)
#   Validator 3:         Port 9003, Metrics 8083 (peers to 0)
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# CONFIGURATION
# ─────────────────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

BIN="${BIN:-${PROJECT_ROOT}/target/release/novai-node}"
LOG_DIR="${LOG_DIR:-/var/log/novai}"
DATA_DIR="${DATA_DIR:-[redacted-server]/.novai/data}"
BASE_TIMEOUT="${BASE_TIMEOUT:-1000}"
PROPOSAL_INTERVAL="${PROPOSAL_INTERVAL:-5}"
PID_DIR="${LOG_DIR}/pids"

declare -a PORTS=(9000 9001 9002 9003)
declare -a METRICS_PORTS=(8080 8081 8082 8083)
NODE_COUNT=4

# ─────────────────────────────────────────────────────────────────────────────
# COLORS
# ─────────────────────────────────────────────────────────────────────────────

if [[ -t 1 ]]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    BOLD='\033[1m'
    NC='\033[0m'
else
    RED='' GREEN='' YELLOW='' BLUE='' BOLD='' NC=''
fi

log_info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
log_success() { echo -e "${GREEN}[OK]${NC} $*"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

# ─────────────────────────────────────────────────────────────────────────────
# HELPERS
# ─────────────────────────────────────────────────────────────────────────────

# Kill only OUR validator processes (by PID file), not any other novai-node
kill_our_validators() {
    local signal="${1:-TERM}"
    local killed=0

    for i in $(seq 0 $((NODE_COUNT - 1))); do
        local pidfile="${PID_DIR}/validator-${i}.pid"
        if [[ -f "${pidfile}" ]]; then
            local pid
            pid=$(cat "${pidfile}")
            if kill -0 "${pid}" 2>/dev/null; then
                kill "-${signal}" "${pid}" 2>/dev/null || true
                killed=$((killed + 1))
            fi
            rm -f "${pidfile}"
        fi
    done

    return ${killed}
}

# Check if a specific validator is running (by PID file)
validator_running() {
    local idx=$1
    local pidfile="${PID_DIR}/validator-${idx}.pid"
    if [[ -f "${pidfile}" ]]; then
        local pid
        pid=$(cat "${pidfile}")
        kill -0 "${pid}" 2>/dev/null && return 0
    fi
    return 1
}

# ─────────────────────────────────────────────────────────────────────────────
# START
# ─────────────────────────────────────────────────────────────────────────────

cmd_start() {
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo -e "  ${BOLD}NOVAI Server Testnet — Start${NC}"
    echo "═══════════════════════════════════════════════════════════════"
    echo ""

    # Preflight
    if [[ ! -x "${BIN}" ]]; then
        log_error "Binary not found: ${BIN}"
        log_info "Build with: cargo build --release -p novai-node"
        exit 1
    fi

    # Stop any existing validators we own
    local any_running=false
    for i in $(seq 0 $((NODE_COUNT - 1))); do
        if validator_running "${i}"; then
            any_running=true
            break
        fi
    done

    if [[ "${any_running}" == "true" ]]; then
        log_warn "Found running validators from a previous start — stopping them first"
        kill_our_validators "TERM"
        sleep 2
        # Force-kill stragglers
        kill_our_validators "KILL" 2>/dev/null || true
        sleep 1
    fi

    # Check ports
    for port in "${PORTS[@]}" "${METRICS_PORTS[@]}"; do
        if ss -tlnp 2>/dev/null | grep -q ":${port} " || \
           lsof -i ":${port}" 2>/dev/null | grep -q LISTEN; then
            log_error "Port ${port} is already in use"
            log_info "Check with: ss -tlnp | grep ${port}"
            exit 1
        fi
    done

    # Create directories
    mkdir -p "${LOG_DIR}" "${PID_DIR}" "${DATA_DIR}"

    log_info "Configuration:"
    log_info "  Binary:            ${BIN}"
    log_info "  Base timeout:      ${BASE_TIMEOUT}ms"
    log_info "  Proposal interval: ${PROPOSAL_INTERVAL}ms"
    log_info "  Log dir:           ${LOG_DIR}"
    log_info "  Data dir:          ${DATA_DIR}"
    echo ""

    # Launch validators
    for i in $(seq 0 $((NODE_COUNT - 1))); do
        local port=${PORTS[$i]}
        local metrics_port=${METRICS_PORTS[$i]}
        local logfile="${LOG_DIR}/node-${i}.log"

        # Build peer args: all nodes peer to validator 0
        local peer_args=""
        if [[ $i -gt 0 ]]; then
            peer_args="--peer 127.0.0.1:${PORTS[0]}"
        fi

        log_info "Starting validator ${i} (port=${port}, metrics=${metrics_port})..."

        nohup "${BIN}" run \
            --port "${port}" \
            --dev-keys --allow-insecure-dev-keys \
            --validator "${i}" \
            --metrics-port "${metrics_port}" \
            --base-timeout "${BASE_TIMEOUT}" \
            --proposal-interval "${PROPOSAL_INTERVAL}" \
            --storage rocksdb \
            --data-dir "${DATA_DIR}" \
            ${peer_args} \
            >> "${logfile}" 2>&1 &

        local pid=$!
        echo "${pid}" > "${PID_DIR}/validator-${i}.pid"
        log_success "Validator ${i} started (PID ${pid})"

        # Give seed node a head start
        if [[ $i -eq 0 ]]; then
            sleep 2
        else
            sleep 1
        fi
    done

    echo ""
    log_info "Waiting for network stabilization..."
    sleep 3

    # Verify all are running
    local all_ok=true
    for i in $(seq 0 $((NODE_COUNT - 1))); do
        if validator_running "${i}"; then
            log_success "Validator ${i}: running"
        else
            log_error "Validator ${i}: NOT running — check ${LOG_DIR}/node-${i}.log"
            all_ok=false
        fi
    done

    echo ""
    if [[ "${all_ok}" == "true" ]]; then
        echo "═══════════════════════════════════════════════════════════════"
        echo -e "  ${GREEN}All 4 validators running!${NC}"
        echo "═══════════════════════════════════════════════════════════════"
    else
        echo "═══════════════════════════════════════════════════════════════"
        echo -e "  ${RED}Some validators failed to start${NC}"
        echo "═══════════════════════════════════════════════════════════════"
        exit 1
    fi

    echo ""
    echo "  Check status:  ./scripts/testnet-server.sh status"
    echo "  View logs:     ./scripts/testnet-server.sh logs"
    echo "  Monitor:       ./scripts/monitor-testnet.sh --target 100000"
    echo "  Stop:          ./scripts/testnet-server.sh stop"
    echo ""
}

# ─────────────────────────────────────────────────────────────────────────────
# STOP
# ─────────────────────────────────────────────────────────────────────────────

cmd_stop() {
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo -e "  ${BOLD}NOVAI Server Testnet — Stop${NC}"
    echo "═══════════════════════════════════════════════════════════════"
    echo ""

    log_info "Sending SIGTERM to validators..."
    kill_our_validators "TERM"
    sleep 3

    # Check for stragglers
    local stragglers=false
    for i in $(seq 0 $((NODE_COUNT - 1))); do
        if validator_running "${i}"; then
            stragglers=true
            break
        fi
    done

    if [[ "${stragglers}" == "true" ]]; then
        log_warn "Some validators still running — sending SIGKILL"
        kill_our_validators "KILL"
        sleep 1
    fi

    # Clean PID files
    rm -f "${PID_DIR}"/validator-*.pid

    log_success "All validators stopped"
    echo ""
}

# ─────────────────────────────────────────────────────────────────────────────
# STATUS
# ─────────────────────────────────────────────────────────────────────────────

cmd_status() {
    echo ""
    echo "═══════════════════════════════════════════════════════════════════════════"
    echo -e "  ${BOLD}NOVAI Server Testnet Status${NC}                   $(date '+%Y-%m-%d %H:%M:%S')"
    echo "═══════════════════════════════════════════════════════════════════════════"
    echo ""

    printf "  ${BOLD}%-6s  %-6s  %-8s  %-10s  %-8s  %-6s  %-6s  %-12s${NC}\n" \
        "Node" "Port" "Metrics" "Process" "Height" "Round" "Peers" "ViewChanges"
    echo "  ──────────────────────────────────────────────────────────────────────────"

    local max_height=0

    for i in $(seq 0 $((NODE_COUNT - 1))); do
        local port=${PORTS[$i]}
        local metrics_port=${METRICS_PORTS[$i]}
        local proc_status

        if validator_running "${i}"; then
            proc_status="${GREEN}UP${NC}"
        else
            proc_status="${RED}DOWN${NC}"
        fi

        # Fetch metrics
        local metrics height round peers view_changes
        metrics=$(curl -s --connect-timeout 2 "http://localhost:${metrics_port}/metrics" 2>/dev/null || echo "")

        if [[ -n "${metrics}" ]]; then
            height=$(echo "${metrics}" | grep "^novai_committed_height " | awk '{print $2}' || echo "0")
            round=$(echo "${metrics}" | grep "^novai_current_round " | awk '{print $2}' || echo "0")
            peers=$(echo "${metrics}" | grep "^novai_peer_count " | awk '{print $2}' || echo "0")
            view_changes=$(echo "${metrics}" | grep "^novai_consensus_view_changes_total " | awk '{print $2}' || echo "0")
        else
            height="N/A"; round="N/A"; peers="N/A"; view_changes="N/A"
        fi

        if [[ "${height}" != "N/A" ]] && [[ "${height}" -gt "${max_height}" ]]; then
            max_height=${height}
        fi

        printf "  %-6s  %-6s  %-8s  %-19s  %-8s  %-6s  %-6s  %-12s\n" \
            "${i}" "${port}" "${metrics_port}" "${proc_status}" "${height}" "${round}" "${peers}" "${view_changes}"
    done

    echo "  ──────────────────────────────────────────────────────────────────────────"
    echo ""

    if [[ "${max_height}" -gt 0 ]]; then
        echo -e "  Highest committed block: ${GREEN}${max_height}${NC}"
    else
        echo -e "  Highest committed block: ${YELLOW}0${NC} (waiting for consensus...)"
    fi
    echo ""
}

# ─────────────────────────────────────────────────────────────────────────────
# LOGS
# ─────────────────────────────────────────────────────────────────────────────

cmd_logs() {
    log_info "Tailing all validator logs (Ctrl-C to stop)..."
    echo ""

    # Check log files exist
    local found=false
    for i in $(seq 0 $((NODE_COUNT - 1))); do
        if [[ -f "${LOG_DIR}/node-${i}.log" ]]; then
            found=true
        fi
    done

    if [[ "${found}" == "false" ]]; then
        log_error "No log files found in ${LOG_DIR}"
        exit 1
    fi

    tail -f "${LOG_DIR}"/node-*.log
}

# ─────────────────────────────────────────────────────────────────────────────
# MAIN
# ─────────────────────────────────────────────────────────────────────────────

case "${1:-}" in
    start)  cmd_start ;;
    stop)   cmd_stop ;;
    status) cmd_status ;;
    logs)   cmd_logs ;;
    *)
        echo "Usage: $0 {start|stop|status|logs}"
        echo ""
        echo "Environment variables:"
        echo "  BASE_TIMEOUT=1000       Consensus timeout (ms)"
        echo "  PROPOSAL_INTERVAL=5     Proposal interval (ms, min 5)"
        echo "  BIN=./target/release/novai-node"
        echo "  LOG_DIR=/var/log/novai"
        echo "  DATA_DIR=[redacted-server]/.novai/data"
        exit 1
        ;;
esac
