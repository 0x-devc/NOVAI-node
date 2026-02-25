#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════════
# NOVAI Testnet Monitor
# ═══════════════════════════════════════════════════════════════════════════════
# PURPOSE: Track block height, calculate throughput, detect stalls, show ETA
#
# USAGE:
#   ./scripts/monitor-testnet.sh [--target <blocks>] [--interval <secs>] [--port <metrics_port>]
#
# OPTIONS:
#   --target    Target block height (default: 100000)
#   --interval  Poll interval in seconds (default: 10)
#   --port      Metrics port of node to monitor (default: 8080)
#
# ALERTS:
#   - Warns if no height increase for 30 seconds (consensus stall)
#   - Reports view changes as indicator of leader failures
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# CONFIGURATION
# ─────────────────────────────────────────────────────────────────────────────

TARGET=100000
INTERVAL=10
METRICS_PORT=8080
STALL_THRESHOLD=30  # seconds without height increase → stall warning

# ─────────────────────────────────────────────────────────────────────────────
# PARSE ARGS
# ─────────────────────────────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target)   TARGET="$2"; shift 2 ;;
        --interval) INTERVAL="$2"; shift 2 ;;
        --port)     METRICS_PORT="$2"; shift 2 ;;
        --help|-h)
            echo "Usage: $0 [--target <blocks>] [--interval <secs>] [--port <metrics_port>]"
            echo ""
            echo "  --target    Target block height (default: 100000)"
            echo "  --interval  Poll interval in seconds (default: 10)"
            echo "  --port      Metrics port to poll (default: 8080)"
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

# ─────────────────────────────────────────────────────────────────────────────
# COLORS
# ─────────────────────────────────────────────────────────────────────────────

if [[ -t 1 ]]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    CYAN='\033[0;36m'
    BOLD='\033[1m'
    NC='\033[0m'
else
    RED='' GREEN='' YELLOW='' BLUE='' CYAN='' BOLD='' NC=''
fi

# ─────────────────────────────────────────────────────────────────────────────
# STATE
# ─────────────────────────────────────────────────────────────────────────────

METRICS_URL="http://localhost:${METRICS_PORT}/metrics"
START_TIME=$(date +%s)
PREV_HEIGHT=0
PREV_TIME=${START_TIME}
LAST_PROGRESS_TIME=${START_TIME}
PREV_VIEW_CHANGES=0
SAMPLE_COUNT=0

# Rolling average (last 6 samples)
declare -a BPS_SAMPLES=()
MAX_SAMPLES=6

# ─────────────────────────────────────────────────────────────────────────────
# HELPERS
# ─────────────────────────────────────────────────────────────────────────────

get_metric() {
    local name="$1"
    local metrics
    metrics=$(curl -s --connect-timeout 3 "${METRICS_URL}" 2>/dev/null || echo "")
    if [[ -z "${metrics}" ]]; then
        echo ""
        return 1
    fi
    echo "${metrics}" | grep "^${name} " | awk '{print $2}' || echo ""
}

format_duration() {
    local secs=$1
    if [[ ${secs} -lt 0 ]]; then
        echo "N/A"
        return
    fi
    local hours=$((secs / 3600))
    local mins=$(( (secs % 3600) / 60 ))
    local s=$((secs % 60))
    if [[ ${hours} -gt 0 ]]; then
        printf "%dh %02dm %02ds" ${hours} ${mins} ${s}
    elif [[ ${mins} -gt 0 ]]; then
        printf "%dm %02ds" ${mins} ${s}
    else
        printf "%ds" ${s}
    fi
}

