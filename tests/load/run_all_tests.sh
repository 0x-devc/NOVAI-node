#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESULTS_DIR="$SCRIPT_DIR/results"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
REPORT_DIR="$RESULTS_DIR/report_$TIMESTAMP"

mkdir -p "$REPORT_DIR"

echo "════════════════════════════════════════════════════"
echo "NOVAI Load Test Suite - Full Report Generation"
echo "════════════════════════════════════════════════════"
echo ""
echo "Report directory: $REPORT_DIR"
echo ""
echo "⚠️  WARNING: This test suite will take approximately 12 minutes"
echo "    Please ensure a NOVAI node is running on http://localhost:8080"
echo ""
read -p "Press Enter to continue or Ctrl+C to cancel..."
echo ""

cd "$PROJECT_ROOT"

# Verify node is running
echo "Checking node health..."
if ! curl -s http://localhost:8080/health > /dev/null 2>&1; then
    echo "❌ ERROR: Node not responding at http://localhost:8080"
    echo "   Please start a node with: cargo run --release -p novai-node"
    exit 1
fi
echo "✅ Node is running"
echo ""

# Capture system info
echo "Capturing system information..."
cat > "$REPORT_DIR/system_info.txt" <<EOF
NOVAI Load Test Report
Generated: $(date)

=== System Information ===
Hostname: $(hostname)
OS: $(uname -s) $(uname -r)
Architecture: $(uname -m)
EOF

if command -v sw_vers &> /dev/null; then
    echo "macOS Version: $(sw_vers -productVersion)" >> "$REPORT_DIR/system_info.txt"
fi

if command -v lscpu &> /dev/null; then
    echo "" >> "$REPORT_DIR/system_info.txt"
    echo "=== CPU Information ===" >> "$REPORT_DIR/system_info.txt"
    lscpu | grep -E "Model name|CPU\(s\)|Thread|Core|MHz" >> "$REPORT_DIR/system_info.txt"
fi

if command -v free &> /dev/null; then
    echo "" >> "$REPORT_DIR/system_info.txt"
    echo "=== Memory Information ===" >> "$REPORT_DIR/system_info.txt"
    free -h >> "$REPORT_DIR/system_info.txt"
fi

echo "Rust Version: $(rustc --version)" >> "$REPORT_DIR/system_info.txt"
echo ""

# Build tx-generator in release mode
echo "Building tx-generator in release mode..."
cargo build --release -p tx-generator --quiet
echo "✅ Build complete"
echo ""

# Capture pre-test metrics
echo "Capturing pre-test baseline metrics..."
curl -s http://localhost:8080/metrics > "$REPORT_DIR/metrics_baseline.txt"
echo ""

# Test 1: Steady 100 TPS
echo "════════════════════════════════════════════════════"
echo "Test 1/4: Steady 100 TPS (5 minutes)"
echo "════════════════════════════════════════════════════"
TEST_START=$(date +%s)
./target/release/tx-generator \
  --tps 100 \
  --senders 20 \
  --duration 300 \
  --endpoint http://localhost:8080 \
  --workers 4 \
  --output json \
  > "$REPORT_DIR/test1_steady_100_tps.json"
TEST_END=$(date +%s)
echo "✅ Test 1 complete (duration: $((TEST_END - TEST_START))s)"
curl -s http://localhost:8080/metrics > "$REPORT_DIR/metrics_after_test1.txt"
echo ""
sleep 5

# Test 2: Steady 500 TPS
echo "════════════════════════════════════════════════════"
echo "Test 2/4: Steady 500 TPS (3 minutes)"
echo "════════════════════════════════════════════════════"
TEST_START=$(date +%s)
./target/release/tx-generator \
  --tps 500 \
  --senders 50 \
  --duration 180 \
  --endpoint http://localhost:8080 \
  --workers 8 \
  --output json \
  > "$REPORT_DIR/test2_steady_500_tps.json"
