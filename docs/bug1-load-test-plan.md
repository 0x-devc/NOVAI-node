# Bug 1 load-test plan: SMT inclusion fix verification

Author: NOVAI operator (handle `0x-devc`), drafted 2026-06-09.
Diagnosis: `docs/gate3-bug1-diagnosis.md`.
Deploy doc this gates: `docs/deploy-bug1-fix.md`.
Verify script used post-deploy: `scripts/verify-host-fix.sh`.

This document is the Gate 5 local verification plan for the Bug 1 SMT inclusion fix. It is structured in three phases:

- **Phase 0 (REQUIRED)**: pre-deploy local gate, run on the MacBook against a local 4-node devnet. Must pass before any [redacted-host] deploy begins. Section A.7 of `docs/deploy-bug1-fix.md` blocks the deploy on Phase 0 passing.
- **Phase 1 (REQUIRED)**: post-deploy soak on [redacted-host], run after the T+24h checkpoint in `docs/deploy-bug1-fix.md` Section H.1 has confirmed AGREE. Validates the fix under sustained mixed load on the production-shape topology.
- **Phase 2 (OPTIONAL)**: non-Transfer-heavy stress workload that exercises the 13 fixed handler sites concentrated, used as a confidence builder if Phase 1 surfaces anything ambiguous.

## What this plan validates

The fix in `docs/gate3-bug1-diagnosis.md` section 2 routes 13 non-Transfer handler sites through the centralized `apply_state_ops_with_smt` helper so every state-mutating handler now authenticates its writes in `KEY_SMT_ROOT`. The load tests below stress every one of those sites under multi-validator consensus and prove three properties:

1. **Determinism under load**: 4 independent validators executing the same block sequence produce byte-identical `state_root` values at every committed height.
2. **Coverage**: every fixed handler site is exercised at least once during the workload, by class.
3. **No regression**: tx-throughput post-fix stays within an operator-defined band of pre-fix throughput (mitigation listed at `docs/gate3-bug1-diagnosis.md:266`).

## Conventions

- The MacBook runs from `~/NOVAI-node`.
- `<FIX_COMMIT>` is the Gate 4 merge commit; pin before starting Phase 0.
- `<BUG_COMMIT>` is `9ac23c4` (the bug-shipping binary commit).
- Phase 0 uses the in-tree `scripts/devnet.sh` for the 4-node local devnet (RPC ports 3030..3033, p2p 9000..9003).
- All workload calls use the `tools/tx-generator` CLI at `target/release/tx-generator`.

---

## Phase 0: pre-deploy local gate

Goal: prove on the MacBook that the fix binary produces deterministic `state_root` across 4 independent validators under a workload that exercises every fixed handler site, and that throughput has not regressed beyond the operator's tolerance band.

### 0.0: Prerequisites

```
git -C ~/NOVAI-node rev-parse HEAD
git -C ~/NOVAI-node status --short
```

Expected output:

- `rev-parse HEAD`: matches `<FIX_COMMIT>`.
- `status --short`: clean working tree (or only untracked files unrelated to the fix).

Branch on failure: if the wrong commit is checked out, `git checkout <FIX_COMMIT>` before proceeding. If the working tree has unrelated modifications, stash them; Phase 0 must run against a known fix-commit tree, not a mixed state.

### 0.1: Build the fix binary

```
cd ~/NOVAI-node
cargo build --release --bin novai-node --bin tx-generator
md5sum target/release/novai-node target/release/tx-generator
```

Expected output: a `Finished release` line, no errors. Record the `novai-node` md5 as `<FIX_BIN_MD5_LOCAL>` for cross-checking against the [redacted-host] build at `docs/deploy-bug1-fix.md` C.4.

Branch on failure: build failure here means the fix code does not compile cleanly; do not deploy. Fix the build before proceeding.

### 0.2: In-process determinism harness

Run the in-process 4-validator harness that was added in Gate 4 (per `docs/gate3-bug1-diagnosis.md:311-312`). This is the fastest signal that the fix routes every site correctly.

```
cargo test --release -p novai-execution --test multi_validator_determinism -- --nocapture
cargo test --release -p novai-execution --test multi_validator_determinism_rocksdb -- --nocapture
```