rolling_avg() {
    local sum=0
    local count=${#BPS_SAMPLES[@]}
    if [[ ${count} -eq 0 ]]; then
        echo "0"
        return
    fi
    for v in "${BPS_SAMPLES[@]}"; do
        sum=$(echo "${sum} + ${v}" | bc -l 2>/dev/null || echo "0")
    done
    echo "scale=2; ${sum} / ${count}" | bc -l 2>/dev/null || echo "0"
}

# ─────────────────────────────────────────────────────────────────────────────
# MAIN LOOP
# ─────────────────────────────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo -e "  ${BOLD}NOVAI Testnet Monitor${NC}"
echo "═══════════════════════════════════════════════════════════════"
echo ""
echo "  Target:   ${TARGET} blocks"
echo "  Interval: ${INTERVAL}s"
echo "  Metrics:  ${METRICS_URL}"
echo ""
echo -e "  ${CYAN}Press Ctrl-C to stop monitoring${NC}"
echo ""
echo "  ──────────────────────────────────────────────────────────────"
printf "  ${BOLD}%-12s  %-10s  %-10s  %-12s  %-8s  %-12s${NC}\n" \
    "Time" "Height" "Blk/s" "ETA" "Peers" "ViewChanges"
echo "  ──────────────────────────────────────────────────────────────"

# Trap Ctrl-C for summary
trap 'print_summary; exit 0' INT TERM

print_summary() {
    local end_time
    end_time=$(date +%s)
    local elapsed=$((end_time - START_TIME))
    local final_height
    final_height=$(get_metric "novai_committed_height" 2>/dev/null || echo "${PREV_HEIGHT}")
    final_height=${final_height:-${PREV_HEIGHT}}
    local final_vc
    final_vc=$(get_metric "novai_consensus_view_changes_total" 2>/dev/null || echo "0")
    final_vc=${final_vc:-0}

    local total_blocks=$((final_height))
    local avg_bps="0"
    if [[ ${elapsed} -gt 0 ]]; then
        avg_bps=$(echo "scale=2; ${total_blocks} / ${elapsed}" | bc -l 2>/dev/null || echo "0")
    fi

    echo ""
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo -e "  ${BOLD}Monitor Summary${NC}"
    echo "═══════════════════════════════════════════════════════════════"
    echo "  Duration:       $(format_duration ${elapsed})"
    echo "  Final height:   ${final_height}"
    echo "  Avg blocks/sec: ${avg_bps}"
    echo "  View changes:   ${final_vc}"
    if [[ ${final_height} -ge ${TARGET} ]]; then
        echo -e "  Status:         ${GREEN}TARGET REACHED${NC}"
    else
        echo -e "  Status:         ${YELLOW}Stopped before target${NC}"
    fi
    echo "═══════════════════════════════════════════════════════════════"
    echo ""
}

while true; do
    NOW=$(date +%s)
    ELAPSED=$((NOW - START_TIME))

    # Fetch metrics
    HEIGHT=$(get_metric "novai_committed_height" 2>/dev/null || echo "")
    ROUND=$(get_metric "novai_current_round" 2>/dev/null || echo "")
    PEERS=$(get_metric "novai_peer_count" 2>/dev/null || echo "")
    VIEW_CHANGES=$(get_metric "novai_consensus_view_changes_total" 2>/dev/null || echo "")

    if [[ -z "${HEIGHT}" ]]; then
        printf "  %-12s  ${RED}%-10s${NC}  %-10s  %-12s  %-8s  %-12s\n" \
            "$(format_duration ${ELAPSED})" "OFFLINE" "-" "-" "-" "-"
        sleep "${INTERVAL}"
        continue
    fi

    HEIGHT=${HEIGHT%.*}  # Strip any decimal
    PEERS=${PEERS:-0}
    VIEW_CHANGES=${VIEW_CHANGES:-0}

    # Calculate blocks/sec for this interval
    local_dt=$((NOW - PREV_TIME))
    local_dh=$((HEIGHT - PREV_HEIGHT))
    BPS="0.00"
    if [[ ${local_dt} -gt 0 ]] && [[ ${local_dh} -ge 0 ]]; then
        BPS=$(echo "scale=2; ${local_dh} / ${local_dt}" | bc -l 2>/dev/null || echo "0")
    fi

    # Update rolling average
    if [[ ${SAMPLE_COUNT} -gt 0 ]]; then
        BPS_SAMPLES+=("${BPS}")
        if [[ ${#BPS_SAMPLES[@]} -gt ${MAX_SAMPLES} ]]; then
            BPS_SAMPLES=("${BPS_SAMPLES[@]:1}")
        fi
    fi
    SAMPLE_COUNT=$((SAMPLE_COUNT + 1))

    AVG_BPS=$(rolling_avg)

    # ETA calculation
    REMAINING=$((TARGET - HEIGHT))
    ETA="-"
    if [[ "${AVG_BPS}" != "0" ]] && [[ "${AVG_BPS}" != "0.00" ]] && [[ ${REMAINING} -gt 0 ]]; then
        ETA_SECS=$(echo "scale=0; ${REMAINING} / ${AVG_BPS}" | bc -l 2>/dev/null || echo "")
        if [[ -n "${ETA_SECS}" ]] && [[ "${ETA_SECS}" -gt 0 ]] 2>/dev/null; then
            ETA=$(format_duration "${ETA_SECS}")
        fi
    fi

    # Stall detection
    if [[ ${local_dh} -gt 0 ]]; then
        LAST_PROGRESS_TIME=${NOW}
    fi
    STALL_SECS=$((NOW - LAST_PROGRESS_TIME))

    # View change delta
    VC_DELTA=$((VIEW_CHANGES - PREV_VIEW_CHANGES))
    VC_DISPLAY="${VIEW_CHANGES}"
    if [[ ${VC_DELTA} -gt 0 ]]; then
        VC_DISPLAY="${VIEW_CHANGES} (+${VC_DELTA})"
    fi

    # Color-code the line
    local line_color="${NC}"
    if [[ ${STALL_SECS} -ge ${STALL_THRESHOLD} ]]; then
        line_color="${RED}"
    elif [[ ${VC_DELTA} -gt 0 ]]; then
        line_color="${YELLOW}"
    fi

    printf "  ${line_color}%-12s  %-10s  %-10s  %-12s  %-8s  %-12s${NC}" \
        "$(format_duration ${ELAPSED})" "${HEIGHT}" "${BPS}" "${ETA}" "${PEERS}" "${VC_DISPLAY}"

    # Stall warning
    if [[ ${STALL_SECS} -ge ${STALL_THRESHOLD} ]]; then
        printf "  ${RED}STALL (${STALL_SECS}s)${NC}"
    fi
    echo ""

    # Check target reached
    if [[ ${HEIGHT} -ge ${TARGET} ]]; then
        echo ""
        echo -e "  ${GREEN}${BOLD}TARGET REACHED: ${HEIGHT} >= ${TARGET}${NC}"
        print_summary
        exit 0
    fi

    # Update state
    PREV_HEIGHT=${HEIGHT}
    PREV_TIME=${NOW}
    PREV_VIEW_CHANGES=${VIEW_CHANGES}

    sleep "${INTERVAL}"
done