TEST_END=$(date +%s)
echo "✅ Test 2 complete (duration: $((TEST_END - TEST_START))s)"
curl -s http://localhost:8080/metrics > "$REPORT_DIR/metrics_after_test2.txt"
echo ""
sleep 5

# Test 3: Burst 1000 TPS
echo "════════════════════════════════════════════════════"
echo "Test 3/4: Burst 1000 TPS (30 seconds)"
echo "════════════════════════════════════════════════════"
TEST_START=$(date +%s)
./target/release/tx-generator \
  --tps 1000 \
  --senders 100 \
  --duration 30 \
  --endpoint http://localhost:8080 \
  --workers 16 \
  --output json \
  > "$REPORT_DIR/test3_burst_1000_tps.json"
TEST_END=$(date +%s)
echo "✅ Test 3 complete (duration: $((TEST_END - TEST_START))s)"
curl -s http://localhost:8080/metrics > "$REPORT_DIR/metrics_after_test3.txt"
echo ""
sleep 5

# Test 4: Mixed Load
echo "════════════════════════════════════════════════════"
echo "Test 4/4: Mixed Load (140 seconds, 4 phases)"
echo "════════════════════════════════════════════════════"
MIXED_DIR="$REPORT_DIR/test4_mixed_load"
mkdir -p "$MIXED_DIR"

echo "Phase 1/4: Ramp up (50 TPS, 30s)..."
./target/release/tx-generator \
  --tps 50 \
  --senders 10 \
  --duration 30 \
  --endpoint http://localhost:8080 \
  --workers 4 \
  --output json \
  > "$MIXED_DIR/phase1_ramp_up.json"

echo "Phase 2/4: Steady (200 TPS, 60s)..."
./target/release/tx-generator \
  --tps 200 \
  --senders 30 \
  --duration 60 \
  --endpoint http://localhost:8080 \
  --workers 8 \
  --output json \
  > "$MIXED_DIR/phase2_steady.json"

echo "Phase 3/4: Burst (500 TPS, 20s)..."
./target/release/tx-generator \
  --tps 500 \
  --senders 50 \
  --duration 20 \
  --endpoint http://localhost:8080 \
  --workers 12 \
  --output json \
  > "$MIXED_DIR/phase3_burst.json"

echo "Phase 4/4: Ramp down (100 TPS, 30s)..."
./target/release/tx-generator \
  --tps 100 \
  --senders 20 \
  --duration 30 \
  --endpoint http://localhost:8080 \
  --workers 4 \
  --output json \
  > "$MIXED_DIR/phase4_ramp_down.json"

echo "✅ Test 4 complete"
curl -s http://localhost:8080/metrics > "$REPORT_DIR/metrics_after_test4.txt"
echo ""

# Generate summary report
echo "════════════════════════════════════════════════════"
echo "Generating summary report..."
echo "════════════════════════════════════════════════════"

SUMMARY_FILE="$REPORT_DIR/SUMMARY.txt"

cat > "$SUMMARY_FILE" <<EOF
NOVAI Load Test Summary
Generated: $(date)
Report Directory: $REPORT_DIR

════════════════════════════════════════════════════════════════

TEST 1: Steady 100 TPS (5 minutes)
EOF

if command -v jq &> /dev/null; then
    echo "" >> "$SUMMARY_FILE"
    jq -r '
      "Submitted:  \(.submitted_count)",
      "Accepted:   \(.accepted_count)",
      "Rejected:   \(.rejected_count)",
      "Failed:     \(.failed_count)",
      "Actual TPS: \(.actual_tps | tonumber | . * 100 | round / 100)",
      "P50:        \(.latency_p50_us)µs",
      "P95:        \(.latency_p95_us)µs",
      "P99:        \(.latency_p99_us)µs",
      "Max:        \(.latency_max_us)µs"
    ' "$REPORT_DIR/test1_steady_100_tps.json" >> "$SUMMARY_FILE"
else
    echo "Install jq to see formatted results" >> "$SUMMARY_FILE"
    cat "$REPORT_DIR/test1_steady_100_tps.json" >> "$SUMMARY_FILE"
