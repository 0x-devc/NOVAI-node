#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESULTS_DIR="$SCRIPT_DIR/results"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)

mkdir -p "$RESULTS_DIR"

echo "════════════════════════════════════════════════════"
echo "NOVAI Load Test: Mixed Load Pattern"
echo "════════════════════════════════════════════════════"
echo "Pattern: Ramp up → Steady → Burst → Ramp down"
echo ""

cd "$PROJECT_ROOT"

# Build tx-generator if needed
cargo build --release -p tx-generator

MIXED_RESULTS="$RESULTS_DIR/mixed_load_$TIMESTAMP"
mkdir -p "$MIXED_RESULTS"

# Phase 1: Ramp up (50 TPS for 30s)
echo "Phase 1: Ramp up (50 TPS, 30s)..."
./target/release/tx-generator \
  --tps 50 \
  --senders 10 \
  --duration 30 \
  --endpoint http://localhost:8080 \
  --workers 4 \
  --output json \
  > "$MIXED_RESULTS/phase1_ramp_up.json"

# Phase 2: Steady state (200 TPS for 60s)
echo "Phase 2: Steady state (200 TPS, 60s)..."
./target/release/tx-generator \
  --tps 200 \
  --senders 30 \
  --duration 60 \
  --endpoint http://localhost:8080 \
  --workers 8 \
  --output json \
  > "$MIXED_RESULTS/phase2_steady.json"

# Phase 3: Burst (500 TPS for 20s)
echo "Phase 3: Burst (500 TPS, 20s)..."
./target/release/tx-generator \
  --tps 500 \
  --senders 50 \
  --duration 20 \
  --endpoint http://localhost:8080 \
  --workers 12 \
  --output json \
  > "$MIXED_RESULTS/phase3_burst.json"

# Phase 4: Ramp down (100 TPS for 30s)
echo "Phase 4: Ramp down (100 TPS, 30s)..."
./target/release/tx-generator \
  --tps 100 \
  --senders 20 \
  --duration 30 \
  --endpoint http://localhost:8080 \
  --workers 4 \
  --output json \
  > "$MIXED_RESULTS/phase4_ramp_down.json"

echo ""
echo "✅ Test complete. Results saved to:"
echo "   $MIXED_RESULTS/"
echo ""

# Display summary for each phase
for phase in phase1_ramp_up phase2_steady phase3_burst phase4_ramp_down; do
  echo "─────────────────────────────────────────────────────"
  echo "${phase}:"
  cat "$MIXED_RESULTS/$phase.json" | jq -r '
    "  Submitted:  \(.submitted_count)",
    "  Accepted:   \(.accepted_count)",
    "  Actual TPS: \(.actual_tps)",
    "  Latency P95: \(.latency_p95_us)µs"
  '
done
echo "═════════════════════════════════════════════════════"
