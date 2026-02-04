# NOVAI Genesis Verification Log

**Template Version**: 1.0.0

This document is a template for recording independent genesis verification results.
Each verifying party must complete a separate copy of this log.

---

## Verification Party Information

| Field | Value |
|-------|-------|
| Party Name | ______________________ |
| Organization | ______________________ |
| Verifier Name | ______________________ |
| Verification Date | ______________________ (YYYY-MM-DD) |
| Verification Time (UTC) | ______________________ (HH:MM) |

---

## 1. Software Verification

### 1.1 Repository Clone

```
Git Repository: https://github.com/novai-protocol/novai-node
Git Tag/Commit: ______________________
Tag Signature Verified: [ ] Yes  [ ] No
```

**Commands executed:**
```bash
git clone https://github.com/novai-protocol/novai-node.git
cd novai-node
git checkout ______________  # Enter tag
git verify-tag ______________  # Enter tag
```

**Output of `git verify-tag`:**
```
(paste output here)
```

### 1.2 Build Verification

**Rust version:**
```bash
rustc --version
# Output: ______________________
```

**Build command:**
```bash
cargo build --release -p genesis-generator
```

**Build successful:** [ ] Yes  [ ] No

If No, describe error: ______________________

### 1.3 Golden Vector Test

**Command:**
```bash
cat > /tmp/golden_test.json << 'EOF'
{
    "chain_id": "novai-testnet-golden",
    "protocol_version": 1,
    "timestamp": "2025-01-17T00:00:00Z",
    "validators": [
        {
            "pubkey": "0000000000000000000000000000000000000000000000000000000000000000",
            "initial_stake": "1000000"
        }
    ],
    "accounts": {
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": "1000000000"
    }
}
EOF

./target/release/genesis-generator \
    --config /tmp/golden_test.json \
    --verify f7501cf414619c9a69c4665ddb50f5d5ef1948c3a73d508d68209e7a18515dd7
```

**Result:** [ ] VERIFICATION PASSED  [ ] VERIFICATION FAILED

If failed, describe: ______________________

---

## 2. Genesis Configuration Verification

### 2.1 Configuration Receipt

**Configuration received via:** [ ] Secure email  [ ] Secure file transfer  [ ] In-person  [ ] Other: ______

**Configuration file name:** `______________________`

**Configuration hash (blake3):**
```bash
blake3sum genesis_config.json
# Output: ______________________
```

**Hash matches coordinator's announced hash:** [ ] Yes  [ ] No

Coordinator's announced hash: `______________________`

### 2.2 Configuration Review

| Field | Value in Config | Valid? |
|-------|-----------------|--------|
| chain_id | ______________________ | [ ] Yes [ ] No |
| protocol_version | ______________________ | [ ] Yes [ ] No |
| timestamp | ______________________ | [ ] Yes [ ] No |
| validator count | ______________________ | [ ] Yes [ ] No |
| account count | ______________________ | [ ] Yes [ ] No |

**All validator pubkeys are 64 hex characters:** [ ] Yes  [ ] No

**All account addresses are 64 hex characters:** [ ] Yes  [ ] No

**Configuration validation passed:**
```bash
./target/release/genesis-generator --config genesis_config.json
# Did it parse without errors? [ ] Yes [ ] No
```

---

## 3. Genesis State Generation

### 3.1 Generation Command

```bash
./target/release/genesis-generator \
    --config genesis_config.json \
    --output-dir ./genesis-output \
    --verbose
```

### 3.2 Generation Output

**Command completed successfully:** [ ] Yes  [ ] No

**Output directory contents:**
```bash
ls -la ./genesis-output/
# (paste output here)
```

### 3.3 State Root Result

**State root (from state_root.hex):**

```
____________________________________________________________
(64 hexadecimal characters)
```

**State root verified by reading genesis_summary.txt:** [ ] Yes  [ ] No

---

## 4. Cross-Verification

### 4.1 State Root Comparison

| Party | State Root | Match? |
|-------|------------|--------|
| This party | `______________________` | — |
| Party 2 | `______________________` | [ ] Yes [ ] No |
| Party 3 | `______________________` | [ ] Yes [ ] No |
| (add more as needed) | | |

### 4.2 Verification Command

```bash
# Verify against another party's announced root
./target/release/genesis-generator \
    --config genesis_config.json \
    --verify <other-party-state-root>
```

**Result:** [ ] VERIFICATION PASSED  [ ] VERIFICATION FAILED

### 4.3 Final Verification Status

**ALL parties have matching state roots:** [ ] Yes  [ ] No

If No, describe discrepancy and resolution:
```
(describe here)
```

---

## 5. Attestation

I, the undersigned, attest that:

1. I independently performed all verification steps documented above.
2. I used the official NOVAI repository at the specified tag/commit.
3. I verified the golden test vector before proceeding.
4. I computed the genesis state root from the provided configuration.
5. My computed state root matches the state roots of other verifying parties.
6. I have not been coerced or influenced in my verification.

**Computed State Root:**
```
____________________________________________________________
```

**Genesis Configuration Hash (blake3):**
```
____________________________________________________________
```

**Signature:**

```
Party Name: ______________________
Verifier Name: ______________________
Date (UTC): ______________________
Signature Method: [ ] GPG  [ ] Ed25519  [ ] Other: ______

Signature (hex or base64):
____________________________________________________________
____________________________________________________________
```

---

## 6. Technical Environment

For reproducibility, document the verification environment:

| Component | Value |
|-----------|-------|
| Operating System | ______________________ |
| OS Version | ______________________ |
| CPU Architecture | [ ] x86_64  [ ] ARM64  [ ] Other: ______ |
| Rust Version | ______________________ |
| Cargo Version | ______________________ |
| blake3 Tool Version | ______________________ |

---

## 7. Checklist Summary

### Pre-Verification
- [ ] Repository cloned from official source
- [ ] Git tag verified with GPG signature
- [ ] genesis-generator tool built successfully
- [ ] Golden vector test passed

### Configuration
- [ ] Configuration received via secure channel
- [ ] Configuration hash matches coordinator's announcement
- [ ] Configuration fields reviewed and validated
- [ ] Configuration parses without errors

### Generation
- [ ] Genesis state generated successfully
- [ ] Output files created in output directory
- [ ] State root recorded

### Cross-Verification
- [ ] State root compared with at least 1 other party
- [ ] All state roots match
- [ ] Verification command passed

### Attestation
- [ ] Attestation statement reviewed
- [ ] Attestation signed
- [ ] Verification log submitted to coordinator

---

## Appendix: Troubleshooting

### State Root Mismatch

If your state root does not match other parties:

1. **Verify configuration hash** — ensure all parties use identical config
2. **Check tool version** — ensure all parties use same git tag
3. **Check Rust version** — different compilers may produce different binaries
4. **Re-run generation** — try generating again from scratch
5. **Contact coordinator** — if issue persists, escalate immediately

### Build Failures

Common causes:
- Missing Rust toolchain: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- Wrong Rust version: `rustup update stable`
- Missing dependencies: Check build output for missing packages

### Golden Vector Mismatch

If the golden vector test fails:
- **STOP** — do not proceed with mainnet verification
- Verify you are on the correct git tag
- Verify your build completed without errors
- Report issue to coordinator before continuing

---

**End of Genesis Verification Log Template**
