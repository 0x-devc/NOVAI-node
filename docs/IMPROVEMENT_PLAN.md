# NOVAI Development Improvement Plan

This document tracks systematic improvements to be implemented in future development weeks, based on feedback and best practices identified during Week 5.

## Week 1 Retrospective Additions

### CI/CD Enhancements

**Priority:** HIGH  
**Effort:** 30 minutes

#### 1. Toolchain Visibility
Add version printing to CI workflow for debugging:
```yaml
- name: Print toolchain versions
  run: |
    rustc --version
    cargo --version
    cargo fmt --version
    cargo clippy --version
```

**Benefits:**
- Easy debugging when CI/local behavior differs
- Clear audit trail of which toolchain version built what
- Helps identify version-specific issues

---

#### 2. Cross-Platform Matrix Testing
Expand CI to test on multiple platforms:
```yaml
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
```

**Benefits:**
- Catches platform-specific bugs early (endianness, path separators, etc.)
- Ensures portability
- Prevents "works on my Mac but not on Linux" issues

**Cons:**
- 3x CI runtime (acceptable for stable only)
- Slightly higher GitHub Actions costs

**Decision:** Worth it for consensus-critical codebase.

---

## Week 2 Additions

### Property-Based Testing

**Priority:** HIGH  
**Effort:** 2-3 hours

#### Add proptest Dependency
```toml
[dev-dependencies]
proptest = "1.4"
```

#### Tests to Implement

**1. QC Permutation Invariant**
```rust
proptest! {
    #[test]
    fn qc_permutation_invariant(
        votes in prop::collection::vec(arbitrary_vote(), 1..100)
    ) {
        let mut permuted = votes.clone();
        permuted.shuffle(&mut rng);
        
        let qc1 = QC { votes: votes.clone(), ..qc };
        let qc2 = QC { votes: permuted, ..qc };
        
        prop_assert_eq!(
            encode_qc_v1(&qc1).unwrap(),
            encode_qc_v1(&qc2).unwrap()
        );
    }
}
```

**Benefits:** Proves normalization works for ALL permutations, not just hand-picked test cases.

---

**2. Codec Rejection Determinism**
```rust
proptest! {
    #[test]
    fn duplicate_voters_always_rejected(
        vote in arbitrary_vote()
    ) {
        let qc = QC {
            votes: vec![vote.clone(), vote.clone()],
            ..qc
        };
        
        prop_assert!(matches!(
            encode_qc_v1(&qc),
            Err(CodecError::DuplicateVoter)
        ));
    }
}
```

**Benefits:** Ensures error handling is consistent across all inputs.

---

**3. Unknown Version Rejection**
```rust
proptest! {
    #[test]
    fn unknown_version_rejected(
        version in 0x02u8..=0xFF
    ) {
        let mut bytes = valid_block_bytes();
        bytes[0] = version; // corrupt version byte
        
        prop_assert!(decode_block_v1(&bytes).is_err());
    }
}
```

**Benefits:** Forward compatibility safety - prevents accidental acceptance of future formats.

---

**Scope Control:**
- Keep generators bounded (e.g., 1..100 votes, not 1..10000)
- Use deterministic seeds for reproducibility
- Add to regression suite, run in CI

---

## Week 3 Additions

### Address ↔ PubKey Mapping

**Priority:** CRITICAL  
**Effort:** 1-2 hours

**Problem:** Current spec doesn't define the relationship between Address and PublicKey. Two implementations could verify different keys for the same address → consensus split.

#### Design Decision Required

**Option A: Address = PubKey (Simplest)**
```rust
pub type Address = [u8; 32];  // Ed25519 pubkey bytes directly

// No conversion needed
fn address_to_pubkey(addr: Address) -> PublicKey {
    PublicKey::from_bytes(&addr)
}
```

**Pros:**
- Simplest implementation
- No hash computation
- Direct verification

