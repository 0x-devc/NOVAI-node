# Playbook: Validator Key Compromise

**Scenario**: A validator's signing key has been leaked, stolen, or is suspected to be compromised. An attacker with a validator key can sign proposals, votes, and timeouts — potentially disrupting consensus.

**Severity**: P0 (Critical)
**Expected Recovery**: 3 actions
**Prerequisites**: Operator has Docker access and can coordinate with other validator operators

---

## Threat Model

With a compromised validator key, an attacker can:

- **Sign conflicting votes**: Vote for multiple proposals at the same height (equivocation)
- **Sign invalid proposals**: Propose blocks with malicious transactions (but other validators will reject invalid state transitions)
- **Sign timeouts**: Trigger unnecessary round advances (but needs 2f+1=3 timeouts)
- **Impersonate the validator**: Appear as the legitimate validator to peers

With a single compromised key (f=1 in a 4-validator set), an attacker **CANNOT**:

- **Break safety**: BFT requires 2f+1=3 votes for a QC; one malicious vote is insufficient
- **Forge QCs**: Needs 3 of 4 signatures
- **Commit invalid blocks**: Other validators verify state transitions independently
- **Halt consensus alone**: Needs 2 of 4 validators to refuse participation

---

## Detection

### Symptoms

- Equivocation detected: same validator signing conflicting votes at same height/round
- Unexpected proposals from a validator that should not be leader
- Log messages about invalid signatures or unknown peers
- Validator appearing online from an unexpected IP address

### Confirm Compromise

```bash
# Check if the compromised validator is producing unexpected messages
docker logs novai-validator-0 | grep "equivocation\|invalid signature\|unknown peer" | tail -20

# Check peer connections — look for unexpected IPs
docker logs novai-validator-0 | grep "Connected" | tail -20

# Compare committed_height across validators
# A compromised validator may show divergent height
for i in {0..4}; do
  echo "V$i: $(curl -s http://localhost:808$i/metrics | grep committed_height | awk '{print $2}')"
done
```

---

## Response

### Action 1: Isolate the Compromised Validator

Immediately stop the compromised validator to prevent further damage:

```bash
# Stop the compromised validator (example: validator 2)
docker stop novai-validator-2

# Verify it's stopped
docker ps | grep novai-validator-2
# Should show no output
```

**Remaining validators**: With 3 of 4 honest validators, consensus continues (quorum = 3).

### Action 2: Block the Attacker's Network Access

If the attacker is running a node with the stolen key, other validators may connect to it. Firewall the compromised validator's expected IP:

```bash
# On each remaining validator's host, block the compromised validator's IP
# Replace COMPROMISED_IP with the attacker's IP if known

# Linux
sudo iptables -A INPUT -s COMPROMISED_IP -j DROP
sudo iptables -A OUTPUT -d COMPROMISED_IP -j DROP

# Verify
sudo iptables -L -n | grep COMPROMISED_IP
```

If the attacker's IP is unknown, monitor peer connections on each validator:

```bash
# Watch for new unexpected connections
docker logs -f novai-validator-0 | grep "Connected"
```

### Action 3: Rotate the Compromised Key

**Current limitation**: The testnet uses hardcoded deterministic keys (`[i; 32]`). Key rotation requires redeploying with a new key.

**Testnet key rotation procedure**:

```bash
# Step 1: Generate new key material (for production, use secure entropy)
# The compromised validator needs a new signing key
# This requires a code change or configuration update

# Step 2: Rebuild the node binary with updated key
# In crates/node/src/main.rs, the key for validator i is:
#   SigningKey::from_bytes(&[i as u8; 32])
# For rotation, this must be replaced with a securely generated key

# Step 3: Update all other validators' validator_set to include new pubkey
# All validators must agree on the validator set

# Step 4: Redeploy all validators with updated configuration
for i in {0..4}; do
  docker stop novai-validator-$i
  docker rm novai-validator-$i
done

# Rebuild with new keys
docker build -t novai-node:latest .

# Redeploy
./scripts/deploy-testnet.sh
```

**Important**: In the current testnet architecture, changing one validator's key requires updating the hardcoded validator set in `main.rs` and redeploying ALL validators, because they all need the new public key for signature verification.

---

## Recovery Verification

