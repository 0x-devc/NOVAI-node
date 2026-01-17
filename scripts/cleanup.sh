#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════════
# NOVAI Deployment - Cleanup Script
# ═══════════════════════════════════════════════════════════════════════════════
# PURPOSE: Stop and remove NOVAI validator containers, volumes, and networks
#
# USAGE:
#   ./scripts/cleanup.sh [OPTIONS]
#
# OPTIONS:
#   --validator-id, --validator  Clean specific validator only (0-4)
#   --keep-data                  Keep data volumes (only remove containers)
#   --keep-network               Keep Docker network
#   --all                        Remove everything including volumes and network
#   --dry-run                    Print actions without executing
#   --force                      Skip confirmation prompts
#   --help                       Show this help message
#
# EXAMPLES:
#   # Stop and remove all validator containers (keep volumes)
#   ./scripts/cleanup.sh
#
#   # Full cleanup including volumes and network
#   ./scripts/cleanup.sh --all
#
#   # Remove specific validator
#   ./scripts/cleanup.sh --validator-id 2
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# SCRIPT SETUP
# ─────────────────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# shellcheck source=common.sh
source "${SCRIPT_DIR}/common.sh"

# ─────────────────────────────────────────────────────────────────────────────
# DEFAULT VALUES
# ─────────────────────────────────────────────────────────────────────────────

VALIDATOR_ID=""
KEEP_DATA="true"
KEEP_NETWORK="true"
REMOVE_ALL="false"
DRY_RUN="false"
FORCE="false"
SHOW_HELP="false"

# ─────────────────────────────────────────────────────────────────────────────
# HELP
# ─────────────────────────────────────────────────────────────────────────────

show_help() {
    cat << 'EOF'
NOVAI Cleanup Script

USAGE:
    ./scripts/cleanup.sh [OPTIONS]

OPTIONS:
    --validator-id, --validator  Clean specific validator only (0-4)
    --keep-data                  Keep data volumes (default)
    --keep-network               Keep Docker network (default)
    --all                        Remove everything including volumes and network
    --dry-run                    Print actions without executing
    --force                      Skip confirmation prompts
    --help                       Show this help message

EXAMPLES:
    ./scripts/cleanup.sh
    ./scripts/cleanup.sh --all
    ./scripts/cleanup.sh --validator-id 2
    ./scripts/cleanup.sh --validator-id 2 --all

DEFAULT BEHAVIOR:
    Removes containers but keeps data volumes and Docker network.
    Use --all to remove everything.

WARNING:
    --all will delete all blockchain data. This cannot be undone.
EOF
}

# ─────────────────────────────────────────────────────────────────────────────
# ARGUMENT PARSING
# ─────────────────────────────────────────────────────────────────────────────

parse_arguments() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --validator-id|--validator)
                VALIDATOR_ID="$2"
                shift 2
                ;;
            --keep-data)
                KEEP_DATA="true"
                shift
                ;;
            --keep-network)
                KEEP_NETWORK="true"
                shift
                ;;
            --all)
                REMOVE_ALL="true"
                KEEP_DATA="false"
                KEEP_NETWORK="false"
                shift
                ;;
            --dry-run)
                DRY_RUN="true"
                shift
                ;;
            --force)
                FORCE="true"
                shift
                ;;
            --help|-h)
                SHOW_HELP="true"
                shift
                ;;
            *)
                log_error "Unknown argument: $1"
                exit 1
                ;;
        esac
    done
}

# ─────────────────────────────────────────────────────────────────────────────
# DRY RUN
# ─────────────────────────────────────────────────────────────────────────────

run_cmd() {
    if [[ "${DRY_RUN}" == "true" ]]; then
        log "[DRY-RUN] Would execute: $*"
        return 0
    fi
    "$@"
}

# ─────────────────────────────────────────────────────────────────────────────
# CLEANUP FUNCTIONS
# ─────────────────────────────────────────────────────────────────────────────

