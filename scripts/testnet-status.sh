#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════════
# NOVAI Local Testnet Status
# ═══════════════════════════════════════════════════════════════════════════════
# PURPOSE: Check health and status of all testnet nodes
#
# USAGE:
#   ./scripts/testnet-status.sh [--watch]
#
# OPTIONS:
#   --watch    Continuously monitor (refresh every 2 seconds)
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
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
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ─────────────────────────────────────────────────────────────────────────────
# STATUS FUNCTIONS
# ─────────────────────────────────────────────────────────────────────────────

check_tmux_session() {
    if tmux has-session -t "${SESSION_NAME}" 2>/dev/null; then
        echo -e "${GREEN}RUNNING${NC}"
        return 0
    else
        echo -e "${RED}NOT RUNNING${NC}"
        return 1
    fi
}

check_port() {
    local port=$1
    if lsof -i ":${port}" &>/dev/null; then
        echo -e "${GREEN}UP${NC}"
        return 0
    else
        echo -e "${RED}DOWN${NC}"
        return 1
    fi
}

get_metric() {
    local port=$1
    local metric=$2
    local value
    value=$(curl -s --connect-timeout 1 "http://localhost:${port}/metrics" 2>/dev/null | grep "^${metric} " | awk '{print $2}' || echo "N/A")
    echo "${value}"
}

get_node_status() {
    local idx=$1
    local metrics_port=${METRICS_PORTS[$idx]}

    local height committed_height round peers view_changes

    # Try to get metrics
    local metrics
    metrics=$(curl -s --connect-timeout 1 "http://localhost:${metrics_port}/metrics" 2>/dev/null || echo "")

    if [[ -z "${metrics}" ]]; then
        echo "N/A|N/A|N/A|N/A"
        return 1
    fi

    committed_height=$(echo "${metrics}" | grep "^novai_committed_height " | awk '{print $2}' || echo "0")
    round=$(echo "${metrics}" | grep "^novai_current_round " | awk '{print $2}' || echo "0")
    peers=$(echo "${metrics}" | grep "^novai_peer_count " | awk '{print $2}' || echo "0")
    view_changes=$(echo "${metrics}" | grep "^novai_consensus_view_changes_total " | awk '{print $2}' || echo "0")

    echo "${committed_height}|${round}|${peers}|${view_changes}"
    return 0
}

print_status() {
    clear 2>/dev/null || true

    echo ""
    echo "═══════════════════════════════════════════════════════════════════════════"
    echo -e "  ${BOLD}NOVAI Local Testnet Status${NC}                   $(date '+%Y-%m-%d %H:%M:%S')"
    echo "═══════════════════════════════════════════════════════════════════════════"
    echo ""

    # Check tmux session
    echo -n "  Tmux Session (${SESSION_NAME}): "
    if ! check_tmux_session; then
        echo ""
        echo -e "  ${YELLOW}Testnet is not running.${NC}"
        echo -e "  Start with: ${CYAN}./scripts/testnet-local.sh${NC}"
        echo ""
        return 1
    fi
    echo ""

    # Table header
    echo ""
    printf "  ${BOLD}%-8s  %-6s  %-10s  %-8s  %-6s  %-6s  %-12s${NC}\n" \
        "Node" "Port" "Metrics" "Status" "Height" "Round" "Peers"
    echo "  ────────────────────────────────────────────────────────────────────────"

    local all_healthy=true
    local max_height=0

    for i in $(seq 0 $((NODE_COUNT - 1))); do
        local port=${PORTS[$i]}
        local metrics_port=${METRICS_PORTS[$i]}

        # Check if port is listening
        local status
        if lsof -i ":${port}" &>/dev/null; then
            status="${GREEN}UP${NC}"
        else
            status="${RED}DOWN${NC}"
            all_healthy=false
        fi

        # Get metrics
        local node_status
        node_status=$(get_node_status $i)
        IFS='|' read -r height round peers view_changes <<< "${node_status}"

        # Track max height
        if [[ "${height}" != "N/A" ]] && [[ "${height}" -gt "${max_height}" ]]; then
            max_height=${height}
        fi

        # Format node name
        local node_name="Node ${i}"
        if [[ $i -eq 0 ]]; then
            node_name="Node 0*"  # Mark seed
        fi

        printf "  %-8s  %-6s  %-10s  %-17s  %-6s  %-6s  %-6s\n" \
            "${node_name}" "${port}" "${metrics_port}" "${status}" "${height}" "${round}" "${peers}"
    done

    echo "  ────────────────────────────────────────────────────────────────────────"
    echo -e "  ${CYAN}* = seed node${NC}"
    echo ""

    # Summary
    echo "  ─────────────────────────────────────────────────────"
    echo -e "  ${BOLD}Summary:${NC}"
    if [[ "${max_height}" -gt 0 ]]; then
        echo -e "    Highest committed block: ${GREEN}${max_height}${NC}"
        echo -e "    Blocks are being produced! ${GREEN}✓${NC}"
    else
        echo -e "    Highest committed block: ${YELLOW}0${NC}"
        echo -e "    ${YELLOW}Waiting for first block...${NC}"
    fi
    echo ""

    # Quick commands
    echo "  ─────────────────────────────────────────────────────"
    echo -e "  ${BOLD}Quick Commands:${NC}"
    echo -e "    Attach to tmux:  ${CYAN}tmux attach -t ${SESSION_NAME}${NC}"
    echo -e "    View node 0 log: ${CYAN}tail -f ${LOG_DIR}/node-0.log${NC}"
    echo -e "    Stop testnet:    ${CYAN}./scripts/testnet-stop.sh${NC}"
    echo ""
    echo "═══════════════════════════════════════════════════════════════════════════"

    return 0
}

# ─────────────────────────────────────────────────────────────────────────────
# MAIN
# ─────────────────────────────────────────────────────────────────────────────

main() {
    if [[ "${1:-}" == "--watch" ]]; then
        echo "Watching testnet status (Ctrl-C to exit)..."
        while true; do
            print_status || true
            sleep 2
        done
    else
        print_status
    fi
}

main "$@"
