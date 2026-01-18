#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESULTS_DIR="$SCRIPT_DIR/results"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)

mkdir -p "$RESULTS_DIR"

echo "════════════════════════════════════════════════════"
echo "NOVAI Load Test: Steady 100 TPS"
echo "════════════════════════════════════════════════════"
echo "Duration: 5 minutes (300 seconds)"
echo "Target TPS: 100"
echo "Senders: 20"
echo "Workers: 4"
echo ""

cd "$PROJECT_ROOT"

# Build tx-generator if needed
cargo build --release -p tx-generator

# Run test
echo "Starting load test..."
./target/release/tx-generator \
  --tps 100 \
  --senders 20 \
  --duration 300 \
  --endpoint http://localhost:8080 \
  --workers 4 \
  --output json \
  > "$RESULTS_DIR/steady_100_tps_$TIMESTAMP.json"

echo ""
echo "✅ Test complete. Results saved to:"
echo "   $RESULTS_DIR/steady_100_tps_$TIMESTAMP.json"
echo ""

# Display summary
cat "$RESULTS_DIR/steady_100_tps_$TIMESTAMP.json" | jq -r '
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