**Cons:**
- Address reveals public key (less privacy)
- No future flexibility (can't change key type)

---

**Option B: Address = Hash(PubKey) (More Common)**
```rust
pub type Address = [u8; 32];  // blake3(pubkey)

pub struct ValidatorSet {
    validators: Vec<ValidatorEntry>,
}

pub struct ValidatorEntry {
    address: Address,
    pubkey: PublicKey,
}

impl ValidatorSet {
    pub fn new(entries: Vec<ValidatorEntry>) -> Result<Self, Error> {
        // Enforce: each address == blake3(pubkey)
        for entry in &entries {
            let computed = blake3::hash(entry.pubkey.as_bytes());
            if computed.as_bytes() != &entry.address {
                return Err(Error::AddressMismatch);
            }
        }
        // ... rest of validation ...
    }
}
```

**Pros:**
- Standard approach (Bitcoin, Ethereum use hash-based addresses)
- Can change key without changing address (future-proof)
- Better privacy (address doesn't reveal pubkey until first use)

**Cons:**
- Need to store/transmit pubkeys separately
- Extra hash computation
- Larger ValidatorSet messages

---

**Recommendation:** Option B (hash-based) for production systems. Add to Week 3 deliverables:

1. Update Address type definition in spec
2. Add ValidatorEntry struct
3. Update ValidatorSet to enforce relationship
4. Add tests proving enforcement

---

## Week 4 Additions

### SMT Order Independence

**Priority:** MEDIUM  
**Effort:** 30 minutes

**Current State:** Week 4 proved root determinism (same inputs → same root).

**Gap:** Didn't prove order independence (same writes in different order → same root).

#### Test to Add
```rust
#[test]
fn smt_insert_order_independent() {
    let writes = vec![
        ([0x01; 32], vec![0xaa]),
        ([0x02; 32], vec![0xbb]),
        ([0x03; 32], vec![0xcc]),
    ];
    
    // Apply in original order
    let mut smt1 = SparseMerkleTree::new();
    for (key, value) in &writes {
        smt1.insert(key, value).unwrap();
    }
    let root1 = smt1.root();
    
    // Apply in reverse order
    let mut smt2 = SparseMerkleTree::new();
    for (key, value) in writes.iter().rev() {
        smt2.insert(key, value).unwrap();
    }
    let root2 = smt2.root();
    
    assert_eq!(root1, root2, "SMT root must be order-independent");
}
```

**Benefits:**
- Catches accidental ordering dependencies
- Critical for consensus (nodes may receive txs in different order)
- Small test, high value

**Also add proptest version:**
```rust
proptest! {
    #[test]
    fn smt_any_permutation_same_root(
        writes in prop::collection::vec(
            (arbitrary_key(), arbitrary_value()),
            1..20
        )
    ) {
        let mut permuted = writes.clone();
        permuted.shuffle(&mut rng);
        
        let root_orig = apply_all(&writes);
        let root_perm = apply_all(&permuted);
        
        prop_assert_eq!(root_orig, root_perm);
    }
}
```

---

## Week 5 Retrospective (COMPLETED)

✅ **Signature domain separation table** - Single source of truth  
✅ **Wire format & network rules** - DoS limits + acceptance policy  
✅ **Forward compatibility rules** - Unknown version rejection  
✅ **Fixed size limit math** - n=16,501 max, quorum=11,001  
✅ **CI fix** - LLVM/Clang dependencies added

---

## Priority Summary

| Week | Item | Priority | Effort | Impact |
|------|------|----------|--------|--------|
| 1 | Toolchain visibility | HIGH | 5 min | Debug aid |
| 1 | Cross-platform matrix | HIGH | 10 min | Portability |
| 2 | Property tests (QC) | HIGH | 1 hour | Bug finding |
| 2 | Property tests (codec) | HIGH | 1 hour | Safety |
| 2 | Property tests (version) | MEDIUM | 30 min | Future-proof |
| 3 | Address ↔ PubKey mapping | CRITICAL | 2 hours | Consensus safety |
| 4 | SMT order independence | MEDIUM | 30 min | Correctness proof |

---

## Implementation Guidelines

### When to Add Property Tests
- After stable implementation exists
- Before Week 6 (consensus engine)
- Keep generators bounded
- Use deterministic seeds

### When to Expand CI
- After Week 1 deliverables stable
- Before team grows
- Monitor CI costs

### When to Define Address Mapping
- Week 3 (before consensus engine uses it)
- Requires design decision from team
- Document in spec before implementing

---

## Maintenance Notes

This document should be reviewed:
- After each week's deliverables complete
- When new feedback is received
- Before planning next week's work

Update status markers:
- 🔴 Not started
- 🟡 In progress
- 🟢 Complete

---

**Document Status:** Living document  
**Last Updated:** Week 5 completion  
**Next Review:** Before Week 6 kickoff
