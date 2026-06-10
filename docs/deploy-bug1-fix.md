# Bug 1 [redacted-host] deploy: SMT inclusion fix (wipe and redeploy)

Author: NOVAI operator (handle `0x-devc`), drafted 2026-06-09.
Source of truth for the fix scope: `docs/gate3-bug1-diagnosis.md`.
Local verification gate: `docs/bug1-load-test-plan.md` (Phase 0 / pre-deploy section).
Path: forward-only wipe and redeploy. No rollback possible after Section D.

## What this deploy ships

The fix described in `docs/gate3-bug1-diagnosis.md` sections 1, 2, and 4 (Option A: centralized `apply_state_ops_with_smt` helper applied to the 13 non-Transfer handler sites listed in section 2). Optionally also commits 2 and 3 from that diagnosis (latent concern B: flush before forced compaction; latent bug A: atomic `KEY_COMMITTED_HEIGHT` in sync path).

After the fix, every non-Transfer state-mutating handler authenticates its writes in `KEY_SMT_ROOT`. Pre-fix and post-fix binaries produce different `state_root` values for any block that contains a non-Transfer transaction. This is a de-facto hard fork at the SMT level; wipe is mandatory.

## Conventions used in this document

- The operator runs from a MacBook on macOS.
- "On [redacted-host] host" means inside an `ssh root@[redacted-ip]` session.
- Placeholder `<FIX_COMMIT>` is the merge commit from Gate 4. Fill in before starting.
- Placeholder `<DEPLOY_DATE>` is the deploy date in `YYYYMMDD` form (UTC).
- Placeholder `<ORACLE_ADDR_HEX>` is the on-chain creator address for the fresh oracle key generated in Section G. Record alongside `<ORACLE_ENTITY_ID>` (the 32-byte hex entity id derived from the address at registration).
- Placeholder `<PRE_DEPLOY_BIN_MD5>` is captured during Section A and pasted back into this doc at Section A.1.

## Hard rules for the operator

1. Do not touch nginx. The host serves `[redacted-domain]`, `[redacted-domain]`, and `[redacted-domain]` from the same box. No `systemctl restart nginx`, no edits to `/etc/nginx/`.
2. Do not delete `[redacted-server]/forensics-2026-06-08/`. The state snapshots and journals there are evidence; preserve through the deploy.
3. After Section D wipe begins, the chain history is gone forward. There is no path back without restoring from the pre-deploy backup created in Section B.

---

## Section A: Pre-deploy checks

Goal: verify the host is in the expected pre-deploy state and the fix commit is ready. If any check fails, stop and investigate before proceeding to Section B.

### A.1: Capture the current binary fingerprint

```
ssh root@[redacted-ip] 'md5sum /usr/local/bin/novai-node'
```

Expected output: a single md5 hash followed by the binary path. Record this hash as `<PRE_DEPLOY_BIN_MD5>`. There is no `--version` flag on `novai-node` (confirmed by `crates/node/src/main.rs:32-43` usage block, which lists no `--version`); md5 is the binary identity.

Branch on failure: if the file does not exist, abort. The bug-shipping binary at commit `9ac23c4` should be installed.

### A.2: Verify repo HEAD on the [redacted-host] host

```
ssh root@[redacted-ip] 'git -C [redacted-server]/NOVAI-node/NOVAI-node rev-parse HEAD'
```

Expected output: `9ac23c4` (full sha will be longer; the first 7 chars must match).

Branch on failure: the source tree is on the wrong commit. Stop and inspect.

### A.3: Verify all 4 validator services active

```
ssh root@[redacted-ip] 'for i in 0 1 2 3; do systemctl is-active novai-node@$i; done'
```

Expected output: four lines, each reading `active`.

Branch on failure: if any line reads `inactive` or `failed`, this is the post-incident state where @3 may be alive but stuck. That is fine for the wipe path (Section D wipes all 4 anyway), but record which units were not active so the post-deploy verification can confirm they all come back.

### A.4: Verify @0, @1, @2 agree on the head block

```
ssh root@[redacted-ip] 'for port in 3030 3031 3032; do
  echo "=== port $port ==="
  curl -s --max-time 5 -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"novai_getLatestBlock\",\"params\":[],\"id\":1}" \
    http://localhost:$port
  echo
done'
```

Expected output: three JSON responses. Compare the `block_hash` (or equivalent block-identity field) across all three. They should match.

Branch on failure: if @0/@1/@2 diverge from each other, the fork has widened beyond the original @3-only divergence. Do not deploy. Take the output to claude.ai for review.

Note: @3 (port 3033) is expected to be stuck at `committed_height=0` per the operator-supplied state; querying it is informational only and not required for this check.

### A.5: Forensic snapshot intact

```
ssh root@[redacted-ip] 'ls -lah [redacted-server]/forensics-2026-06-08/'
```