discover_resources() {
    log_section "Discovering NOVAI Resources"

    local -a containers=()
    local -a volumes=()
    local network_exists="false"

    while IFS= read -r container; do
        if [[ -n "${container}" ]]; then
            containers+=("${container}")
        fi
    done < <(docker ps -a --filter "label=novai.network=${NOVAI_NETWORK}" --format '{{.Names}}' 2>/dev/null || true)

    for i in $(seq 0 4); do
        local container
        container=$(get_container_name "${i}")
        if container_exists "${container}" && [[ ! " ${containers[*]} " =~ " ${container} " ]]; then
            containers+=("${container}")
        fi
    done

    for i in $(seq 0 4); do
        local volume
        volume=$(get_volume_name "${i}")
        if docker volume inspect "${volume}" &>/dev/null; then
            volumes+=("${volume}")
        fi
    done

    if docker network inspect "${NOVAI_NETWORK}" &>/dev/null; then
        network_exists="true"
    fi

    echo "Found resources:"
    echo ""

    if [[ ${#containers[@]} -gt 0 ]]; then
        echo "Containers (${#containers[@]}):"
        for container in "${containers[@]}"; do
            local status
            if container_is_running "${container}"; then
                status="${COLOR_GREEN}running${COLOR_RESET}"
            else
                status="${COLOR_YELLOW}stopped${COLOR_RESET}"
            fi
            echo "  - ${container} (${status})"
        done
    else
        echo "Containers: none"
    fi

    echo ""

    if [[ ${#volumes[@]} -gt 0 ]]; then
        echo "Volumes (${#volumes[@]}):"
        for volume in "${volumes[@]}"; do
            echo "  - ${volume}"
        done
    else
        echo "Volumes: none"
    fi

    echo ""

    if [[ "${network_exists}" == "true" ]]; then
        echo "Network: ${NOVAI_NETWORK}"
    else
        echo "Network: none"
    fi

    echo ""
}

clean_validator() {
    local validator_id="$1"
    local remove_data="${2:-false}"

    local container
    local volume

    container=$(get_container_name "${validator_id}")
    volume=$(get_volume_name "${validator_id}")

    log "Cleaning validator ${validator_id}..."

    if container_exists "${container}"; then
        if container_is_running "${container}"; then
            log "  Stopping container: ${container}"
            run_cmd docker stop -t 10 "${container}"
        fi
        log "  Removing container: ${container}"
        run_cmd docker rm -f "${container}"
    else
        log "  Container not found: ${container}"
    fi

    if [[ "${remove_data}" == "true" ]]; then
        if docker volume inspect "${volume}" &>/dev/null; then
            log "  Removing volume: ${volume}"
            run_cmd docker volume rm "${volume}"
        else
            log "  Volume not found: ${volume}"
        fi
    fi

    log_success "Validator ${validator_id} cleaned"
}

clean_all_validators() {
    local remove_data="${1:-false}"

    log_section "Cleaning Validators"

    for i in $(seq 0 4); do
        clean_validator "${i}" "${remove_data}"
    done
}

clean_network() {
    log_section "Cleaning Network"

    if docker network inspect "${NOVAI_NETWORK}" &>/dev/null; then
        local attached
        attached=$(docker network inspect "${NOVAI_NETWORK}" --format '{{len .Containers}}' 2>/dev/null || echo "0")

        if [[ "${attached}" != "0" ]]; then
            log_warn "Network has ${attached} containers attached"
            log_warn "Disconnecting containers first..."

            while IFS= read -r container_id; do
                if [[ -n "${container_id}" ]]; then
                    run_cmd docker network disconnect -f "${NOVAI_NETWORK}" "${container_id}" || true
                fi
            done < <(docker network inspect "${NOVAI_NETWORK}" --format '{{range .Containers}}{{.Name}} {{end}}' 2>/dev/null | tr ' ' '\n')
        fi

        log "Removing network: ${NOVAI_NETWORK}"
        run_cmd docker network rm "${NOVAI_NETWORK}"
        log_success "Network removed"
    else
        log "Network not found: ${NOVAI_NETWORK}"
    fi
}

confirm_cleanup() {
    local what="$1"

    if [[ "${FORCE}" == "true" ]]; then
        return 0
    fi

    echo ""
    log_warn "This will ${what}"

    if ! confirm "Are you sure you want to proceed?"; then
        log "Cleanup cancelled"
        exit 0
    fi
}

# ─────────────────────────────────────────────────────────────────────────────
# MAIN
# ─────────────────────────────────────────────────────────────────────────────

main() {
    parse_arguments "$@"

    if [[ "${SHOW_HELP}" == "true" ]]; then
        show_help
        exit 0
    fi

    cd "${PROJECT_ROOT}"

    log_section "NOVAI Cleanup"

    discover_resources

    if [[ -n "${VALIDATOR_ID}" ]]; then
        if ! validate_validator_id "${VALIDATOR_ID}"; then
            exit 1
        fi

        local remove_data="false"
        if [[ "${REMOVE_ALL}" == "true" ]] || [[ "${KEEP_DATA}" == "false" ]]; then
            remove_data="true"
            confirm_cleanup "remove validator ${VALIDATOR_ID} container AND data volume"
        else
            confirm_cleanup "remove validator ${VALIDATOR_ID} container (data preserved)"
        fi

        clean_validator "${VALIDATOR_ID}" "${remove_data}"
        log_success "Cleanup complete"
        exit 0
    fi

    local actions=()
    actions+=("remove all validator containers")

    if [[ "${KEEP_DATA}" == "false" ]]; then
        actions+=("delete all data volumes")
    fi

    if [[ "${KEEP_NETWORK}" == "false" ]]; then
        actions+=("remove Docker network")
    fi

    local action_summary
    action_summary=$(IFS=", "; echo "${actions[*]}")
    confirm_cleanup "${action_summary}"

    clean_all_validators "${KEEP_DATA}"

    if [[ "${KEEP_NETWORK}" == "false" ]]; then
        clean_network
    fi

    log_section "Cleanup Summary"

    echo "Actions performed:"
    echo "  - Removed validator containers"
    if [[ "${KEEP_DATA}" == "false" ]]; then
        echo "  - Deleted data volumes"
    else
        echo "  - Data volumes preserved"
    fi
    if [[ "${KEEP_NETWORK}" == "false" ]]; then
        echo "  - Removed Docker network"
    else
        echo "  - Docker network preserved"
    fi

    echo ""
    log_success "Cleanup complete!"

    if [[ "${KEEP_DATA}" == "true" ]]; then
        echo ""
        log "To restart the testnet with existing data:"
        echo "  ./scripts/deploy-testnet.sh"
        echo ""
        log "To start fresh (remove data):"
        echo "  ./scripts/cleanup.sh --all"
    fi
}

main "$@"
