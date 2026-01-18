#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESULTS_DIR="$SCRIPT_DIR/results"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)

mkdir -p "$RESULTS_DIR"

echo "════════════════════════════════════════════════════"
echo "NOVAI Load Test: Steady 500 TPS"
echo "════════════════════════════════════════════════════"
echo "Duration: 3 minutes (180 seconds)"
echo "Target TPS: 500"
echo "Senders: 50"
echo "Workers: 8"
echo ""

cd "$PROJECT_ROOT"

# Build tx-generator if needed
cargo build --release -p tx-generator

# Run test
echo "Starting load test..."
./target/release/tx-generator \
  --tps 500 \
  --senders 50 \
  --duration 180 \
  --endpoint http://localhost:8080 \
  --workers 8 \
  --output json \
  > "$RESULTS_DIR/steady_500_tps_$TIMESTAMP.json"

echo ""
echo "✅ Test complete. Results saved to:"
echo "   $RESULTS_DIR/steady_500_tps_$TIMESTAMP.json"
echo ""

# Display summary
cat "$RESULTS_DIR/steady_500_tps_$TIMESTAMP.json" | jq -r '
  "Summary:",
  "  Submitted:  \(.submitted_count)",
  "  Accepted:   \(.accepted_count)",
  "  Rejected:   \(.rejected_count)",
  "  Failed:     \(.failed_count)",
  "  Actual TPS: \(.actual_tps)",
  "  Latency P50: \(.latency_p50_us)µs",
  "  Latency P95: \(.latency_p95_us)µs",
  "  Latency P99: \(.latency_p99_us)µs"
'