Expected output: both binaries report `test result: ok`. Every test name matching `w20_*`, `w21_*`, `w22_*`, ..., `w28_*` (the fix-validation suite added in Gate 4) must pass. Every `r1_*`..`r6_*` RocksDB-backed test must also pass; these prove compaction does not perturb the fix.

Branch on failure: a single failure in the W20-W28 set means the fix has missed a handler site (or has mis-routed one through the helper). Capture the test output, do NOT deploy, take the failure to claude.ai for review against `docs/gate3-bug1-diagnosis.md` section 2.

### 0.3: Local 4-node devnet spin-up

```
pkill -f 'novai-node run' 2>/dev/null || true
rm -rf /tmp/novai-devnet-* /tmp/node{0,1,2,3}.log
bash scripts/devnet.sh
sleep 20
```

Expected output: `scripts/devnet.sh` reports "All 4 nodes started" and the 20-second consensus warm-up gives validators time to bind p2p, exchange the first proposal round, and commit the first block.

Verify the four loopback RPC ports are responding:

```
for port in 3030 3031 3032 3033; do
  printf '%s: ' $port
  curl -s --max-time 3 -o /dev/null -w '%{http_code}\n' http://localhost:$port
done
```

Expected output: four lines each ending in `200`.

Branch on failure: if any port fails, inspect `/tmp/node$N.log` for the failing validator. Common cause: a stale `novai-node` process from a prior run that did not get killed; rerun the `pkill` from the top of this section.

### 0.4: Mixed-workload tx-generator run

Run the tx-generator with a workload that covers Transfer (control), entity registration, signal commitments, payments, oracle anchors, payment channels, and SLA flows. The exact subcommand is in `tools/tx-generator/src/main.rs`; the typical invocation is:

```
./target/release/tx-generator \
    --rpc http://localhost:3030 \
    --duration-secs 300 \
    --tps 50 \
    --mix transfer:50,entity_register:5,signal:20,payment:10,oracle_anchor:5,memory_object:5,sla:5 \
    --senders 20 \
    --log-level info \
    2>&1 | tee /tmp/txgen-fix.log
```

The mix splits 50% Transfer (control path, unchanged by the fix) and 50% across the seven non-Transfer classes that map onto the 13 fixed handler sites listed in `docs/gate3-bug1-diagnosis.md` section 2. The `--tps 50` rate is well within the 4-node local devnet's capacity and runs in about 5 minutes wall-clock.

Expected output: the tx-generator's final summary reports `submitted >= duration_secs * tps * 0.95` (i.e. at most 5% rejection at the mempool), no `panic` lines in the log, and the periodic per-class breakdown shows non-zero counts for every requested class.

Branch on failure: if any class returns zero accepted txs, that handler site is not being exercised; either the mix string is malformed for the tx-generator's CLI (re-check with `--help`) or the fix has broken acceptance for that class. Stop and inspect.

### 0.5: State root agreement verification

After the 300-second tx-generator run completes, sample state_root agreement across all 4 local validators at the same 4-height pattern used by `scripts/verify-host-fix.sh`. The simplest path is to run the verify script directly against the local devnet:

```
ORACLE_ADDR_HEX= bash scripts/verify-host-fix.sh
```

(Empty `ORACLE_ADDR_HEX` is the documented skip path for evidence section 5; the local devnet has no oracle.)

Expected output: the script's `FINAL VERDICT` section reports `PASS`. Heights tested cover head, head-10, head-100, head-1000 (all available after a 300s run at 50 TPS, which puts head well above 1000 blocks under one-second block intervals).

The journalctl-based section 7 and 8 of the verify script will be empty or noisy on the MacBook because the local devnet writes to `/tmp/node$N.log`, not systemd journals. That is expected; the AGREE/DIVERGE verdict in section 4 is the load-bearing signal for Phase 0.

Branch on failure: any DIVERGE means the fix is incomplete. Capture the verify output, the four `/tmp/node$N.log` files, and the tx-generator summary. Do NOT deploy. Take to claude.ai for review.

### 0.6: Performance regression check

