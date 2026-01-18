# NOVAI Node Performance Report

**Report Date**: Week 11 (January 2026)
**Test Version**: v0.1.0
**Protocol Version**: 1
**Consensus**: HotStuff BFT with 3-chain commit rule

---

## Executive Summary

This report documents the performance characteristics of the NOVAI blockchain node under various load conditions. Tests were conducted using the tx-generator load testing tool against a local development network.

**Key Findings**:
- Maximum sustained TPS: [TBD after testing]
- Peak burst TPS: [TBD after testing]
- Block production rate: [TBD after testing]
- Consensus stability: [TBD after testing]
- Primary bottleneck: [TBD after testing]

---

## Table of Contents

1. [Test Environment](#test-environment)
2. [Test Methodology](#test-methodology)
3. [Baseline Performance](#baseline-performance)
4. [Load Test Results](#load-test-results)
5. [Bottleneck Analysis](#bottleneck-analysis)
6. [Resource Utilization](#resource-utilization)
7. [Scalability Analysis](#scalability-analysis)
8. [Recommendations](#recommendations)
9. [Appendix](#appendix)

---

## Test Environment

### Hardware Specifications

**Test Machine**:
```
CPU:     [e.g., Apple M1 Pro, 8 cores]
RAM:     [e.g., 16 GB]
Disk:    [e.g., 512 GB NVMe SSD]
OS:      [e.g., macOS 14.x / Linux Ubuntu 22.04]
```

**Network Configuration**:
```
Topology:    Local (single machine)
Validators:  4 nodes
Latency:     <1ms (loopback)
Bandwidth:   Unlimited (local)
```

### Software Configuration

**Node Parameters** (from `docs/TUNING_PARAMETERS.md`):
```rust
// Consensus
BASE_TIMEOUT_MS = 2000
TIMEOUT_MULTIPLIER = 2
MAX_TIMEOUT_MS = 60_000

// Mempool
MIN_TX_FEE = 1
FAIRNESS_CAP_PER_SENDER = 1000

// Network
MAX_TXS_PER_BLOCK = 10_000
MAX_BLOCKS_PER_RESPONSE = 1000
```

**Build Configuration**:
```bash
Rust version:   [e.g., rustc 1.75.0]
Build profile:  --release
Optimizations:  enabled
Debug symbols:  disabled
```

---

## Test Methodology

### Test Tools

**Transaction Generator**: `tx-generator` v0.1.0
- Located in: `tools/tx-generator/`
- Language: Rust (compiled with --release)
- Features: Rate-controlled generation, retry logic, latency tracking

**Monitoring**:
- Prometheus metrics endpoint: `http://localhost:8080/metrics`
- System monitoring: `top`, `htop`, `iostat`
- Network monitoring: libp2p built-in metrics

### Test Scenarios

Four load test scenarios were executed (from `tests/load/`):

1. **Steady 100 TPS**: Baseline steady-state test (5 minutes, 20 senders, 4 workers)
2. **Steady 500 TPS**: Higher sustained load (3 minutes, 50 senders, 8 workers)
3. **Burst 1000 TPS**: Stress burst test (30 seconds, 100 senders, 16 workers)
4. **Mixed Load**: Variable pattern (140 seconds, 4 phases with varying TPS)

### Metrics Collected

**Transaction Metrics**:
- Submitted count
- Accepted count
- Rejected count
- Failed count
- Actual TPS (measured)

**Latency Metrics** (microseconds):
- P50 (median)
- P95 (95th percentile)
- P99 (99th percentile)
- Max
- Mean

**Node Metrics** (from Prometheus):
- `novai_committed_height` - Blocks committed
- `novai_current_round` - Consensus round
- `novai_mempool_size` - Pending transactions
- `novai_consensus_view_changes_total` - Timeout/failure count
- `novai_block_tx_count` - Transactions per block
- `novai_total_txs_committed` - Cumulative throughput

**System Metrics**:
- CPU utilization (%)
- Memory usage (MB)
- Disk I/O (MB/s)
- Network I/O (MB/s)

---

## Baseline Performance

### Idle State Metrics

**Before Load Tests** (node running, no transactions):

```
CPU Usage:            [e.g., 2-5%]
Memory Usage:         [e.g., 150 MB]
Block Height:         0 (genesis)
Mempool Size:         0
View Changes:         0
```

### Single Transaction Performance

**Test**: Submit single transaction, measure end-to-end latency

```
Submission latency:   [TBD] µs
Mempool insertion:    [TBD] µs
Block inclusion:      [TBD] seconds
Commitment latency:   [TBD] seconds
Total latency:        [TBD] seconds
```

---

## Load Test Results

### Test 1: Steady 100 TPS

**Configuration**:
```bash
Duration:    300 seconds (5 minutes)
Target TPS:  100
Senders:     20 accounts
Workers:     4 threads
```

**Command**:
```bash
./tests/load/steady_100_tps.sh
```

**Results**:

| Metric              | Value    |
|---------------------|----------|
| Submitted           | [30,000] |
| Accepted            | [TBD]    |
| Rejected            | [TBD]    |
| Failed              | [TBD]    |
| **Actual TPS**      | [TBD]    |
| **Acceptance Rate** | [TBD]%   |

**Latency Distribution**:

| Percentile | Latency (µs) |
|------------|--------------|
| P50        | [TBD]        |
| P95        | [TBD]        |
| P99        | [TBD]        |
| P99.9      | [TBD]        |
| Max        | [TBD]        |
| Mean       | [TBD]        |

**Node Metrics** (sampled at test end):

```
Committed Height:        [TBD] blocks
Average Block Interval:  [TBD] seconds
Avg Txs Per Block:       [TBD]
View Changes:            [TBD]
Final Mempool Size:      [TBD]
```

**System Resources** (peak values):

```
CPU Usage:      [TBD]%
Memory Usage:   [TBD] MB
Disk Write:     [TBD] MB/s
Network Send:   [TBD] MB/s
```

**Analysis**:
- [TBD: Was target TPS achieved?]
- [TBD: Were there any failures or rejections?]
- [TBD: Was latency within acceptable bounds?]
- [TBD: Did mempool remain stable?]

---

### Test 2: Steady 500 TPS

**Configuration**:
```bash
Duration:    180 seconds (3 minutes)
Target TPS:  500
Senders:     50 accounts
Workers:     8 threads
```

**Command**:
```bash
./tests/load/steady_500_tps.sh
```

**Results**:

| Metric              | Value    |
|---------------------|----------|
| Submitted           | [90,000] |
| Accepted            | [TBD]    |
| Rejected            | [TBD]    |
| Failed              | [TBD]    |
| **Actual TPS**      | [TBD]    |
| **Acceptance Rate** | [TBD]%   |

**Latency Distribution**:

| Percentile | Latency (µs) |
|------------|--------------|
| P50        | [TBD]        |
| P95        | [TBD]        |
| P99        | [TBD]        |
| P99.9      | [TBD]        |
| Max        | [TBD]        |
| Mean       | [TBD]        |

**Node Metrics** (sampled at test end):

```
Committed Height:        [TBD] blocks
Average Block Interval:  [TBD] seconds
Avg Txs Per Block:       [TBD]
View Changes:            [TBD]
Final Mempool Size:      [TBD]
```

**System Resources** (peak values):

```
CPU Usage:      [TBD]%
Memory Usage:   [TBD] MB
Disk Write:     [TBD] MB/s
Network Send:   [TBD] MB/s
```

**Analysis**:
- [TBD: Did performance degrade compared to 100 TPS test?]
- [TBD: Was the node able to keep up with submission rate?]
- [TBD: Did mempool grow unbounded or stabilize?]
- [TBD: Were view changes more frequent?]

---

### Test 3: Burst 1000 TPS

**Configuration**:
```bash
Duration:    30 seconds (burst)
Target TPS:  1000
Senders:     100 accounts
Workers:     16 threads
```

**Command**:
```bash
./tests/load/burst_1000_tps.sh
```

**Results**:

| Metric              | Value    |
|---------------------|----------|
| Submitted           | [30,000] |
| Accepted            | [TBD]    |
| Rejected            | [TBD]    |
| Failed              | [TBD]    |
| **Actual TPS**      | [TBD]    |
| **Acceptance Rate** | [TBD]%   |

**Latency Distribution**:

| Percentile | Latency (µs) |
|------------|--------------|
| P50        | [TBD]        |
| P95        | [TBD]        |
| P99        | [TBD]        |
| P99.9      | [TBD]        |
| Max        | [TBD]        |
| Mean       | [TBD]        |

**Node Metrics** (sampled at test end):

```
Committed Height:        [TBD] blocks
Average Block Interval:  [TBD] seconds
Avg Txs Per Block:       [TBD]
View Changes:            [TBD]
Final Mempool Size:      [TBD]
```

**System Resources** (peak values):

```
CPU Usage:      [TBD]%
Memory Usage:   [TBD] MB
Disk Write:     [TBD] MB/s
Network Send:   [TBD] MB/s
```

**Analysis**:
- [TBD: Could the node handle burst load?]
- [TBD: Did the node catch up after burst ended?]
- [TBD: Were there significant failures or rejections?]
- [TBD: What was the peak latency during burst?]

---

### Test 4: Mixed Load

**Configuration**:
```bash
Total Duration:  140 seconds
Phases:          4 (ramp up → steady → burst → ramp down)
```

**Phase Breakdown**:

| Phase | Duration | Target TPS | Senders | Workers |
|-------|----------|------------|---------|---------|
| 1     | 30s      | 50         | 10      | 4       |
| 2     | 60s      | 200        | 30      | 8       |
| 3     | 20s      | 500        | 50      | 12      |
| 4     | 30s      | 100        | 20      | 4       |

**Command**:
```bash
./tests/load/mixed_load.sh
```

**Results by Phase**:

#### Phase 1: Ramp Up (50 TPS, 30s)

| Metric         | Value |
|----------------|-------|
| Submitted      | 1,500 |
| Accepted       | [TBD] |
| Actual TPS     | [TBD] |
| P95 Latency    | [TBD] |

#### Phase 2: Steady (200 TPS, 60s)

| Metric         | Value  |
|----------------|--------|
| Submitted      | 12,000 |
| Accepted       | [TBD]  |
| Actual TPS     | [TBD]  |
| P95 Latency    | [TBD]  |

#### Phase 3: Burst (500 TPS, 20s)

| Metric         | Value  |
|----------------|--------|
| Submitted      | 10,000 |
| Accepted       | [TBD]  |
| Actual TPS     | [TBD]  |
| P95 Latency    | [TBD]  |

#### Phase 4: Ramp Down (100 TPS, 30s)

| Metric         | Value |
|----------------|-------|
| Submitted      | 3,000 |
| Accepted       | [TBD] |
| Actual TPS     | [TBD] |
| P95 Latency    | [TBD] |

**Overall Results**:

```
Total Submitted:    26,500
Total Accepted:     [TBD]
Total Rejected:     [TBD]
Total Failed:       [TBD]
Overall TPS:        [TBD]
```

**Analysis**:
- [TBD: How did the node handle variable load patterns?]
- [TBD: Did latency spike during phase transitions?]
- [TBD: Did the node recover after burst phase?]
- [TBD: Was mempool size bounded throughout test?]

---

## Bottleneck Analysis

### Identified Bottlenecks

Based on load test results, the following bottlenecks were identified:

#### 1. [Primary Bottleneck - TBD]

**Symptom**: [e.g., "Actual TPS plateaued at 200 despite target of 500"]

**Evidence**:
- [e.g., "CPU utilization at 100%"]
- [e.g., "Mempool size grew unbounded"]
- [e.g., "Block production rate did not increase"]

**Root Cause**: [Analysis of why this occurred]

**Mitigation**:
- [Suggested parameter changes]
- [Hardware upgrades needed]
- [Code optimizations required]

---

#### 2. [Secondary Bottleneck - TBD]

**Symptom**: [Observable performance issue]

**Evidence**:
- [Metric 1]
- [Metric 2]

**Root Cause**: [Analysis]

**Mitigation**: [Recommendations]

---

### Performance Limits

Based on testing, the following performance limits were observed:

**Sustainable TPS**:
```
100 TPS:  ✅ Achieved with [X]% headroom
500 TPS:  [✅/⚠️/❌] [Status and observations]
1000 TPS: [✅/⚠️/❌] [Status and observations]
```

**Burst Capacity**:
```
Peak TPS achieved:     [TBD]
Sustained duration:    [TBD] seconds
Recovery time:         [TBD] seconds
```

**Resource Limits**:
```
CPU bottleneck:        [TBD]%
Memory limit:          [TBD] MB
Disk I/O limit:        [TBD] MB/s
Network limit:         [TBD] MB/s
```

---

## Resource Utilization

### CPU Usage

**CPU Usage by Load**:

| Test Scenario   | Average CPU | Peak CPU | CPU-Bound? |
|-----------------|-------------|----------|------------|
| Steady 100 TPS  | [TBD]%      | [TBD]%   | [Yes/No]   |
| Steady 500 TPS  | [TBD]%      | [TBD]%   | [Yes/No]   |
| Burst 1000 TPS  | [TBD]%      | [TBD]%   | [Yes/No]   |
| Mixed Load      | [TBD]%      | [TBD]%   | [Yes/No]   |

**CPU Breakdown** (if profiled):
```
Consensus logic:        [TBD]%
Transaction validation: [TBD]%
Signature verification: [TBD]%
State updates:          [TBD]%
Network I/O:            [TBD]%
Other:                  [TBD]%
```

---

### Memory Usage

**Memory Usage by Load**:

| Test Scenario   | Average Memory | Peak Memory | Growth Rate |
|-----------------|----------------|-------------|-------------|
| Steady 100 TPS  | [TBD] MB       | [TBD] MB    | [TBD] MB/s  |
| Steady 500 TPS  | [TBD] MB       | [TBD] MB    | [TBD] MB/s  |
| Burst 1000 TPS  | [TBD] MB       | [TBD] MB    | [TBD] MB/s  |
| Mixed Load      | [TBD] MB       | [TBD] MB    | [TBD] MB/s  |

**Memory Breakdown**:
```
Mempool:         [TBD] MB
Block cache:     [TBD] MB
State tree:      [TBD] MB
Network buffers: [TBD] MB
Other:           [TBD] MB
```

**Memory Leak Detection**: [None detected / Slow leak observed / etc.]

---

### Disk I/O

**Disk I/O by Load**:

| Test Scenario   | Avg Read  | Avg Write | Peak Write | Total Written |
|-----------------|-----------|-----------|------------|---------------|
| Steady 100 TPS  | [TBD] MB/s| [TBD] MB/s| [TBD] MB/s | [TBD] MB      |
| Steady 500 TPS  | [TBD] MB/s| [TBD] MB/s| [TBD] MB/s | [TBD] MB      |
| Burst 1000 TPS  | [TBD] MB/s| [TBD] MB/s| [TBD] MB/s | [TBD] MB      |
| Mixed Load      | [TBD] MB/s| [TBD] MB/s| [TBD] MB/s | [TBD] MB      |

**Write Patterns**:
- State updates: [TBD] MB/s
- Block commits: [TBD] MB/s
- Consensus checkpoints: [TBD] MB/s

---

### Network I/O

**Network I/O by Load**:

| Test Scenario   | Avg Send  | Avg Recv  | Peak Send | Peak Recv |
|-----------------|-----------|-----------|-----------|-----------|
| Steady 100 TPS  | [TBD] MB/s| [TBD] MB/s| [TBD] MB/s| [TBD] MB/s|
| Steady 500 TPS  | [TBD] MB/s| [TBD] MB/s| [TBD] MB/s| [TBD] MB/s|
| Burst 1000 TPS  | [TBD] MB/s| [TBD] MB/s| [TBD] MB/s| [TBD] MB/s|
| Mixed Load      | [TBD] MB/s| [TBD] MB/s| [TBD] MB/s| [TBD] MB/s|

**Network Patterns**:
- Proposal broadcast: [TBD] KB per message
- Vote broadcast: [TBD] KB per message
- Block sync: [TBD] KB per block

---

## Scalability Analysis

### Horizontal Scalability (Validator Count)

**Test Configuration**: Run steady 100 TPS test with varying validator counts

| Validators | Actual TPS | Avg Latency | View Changes | Notes          |
|------------|------------|-------------|--------------|----------------|
| 1          | [TBD]      | [TBD] ms    | [TBD]        | Single node    |
| 4          | [TBD]      | [TBD] ms    | [TBD]        | Baseline       |
| 7          | [TBD]      | [TBD] ms    | [TBD]        | BFT 2f+1       |
| 10         | [TBD]      | [TBD] ms    | [TBD]        | Larger network |

**Observations**:
- [TBD: How does performance degrade with more validators?]
- [TBD: Is consensus latency linear with validator count?]
- [TBD: Are there coordination bottlenecks?]

---

### Vertical Scalability (Hardware)

**Test Configuration**: Run steady 500 TPS test with varying CPU/memory

| CPU Cores | Memory | Actual TPS | CPU Usage | Notes             |
|-----------|--------|------------|-----------|-------------------|
| 2         | 4 GB   | [TBD]      | [TBD]%    | Minimum config    |
| 4         | 8 GB   | [TBD]      | [TBD]%    | Recommended       |
| 8         | 16 GB  | [TBD]      | [TBD]%    | High performance  |
| 16        | 32 GB  | [TBD]      | [TBD]%    | Maximum tested    |

**Observations**:
- [TBD: Is there a linear relationship between cores and TPS?]
- [TBD: At what point do additional cores provide diminishing returns?]
- [TBD: What is the memory scaling factor?]

---

### Transaction Complexity Scaling

**Test Configuration**: Vary transaction types and complexity

| Tx Type       | Tx Size | Validation Time | Effective TPS | Notes               |
|---------------|---------|-----------------|---------------|---------------------|
| Simple xfer   | ~200 B  | [TBD] µs        | [TBD]         | Baseline            |
| AI register   | ~500 B  | [TBD] µs        | [TBD]         | More complex        |
| AI signal     | ~1000 B | [TBD] µs        | [TBD]         | Largest payload     |

**Observations**:
- [TBD: How does transaction complexity affect throughput?]
- [TBD: Is signature verification the dominant cost?]

---

## Recommendations

### Short-Term Optimizations (Week 11-12)

1. **[Recommendation 1 - TBD]**
   - **Issue**: [Problem identified]
   - **Solution**: [Specific action]
   - **Expected Impact**: [Performance improvement]
   - **Effort**: [Low/Medium/High]

2. **[Recommendation 2 - TBD]**
   - **Issue**: [Problem identified]
   - **Solution**: [Specific action]
   - **Expected Impact**: [Performance improvement]
   - **Effort**: [Low/Medium/High]

---

### Medium-Term Improvements (Week 13-20)

1. **Parallel Transaction Validation**
   - **Current**: Sequential validation in single thread
   - **Proposed**: Parallel validation using thread pool
   - **Expected Impact**: 2-4x throughput improvement
   - **Effort**: High (requires transaction independence analysis)

2. **Mempool Priority Queue Optimization**
   - **Current**: Linear scan for highest-fee transactions
   - **Proposed**: Binary heap or B-tree for O(log n) insertion
   - **Expected Impact**: 10-100x faster drain on large mempools
   - **Effort**: Medium

3. **Block Propagation Compression**
   - **Current**: Full block broadcast
   - **Proposed**: Transaction hash list + compact block recovery
   - **Expected Impact**: 50-80% bandwidth reduction
   - **Effort**: High

---

### Long-Term Architectural Changes (Week 21+)

1. **State Sharding**
   - **Current**: Single state tree
   - **Proposed**: Sharded state for parallel execution
   - **Expected Impact**: 10-100x throughput
   - **Effort**: Very High

2. **Optimistic Execution**
   - **Current**: Execute after consensus
   - **Proposed**: Speculative execution + rollback
   - **Expected Impact**: 2-5x latency reduction
   - **Effort**: Very High

---

### Parameter Tuning Recommendations

Based on test results, the following parameter adjustments are recommended:

```rust
// RECOMMENDED CHANGES (if different from baseline)

// Consensus
BASE_TIMEOUT_MS = [TBD: Keep 2000 or adjust?]
MAX_TXS_PER_BLOCK = [TBD: Increase/decrease?]

// Mempool
MIN_TX_FEE = [TBD: Adjust based on spam observed?]
FAIRNESS_CAP_PER_SENDER = [TBD: Adjust based on fairness issues?]
```

**Rationale**: [Explanation for each recommended change]

---

## Appendix

### A. Test Commands Reference

```bash
# Run all load tests
cd /path/to/NOVAI-node

# Start node in separate terminal
cargo run --release -p novai-node -- --genesis-path genesis.json

# Run tests (in another terminal)
./tests/load/steady_100_tps.sh
./tests/load/steady_500_tps.sh
./tests/load/burst_1000_tps.sh
./tests/load/mixed_load.sh

# Check results
ls -lh tests/load/results/
```

### B. Monitoring Commands

```bash
# Real-time Prometheus metrics
watch -n 1 'curl -s http://localhost:8080/metrics | grep novai_'

# System monitoring
htop
iostat -x 1
iftop

# Process-specific monitoring
ps aux | grep novai-node
top -pid $(pgrep novai-node)
```

### C. Raw Test Data

All raw test data is available in:
```
tests/load/results/
├── steady_100_tps_YYYYMMDD_HHMMSS.json
├── steady_500_tps_YYYYMMDD_HHMMSS.json
├── burst_1000_tps_YYYYMMDD_HHMMSS.json
└── mixed_load_YYYYMMDD_HHMMSS/
    ├── phase1_ramp_up.json
    ├── phase2_steady.json
    ├── phase3_burst.json
    └── phase4_ramp_down.json
```

### D. Hardware Specifications (Detailed)

```
[To be filled with detailed hardware specs from test machine]

CPU:
  Model:       [e.g., Apple M1 Pro]
  Cores:       [e.g., 8 cores (6 performance + 2 efficiency)]
  Clock Speed: [e.g., 3.2 GHz base, 3.5 GHz boost]
  Cache:       [e.g., L1: 192KB, L2: 12MB, L3: 24MB]

Memory:
  Total:       [e.g., 16 GB]
  Type:        [e.g., LPDDR5]
  Speed:       [e.g., 6400 MT/s]
  Channels:    [e.g., Dual channel]

Storage:
  Type:        [e.g., NVMe SSD]
  Model:       [e.g., Apple integrated SSD]
  Capacity:    [e.g., 512 GB]
  Read Speed:  [e.g., 5000 MB/s]
  Write Speed: [e.g., 4500 MB/s]
  IOPS:        [e.g., 1M+ read, 800K write]

Operating System:
  OS:          [e.g., macOS 14.2]
  Kernel:      [e.g., Darwin 23.2.0]
  Filesystem:  [e.g., APFS]
```

### E. Network Topology Diagram

```
[Diagram of test network topology]

For local testing:
┌──────────────────────────────────────┐
│       Test Machine (Localhost)       │
│                                      │
│  ┌─────────┐  ┌─────────┐           │
│  │ Node 1  │  │ Node 2  │           │
│  │ :8001   │  │ :8002   │           │
│  └────┬────┘  └────┬────┘           │
│       │            │                 │
│  ┌────┴─────────┬──┴────┐           │
│  │              │       │           │
│  ┌────┴────┐  ┌─┴─────┐ │           │
│  │ Node 3  │  │ Node 4│ │           │
│  │ :8003   │  │ :8004 │ │           │
│  └─────────┘  └───────┘ │           │
│                          │           │
│           ┌──────────────┘           │
│           │ tx-generator             │
│           │ → RPC :8080              │
│           └──────────────            │
└──────────────────────────────────────┘

Latency: <1ms (loopback)
Bandwidth: Unlimited
```

### F. Glossary

**TPS**: Transactions Per Second - rate at which transactions are committed to the blockchain

**P50/P95/P99**: Percentile latency metrics (50th, 95th, 99th percentile)

**View Change**: Consensus round advance due to timeout or failure (leader rotation)

**Mempool**: In-memory pool of pending transactions awaiting block inclusion

**Acceptance Rate**: Percentage of submitted transactions accepted by node (submitted/accepted)

**Block Interval**: Average time between committed blocks

**Sustained TPS**: TPS that can be maintained indefinitely without degradation

**Burst TPS**: Maximum TPS achievable for short duration (typically seconds)

---

## Conclusion

This performance report documents the baseline performance characteristics of the NOVAI blockchain node.

**Summary of Results**:
- [TBD: Overall performance verdict]
- [TBD: Bottlenecks identified]
- [TBD: Scalability assessment]
- [TBD: Production readiness]

**Next Steps**:
1. [TBD: Immediate actions based on findings]
2. [TBD: Follow-up tests needed]
3. [TBD: Optimization priorities]

---

**Report Metadata**:
- Generated by: [Tester name]
- Review status: [Draft/Final]
- Related documents:
  - [TUNING_PARAMETERS.md](./TUNING_PARAMETERS.md)
  - [Load Test README](../tests/load/README.md)
  - [CONSENSUS_V1.md](./CONSENSUS_V1.md)
