# NOVAI Load Test Scenarios

This directory contains load testing scripts for the NOVAI blockchain node.

## Prerequisites

1. **Running NOVAI Node**: Start a node with RPC endpoint on port 8080:
   ```bash
   cargo run -p novai-node -- --genesis-path genesis.json
   ```

2. **jq**: JSON processor for displaying results (install via `brew install jq` or `apt-get install jq`)

## Quick Start: Run All Tests

To run the complete test suite and generate a comprehensive report:

```bash
./tests/load/run_all_tests.sh
```

This will:
- Run all 4 test scenarios sequentially (~12 minutes total)
- Capture system information and Prometheus metrics
- Generate timestamped JSON results for each test
- Create a summary report with all findings
- Output results to `tests/load/results/report_YYYYMMDD_HHMMSS/`

**Requirements**: Node must be running on `http://localhost:8080` before starting.

---

## Individual Test Scenarios

### 1. Steady 100 TPS (`steady_100_tps.sh`)
- **Purpose**: Baseline steady-state load test
- **Duration**: 5 minutes (300 seconds)
- **Target TPS**: 100
- **Senders**: 20 accounts
- **Workers**: 4 threads

**Run**:
```bash
./tests/load/steady_100_tps.sh
```

### 2. Steady 500 TPS (`steady_500_tps.sh`)
- **Purpose**: Higher sustained load
- **Duration**: 3 minutes (180 seconds)
- **Target TPS**: 500
- **Senders**: 50 accounts
- **Workers**: 8 threads

**Run**:
```bash
./tests/load/steady_500_tps.sh
```

### 3. Burst 1000 TPS (`burst_1000_tps.sh`)
- **Purpose**: Stress test with high burst load
- **Duration**: 30 seconds (short burst)
- **Target TPS**: 1000
- **Senders**: 100 accounts
- **Workers**: 16 threads

**Run**:
```bash
./tests/load/burst_1000_tps.sh
```

### 4. Mixed Load (`mixed_load.sh`)
- **Purpose**: Realistic variable load pattern
- **Pattern**:
  - Phase 1: Ramp up (50 TPS, 30s)
  - Phase 2: Steady (200 TPS, 60s)
  - Phase 3: Burst (500 TPS, 20s)
  - Phase 4: Ramp down (100 TPS, 30s)
- **Total Duration**: 140 seconds

**Run**:
```bash
./tests/load/mixed_load.sh
```

## Results

Results are saved to `tests/load/results/` with timestamped filenames in JSON format.

**Example output**:
```json
{
  "submitted_count": 30000,
  "accepted_count": 29985,
  "rejected_count": 5,
  "failed_count": 10,
  "confirmed_count": 0,
  "latency_p50_us": 1250,
  "latency_p95_us": 3200,
  "latency_p99_us": 5800,
  "latency_max_us": 12000,
  "latency_mean_us": 1450.5,
  "elapsed_secs": 300,
  "actual_tps": 99.95
}
```

## Metrics to Monitor

While running tests, monitor:

1. **Node Prometheus metrics** (`http://localhost:8080/metrics`):
   - `novai_committed_height` - Block production rate
   - `novai_mempool_size` - Mempool backlog
   - `novai_total_txs_committed` - Transaction throughput

2. **System metrics**:
   - CPU usage (`top` or `htop`)
   - Memory usage
   - Disk I/O (`iostat`)

3. **Test results**:
   - Actual TPS vs target TPS
   - Acceptance rate (accepted / submitted)
   - Latency percentiles (P50, P95, P99)
   - Failed submissions

## Interpreting Results

**Good performance indicators**:
- Actual TPS ≥ 95% of target TPS
- Acceptance rate > 99%
- P95 latency < 5ms
- Failed count = 0

**Performance issues**:
- Actual TPS << target TPS → Node bottleneck
- High rejection rate → Validation errors or nonce issues
- High P99 latency → Timeout or network congestion
- Failed submissions → Connection errors or node down

## Troubleshooting

**Node not responding**:
- Check node is running: `curl http://localhost:8080/health`
- Check logs for errors

**High rejection rate**:
- Check node logs for validation errors
- Verify nonce provider is working correctly

**Low actual TPS**:
- Increase `--workers` for more parallelism
- Check if node mempool is full
- Monitor node CPU/memory usage