fi

cat >> "$SUMMARY_FILE" <<EOF

════════════════════════════════════════════════════════════════

TEST 2: Steady 500 TPS (3 minutes)
EOF

if command -v jq &> /dev/null; then
    echo "" >> "$SUMMARY_FILE"
    jq -r '
      "Submitted:  \(.submitted_count)",
      "Accepted:   \(.accepted_count)",
      "Rejected:   \(.rejected_count)",
      "Failed:     \(.failed_count)",
      "Actual TPS: \(.actual_tps | tonumber | . * 100 | round / 100)",
      "P50:        \(.latency_p50_us)µs",
      "P95:        \(.latency_p95_us)µs",
      "P99:        \(.latency_p99_us)µs",
      "Max:        \(.latency_max_us)µs"
    ' "$REPORT_DIR/test2_steady_500_tps.json" >> "$SUMMARY_FILE"
fi

cat >> "$SUMMARY_FILE" <<EOF

════════════════════════════════════════════════════════════════

TEST 3: Burst 1000 TPS (30 seconds)
EOF

if command -v jq &> /dev/null; then
    echo "" >> "$SUMMARY_FILE"
    jq -r '
      "Submitted:  \(.submitted_count)",
      "Accepted:   \(.accepted_count)",
      "Rejected:   \(.rejected_count)",
      "Failed:     \(.failed_count)",
      "Actual TPS: \(.actual_tps | tonumber | . * 100 | round / 100)",
      "P50:        \(.latency_p50_us)µs",
      "P95:        \(.latency_p95_us)µs",
      "P99:        \(.latency_p99_us)µs",
      "Max:        \(.latency_max_us)µs"
    ' "$REPORT_DIR/test3_burst_1000_tps.json" >> "$SUMMARY_FILE"
fi

cat >> "$SUMMARY_FILE" <<EOF

════════════════════════════════════════════════════════════════

TEST 4: Mixed Load (4 phases, 140 seconds total)
EOF

if command -v jq &> /dev/null; then
    for phase in phase1_ramp_up phase2_steady phase3_burst phase4_ramp_down; do
        echo "" >> "$SUMMARY_FILE"
        echo "─── ${phase} ───" >> "$SUMMARY_FILE"
        jq -r '
          "  Submitted:  \(.submitted_count)",
          "  Accepted:   \(.accepted_count)",
          "  Actual TPS: \(.actual_tps | tonumber | . * 100 | round / 100)",
          "  P95:        \(.latency_p95_us)µs"
        ' "$MIXED_DIR/$phase.json" >> "$SUMMARY_FILE"
    done
fi

cat >> "$SUMMARY_FILE" <<EOF

════════════════════════════════════════════════════════════════

FILES GENERATED:
  - system_info.txt             System and environment details
  - metrics_baseline.txt        Pre-test Prometheus metrics
  - test1_steady_100_tps.json   100 TPS test results
  - metrics_after_test1.txt     Post-test 1 metrics
  - test2_steady_500_tps.json   500 TPS test results
  - metrics_after_test2.txt     Post-test 2 metrics
  - test3_burst_1000_tps.json   1000 TPS burst results
  - metrics_after_test3.txt     Post-test 3 metrics
  - test4_mixed_load/           Mixed load test (4 phases)
  - metrics_after_test4.txt     Post-test 4 metrics
  - SUMMARY.txt                 This summary file

NEXT STEPS:
  1. Review SUMMARY.txt for high-level results
  2. Analyze individual JSON files for detailed latency data
  3. Compare metrics_*.txt files to see node state changes
  4. Update docs/PERFORMANCE_REPORT.md with these results
  5. Update docs/TUNING_PARAMETERS.md load test section

════════════════════════════════════════════════════════════════
EOF

echo ""
echo "✅ All tests complete!"
echo ""
echo "Summary report: $SUMMARY_FILE"
echo ""
cat "$SUMMARY_FILE"
echo ""
echo "Full results available in: $REPORT_DIR"
