#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════════
# NOVAI Local Testnet Launcher
# ═══════════════════════════════════════════════════════════════════════════════
# PURPOSE: Launch a 4-validator local testnet in tmux for development/testing
#
# USAGE:
#   ./scripts/testnet-local.sh [--clean]
#
# OPTIONS:
#   --clean    Remove existing logs and start fresh
#
# NETWORK TOPOLOGY:
#   Node 0 (seed):  Port 9000, Metrics 8080
#   Node 1:         Port 9001, Metrics 8081 (peers to node 0)
#   Node 2:         Port 9002, Metrics 8082 (peers to node 0)
#   Node 3:         Port 9003, Metrics 8083 (peers to node 0)
#
# TMUX SESSION: novai-testnet
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# CONFIGURATION
# ─────────────────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BINARY="${PROJECT_ROOT}/target/release/novai-node"
SESSION_NAME="novai-testnet"
LOG_DIR="${PROJECT_ROOT}/testnet-logs"

# Node configuration
declare -a PORTS=(9000 9001 9002 9003)
declare -a METRICS_PORTS=(8080 8081 8082 8083)
NODE_COUNT=4

# ─────────────────────────────────────────────────────────────────────────────
# COLORS
# ─────────────────────────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# ─────────────────────────────────────────────────────────────────────────────
# PREFLIGHT CHECKS
# ─────────────────────────────────────────────────────────────────────────────

preflight_checks() {
    log_info "Running preflight checks..."

    # Check tmux is installed
    if ! command -v tmux &> /dev/null; then
        log_error "tmux is not installed. Install with: brew install tmux"
        exit 1
    fi

    # Check binary exists
    if [[ ! -x "${BINARY}" ]]; then
        log_error "Binary not found: ${BINARY}"
        log_info "Building release binary..."
        (cd "${PROJECT_ROOT}" && cargo build --release -p novai-node)
        if [[ ! -x "${BINARY}" ]]; then
            log_error "Build failed"
            exit 1
        fi
    fi

    # Check if session already exists
    if tmux has-session -t "${SESSION_NAME}" 2>/dev/null; then
        log_warn "Testnet session '${SESSION_NAME}' already running"
        log_info "Use './scripts/testnet-stop.sh' to stop it first, or attach with:"
        log_info "  tmux attach -t ${SESSION_NAME}"
        exit 1
    fi

    # Check ports are available
    for port in "${PORTS[@]}" "${METRICS_PORTS[@]}"; do
        if lsof -i ":${port}" &>/dev/null; then
            log_error "Port ${port} is already in use"
            exit 1
        fi
    done

    log_success "Preflight checks passed"
}

# ─────────────────────────────────────────────────────────────────────────────
# SETUP
# ─────────────────────────────────────────────────────────────────────────────

setup_directories() {
    log_info "Setting up directories..."

    # Handle --clean flag
    if [[ "${1:-}" == "--clean" ]]; then
        log_info "Cleaning old logs..."
        rm -rf "${LOG_DIR}"
    fi

    mkdir -p "${LOG_DIR}"
    log_success "Log directory: ${LOG_DIR}"
}

# ─────────────────────────────────────────────────────────────────────────────
# LAUNCH NODES
# ─────────────────────────────────────────────────────────────────────────────

launch_testnet() {
    log_info "Launching testnet in tmux session '${SESSION_NAME}'..."

    # Create new tmux session with first node
    tmux new-session -d -s "${SESSION_NAME}" -n "nodes"

    # Split into 4 panes (2x2 grid)
    tmux split-window -h -t "${SESSION_NAME}:nodes"
    tmux split-window -v -t "${SESSION_NAME}:nodes.0"
    tmux split-window -v -t "${SESSION_NAME}:nodes.1"

    # Start Node 0 (seed) - no peers
    local cmd0="${BINARY} run --port ${PORTS[0]} --validator 0 --metrics-port ${METRICS_PORTS[0]} 2>&1 | tee ${LOG_DIR}/node-0.log"
    tmux send-keys -t "${SESSION_NAME}:nodes.0" "echo '=== Node 0 (Seed) ===' && ${cmd0}" C-m

    # Give seed node time to start
    sleep 2

    # Start Nodes 1-3 - FULL MESH: each node connects to ALL previous nodes
    # This ensures all nodes can see each other's messages for consensus
    for i in 1 2 3; do
        # Build peer list: connect to all nodes with lower index
        local peer_args=""
        for j in $(seq 0 $((i - 1))); do
            peer_args="${peer_args} --peer 127.0.0.1:${PORTS[$j]}"
        done

        local cmd="${BINARY} run --port ${PORTS[$i]}${peer_args} --validator ${i} --metrics-port ${METRICS_PORTS[$i]} 2>&1 | tee ${LOG_DIR}/node-${i}.log"
        tmux send-keys -t "${SESSION_NAME}:nodes.${i}" "echo '=== Node ${i} ===' && ${cmd}" C-m
        sleep 1
    done

    # Add a control pane at the bottom
    tmux split-window -v -t "${SESSION_NAME}:nodes.2" -l 5
    tmux send-keys -t "${SESSION_NAME}:nodes.4" "echo 'Control pane - run commands here'" C-m

    # Select first pane
    tmux select-pane -t "${SESSION_NAME}:nodes.0"

    log_success "Testnet launched!"
}

# ─────────────────────────────────────────────────────────────────────────────
# MAIN
# ─────────────────────────────────────────────────────────────────────────────

main() {
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo "  NOVAI Local Testnet Launcher"
    echo "═══════════════════════════════════════════════════════════════"
    echo ""

    preflight_checks
    setup_directories "$@"
    launch_testnet

    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo "  Testnet is running!"
    echo "═══════════════════════════════════════════════════════════════"
    echo ""
    echo "  Nodes:"
    for i in $(seq 0 $((NODE_COUNT - 1))); do
        echo "    Node ${i}: port=${PORTS[$i]}, metrics=http://localhost:${METRICS_PORTS[$i]}/metrics"
    done
    echo ""
    echo "  Logs: ${LOG_DIR}/"
    echo ""
    echo "  Commands:"
    echo "    Attach:  tmux attach -t ${SESSION_NAME}"
    echo "    Status:  ./scripts/testnet-status.sh"
    echo "    Stop:    ./scripts/testnet-stop.sh"
    echo ""
    echo "  In tmux:"
    echo "    Switch panes: Ctrl-b + arrow keys"
    echo "    Detach:       Ctrl-b + d"
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
}

main "$@"