Expected output: a directory listing showing `state-snapshots.tar.gz` (about 8.6GB) and `journals.tar.gz` (about 1.5MB). Both files present and non-empty.

Branch on failure: if missing or zero-byte, halt. The forensic record must survive the deploy.

### A.6: Disk space headroom

```
ssh root@[redacted-ip] 'df -BG / | awk "NR==2 {print \$4}" && df -BG /var/lib/novai | awk "NR==2 {print \$4}"'
```

Expected output: two integers (gigabytes free), each followed by `G`. Both numbers must be at least `40G`.

Branch on failure: if either is below 40G, free space before proceeding. Likely candidates: old log files under `/var/log/novai/`, stale RocksDB SST files outside the active data dirs, prior backup tarballs. Do not delete `[redacted-server]/forensics-2026-06-08/`.

### A.7: Verify local repo HEAD on the MacBook matches the fix commit

On the MacBook (not over ssh):

```
git -C ~/NOVAI-node rev-parse HEAD
```

Expected output: the full sha of `<FIX_COMMIT>` (the Gate 4 merge commit).

Branch on failure: Gate 4 has not landed, or has not been pulled. Do not deploy until the fix commit exists locally and has passed the Gate 5 local verification described in `docs/bug1-load-test-plan.md` Phase 0.

### A.8: RPC reachability on all 4 loopback ports

```
ssh root@[redacted-ip] 'for port in 3030 3031 3032 3033; do
  printf "%s: " $port
  curl -s --max-time 3 -o /dev/null -w "%{http_code}\n" http://localhost:$port
done'
```

Expected output: four lines of the form `30NN: 200` (or any 2xx). A non-2xx on @3 is acceptable per the stuck-at-zero state; @0/@1/@2 must respond.

Branch on failure: an unexpected port not responding indicates the systemd unit is not bound. Investigate before deploy.

### A.9: Stop checkpoint

All A.1 through A.8 must pass before proceeding. Paste each output into the deploy log notebook. If anything is unexpected, halt and bring it to claude.ai for review.

---

## Section B: Backup phase

Goal: take a single-step-back snapshot of all 4 validator data dirs and the current binary, in case the post-deploy chain cannot recover forward and panic-recovery is needed.

This phase is not optional.

### B.1: Stop the validators with rolling gaps

On [redacted-host] host (single ssh session is easiest):

```
ssh root@[redacted-ip]
```

Then inside that session:

```
for i in 0 1 2 3; do
  systemctl stop novai-node@$i
  echo "stopped novai-node@$i at $(date -u +%H:%M:%S)"
  sleep 8
done
```

Expected output: four `stopped ...` lines, 8 seconds apart.

Verify after the loop:

```
for i in 0 1 2 3; do systemctl is-active novai-node@$i; done
```

Expected output: four lines, each reading `inactive` (or `failed`; both indicate stopped).

### B.2: Tar each validator's data dir

```
BACKUP_DIR=[redacted-server]/pre-deploy-backup-<DEPLOY_DATE>
mkdir -p "$BACKUP_DIR"
for i in 0 1 2 3; do
  tar czf "$BACKUP_DIR/node$i.tar.gz" -C /var/lib/novai/node$i .
  echo "backed up node$i"
done
```

### B.3: Verify backup integrity

```
ls -lah [redacted-server]/pre-deploy-backup-<DEPLOY_DATE>/
```

Expected output: four `node{N}.tar.gz` files, each non-empty (size > 0).

Spot-check all 4 tarballs:

```
for i in 0 1 2 3; do
  echo "=== node$i ==="
  tar tzf [redacted-server]/pre-deploy-backup-<DEPLOY_DATE>/node$i.tar.gz | head -3 | wc -l
done
```

Expected: four blocks, each printing a node label followed by the integer `3` (three entries listed from the head of each tarball). If any reports `0`, that tarball is empty; redo the tar for that node.

### B.4: Backup the running binary

```
cp /usr/local/bin/novai-node /usr/local/bin/novai-node.bak-pre-fix-<DEPLOY_DATE>
md5sum /usr/local/bin/novai-node.bak-pre-fix-<DEPLOY_DATE>
```

Expected output: an md5 hash that matches `<PRE_DEPLOY_BIN_MD5>` captured in A.1.

Branch on failure: if md5 differs, the copy is corrupt; redo.

### B.5: Record paths

Note these paths in the deploy log:

- Data backups: `[redacted-server]/pre-deploy-backup-<DEPLOY_DATE>/node{0,1,2,3}.tar.gz`
- Binary backup: `/usr/local/bin/novai-node.bak-pre-fix-<DEPLOY_DATE>`
- Pre-existing bug-shipping binary backup: `/usr/local/bin/novai-node.bak-pre-9ac23c4` (left in place from prior ops)

These are the only restore points for Section I panic-recovery.

---

## Section C: Build phase

