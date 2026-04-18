# NOVAI Genesis Ceremony Procedure

**Version**: 1.0.0
**Status**: READY
**Last Updated**: 2026-02-03

This document defines the secure procedure for generating the NOVAI mainnet genesis state. Multiple independent parties must participate to ensure no single entity controls the genesis configuration.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Participants and Roles](#2-participants-and-roles)
3. [Pre-Ceremony Preparation](#3-pre-ceremony-preparation)
4. [Key Generation Procedure](#4-key-generation-procedure)
5. [Secure Key Storage Requirements](#5-secure-key-storage-requirements)
6. [Genesis Configuration Assembly](#6-genesis-configuration-assembly)
7. [Genesis Verification Procedure](#7-genesis-verification-procedure)
8. [Ceremony Schedule Template](#8-ceremony-schedule-template)
9. [Emergency Procedures](#9-emergency-procedures)
10. [Checklist](#10-checklist)

---

## 1. Overview

### 1.1 Purpose

The genesis ceremony produces:

1. **Genesis configuration** (`mainnet_config.json`) — validator set, initial balances, protocol parameters
2. **Genesis state root** — deterministic 32-byte blake3 hash of initial state
3. **Genesis block** — height=0 block with state root commitment
4. **Validator key attestations** — proof that each validator controls their key

### 1.2 Security Properties

- **Determinism**: Same config → same state root (verified by multiple parties)
- **No single point of trust**: At least 2 independent parties verify genesis
- **Key sovereignty**: Each validator generates and controls their own keys
- **Auditability**: All ceremony steps are logged and timestamped

### 1.3 Cryptographic Primitives

| Primitive | Algorithm | Library |
|-----------|-----------|---------|
| Signatures | Ed25519 (RFC 8032) | `ed25519-dalek 2.1` |
| Hashing | blake3 | `blake3` crate |
| Address derivation | `blake3(pubkey)` | No domain tag |

---

## 2. Participants and Roles

### 2.1 Required Participants

| Role | Count | Responsibility |
|------|-------|----------------|
| **Ceremony Coordinator** | 1 | Facilitates ceremony, maintains timeline |
| **Validators** | 4+ | Generate keys, provide pubkeys, sign attestations |
| **Independent Verifiers** | 2+ | Verify genesis state root independently |
| **Witness** | 1+ | Observe and attest to procedure compliance |

### 2.2 Validator Requirements

Each validator participant must:

- [ ] Have secure computing environment (air-gapped preferred)
- [ ] Have hardware security module (HSM) or secure enclave available
- [ ] Be available for entire ceremony duration
- [ ] Have verified communication channel with coordinator

### 2.3 Verifier Requirements

Each independent verifier must:

- [ ] Have `genesis-generator` tool installed and verified
- [ ] Have independent computing environment (not shared with validators)
- [ ] Be able to compute state root from config
- [ ] Report results through secure channel

---

## 3. Pre-Ceremony Preparation

### 3.1 Software Verification (All Participants)

**Step 1: Clone and verify repository**

```bash
git clone https://github.com/novai-protocol/novai-node.git
cd novai-node
git checkout mainnet-v1.0.0  # Use tagged release
git verify-tag mainnet-v1.0.0  # Verify GPG signature
```

**Step 2: Build genesis-generator tool**

```bash
cargo build --release -p genesis-generator
./target/release/genesis-generator --help
```

**Step 3: Verify tool produces expected test vector**

```bash
# Create test config
cat > /tmp/test_genesis.json << 'EOF'
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

# Verify golden state root
./target/release/genesis-generator \
    --config /tmp/test_genesis.json \
    --verify f7501cf414619c9a69c4665ddb50f5d5ef1948c3a73d508d68209e7a18515dd7
```

Expected output: `✅ VERIFICATION PASSED`

If verification fails, **STOP** and investigate before proceeding.

### 3.2 Communication Setup

- [ ] Establish secure video conference (end-to-end encrypted)
- [ ] Verify participant identities via pre-shared secrets
- [ ] Establish backup communication channel
- [ ] Designate ceremony log keeper

### 3.3 Environment Checklist

Each validator must confirm:

- [ ] Clean operating system (freshly installed or verified)
- [ ] No network connection during key generation (air-gapped)
- [ ] Entropy source verified (hardware RNG or sufficient system entropy)
- [ ] HSM firmware version documented
- [ ] Screen recording disabled (privacy)
- [ ] Secure deletion tools available

---

## 4. Key Generation Procedure

### 4.1 Overview

Each validator generates their own Ed25519 keypair. Private keys **never** leave the validator's secure environment.

### 4.2 Option A: HSM-Based Generation (Recommended)

**For YubiHSM 2:**

```bash
# Initialize HSM (if new)
yubihsm-shell -a put-opaque \
    --object-id 1 \
    --label "novai-validator" \
    --algorithm ed25519

# Generate key inside HSM
yubihsm-shell -a generate-asymmetric \
    --object-id 100 \
    --label "novai-mainnet-validator" \
    --algorithm ed25519 \
    --capabilities sign-eddsa

# Export public key
yubihsm-shell -a get-public-key --object-id 100 > validator_pubkey.bin
xxd -p validator_pubkey.bin | tr -d '\n' > validator_pubkey.hex
```

**For AWS CloudHSM:**

```bash
# Using PKCS#11 interface
pkcs11-tool --module /opt/cloudhsm/lib/libcloudhsm_pkcs11.so \
    --keypairgen \
    --key-type EC:ed25519 \
    --label "novai-mainnet-validator" \
    --id 01

# Export public key
pkcs11-tool --module /opt/cloudhsm/lib/libcloudhsm_pkcs11.so \
    --read-object \
    --type pubkey \
    --label "novai-mainnet-validator" \
    -o validator_pubkey.der
```

### 4.3 Option B: Software-Based Generation (Air-Gapped)

**WARNING**: Only use this method if HSM is unavailable. Requires strict air-gap discipline.

```bash
# On air-gapped machine with verified entropy
# Using openssl (ensure ed25519 support)
openssl genpkey -algorithm ed25519 -out validator_key.pem

# Extract public key
openssl pkey -in validator_key.pem -pubout -out validator_pubkey.pem

# Convert to raw hex (32 bytes)
openssl pkey -in validator_pubkey.pem -pubin -outform DER | tail -c 32 | xxd -p | tr -d '\n' > validator_pubkey.hex

# Verify length (must be exactly 64 hex characters)
wc -c validator_pubkey.hex  # Should output: 64
```

**Alternative using Rust:**

```rust
// keygen.rs - compile on air-gapped machine
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;

fn main() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    // Print public key as hex
    println!("PUBKEY: {}", hex::encode(verifying_key.as_bytes()));

    // Save private key securely (encrypted!)
    // ... implementation depends on your secure storage
}
```

### 4.4 Public Key Submission

After key generation, each validator submits their public key to the coordinator:

1. **Format**: 64 hexadecimal characters (32 bytes)
2. **Channel**: Secure, authenticated channel (GPG-signed email, secure form)
3. **Verification**: Validator signs a challenge message to prove key ownership

**Challenge-response verification:**

```bash
# Coordinator sends random challenge
CHALLENGE="novai-genesis-ceremony-2026-validator-<N>-<random-nonce>"

# Validator signs challenge
echo -n "$CHALLENGE" | openssl pkeyutl -sign -inkey validator_key.pem | xxd -p | tr -d '\n' > challenge_sig.hex

# Validator sends: pubkey.hex + challenge_sig.hex
```

### 4.5 Key Backup Requirements

- [ ] Create encrypted backup of private key (if software-based)
- [ ] Use strong passphrase (20+ characters, high entropy)
- [ ] Store backup in geographically separate location
- [ ] Test backup restoration procedure
- [ ] Document backup location securely (not digitally connected to key)

---

## 5. Secure Key Storage Requirements

### 5.1 Production Requirements

| Requirement | Minimum | Recommended |
|-------------|---------|-------------|
| HSM certification | FIPS 140-2 Level 2 | FIPS 140-2 Level 3 |
| Physical security | Locked cabinet | Secure data center |
| Access control | 2-person rule | Multi-party computation |
| Audit logging | Enabled | Tamper-evident |
| Backup | Encrypted offline | Shamir secret sharing |

### 5.2 Approved HSM Options

| HSM | Certification | Notes |
|-----|---------------|-------|
| YubiHSM 2 | FIPS 140-2 Level 3 | Cost-effective, Ed25519 native |
| AWS CloudHSM | FIPS 140-2 Level 3 | Cloud-based, high availability |
| Thales Luna | FIPS 140-2 Level 3 | Enterprise-grade |
| Nitrokey HSM 2 | Common Criteria EAL4+ | Open-source friendly |

### 5.3 Software Key Storage (Fallback Only)

If HSM is unavailable:

1. **Encryption**: AES-256-GCM with Argon2id key derivation
2. **Storage**: Air-gapped machine, full-disk encryption
3. **Access**: Strong passphrase + hardware token (YubiKey)
4. **Backup**: Split across 3 locations using Shamir secret sharing (2-of-3)

### 5.4 Key Rotation Policy

- **Routine rotation**: Not recommended for genesis validators
- **Compromise response**: See [Emergency Procedures](#9-emergency-procedures)
- **Succession**: Document key custody transfer procedure

---

## 6. Genesis Configuration Assembly

### 6.1 Configuration Template

```json
{
    "chain_id": "novai-mainnet-1",
    "protocol_version": 1,
    "timestamp": "2026-MM-DDTHH:MM:SSZ",
    "validators": [
        {
            "pubkey": "<64-hex-chars>",
            "initial_stake": "1000000000000",
            "name": "validator-name"
        }
    ],
    "accounts": {
        "<64-hex-address>": "<balance-as-string>"
    },
    "ai_entities": [],
    "approval_gates": []
}
```

### 6.2 Parameter Constraints

| Parameter | Constraint | Source |
|-----------|------------|--------|
| `chain_id` | Non-empty string | `crates/genesis/src/lib.rs:194-198` |
| `protocol_version` | >= 1 | `crates/genesis/src/lib.rs:200-205` |
| `timestamp` | RFC3339 format | `crates/genesis/src/lib.rs:207-209` |
| `validators` | 1-100 entries | `crates/genesis/src/lib.rs:211-220` |
| `pubkey` | 64 hex chars (32 bytes) | `crates/genesis/src/lib.rs:224-233` |
| `initial_stake` | Valid u64 string | `crates/genesis/src/lib.rs:235-241` |

### 6.3 Assembly Procedure

**Step 1: Coordinator collects validator pubkeys**

```bash
# Create validators section
VALIDATORS='['
for i in 1 2 3 4; do
    PUBKEY=$(cat validator_${i}_pubkey.hex)
    VALIDATORS="${VALIDATORS}{\"pubkey\":\"${PUBKEY}\",\"initial_stake\":\"1000000000000\",\"name\":\"validator-${i}\"},"
done
VALIDATORS="${VALIDATORS%,}]"  # Remove trailing comma
```

**Step 2: Assemble full config**

```bash
cat > mainnet_config.json << EOF
{
    "chain_id": "novai-mainnet-1",
    "protocol_version": 1,
    "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
    "validators": ${VALIDATORS},
    "accounts": {},
    "ai_entities": [],
    "approval_gates": []
}
EOF
```

**Step 3: Validate config**

```bash
./target/release/genesis-generator --config mainnet_config.json
```

**Step 4: Distribute config to all verifiers**

- Hash config file: `blake3sum mainnet_config.json`
- Distribute via multiple channels
- All parties verify hash matches before proceeding

---

## 7. Genesis Verification Procedure

### 7.1 Independent Verification Requirement

**At least 2 independent parties** must generate the genesis state and compare state roots.

### 7.2 Verification Steps (Each Party)

**Step 1: Receive and verify config**

```bash
# Verify config hash matches coordinator's announcement
blake3sum mainnet_config.json
# Must match: <announced-hash>
```

**Step 2: Generate genesis state**

```bash
./target/release/genesis-generator \
    --config mainnet_config.json \
    --output-dir ./genesis-output \
    --verbose
```

**Step 3: Record state root**

```bash
cat ./genesis-output/state_root.hex
# Example: a1b2c3d4e5f6...
```

**Step 4: Report state root to coordinator**

- Use secure channel
- Include: party name, timestamp, state root hex, tool version

### 7.3 Verification Consensus

| Scenario | Action |
|----------|--------|
| All parties match | Genesis verified, proceed to signing |
| 1 party differs | Re-run verification, check tool version |
| Multiple differ | STOP ceremony, investigate root cause |

### 7.4 Cross-Verification Command

```bash
# Party A verifies against Party B's announced root
./target/release/genesis-generator \
    --config mainnet_config.json \
    --verify <party-b-state-root>
```

Expected: `✅ VERIFICATION PASSED`

---

## 8. Ceremony Schedule Template

### 8.1 Timeline

| Phase | Duration | Activities |
|-------|----------|------------|
| **T-7 days** | — | Distribute software, verify builds |
| **T-3 days** | — | Validator key generation begins |
| **T-1 day** | — | All pubkeys submitted, config draft |
| **T+0 (Ceremony Day)** | | |
| Hour 0 | 30 min | Roll call, identity verification |
| Hour 0.5 | 30 min | Config review and approval |
| Hour 1 | 1 hour | Independent genesis generation |
| Hour 2 | 30 min | State root comparison |
| Hour 2.5 | 30 min | Attestation signing |
| Hour 3 | 30 min | Final documentation |
| **T+1 day** | — | Public announcement |

### 8.2 Ceremony Day Agenda

```
09:00 UTC — Roll call and identity verification
09:30 UTC — Review mainnet_config.json
10:00 UTC — Genesis generation (parallel, all parties)
11:00 UTC — State root comparison and verification
11:30 UTC — Sign genesis attestations
12:00 UTC — Document ceremony completion
12:30 UTC — Ceremony concluded
```

### 8.3 Attestation Format

Each validator signs an attestation:

```
NOVAI GENESIS ATTESTATION

I, [validator name], attest that:

1. I generated my validator keypair on [date] using [method/HSM].
2. My public key is: [64-hex-chars]
3. I independently verified the genesis state root: [64-hex-chars]
4. The genesis configuration hash (blake3) is: [64-hex-chars]
5. I will secure my private key according to the documented requirements.

Signed: [Ed25519 signature of above text]
Date: [ISO 8601 timestamp]
```

---

## 9. Emergency Procedures

### 9.1 Key Compromise (Pre-Launch)

If a validator key is compromised before mainnet launch:

1. **Notify coordinator immediately**
2. **Generate new keypair** following Section 4
3. **Update genesis config** with new pubkey
4. **Re-run verification** (all parties)
5. **Document incident** in ceremony log

### 9.2 Key Compromise (Post-Launch)

Post-launch key compromise requires governance action:

1. **Disable compromised validator** (if possible via slashing)
2. **Coordinate with other validators** for network security
3. **Follow incident response playbook** (`docs/playbooks/validator_key_compromise.md`)

### 9.3 Ceremony Abort Conditions

**ABORT ceremony if:**

- [ ] State roots do not match across verifiers
- [ ] Validator cannot prove key ownership
- [ ] Secure communication channel compromised
- [ ] Participant identity cannot be verified
- [ ] Software verification fails

**Abort procedure:**

1. Coordinator announces abort with reason
2. All parties destroy any generated artifacts
3. Schedule new ceremony after root cause analysis

---

## 10. Checklist

### 10.1 Pre-Ceremony (T-7 to T-1)

- [ ] All participants identified and confirmed
- [ ] Software repository cloned and verified
- [ ] `genesis-generator` tool built and tested
- [ ] Golden vector verification passed
- [ ] HSMs provisioned (if applicable)
- [ ] Secure communication channels established
- [ ] Ceremony schedule distributed

### 10.2 Key Generation (T-3 to T-1)

- [ ] Each validator generated keypair
- [ ] Each validator backed up private key securely
- [ ] Each validator submitted pubkey via secure channel
- [ ] Coordinator verified all pubkeys (challenge-response)
- [ ] Draft genesis config created

### 10.3 Ceremony Day (T+0)

- [ ] All participants present
- [ ] Identities verified
- [ ] Genesis config reviewed and approved
- [ ] All parties generated genesis state
- [ ] State roots compared — **ALL MATCH**
- [ ] Attestations signed
- [ ] Ceremony log completed

### 10.4 Post-Ceremony (T+1)

- [ ] Genesis files published
- [ ] State root announced publicly
- [ ] Ceremony log archived
- [ ] Attestations published
- [ ] Mainnet launch scheduled

---

## Appendix A: Tool Reference

### Genesis Generator Commands

```bash
# Generate and display summary
genesis-generator --config mainnet_config.json

# Generate and write output files
genesis-generator --config mainnet_config.json --output-dir ./output

# Verify against expected state root
genesis-generator --config mainnet_config.json --verify <64-hex-chars>

# Print only state root (for scripting)
genesis-generator --config mainnet_config.json --state-root-only

# Verbose output
genesis-generator --config mainnet_config.json --verbose
```

### Output Files

| File | Description |
|------|-------------|
| `genesis_config.json` | Canonical config (for reproducibility) |
| `genesis_block.bin` | Binary-encoded genesis block |
| `state_root.hex` | 64-char hex state root |
| `validator_set.json` | Sorted validator addresses |
| `genesis_summary.txt` | Human-readable summary |

---

## Appendix B: Security Contacts

| Role | Contact | Backup |
|------|---------|--------|
| Ceremony Coordinator | NOVAInetwork@protonmail.com | See SECURITY.md |
| Security Lead | NOVAInetwork@protonmail.com | See SECURITY.md |
| Emergency Hotline | NOVAInetwork@protonmail.com | See SECURITY.md |

---

## Joining an Existing Network (Post-Genesis)

If the genesis ceremony has already been completed and the network is running, follow these steps to join as a new validator:

### 1. Obtain the Genesis Configuration

Download the canonical `genesis.json` from the network operator or the official repository. Verify the file hash matches the published value.

### 2. Verify the Genesis State Root

Run the genesis generator tool to independently verify the state root:

```bash
cargo run --release -p genesis-generator -- --config genesis.json --verify <published_state_root_hex>
```

If verification succeeds, the tool prints `MATCH`. If it fails, **do not start your node** — contact the ceremony coordinator.

### 3. Generate Your Validator Key

```bash
novai-node generate-key --output ~/.novai/data/validator.key
```

Share your **public key** (printed to stdout) with the network operator for inclusion in the next validator set update.

### 4. Start Your Node

```bash
novai-node run \
  --port 9090 \
  --genesis genesis.json \
  --key-file ~/.novai/data/validator.key \
  --seed seed.novai.network:9090 \
  --data-dir ~/.novai/data
```

Your node will connect to seed nodes, sync the chain from genesis height, and begin participating in consensus once caught up.

### 5. Verify Sync

Monitor your node's committed height via the metrics endpoint:

```bash
curl -s http://localhost:8080/metrics | grep novai_committed_height
```

Once your committed height matches the network's latest height, your node is fully synced.

---

**End of Genesis Ceremony Procedure**
