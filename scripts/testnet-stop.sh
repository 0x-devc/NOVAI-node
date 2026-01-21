#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════════
# NOVAI Local Testnet Shutdown
# ═══════════════════════════════════════════════════════════════════════════════
# PURPOSE: Gracefully stop all testnet nodes and cleanup
#
# USAGE:
#   ./scripts/testnet-stop.sh [--force]
#
# OPTIONS:
#   --force    Kill processes immediately (SIGKILL instead of SIGTERM)
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

SESSION_NAME="novai-testnet"

# ─────────────────────────────────────────────────────────────────────────────
# COLORS
# ─────────────────────────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# ─────────────────────────────────────────────────────────────────────────────
# MAIN
# ─────────────────────────────────────────────────────────────────────────────

main() {
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo "  NOVAI Local Testnet Shutdown"
    echo "═══════════════════════════════════════════════════════════════"
    echo ""

    local force="${1:-}"

    # Check if tmux session exists
    if ! tmux has-session -t "${SESSION_NAME}" 2>/dev/null; then
        log_warn "No testnet session '${SESSION_NAME}' found"

        # Check for orphan processes
        local orphans
        orphans=$(pgrep -f "novai-node run" 2>/dev/null || true)
        if [[ -n "${orphans}" ]]; then
            log_warn "Found orphan novai-node processes: ${orphans}"
            if [[ "${force}" == "--force" ]]; then
                log_info "Force killing orphan processes..."
                pkill -9 -f "novai-node run" || true
                log_success "Orphan processes killed"
            else
                log_info "Run with --force to kill them, or:"
                log_info "  pkill -f 'novai-node run'"
            fi
        fi
        exit 0
    fi

    log_info "Stopping testnet session '${SESSION_NAME}'..."

    if [[ "${force}" == "--force" ]]; then
        log_info "Force killing (SIGKILL)..."
        # Kill all processes in the session
        tmux list-panes -t "${SESSION_NAME}" -F '#{pane_pid}' | xargs -I {} kill -9 {} 2>/dev/null || true
    else
        log_info "Graceful shutdown (SIGTERM)..."
        # Send Ctrl-C to all panes
        for pane in $(tmux list-panes -t "${SESSION_NAME}" -F '#{pane_index}'); do
            tmux send-keys -t "${SESSION_NAME}:nodes.${pane}" C-c 2>/dev/null || true
        done
        sleep 2
    fi

    # Kill the tmux session
    tmux kill-session -t "${SESSION_NAME}" 2>/dev/null || true

    log_success "Testnet stopped"

    # Double-check no processes remain
    sleep 1
    local remaining
    remaining=$(pgrep -f "novai-node run" 2>/dev/null || true)
    if [[ -n "${remaining}" ]]; then
        log_warn "Some processes still running: ${remaining}"
        log_info "Killing remaining processes..."
        pkill -9 -f "novai-node run" || true
        log_success "All processes terminated"
    fi

    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo "  Testnet shutdown complete"
    echo "═══════════════════════════════════════════════════════════════"
    echo ""
}

main "$@"