Goal: produce the post-fix binary on the [redacted-host] host from `<FIX_COMMIT>`.

### C.1: Fetch and check out the fix commit

```
cd [redacted-server]/NOVAI-node/NOVAI-node
git fetch origin
git checkout <FIX_COMMIT>
git status
```

Expected output of `git status`: clean working tree, `HEAD detached at <FIX_COMMIT>` or equivalent.

Branch on failure: if the working tree is dirty, stash or discard (the deploy host should never have local changes); if `<FIX_COMMIT>` is unknown to the fetch, the operator pushed to a branch other than `main` and the commit is not on origin yet.

### C.2: Verify the relevant fix sites compile from this commit

A sanity grep before building (no edit). This confirms the helper landed where the diagnosis said it would:

```
grep -n "apply_state_ops_with_smt" crates/execution/src/lib.rs | head -20
```

Expected output: multiple lines. At minimum, the definition of `apply_state_ops_with_smt` and a number of call sites (target: 15 sites total per `docs/gate3-bug1-diagnosis.md` section 2, including the 13 newly fixed plus the 2 Transfer refactor sites).

Branch on failure: if grep returns nothing, the wrong commit is checked out, or the fix did not use the helper name from the diagnosis. Stop.

### C.3: Build release binary

```
cargo build --release --bin novai-node
```

Expected output: ends with a `Compiling` cascade then `Finished release` or `Finished` line. No errors.

Branch on failure: if the build fails on [redacted-host] but succeeded locally on the MacBook, suspect dependency state (run `cargo clean` then rebuild) or disk space (rerun A.6).

### C.4: Verify the binary built

```
ls -lah target/release/novai-node
file target/release/novai-node
md5sum target/release/novai-node
```

Expected output:

- `ls -lah`: file present, size > 0, recent timestamp.
- `file`: reports an ELF 64-bit LSB executable, x86-64.
- `md5sum`: a hash distinct from `<PRE_DEPLOY_BIN_MD5>`.

Record the new md5 as `<POST_FIX_BIN_MD5>` in the deploy log.

Branch on failure: if md5 equals the pre-deploy hash, the binary did not actually change (suspect a stale `target/release/` cache; `cargo clean -p novai-node` and rebuild).

---

## Section D: Wipe phase

Hard line: after D.2 executes, the chain history on this host is gone forward. There is no rollback path other than the panic-recovery sequence in Section I.

### D.1: Confirm validators still stopped

```
for i in 0 1 2 3; do systemctl is-active novai-node@$i; done
```

Expected output: four lines of `inactive` or `failed`.

Branch on failure: if any unit became active between Section B.1 and now, stop it immediately:

```
systemctl stop novai-node@$i
```

Re-verify before proceeding.

### D.2: Wipe data directories

```
for i in 0 1 2 3; do
  rm -rf /var/lib/novai/node$i/*
  rm -rf /var/lib/novai/node$i/.??*
  echo "wiped node$i"
done
```

The second `rm` removes dotfiles (RocksDB sometimes leaves `.identity` etc).

### D.3: Verify wipes succeeded

```
for i in 0 1 2 3; do
  count=$(ls -la /var/lib/novai/node$i | wc -l)
  echo "node$i: $count entries (expect 3: ., .., and nothing else)"
done
```

Expected output: each line ends in `3 entries (expect 3: ., .., and nothing else)`. The directories themselves remain (do not remove the directories themselves).

Branch on failure: if any node dir has more than 3 entries listed, redo the rm for that node. If the directory itself is missing, recreate with `mkdir -p /var/lib/novai/node$i && chown novai:novai /var/lib/novai/node$i`.

### D.4: Install new binary

```
cp [redacted-server]/NOVAI-node/NOVAI-node/target/release/novai-node /usr/local/bin/novai-node
chmod 755 /usr/local/bin/novai-node
chown root:root /usr/local/bin/novai-node
md5sum /usr/local/bin/novai-node
```

Expected output of md5sum: equals `<POST_FIX_BIN_MD5>` from C.4.

Branch on failure: hash mismatch indicates a partial copy (disk full, signal interruption). Redo.

### D.5: Verify systemd will run the new binary as the right user

```
systemctl cat novai-node@0 | grep -E "ExecStart|User|Group"
```

Expected output: the `ExecStart=` line references `/usr/local/bin/novai-node` (or whatever path the systemd template uses). The `User=` line should read `novai`. If the unit runs as `root`, this is a topology drift versus the operator-supplied notes; pause and reconcile before starting.

---

## Section E: Dev-keys bootstrap verification

This deploy runs in **dev-keys mode**, not production mode. The four validators sign with deterministic seeds `[0;32]`, `[1;32]`, `[2;32]`, `[3;32]` hardcoded in `crates/node/src/main.rs:948`, selected per-instance by `--validator <index>`. Genesis state is funded on first start by `apply_dev_genesis` (`crates/node/src/main.rs:551-601`), which seeds 100 sender accounts and writes the initial SMT root in a single atomic batch. There is no `--genesis <path>` file, no `--key-file <path>`, and no separate bootstrap script for this deploy.