| Check | Command | Expected |
|-------|---------|----------|
| Compromised node stopped | `docker ps \| grep novai-validator-2` | No output |
| Remaining consensus healthy | `curl -s localhost:8080/metrics \| grep committed_height` | Increasing |
| Consensus round normal | `curl -s localhost:8080/metrics \| grep current_round` | 0 or 1 |
| 3 validators connected | `curl -s localhost:8080/metrics \| grep peer_count` | 2 (with 1 stopped) |
| No fork detected | `docker logs novai-validator-0 \| grep FORK` | No output |

---

## Post-Incident

1. File P0 postmortem using `docs/POSTMORTEM_TEMPLATE.md`
2. Determine how the key was compromised (access logs, credential audit)
3. Rotate ALL validator keys (assume other keys may also be at risk)
4. Review key storage practices:
   - Keys should never be in source control
   - Use environment variables or secret management (Vault, AWS KMS)
   - Consider HSM for production signing
5. Implement key rotation mechanism for future incidents (governance proposal)
6. Add equivocation detection to consensus code

---

## Future Improvements

The current testnet has limitations that a production system must address:

1. **Dynamic validator set**: Allow adding/removing validators via governance
2. **Key rotation without downtime**: Support key update proposals
3. **Equivocation slashing**: Detect and penalize double-voting
4. **Key derivation from HSM**: Never expose raw signing keys
5. **Separate signing key from identity**: Allow key rotation without changing validator address

---

## Architecture Reference

- Validator key generation: `crates/node/src/main.rs:189-199`
- Ed25519 signing: `crates/crypto/src/lib.rs:33-43`
- Address derivation: `crates/crypto/src/lib.rs:19-31` (blake3 of pubkey)
- Signature verification: `crates/crypto/src/lib.rs:38-43`
- Quorum calculation: `2f+1 where f = (n-1)/3`
- Leader selection: `crates/consensus/src/lib.rs:487-509`
- Fork detection: `crates/consensus/src/lib.rs:926-950`

---

## Drill Results

### Drill 1: Validator Kill & Recovery (2026-02-01)

**Environment:** Private testnet, 4 validators (native binaries), height ~51,300

**Execution:**
- 22:44:12 UTC — Killed validator 3 (PID 189064) via `kill`
- Consensus continued immediately with 3/4 validators (quorum maintained)
- ~85 blocks produced in 30s with zero degradation
- 22:45:18 UTC — Restarted validator 3 on port 9003

**Recovery Result: PARTIAL FAILURE**
- Validator 3 rejoined the network (3 peers connected)
- Validator 3 participated in voting at current height
- Block sync FAILED: requested entire chain (blocks 1-51434), sync timed out repeatedly
- Validator 3 stuck at committed_height=0 despite voting at height 51434
- Error: `StateError("Missing block at height 51415 (chain broken)")`

**Findings:**
1. ✅ BFT quorum (3/4) maintains liveness — no block production gap
2. ❌ Block sync protocol cannot handle catch-up at 50k+ heights — requests entire chain from genesis
3. ❌ No chunked sync or state snapshot mechanism exists for recovery
4. ❌ Only recovery path would be full state wipe + bootstrap (no snapshot tooling exists)

**Action Items:**
- [x] Implement chunked block sync (request ranges, not full chain) — fixed in 7b63783 + be1ca1a
- [ ] Add state snapshot export/import for fast validator recovery
- [ ] Add `novai-node sync-from <peer> --start-height <N>` CLI subcommand

### Drill 2: Post-Fix Re-Test (2026-02-01)

**Environment:** Private testnet, 4 validators (native binaries), chunked sync fix deployed (7b63783 + be1ca1a)

**Execution:**
- 23:20:14 UTC — Killed validator 3 at height 2058
- 3/4 validators maintained consensus (height reached 2133 in ~60s)
- 23:21:14 UTC — Restarted validator 3

**Recovery Result: PASS**
- Chunked sync requested blocks in 500-block batches:
  - 1-500 (committed_height=500)
  - 501-1000 (committed_height=1000)
  - 1001-1500 (committed_height=1500)
  - 1501-2000 (committed_height=2000)
  - 2001-2137 (caught up)
- Validator 3 fully recovered and committing normally at height 2701 within 30 seconds

**Verdict:** PASS — Full automated recovery via chunked block sync
