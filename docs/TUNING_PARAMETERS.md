# NOVAI Node Tuning Parameters

**Last Updated**: Week 11 (January 2026)
**Status**: Recommended baseline values

This document describes configurable parameters for NOVAI blockchain nodes and provides tuning recommendations based on network conditions and hardware capabilities.

---

## Table of Contents

1. [Consensus Parameters](#consensus-parameters)
2. [Mempool Parameters](#mempool-parameters)
3. [Network Parameters](#network-parameters)
4. [Performance Parameters](#performance-parameters)
5. [Recommended Configurations](#recommended-configurations)
6. [Load Test Results](#load-test-results)

---

## Consensus Parameters

### Timeout Configuration

**Location**: `crates/consensus/src/lib.rs`

```rust
pub const BASE_TIMEOUT_MS: u64 = 2000;        // 2 seconds
pub const TIMEOUT_MULTIPLIER: u64 = 2;        // Exponential backoff multiplier
pub const MAX_TIMEOUT_MS: u64 = 60_000;       // 60 seconds
```

#### `BASE_TIMEOUT_MS`

**Purpose**: Initial timeout for consensus round progression. If no proposal or quorum is reached within this time, the round advances (view change).

**Current Value**: `2000` ms (2 seconds)

**Tuning Guidelines**:
- **Lower values (1000-1500 ms)**: Faster recovery from failures, but may cause unnecessary view changes under normal network latency
- **Higher values (3000-5000 ms)**: More tolerance for network delays, but slower failure recovery
- **Recommended**:
  - LAN environments: 1000-2000 ms
  - WAN with <100ms latency: 2000-3000 ms
  - High-latency networks: 3000-5000 ms

**Impact**:
- Lower → More view changes, potentially lower throughput
- Higher → Slower failure recovery, longer block times under normal operation

#### `TIMEOUT_MULTIPLIER`

**Purpose**: Exponential backoff multiplier for repeated view changes. Each timeout failure increases the wait time by this factor.

**Current Value**: `2` (doubles each round)

**Calculation**: `timeout_ms = BASE_TIMEOUT_MS * (TIMEOUT_MULTIPLIER ^ consecutive_view_changes)`

**Tuning Guidelines**:
- **Value of 2**: Standard exponential backoff (2s → 4s → 8s → 16s → 32s → 60s cap)
- **Value of 1**: No backoff (constant timeout)
- **Value of 3**: Aggressive backoff (2s → 6s → 18s → 54s → 60s cap)

**Recommended**: Keep at `2` for balanced behavior. Only adjust for extreme network conditions.

#### `MAX_TIMEOUT_MS`

**Purpose**: Upper bound on timeout duration to prevent infinite waiting.

**Current Value**: `60000` ms (60 seconds)

**Tuning Guidelines**:
- **Recommended**: 30,000-120,000 ms (30-120 seconds)
- Should be at least 10x `BASE_TIMEOUT_MS` to allow several backoff iterations
- Lower values → Network may never stabilize under sustained partitions
- Higher values → Prolonged waiting during persistent failures

---

## Mempool Parameters

### Transaction Pool Configuration

**Location**: `crates/node/src/main.rs` (mempool initialization)

```rust
let mempool = Arc::new(Mutex::new(TxMempool::new(min_fee, fairness_cap)));
```

#### `MIN_TX_FEE`

**Purpose**: Minimum transaction fee (in smallest currency unit) required for mempool acceptance.

**Current Value**: `1`

**Tuning Guidelines**:
- **Value of 0**: Accept all transactions (useful for testnets, risky for production)
- **Value of 1-1000**: Low barrier for inclusion (suitable for low-traffic networks)
- **Value of 1000+**: Higher spam protection (recommended for production)

**Recommended**:
- Testnet: 0-10
- Low-traffic mainnet: 100-1000
- High-traffic mainnet: 1000-10000

**Impact**:
- Lower → More spam vulnerability, larger mempool size
- Higher → Higher barrier to entry, potentially excludes legitimate low-value transactions

#### `FAIRNESS_CAP_PER_SENDER`

**Purpose**: Maximum number of transactions per sender account allowed in mempool simultaneously.

**Current Value**: `1000`

**Tuning Guidelines**:
- **Low values (10-100)**: Strict fairness, prevents sender from filling mempool
- **Medium values (100-1000)**: Balanced throughput and fairness
- **High values (1000+)**: Maximum throughput, but allows single sender to dominate

**Recommended**:
- Small validator sets (<10 nodes): 100-500
- Large validator sets (10+ nodes): 500-1000
- High-frequency applications: 1000-5000

**Impact**:
- Lower → Better fairness, lower mempool memory usage
- Higher → Higher throughput for bulk senders, more memory usage

---

## Network Parameters

### Block Encoding Configuration

**Location**: `crates/consensus_types/src/codec.rs`

```rust
pub const MAX_TXS_PER_BLOCK: usize = 10_000;
pub const MAX_BLOCKS_PER_RESPONSE: usize = 1000;
```

#### `MAX_TXS_PER_BLOCK`

**Purpose**: Hard limit on number of transactions that can be included in a single block.

**Current Value**: `10000` transactions

**Tuning Guidelines**:
- **Small blocks (100-1000 tx)**: Faster propagation, lower bandwidth, lower throughput
- **Medium blocks (1000-10000 tx)**: Balanced performance
- **Large blocks (10000+ tx)**: Maximum throughput, but higher latency and bandwidth requirements

**Recommended**:
- Based on target TPS and block interval:
  - `MAX_TXS_PER_BLOCK = target_TPS * BLOCK_INTERVAL_SECONDS`
  - Example: 100 TPS × 3 seconds = 300 tx/block minimum
  - Recommend 3-5x the calculated minimum for burst handling

**Performance Impact**:
| Block Size | Block Propagation | Validation Time | Memory Usage |
|------------|-------------------|-----------------|--------------|
| 100 tx     | ~10-50ms         | <1ms            | Low          |
| 1,000 tx   | ~50-200ms        | 1-10ms          | Medium       |
| 10,000 tx  | ~200-1000ms      | 10-100ms        | High         |

**Constraints**:
- Must be large enough to handle target TPS with comfortable headroom
- Must not exceed network bandwidth or block propagation requirements
- Validator hardware must handle validation time for full blocks

#### `MAX_BLOCKS_PER_RESPONSE`

**Purpose**: Maximum number of blocks returned in a single sync/catch-up response.

**Current Value**: `1000` blocks

**Tuning Guidelines**:
- Higher values → Faster sync for nodes catching up from far behind
- Lower values → Smaller messages, more protocol overhead

**Recommended**: 500-2000 blocks (current value is appropriate)

---

## Performance Parameters

### Block Production Timing

**Location**: Consensus loop in `crates/node/src/main.rs`

**Current Implementation**: Implicit timing based on timeout and network latency

**Ideal Block Interval** (not yet explicitly configured):
- **1-3 seconds**: High throughput, requires low-latency network
- **3-5 seconds**: Balanced for most deployments
- **5-10 seconds**: Conservative for high-latency networks

**Target TPS Calculation**:
```
Effective TPS = (AVG_TXS_PER_BLOCK / BLOCK_INTERVAL_SECONDS) * SUCCESS_RATE

Where:
- AVG_TXS_PER_BLOCK: Actual average block size (≤ MAX_TXS_PER_BLOCK)
- BLOCK_INTERVAL_SECONDS: Time between committed blocks
- SUCCESS_RATE: Percentage of rounds that commit without view change (0.9-0.99)
```

**Example**:
- Block interval: 3 seconds
- Average block size: 500 tx
- Success rate: 95%
- **Effective TPS**: (500 / 3) * 0.95 = **158 TPS**

---

## Recommended Configurations

### Configuration A: Low-Traffic Testnet

**Use Case**: Development, testing, low transaction volume

```rust
// Consensus
BASE_TIMEOUT_MS = 1500
TIMEOUT_MULTIPLIER = 2
MAX_TIMEOUT_MS = 30_000

// Mempool
MIN_TX_FEE = 0
FAIRNESS_CAP_PER_SENDER = 100

// Network
MAX_TXS_PER_BLOCK = 1_000
```

**Expected Performance**:
- Target TPS: 50-100
- Block interval: ~2-3 seconds
- Memory usage: Low (<500 MB)

---

### Configuration B: Medium-Traffic Network

**Use Case**: Production deployment with moderate load

```rust
// Consensus
BASE_TIMEOUT_MS = 2000
TIMEOUT_MULTIPLIER = 2
MAX_TIMEOUT_MS = 60_000

// Mempool
MIN_TX_FEE = 100
FAIRNESS_CAP_PER_SENDER = 500

// Network
MAX_TXS_PER_BLOCK = 5_000
```

**Expected Performance**:
- Target TPS: 200-500
- Block interval: ~3-4 seconds
- Memory usage: Medium (500 MB - 2 GB)

**Hardware Requirements**:
- CPU: 4+ cores
- RAM: 4+ GB
- Network: 10+ Mbps
- Disk: SSD recommended

---

### Configuration C: High-Throughput Network

**Use Case**: High transaction volume, optimized for throughput

```rust
// Consensus
BASE_TIMEOUT_MS = 2000
TIMEOUT_MULTIPLIER = 2
MAX_TIMEOUT_MS = 60_000

// Mempool
MIN_TX_FEE = 1000
FAIRNESS_CAP_PER_SENDER = 1000

// Network
MAX_TXS_PER_BLOCK = 10_000
```

**Expected Performance**:
- Target TPS: 500-1000+
- Block interval: ~3-5 seconds
- Memory usage: High (2-8 GB)

**Hardware Requirements**:
- CPU: 8+ cores
- RAM: 16+ GB
- Network: 100+ Mbps
- Disk: NVMe SSD

---

## Load Test Results

### Baseline Results (Configuration B)

**Test Setup**:
- Hardware: [To be filled after running tests]
- Network: Local (single machine)
- Validators: 4 nodes

**Steady 100 TPS Test** (5 minutes):
```
Submitted:    30,000 tx
Accepted:     TBD
Rejected:     TBD
Failed:       TBD
Actual TPS:   TBD
P50 Latency:  TBD µs
P95 Latency:  TBD µs
P99 Latency:  TBD µs
```

**Steady 500 TPS Test** (3 minutes):
```
Submitted:    90,000 tx
Accepted:     TBD
Rejected:     TBD
Failed:       TBD
Actual TPS:   TBD
P50 Latency:  TBD µs
P95 Latency:  TBD µs
P99 Latency:  TBD µs
```

**Burst 1000 TPS Test** (30 seconds):
```
Submitted:    30,000 tx
Accepted:     TBD
Rejected:     TBD
Failed:       TBD
Actual TPS:   TBD
P50 Latency:  TBD µs
P95 Latency:  TBD µs
P99 Latency:  TBD µs
```

**Note**: Run load tests using scripts in `tests/load/` to populate these results.

---

## Tuning Process

### Step 1: Establish Baseline

1. Deploy network with default Configuration B values
2. Run `./tests/load/steady_100_tps.sh`
3. Monitor metrics at `http://localhost:8080/metrics`
4. Record baseline performance

### Step 2: Identify Bottlenecks

Check for symptoms:

**High view change rate** (`novai_consensus_view_changes_total` increasing rapidly):
- Increase `BASE_TIMEOUT_MS` by 500-1000 ms
- Check network latency between validators

**Low TPS despite available capacity** (mempool not empty, blocks small):
- Increase `MAX_TXS_PER_BLOCK`
- Decrease `MIN_TX_FEE` (if rejections are high)

**Memory exhaustion** (OOM errors, high `novai_mempool_size`):
- Decrease `FAIRNESS_CAP_PER_SENDER`
- Increase `MIN_TX_FEE` to filter spam
- Increase block production rate (if possible)

**High P99 latency** (>10ms):
- Increase `--workers` in tx-generator (not a node parameter)
- Check network bandwidth saturation
- Verify node CPU is not maxed out

### Step 3: Iterative Tuning

1. Change ONE parameter at a time
2. Run appropriate load test (steady_100, steady_500, or burst_1000)
3. Compare results to baseline
4. If improved, keep change; if degraded, revert
5. Repeat for next parameter

### Step 4: Validate at Scale

After tuning:
1. Run `./tests/load/mixed_load.sh` to test variable load patterns
2. Verify stability over extended period (1+ hours)
3. Test with multiple nodes in realistic network topology
4. Monitor for memory leaks or resource exhaustion

---

## Monitoring and Metrics

### Key Metrics to Track

**Prometheus Endpoints** (`http://localhost:8080/metrics`):

```
novai_committed_height          # Block production rate
novai_current_round             # Current consensus round
novai_consensus_view_changes_total  # Timeout/failure recovery count
novai_mempool_size              # Transactions waiting
novai_block_tx_count            # Transactions in last block
novai_total_txs_committed       # Cumulative throughput
```

**Derived Metrics**:
- **Blocks per second**: `rate(novai_committed_height[1m])`
- **View change rate**: `rate(novai_consensus_view_changes_total[5m])`
- **Effective TPS**: `rate(novai_total_txs_committed[1m])`

### Health Indicators

**Healthy Network**:
- View change rate: <0.1 per second (< 10% of rounds fail)
- Mempool size: Stable or slowly growing (not unbounded)
- Block tx count: Consistently near capacity (good utilization)
- P95 latency: <5ms

**Unhealthy Network**:
- View change rate: >0.5 per second (network instability)
- Mempool size: Rapidly growing (backlog forming)
- Block tx count: Consistently small (<10% of `MAX_TXS_PER_BLOCK`)
- P95 latency: >50ms

---

## FAQ

### Q: What's the maximum theoretical TPS?

**A**: Theoretical maximum is constrained by:
```
Max TPS = MAX_TXS_PER_BLOCK / (BASE_TIMEOUT_MS / 1000)
         = 10,000 / 2
         = 5,000 TPS
```

However, realistic sustainable TPS is 20-50% of theoretical maximum due to:
- Network latency
- Validation time
- View changes
- Mempool contention

**Realistic sustained TPS**: 500-1000 TPS with current parameters.

### Q: Should I increase MAX_TXS_PER_BLOCK indefinitely?

**A**: No. Diminishing returns occur because:
1. Block propagation time increases linearly with block size
2. Validation time increases with transaction count
3. Memory usage for pending blocks grows
4. Larger blocks increase probability of timeouts

Recommended maximum: 10,000-50,000 transactions per block depending on transaction complexity.

### Q: How do I tune for a high-latency WAN deployment?

**A**: Increase timeouts proportionally to network latency:
```rust
BASE_TIMEOUT_MS = (P95_NETWORK_LATENCY_MS * 3) + 500
```

Example: 200ms P95 latency → `BASE_TIMEOUT_MS = 1100ms`

Also consider:
- Reduce `MAX_TXS_PER_BLOCK` to compensate for longer propagation
- Increase `MAX_TIMEOUT_MS` to allow more backoff attempts
- Deploy validators geographically closer together if possible

### Q: My node keeps falling behind. What should I tune?

**A**: This indicates the node cannot keep up with consensus. Check:

1. **CPU saturation**: Upgrade hardware or reduce `MAX_TXS_PER_BLOCK`
2. **Disk I/O**: Use SSD, enable write caching
3. **Network bandwidth**: Reduce `MAX_TXS_PER_BLOCK` or upgrade connection
4. **Mempool exhaustion**: Increase `MIN_TX_FEE` to reduce load

---

## Change Log

| Date       | Parameter              | Old Value | New Value | Reason                        |
|------------|------------------------|-----------|-----------|-------------------------------|
| 2026-01-18 | Initial configuration  | -         | See above | Week 11 baseline values       |

---

## References

- [NOVAI Architecture Decisions](./ARCHITECTURE_DECISIONS.md)
- [Consensus Specification](./CONSENSUS_V1.md)
- [Load Test Scenarios](../tests/load/README.md)
- [Week 11 Implementation Plan](./WEEKS_1_12_IMPLEMENTATION_PLAN.md)