Per `docs/gate3-bug1-diagnosis.md:266`, the operator's mitigation for latent concern C (SMT-per-tx cost) is a side-by-side throughput comparison.

If a pre-fix binary at `<BUG_COMMIT>` is available locally as `target/release/novai-node.pre-fix`, repeat 0.3-0.4 against it (swap the binary in `scripts/devnet.sh` or stage it as `/usr/local/bin/novai-node-pre-fix` and edit the spinup), record the tx-generator final TPS, and compute the ratio:

```
post_fix_tps / pre_fix_tps
```

Acceptable band: `>= 0.85` (i.e. up to a 15% throughput regression is acceptable for a correctness fix). Below 0.85, the deploy is still go (correctness over throughput), but file a follow-up to batch SMT updates across txs in a block per the diagnosis doc's deferred optimization note.

Branch on failure: if a pre-fix binary is not available locally (e.g. the operator rebuilt over it), skip this step. The deploy is not gated on 0.6; this is a measurement, not a verdict.

### 0.7: Phase 0 stop checkpoint

All of 0.1, 0.2, 0.4, 0.5 must report success before Phase 0 is considered passed. 0.0 and 0.3 are setup; 0.6 is informational. Record the following in the deploy log:

- `<FIX_BIN_MD5_LOCAL>` from 0.1
- in-process harness pass count from 0.2
- tx-generator final summary from 0.4
- `FINAL VERDICT: PASS` line from 0.5
- (optional) throughput ratio from 0.6

Tear the local devnet down before moving to the [redacted-host] deploy:

```
pkill -f 'novai-node run' 2>/dev/null || true
```

Phase 0 passing is the entry condition for `docs/deploy-bug1-fix.md` Section A. If Phase 0 has not passed, do not start the deploy.

---

## Phase 1: post-deploy soak ([redacted-host])

Goal: prove the fix holds under sustained mixed load on the production-shape topology, after the T+24h H.1 checkpoint in `docs/deploy-bug1-fix.md` has confirmed cold-start agreement.

Entry condition: `docs/deploy-bug1-fix.md` Section H.1 reports AGREE at every checkpoint through T+24h, AND the price-oracle is running and posting anchors (Section G.6 verified).

### 1.0: Scope and timing

Run a 2-hour mixed-workload tx-generator from the MacBook against the [redacted-host] @0 RPC at 25 TPS (half the local devnet rate, to leave headroom for the oracle and any organic traffic on the live testnet). Sample state_root agreement every 15 minutes during the run.

Wall-clock cost: about 2 hours of generator + 2.25 hours of operator attention with the verify script polled on a timer.

### 1.1: Workload

From the MacBook:

```
./target/release/tx-generator \
    --rpc http://[redacted-ip]:3030 \
    --duration-secs 7200 \
    --tps 25 \
    --mix transfer:40,entity_register:5,signal:25,payment:10,oracle_anchor:5,memory_object:10,sla:5 \
    --senders 40 \
    --log-level info \
    2>&1 | tee /tmp/txgen-soak-[redacted-host].log
```

The mix shifts a notch more toward non-Transfer classes (60% non-Transfer) versus Phase 0 (50%) to put more sustained pressure on the fixed handler sites.