Section E confirms the systemd template is configured for dev-keys mode and will boot cleanly on the empty data directories produced by Section D.

### E.1: Confirm ExecStart uses the dev-keys flag set

```
systemctl cat novai-node@0 | grep ExecStart
```

Expected output: the `ExecStart=` line includes all three of:

- `--dev-keys`
- `--allow-insecure-dev-keys`
- `--validator %i` (where `%i` is the systemd template instance specifier)

Per `crates/node/src/main.rs:912-919`, the binary refuses to start with `--dev-keys` unless `--allow-insecure-dev-keys` is also present. If either flag is missing, the unit will not start after the wipe.

Branch on failure: if any of the three flags is missing, do not proceed. Edit `/etc/systemd/system/novai-node@.service` to restore the full flag set, `systemctl daemon-reload`, and re-run this check before moving to Section F.

### E.2: Confirm --validator <N> matches the systemd template instance index

```
for i in 0 1 2 3; do
  echo -n "novai-node@$i ExecStart --validator: "
  systemctl cat novai-node@$i | grep ExecStart | grep -oE '\-\-validator [0-9]+'
done
```

Expected output: four lines, each printing `--validator 0`, `--validator 1`, `--validator 2`, `--validator 3` respectively. The numeric argument must match the instance suffix because the systemd template resolves `%i` per instance; a mismatch means a non-template ExecStart line is hardcoded and the validator identity would not align with the instance.

Branch on failure: any mismatch is a deploy-blocking config drift. Per `main.rs:946`, `--validator <N>` selects which of the four hardcoded dev seeds the node will sign with. If `novai-node@2` ran as `--validator 0`, two validators would sign with seed `[0;32]` and consensus would deadlock. Fix the template (use `--validator %i`) before proceeding.

### E.3: Verify the systemd unit template lives at the expected path

```
ls -lah /etc/systemd/system/novai-node@.service
systemctl cat novai-node@0 | head -1
```

Expected output:

- `ls -lah`: file present, non-empty, owned by `root:root`.
- `head -1` of `systemctl cat`: prints a comment line of the form `# /etc/systemd/system/novai-node@.service` confirming the template is loaded from that path (not from a drop-in `/etc/systemd/system/novai-node@.service.d/` override that could shadow the ExecStart).

Branch on failure: if the file is missing, the four `novai-node@N` services have no template to instantiate from and Section F will fail. If `systemctl cat` shows the unit is loaded from a different path (e.g., `/run/systemd/`), an out-of-band override is active; resolve before starting validators.

### E.4: Confirm production-mode flags are NOT present

This deploy must not include `--genesis <path>` or `--key-file <path>` on any of the four ExecStart lines. Those flags are mutually exclusive with dev-keys mode (`crates/node/src/main.rs:943-987` branches on `dev_keys` and reaches the `--genesis <path> required` panic only when `dev_keys=false`).

```
for i in 0 1 2 3; do
  echo "=== novai-node@$i ==="
  systemctl cat novai-node@$i | grep -E "\-\-genesis|\-\-key-file" || echo "  (clean: no production flags)"
done
```

Expected output: four blocks, each printing `(clean: no production flags)`.

Branch on failure: if any `--genesis` or `--key-file` line is present, the unit is in a hybrid state. Edit the template to remove the production flags before proceeding; the dev-genesis path in `apply_dev_genesis` will not run if production genesis parsing intercepts startup first.

### E.5: Confirm `--faucet-key` is present in dev-keys ExecStart

The dev-keys ExecStart must include `--faucet-key <path>` so the public faucet endpoint is mounted. If it is missing, the oracle bootstrap in G.2 will exit `3` (`cooldown_and_insufficient_balance`) because `chain.faucet()` will fail outright, not because of a cooldown.

```
for i in 0 1 2 3; do
  echo "=== novai-node@$i ==="
  systemctl cat novai-node@$i | grep -oE '\-\-faucet-key [^ ]+' || echo "  FAIL: --faucet-key NOT present"
done
```

Expected output: four blocks, each printing a `--faucet-key <path>` line.

Branch on failure: if any unit is missing `--faucet-key`, faucet calls from `bootstrap.py` will fail with exit code 3 at G.2. Edit the template to restore `--faucet-key` before proceeding.

---

## Section F: Rolling start

Goal: bring validators up in sequence, give consensus a chance to warm up, and run the first verification round.

### F.1: Start the seed validator

```
systemctl start novai-node@0
sleep 8
systemctl is-active novai-node@0
journalctl -u novai-node@0 -n 20 --no-pager
```

Expected output:

- `is-active`: `active`.
- `journalctl`: recent log lines showing the `Dev genesis: funded tx-generator sender accounts` line from `apply_dev_genesis` (`main.rs:595-600`), the `WARNING: Running with DETERMINISTIC dev keys` warning from the dev-keys check (`main.rs:922`), p2p bind, and either "waiting for peers" or initial consensus activity. No `panic`, no `ERROR` lines.

Branch on failure: if the unit is `failed`, inspect the journal for the cause. Common issues at this point: missing `--allow-insecure-dev-keys` (rerun E.1), `--validator <N>` out of range 0..=3 (rerun E.2), data dir not writable by `novai` user (`chown -R novai:novai /var/lib/novai`).

### F.2: Start the remaining validators with 8-second gaps

```
for i in 1 2 3; do
  systemctl start novai-node@$i
  echo "started novai-node@$i at $(date -u +%H:%M:%S)"
  sleep 8
done
```

### F.3: Consensus warm-up

```
sleep 20
```

The 20-second warm-up gives the 4 validators time to discover each other, exchange the first proposal round, and produce the genesis-committing block.

### F.4: First verification round

```
bash /tmp/verify-host-fix.sh
```

(See Section H for how the script gets to the host. The simplest path is `scp scripts/verify-host-fix.sh root@[redacted-ip]:/tmp/verify-host-fix.sh` from the MacBook before Section F.)

Expected output: all 4 validators agree at the head block, no `State root mismatch` lines, height has advanced past 0.

Branch on failure: if any DIVERGE is reported in this first round, stop. Take the verify output to claude.ai for review. Do not start the oracle or txgen until agreement holds.

---

## Section G: Oracle re-registration