Branch on failure: if the tx-generator reports rejection rates above 10% (versus Phase 0's 5% expectation), stop the workload. A high rejection rate post-deploy means either the mempool is saturated (which is acceptable; reduce `--tps` and retry) or the fix has introduced a per-tx cost spike that pushes consensus into late commits.

### 1.2: Monitoring cadence during the soak

In a separate shell on the MacBook:

```
ORACLE_ADDR_HEX=<value from docs/deploy-bug1-fix.md G.2>
while true; do
  echo "=== $(date -u +%H:%M:%S)Z ==="
  ssh root@[redacted-ip] "ORACLE_ADDR_HEX='$ORACLE_ADDR_HEX' bash /tmp/verify-host-fix.sh" \
    | tee -a /tmp/soak-verify.log
  sleep 900
done
```

The 15-minute polling interval gives 8 verify-script runs over the 2-hour window. Every run must report `FINAL VERDICT: PASS`. The `tee -a` accumulates a single combined log for paste-back if anything fails.

Branch on failure: any FAIL verdict during the soak is a deploy-rollback signal IF the failure is a DIVERGE on a height beyond the T+24h reference (i.e. a fix that was OK cold-start but breaks under load). If the failure is a single transient `RPC UNREACHABLE`, retry once before escalating.

### 1.3: Phase 1 stop checkpoint

All 8 verify-script runs must report PASS. Record the combined `/tmp/soak-verify.log` and the tx-generator final summary in the deploy log. Phase 1 passing is the entry condition for declaring Bug 1 closed.

---

## Phase 2 (OPTIONAL): non-Transfer stress

Goal: a confidence-builder if Phase 1 surfaced anything ambiguous (transient DIVERGE, throughput cliffs, single-validator lag). Phase 2 concentrates load on the fixed handler sites by dropping Transfer to 10%.

Skip Phase 2 if Phase 1 reported a clean 8-of-8 PASS.

### 2.0: Workload

```
./target/release/tx-generator \
    --rpc http://[redacted-ip]:3030 \
    --duration-secs 3600 \
    --tps 20 \
    --mix transfer:10,entity_register:10,signal:35,payment:10,oracle_anchor:10,memory_object:15,sla:10 \
    --senders 40 \
    --log-level info \
    2>&1 | tee /tmp/txgen-stress-[redacted-host].log
```

### 2.1: Monitoring

Same polling shell as 1.2, with the interval tightened to 5 minutes for the 1-hour run (12 verify samples).

Phase 2 PASS criterion: 12-of-12 verify reports report PASS.

---

## What this plan does NOT cover

- **Byzantine validators**: the load tests assume all 4 validators are honest. Adversarial validator behavior is out of scope for the Bug 1 fix verification.
- **Cross-binary determinism with the pre-fix chain**: the wipe in `docs/deploy-bug1-fix.md` Section D is the explicit acknowledgment that the fix is a hard fork at the SMT level. There is no validation here that pre-fix and post-fix binaries agree on the same block; they intentionally do not.
- **SDK / CLI determinism**: the Python SDK and `novai-cli` are upstream of consensus and unchanged by this fix. Their behavior is covered by their own test suites.
- **Sustained throughput beyond 2 hours**: the Phase 1 soak is bounded at 2 hours because that window covers approximately 180k transactions at 25 TPS, which is enough to exercise every fixed handler site many times over. A longer soak buys diminishing confidence.

---

## Appendix: Quick reference

```
# Phase 0 (local)
git -C ~/NOVAI-node rev-parse HEAD
cargo build --release --bin novai-node --bin tx-generator
cargo test --release -p novai-execution --test multi_validator_determinism -- --nocapture
cargo test --release -p novai-execution --test multi_validator_determinism_rocksdb -- --nocapture
pkill -f 'novai-node run' 2>/dev/null || true
bash scripts/devnet.sh
sleep 20
./target/release/tx-generator --rpc http://localhost:3030 --duration-secs 300 --tps 50 \
    --mix transfer:50,entity_register:5,signal:20,payment:10,oracle_anchor:5,memory_object:5,sla:5 \
    --senders 20
ORACLE_ADDR_HEX= bash scripts/verify-host-fix.sh
pkill -f 'novai-node run' 2>/dev/null || true

# Phase 1 ([redacted-host])
./target/release/tx-generator --rpc http://[redacted-ip]:3030 --duration-secs 7200 --tps 25 \
    --mix transfer:40,entity_register:5,signal:25,payment:10,oracle_anchor:5,memory_object:10,sla:5 \
    --senders 40 &
ORACLE_ADDR_HEX=<value from deploy doc G.2>
while true; do
  ssh root@[redacted-ip] "ORACLE_ADDR_HEX='$ORACLE_ADDR_HEX' bash /tmp/verify-host-fix.sh" \
    | tee -a /tmp/soak-verify.log
  sleep 900
done
```

Stopping point. The three Gate 5/6 artifacts (this plan, `docs/deploy-bug1-fix.md`, `scripts/verify-host-fix.sh`) are now complete and consistent with each other.