Goal: rotate the oracle key (the prior key signed transactions against the pre-fix chain identity; the operator's notes also say the existing oracle is currently STOPPED post-incident) and re-register the oracle entity on the fresh chain.

`agents/price-oracle/bootstrap.py` is a **one-shot idempotent script** (`agents/price-oracle/bootstrap.py:2-29`). A single invocation:

1. Generates a fresh ed25519 keypair and writes `/etc/novai/oracle-keys.json` at `0600` if the file does not exist.
2. Requests faucet funds if balance is below `MIN_BALANCE_FOR_REGISTER = 50_000`.
3. Submits a `RegisterEntity` transaction with capabilities `0x47` (bits 0, 1, 2, 6 - including `post_oracle_anchors`).
4. Polls the chain until the entity appears with bit 6 set, then rewrites the keyfile with `entity_id_hex`, `capabilities_byte`, `registered_at_unix`.

There is no separate `--out` flag, no separate `register` subcommand, and no Rust CLI step. The same one command in G.2 produces the new key file AND submits the registration; G.3 is the post-condition verification that bootstrap exited 0 and the entity is on-chain. Re-running the same command is a no-op once the oracle is registered.

### G.1: Quarantine the existing oracle key file

```
mv /etc/novai/oracle-keys.json /etc/novai/oracle-keys.dead-pre-fix-<DEPLOY_DATE>.json
ls -lah /etc/novai/
```

Expected output: the new dead-pre-fix filename present, the old `/etc/novai/oracle-keys.json` absent. With the keyfile absent, the next bootstrap.py run will hit the `path.exists() is False` branch at `bootstrap.py:104-118` and generate a fresh keypair.

### G.2: Generate the new key and register the entity (single bootstrap.py invocation)

The canonical invocation from `agents/price-oracle/README.md:62-63`, run on the [redacted-host] host:

```
set -a && source /etc/novai/oracle.env && set +a
/opt/novai-price-oracle/.venv/bin/python /opt/novai-price-oracle/bootstrap.py
```

What the two lines do:

- `set -a && source /etc/novai/oracle.env && set +a` exports the env file at `/etc/novai/oracle.env`. The relevant variables (per `bootstrap.py:68-74` and `README.md:101-111`) are `PRICE_ORACLE_RPC_ENDPOINT` (default `http://localhost:3030`), `PRICE_ORACLE_KEY_PATH` (default `/etc/novai/oracle-keys.json`), and `PRICE_ORACLE_LOG_LEVEL` (default `INFO`). The defaults are correct for this host, but the env file is the deployed source of truth; do not skip sourcing it.
- `/opt/novai-price-oracle/.venv/bin/python /opt/novai-price-oracle/bootstrap.py` runs the script inside the per-agent venv installed at `/opt/novai-price-oracle/.venv` (which has the `novai-python-sdk` editable-installed per `README.md:48-49`). Do not invoke the script with the system `python3`; the SDK is not on the system path.

Expected output on success: structured log lines `keypair_create event=generated`, `faucet event=requested ...` or `faucet event=funded`, `register event=submitted`, `register event=verified entity_id=... caps=0x47`, `keyfile event=updated`, followed by the summary block:

```
price-oracle bootstrap complete
  address:          <hex>
  pubkey:           <hex>
  balance:          <int>
  entity_id:        <hex>
  capabilities:     0x47
  post_oracle_anchors (bit 6): True
  registered_at:    <unix>
```

Exit code: `0`.

After the script exits 0, fix the file ownership and record both the new on-chain creator address AND the derived entity_id from the keyfile:

```
chmod 600 /etc/novai/oracle-keys.json
chown novai:novai /etc/novai/oracle-keys.json
python3 -c "import json; d=json.load(open('/etc/novai/oracle-keys.json')); print('addr:', d['address_hex']); print('entity_id:', d['entity_id_hex'])"
```

Record both values in the deploy log:

- `<ORACLE_ADDR_HEX>` ← the `addr:` line (keyfile field `address_hex`, per `bootstrap.py:86-92`).
- `<ORACLE_ENTITY_ID>` ← the `entity_id:` line (keyfile field `entity_id_hex`, written by `update_keyfile` at `bootstrap.py:252-268` after registration verify).

Both are needed downstream: `<ORACLE_ADDR_HEX>` is the recipient-identity field used by `novai_getNonce` and for the `verify-host-fix.sh` cross-check, while `<ORACLE_ENTITY_ID>` is the parameter the entity-keyed RPCs (`novai_getAiEntity`, `novai_getOracleAnchorsByEntity`) actually take.

Branch on failure by exit code (per `bootstrap.py:23-28`):

- Exit `2`: `PRICE_ORACLE_RPC_ENDPOINT` missing or env not sourced. Re-source `/etc/novai/oracle.env` and retry.
- Exit `3`: faucet cooldown blocked the new key AND balance is below `MIN_BALANCE_TO_OPERATE = 5_000`. The wipe in Section D wiped the chain's faucet state, so the per-IP-per-24h cooldown should be reset; an exit 3 here means either the chain's faucet endpoint is misconfigured (no `--faucet-key` on the validator; verify with E.4 against the ExecStart) or the local faucet was somehow drawn within the new chain's first minutes. Stop and inspect.
- Exit `4`: the `(code_hash, creator_addr)` pair already maps to an entity without bit 6. Cannot happen on a wiped chain unless this exact key was already registered without `post_oracle_anchors` since the rolling start; stop and inspect.
- Exit `5`: RPC unreachable or registration verify polled past `REGISTER_POLL_TIMEOUT_SECS = 30`. Check that `novai-node@0` is still `active` and that `curl http://localhost:3030` returns a 2xx; then re-run bootstrap.py (it is idempotent; the existing keyfile will be reused).

### G.3: Verify on-chain registration

Bootstrap.py only exits 0 after it has polled the chain and confirmed the entity exists with bit 6 set (see `bootstrap.py:231-249`). G.3 is the operator-side cross-check using a raw RPC, recorded in the deploy log.

`novai_getAiEntity` takes a 32-byte hex `entity_id` (per `GetAiEntityParams` at `crates/node/src/rpc.rs:974-976`), NOT an address. Pull `entity_id_hex` from the keyfile written in G.2, then query as an object-form `params`:

```
ENTITY_ID=$(python3 -c "import json; print(json.load(open('/etc/novai/oracle-keys.json'))['entity_id_hex'])")
curl -s -H "Content-Type: application/json" \
  -d "{\"jsonrpc\":\"2.0\",\"method\":\"novai_getAiEntity\",\"params\":{\"entity_id\":\"$ENTITY_ID\"},\"id\":1}" \
  http://localhost:3030 | jq .
```

The `$ENTITY_ID` value should match the `<ORACLE_ENTITY_ID>` recorded in G.2.

Expected output: a JSON response whose `result` field contains the oracle entity record with `is_active: true` and a `capabilities` byte of `71` (= `0x47`, bits 0, 1, 2, 6 - including `post_oracle_anchors` per Week 35 in `CLAUDE.md`).

Branch on failure:

- `result: null`: the entity_id is not on-chain. Either bootstrap submitted but consensus has not yet committed (which would also have triggered bootstrap exit 5), or the keyfile's `entity_id_hex` was computed against a different `code_hash` than the validator is using. Inspect the @0 journal:

  ```
  journalctl -u novai-node@0 -n 100 --no-pager | grep -iE "register|reject"
  ```

- `error.code: -32602` with `Invalid params`: a serde mismatch - most likely a stray leading `0x` on the entity_id or wrong field name in the request. Re-derive `$ENTITY_ID` from the keyfile and retry.

Do not start the oracle service until the entity is confirmed on-chain.

### G.4: Start the oracle service

```
systemctl start novai-price-oracle
sleep 5
systemctl is-active novai-price-oracle
journalctl -u novai-price-oracle -n 30 --no-pager
```

Expected output:

- `is-active`: `active`.
- `journalctl`: shows the oracle initializing, connecting to RPC, and posting the first OracleAnchor (or scheduling one).

Branch on failure: if the oracle fails to start, the most common cause is the new key file ownership; verify `chown novai:novai /etc/novai/oracle-keys.json`.

### G.5: Verify the first anchor lands on-chain

`novai_getOracleAnchorsByEntity` takes the same `entity_id` field as `novai_getAiEntity`, plus required `start_height` and `end_height` u64 fields that bound the index scan (per `GetOracleAnchorsByEntityParams` at `crates/node/src/rpc.rs:1051-1059` and confirmed by the deserializer test at `rpc.rs:4441-4451`). Use `$ENTITY_ID` from G.3 and a height window from `0` to a value at or above the current head (the post-wipe chain head will be well under one million for a long time):

After 90 seconds (one anchor-post cycle plus margin):

```
HEAD=$(curl -s -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"novai_getLatestBlock","params":[],"id":1}' \
  http://localhost:3030 | jq -r '.result.height')

curl -s -H "Content-Type: application/json" \
  -d "{\"jsonrpc\":\"2.0\",\"method\":\"novai_getOracleAnchorsByEntity\",\"params\":{\"entity_id\":\"$ENTITY_ID\",\"start_height\":0,\"end_height\":$HEAD},\"id\":1}" \
  http://localhost:3030 | jq '.result | length'
```

Expected output: an integer of at least 1.

Branch on failure:

- Result `0` after 2 anchor cycles (180 seconds): inspect both the oracle journal and the @0 validator journal for rejection reasons.

  ```
  journalctl -u novai-price-oracle -n 100 --no-pager | grep -iE "submission|fee|nonce|reject"
  journalctl -u novai-node@0 -n 200 --no-pager | grep -iE "OracleAnchor|reject"
  ```

- `error.code: -32602` `Invalid params`: a missing or non-numeric `start_height` / `end_height`. The fields are required; do not omit them.

---

## Section H: Post-deploy verification timeline

Goal: confirm the 4 validators agree on `block_hash`, `state_root`, and `parent_hash` at increasing time horizons. Each checkpoint runs `scripts/verify-host-fix.sh` and inspects the AGREE / DIVERGE lines.

### H.0: Stage the verify script on the host

From the MacBook, before Section F (or at any point before the checkpoints below):

```
scp scripts/verify-host-fix.sh root@[redacted-ip]:/tmp/verify-host-fix.sh
ssh root@[redacted-ip] 'chmod +x /tmp/verify-host-fix.sh'
```

### H.1: Checkpoint schedule

Run on the [redacted-host] host at each interval below. Use a timer; do not eyeball.

| Checkpoint | When | Action |
|------------|------|--------|
| T+1m | 1 minute after F.4 completes | Run verify script. Expect all AGREE. |
| T+5m | 5 minutes | Run verify script. Confirm height advanced versus T+1m. |
| T+10m | 10 minutes | Run verify script. |
| T+20m | 20 minutes | Run verify script. Transition to hourly. |
| T+1h | 1 hour | Run verify script. |
| T+2h | 2 hours | Run verify script. |
| T+4h | 4 hours | Run verify script. |
| T+8h | 8 hours | Run verify script. |
| T+12h | 12 hours | Run verify script. |
| T+24h | 24 hours | Run verify script. Phase ends. |

For each checkpoint, paste the full verify output into the deploy log and review for:

1. All 4 validators report `active`.
2. Heights are monotonically increasing across checkpoints (no validator stuck).
3. State root agreement section shows `AGREE` for every height tested.
4. The `State root mismatch` grep returns no lines.

### H.2: If a checkpoint fails

Failure modes and response:

- One validator drops to `inactive` or `failed`: stop all 4 validators, capture journals (`journalctl -u novai-node@N -n 500 --no-pager > /tmp/node$N.log` for each), bring logs to claude.ai. Do not restart blindly.
- Any DIVERGE reported: do not redeploy. Capture the verify output and the journals from all 4 validators for the 10 blocks bracketing the reported diverging height. Bring to claude.ai.
- Height stops advancing on all 4 (chain halt): capture all 4 journals from the 5 minutes preceding the halt. Bring to claude.ai.

Do not attempt to "fix" a divergence by re-wiping. The investigation needs the diverged state intact.

---

## Section I: Rollback procedure

There is NO ROLLBACK POSSIBLE AFTER SECTION D.2 (wipe phase) COMPLETES. Forward-only.

The path documented here is panic-recovery only. Use only if the chain post-deploy is unrecoverable forward (e.g., validators cannot agree at all after rolling start, repeated DIVERGE at every checkpoint, complete consensus halt with no recovery).

### I.1: Stop all validators

```
for i in 0 1 2 3; do systemctl stop novai-node@$i; sleep 4; done
```

### I.2: Restore data directories from pre-deploy backup

```
for i in 0 1 2 3; do
  rm -rf /var/lib/novai/node$i/*
  rm -rf /var/lib/novai/node$i/.??*
  tar xzf [redacted-server]/pre-deploy-backup-<DEPLOY_DATE>/node$i.tar.gz -C /var/lib/novai/node$i/
  chown -R novai:novai /var/lib/novai/node$i
  echo "restored node$i"
done
```

### I.3: Restore the pre-fix binary

```
cp /usr/local/bin/novai-node.bak-pre-fix-<DEPLOY_DATE> /usr/local/bin/novai-node
chmod 755 /usr/local/bin/novai-node
chown root:root /usr/local/bin/novai-node
md5sum /usr/local/bin/novai-node
```

Expected output: hash equals `<PRE_DEPLOY_BIN_MD5>` from A.1.

### I.4: Restart the validators (rolling)

```
for i in 0 1 2 3; do
  systemctl start novai-node@$i
  sleep 8
done
sleep 20
```

### I.5: Verify recovery

```
bash /tmp/verify-host-fix.sh
```

Expected: the chain resumes at the pre-deploy state. The same Bug 1 conditions (state_root divergence) will still be present because this is the pre-fix binary. This is recovery to a known-bad state, not to a healthy state. The next step is to bring the verify output to claude.ai and design a different fix or different deploy approach.

---

## Section J: Pseudonymity check

This document contains no real-name references, no internal identifiers, no information that links the project to a real person.

Identifiers used in this document:

- `0x-devc`: the project's public GitHub identity, attached to `repository = "https://github.com/0x-devc/NOVAI-node"` in `Cargo.toml:32`. Public.
- `novai.network`: the project's public domain (`homepage = "https://novai.network"` in `Cargo.toml:33`). Public.
- `[redacted-ip]`: the [redacted-host] VPS IP. Public infrastructure identifier; not linked to a person.
- File paths under `[redacted-server]/`, `/var/lib/novai/`, `/etc/novai/`, `/opt/novai-price-oracle/`, `/usr/local/bin/novai-node`: standard Linux service paths. Not person-identifying.

Other domains co-located on the same [redacted-host] box (`[redacted-domain]`, `[redacted-domain]`, `[redacted-domain]`) are referenced for safety (do not touch nginx). All are public domains.

No real name appears in this document. No internal handle, internal Slack channel, internal ticket ID, or non-public identifier appears.

---

## Appendix: command quick reference

For pasteback into a deploy log without context. All commands run on the [redacted-host] host unless noted.

```
# Pre-deploy capture
md5sum /usr/local/bin/novai-node
git -C [redacted-server]/NOVAI-node/NOVAI-node rev-parse HEAD
df -BG / /var/lib/novai

# Stop, backup, wipe
for i in 0 1 2 3; do systemctl stop novai-node@$i; sleep 8; done
mkdir -p [redacted-server]/pre-deploy-backup-<DEPLOY_DATE>
for i in 0 1 2 3; do tar czf [redacted-server]/pre-deploy-backup-<DEPLOY_DATE>/node$i.tar.gz -C /var/lib/novai/node$i .; done
for i in 0 1 2 3; do echo "=== node$i ==="; tar tzf [redacted-server]/pre-deploy-backup-<DEPLOY_DATE>/node$i.tar.gz | head -3 | wc -l; done
cp /usr/local/bin/novai-node /usr/local/bin/novai-node.bak-pre-fix-<DEPLOY_DATE>

# Build
cd [redacted-server]/NOVAI-node/NOVAI-node && git fetch origin && git checkout <FIX_COMMIT>
cargo build --release --bin novai-node
md5sum target/release/novai-node

# Wipe and install
for i in 0 1 2 3; do rm -rf /var/lib/novai/node$i/* /var/lib/novai/node$i/.??*; done
cp target/release/novai-node /usr/local/bin/novai-node
chmod 755 /usr/local/bin/novai-node
chown root:root /usr/local/bin/novai-node

# Dev-keys mode verification
for i in 0 1 2 3; do systemctl cat novai-node@$i | grep ExecStart | grep -oE '\-\-validator [0-9]+'; done

# Rolling start
for i in 0 1 2 3; do systemctl start novai-node@$i; sleep 8; done
sleep 20
bash /tmp/verify-host-fix.sh

# Oracle re-register (after G.1 quarantine)
set -a && source /etc/novai/oracle.env && set +a
/opt/novai-price-oracle/.venv/bin/python /opt/novai-price-oracle/bootstrap.py
chmod 600 /etc/novai/oracle-keys.json
chown novai:novai /etc/novai/oracle-keys.json
python3 -c "import json; print(json.load(open('/etc/novai/oracle-keys.json'))['address_hex'])"
```

Stopping point. The next deploy artifact is `scripts/verify-host-fix.sh`, described in Section H.0 and used at every checkpoint in Section H.1.
