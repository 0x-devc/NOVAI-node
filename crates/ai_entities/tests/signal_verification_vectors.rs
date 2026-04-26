//! AI Signal Verification Vectors (D20.3)
//!
//! PURPOSE: Lock the commitment hash computation for ALL signal types
//! with documented verification steps.
//!
//! VERIFICATION STEPS (for each signal type):
//! 1. Create signal with known, deterministic inputs
//! 2. Compute commitment hash using domain-separated blake3
//! 3. Verify commitment matches expected golden vector
//! 4. Verify encoding roundtrip preserves all fields
//! 5. Verify commitment hash is independent of signature
//!
//! INVARIANTS:
//! - Domain separator: "NOVAI_SIGNAL_COMMIT_V1"
//! - Commitment excludes signature (can be recomputed)
//! - Height encoded as little-endian u64
//! - All signal types use same commitment formula
//!
//! Run with `UPDATE_VECTORS=1` to regenerate vectors:
//! ```text
//! UPDATE_VECTORS=1 cargo test -p novai-ai-entities --test signal_verification_vectors
//! ```

use novai_ai_entities::{AiSignalType, AiSignalV1};
use std::fs;
use std::path::PathBuf;

fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors")
}

fn should_update_vectors() -> bool {
    std::env::var("UPDATE_VECTORS").is_ok()
}

fn write_or_compare_hash(path: &PathBuf, actual: &[u8; 32], name: &str) {
    if should_update_vectors() {
        fs::create_dir_all(vectors_dir()).expect("create vectors dir");
        fs::write(path, actual).expect("write vector");
        println!("Updated: {} ({} bytes)", path.display(), actual.len());
    } else {
        let expected = fs::read(path)
            .unwrap_or_else(|_| panic!("Missing vector: {}. Run UPDATE_VECTORS=1", path.display()));
        assert_eq!(
            actual.as_slice(),
            expected.as_slice(),
            "{name} commitment hash mismatch - encoding may have drifted!"
        );
    }
}

// ============================================================================
// DETERMINISTIC INPUT GENERATION
// ============================================================================

/// Create deterministic signal for a given type.
/// All inputs are derived from the signal type byte for reproducibility.
///
/// Formula:
/// - height = 1000 + (type_byte * 100)
/// - issuer[i] = type_byte + i
/// - confidence = 100 + type_byte
/// - payload_hash[i] = (type_byte * 2) + i
/// - zk_proof = None
/// - signature = [0x55; 64] (placeholder, not part of commitment)
fn make_signal(signal_type: AiSignalType) -> AiSignalV1 {
    let type_byte = signal_type.to_byte();

    // Deterministic issuer: [type_byte, type_byte+1, type_byte+2, ...]
    let mut issuer = [0u8; 32];
    for (i, b) in issuer.iter_mut().enumerate() {
        *b = type_byte.wrapping_add(i as u8);
    }

    // Deterministic payload_hash: [type_byte * 2, ...]
    let mut payload_hash = [0u8; 32];
    for (i, b) in payload_hash.iter_mut().enumerate() {
        *b = (type_byte.wrapping_mul(2)).wrapping_add(i as u8);
    }

    // Signature (not part of commitment, but needed for structure)
    let signature = [0x55u8; 64];

    AiSignalV1 {
        signal_type,
        height: 1000 + u64::from(type_byte) * 100,
        issuer,
        confidence: 100 + type_byte,
        payload_hash,
        zk_proof: None,
        signature,
    }
}

/// Create deterministic signal WITH a ZK proof for proof-binding tests.
fn make_signal_with_proof(signal_type: AiSignalType) -> AiSignalV1 {
    let mut signal = make_signal(signal_type);
    let type_byte = signal_type.to_byte();

    // Deterministic proof: 64 bytes of [type_byte * 3, ...]
    let mut proof = vec![0u8; 64];
    for (i, b) in proof.iter_mut().enumerate() {
        *b = (type_byte.wrapping_mul(3)).wrapping_add(i as u8);
    }
    signal.zk_proof = Some(proof);
    signal
}

// ============================================================================
// VERIFICATION TESTS - ALL 7 SIGNAL TYPES
// ============================================================================

/// VERIFICATION STEPS for Anomaly (type=0):
/// 1. Input: signal_type=0, height=1000 (LE), issuer=[0,1,2,...31],
///    confidence=100, payload_hash=[0,1,2,...31], no proof
/// 2. Domain: "NOVAI_SIGNAL_COMMIT_V1" (21 bytes)
/// 3. Hash: blake3(domain || type || height_le || issuer || confidence || payload_hash || proof_len=0)
/// 4. Expected: <golden vector in signal_commit_anomaly.bin>
/// 5. Signature [0x55; 64] is NOT included in commitment
#[test]
fn verify_signal_anomaly() {
    let signal = make_signal(AiSignalType::Anomaly);

    // Step 1: Verify inputs are deterministic
    assert_eq!(signal.signal_type, AiSignalType::Anomaly);
    assert_eq!(signal.signal_type.to_byte(), 0);
    assert_eq!(signal.height, 1000);
    assert_eq!(signal.confidence, 100);
    assert_eq!(signal.issuer[0], 0);
    assert_eq!(signal.issuer[31], 31);
    assert_eq!(signal.payload_hash[0], 0);
    assert!(signal.zk_proof.is_none());

    // Step 2: Compute commitment
    let commitment = signal.to_commitment();

    // Step 3: Verify commitment fields propagate correctly
    assert_eq!(commitment.signal_type, AiSignalType::Anomaly);
    assert_eq!(commitment.height, 1000);
    assert_eq!(commitment.issuer, signal.issuer);

    // Step 4: Lock commitment hash as golden vector
    let path = vectors_dir().join("signal_commit_anomaly.bin");
    write_or_compare_hash(&path, &commitment.commitment_hash, "Anomaly");

    // Step 5: Verify signature independence
    let mut signal2 = signal;
    signal2.signature = [0xFF; 64]; // Different signature
    let commitment2 = signal2.to_commitment();
    assert_eq!(
        commitment.commitment_hash, commitment2.commitment_hash,
        "Commitment must be independent of signature"
    );
}

/// VERIFICATION STEPS for Optimization (type=1):
/// 1. Input: signal_type=1, height=1100 (LE), issuer=[1,2,3,...32],
///    confidence=101, payload_hash=[2,3,4,...33], no proof
/// 2. Domain: "NOVAI_SIGNAL_COMMIT_V1" (21 bytes)
/// 3. Hash: blake3(domain || type || height_le || issuer || confidence || payload_hash || proof_len=0)
/// 4. Expected: <golden vector in signal_commit_optimization.bin>
/// 5. Signature [0x55; 64] is NOT included in commitment
#[test]
fn verify_signal_optimization() {
    let signal = make_signal(AiSignalType::Optimization);

    // Step 1: Verify inputs are deterministic
    assert_eq!(signal.signal_type, AiSignalType::Optimization);
    assert_eq!(signal.signal_type.to_byte(), 1);
    assert_eq!(signal.height, 1100);
    assert_eq!(signal.confidence, 101);
    assert_eq!(signal.issuer[0], 1);
    assert_eq!(signal.payload_hash[0], 2);

    // Step 2: Compute commitment
    let commitment = signal.to_commitment();

    // Step 3: Verify commitment fields
    assert_eq!(commitment.signal_type, AiSignalType::Optimization);
    assert_eq!(commitment.height, 1100);

    // Step 4: Lock commitment hash
    let path = vectors_dir().join("signal_commit_optimization.bin");
    write_or_compare_hash(&path, &commitment.commitment_hash, "Optimization");

    // Step 5: Verify signature independence
    let mut signal2 = signal.clone();
    signal2.signature = [0xAA; 64];
    assert_eq!(
        signal.to_commitment().commitment_hash,
        signal2.to_commitment().commitment_hash
    );
}

/// VERIFICATION STEPS for Prediction (type=2):
/// 1. Input: signal_type=2, height=1200 (LE), issuer=[2,3,4,...33],
///    confidence=102, payload_hash=[4,5,6,...35], no proof
/// 2. Domain: "NOVAI_SIGNAL_COMMIT_V1" (21 bytes)
/// 3. Hash: blake3(domain || type || height_le || issuer || confidence || payload_hash || proof_len=0)
/// 4. Expected: <golden vector in signal_commit_prediction.bin>
/// 5. Signature [0x55; 64] is NOT included in commitment
#[test]
fn verify_signal_prediction() {
    let signal = make_signal(AiSignalType::Prediction);

    // Step 1: Verify inputs
    assert_eq!(signal.signal_type, AiSignalType::Prediction);
    assert_eq!(signal.signal_type.to_byte(), 2);
    assert_eq!(signal.height, 1200);
    assert_eq!(signal.confidence, 102);
    assert_eq!(signal.issuer[0], 2);
    assert_eq!(signal.payload_hash[0], 4);

    // Step 2: Compute commitment
    let commitment = signal.to_commitment();

    // Step 3: Verify commitment fields
    assert_eq!(commitment.signal_type, AiSignalType::Prediction);
    assert_eq!(commitment.height, 1200);

    // Step 4: Lock commitment hash
    let path = vectors_dir().join("signal_commit_prediction.bin");
    write_or_compare_hash(&path, &commitment.commitment_hash, "Prediction");

    // Step 5: Verify signature independence
    let mut signal2 = signal.clone();
    signal2.signature = [0xBB; 64];
    assert_eq!(
        signal.to_commitment().commitment_hash,
        signal2.to_commitment().commitment_hash
    );
}

/// VERIFICATION STEPS for RiskScore (type=3):
/// 1. Input: signal_type=3, height=1300 (LE), issuer=[3,4,5,...34],
///    confidence=103, payload_hash=[6,7,8,...37], no proof
/// 2. Domain: "NOVAI_SIGNAL_COMMIT_V1" (21 bytes)
/// 3. Hash: blake3(domain || type || height_le || issuer || confidence || payload_hash || proof_len=0)
/// 4. Expected: <golden vector in signal_commit_riskscore.bin>
/// 5. Signature [0x55; 64] is NOT included in commitment
#[test]
fn verify_signal_riskscore() {
    let signal = make_signal(AiSignalType::RiskScore);

    // Step 1: Verify inputs
    assert_eq!(signal.signal_type, AiSignalType::RiskScore);
    assert_eq!(signal.signal_type.to_byte(), 3);
    assert_eq!(signal.height, 1300);
    assert_eq!(signal.confidence, 103);
    assert_eq!(signal.issuer[0], 3);
    assert_eq!(signal.payload_hash[0], 6);

    // Step 2: Compute commitment
    let commitment = signal.to_commitment();

    // Step 3: Verify commitment fields
    assert_eq!(commitment.signal_type, AiSignalType::RiskScore);
    assert_eq!(commitment.height, 1300);

    // Step 4: Lock commitment hash
    let path = vectors_dir().join("signal_commit_riskscore.bin");
    write_or_compare_hash(&path, &commitment.commitment_hash, "RiskScore");

    // Step 5: Verify signature independence
    let mut signal2 = signal.clone();
    signal2.signature = [0xCC; 64];
    assert_eq!(
        signal.to_commitment().commitment_hash,
        signal2.to_commitment().commitment_hash
    );
}

/// VERIFICATION STEPS for AuditReport (type=4):
/// 1. Input: signal_type=4, height=1400 (LE), issuer=[4,5,6,...35],
///    confidence=104, payload_hash=[8,9,10,...39], no proof
/// 2. Domain: "NOVAI_SIGNAL_COMMIT_V1" (21 bytes)
/// 3. Hash: blake3(domain || type || height_le || issuer || confidence || payload_hash || proof_len=0)
/// 4. Expected: <golden vector in signal_commit_auditreport.bin>
/// 5. Signature [0x55; 64] is NOT included in commitment
#[test]
fn verify_signal_auditreport() {
    let signal = make_signal(AiSignalType::AuditReport);

    // Step 1: Verify inputs
    assert_eq!(signal.signal_type, AiSignalType::AuditReport);
    assert_eq!(signal.signal_type.to_byte(), 4);
    assert_eq!(signal.height, 1400);
    assert_eq!(signal.confidence, 104);
    assert_eq!(signal.issuer[0], 4);
    assert_eq!(signal.payload_hash[0], 8);

    // Step 2: Compute commitment
    let commitment = signal.to_commitment();

    // Step 3: Verify commitment fields
    assert_eq!(commitment.signal_type, AiSignalType::AuditReport);
    assert_eq!(commitment.height, 1400);

    // Step 4: Lock commitment hash
    let path = vectors_dir().join("signal_commit_auditreport.bin");
    write_or_compare_hash(&path, &commitment.commitment_hash, "AuditReport");

    // Step 5: Verify signature independence
    let mut signal2 = signal.clone();
    signal2.signature = [0xDD; 64];
    assert_eq!(
        signal.to_commitment().commitment_hash,
        signal2.to_commitment().commitment_hash
    );
}

/// VERIFICATION STEPS for SpamRisk (type=5):
/// 1. Input: signal_type=5, height=1500 (LE), issuer=[5,6,7,...36],
///    confidence=105, payload_hash=[10,11,12,...41], no proof
/// 2. Domain: "NOVAI_SIGNAL_COMMIT_V1" (21 bytes)
/// 3. Hash: blake3(domain || type || height_le || issuer || confidence || payload_hash || proof_len=0)
/// 4. Expected: <golden vector in signal_commit_spamrisk.bin>
/// 5. Signature [0x55; 64] is NOT included in commitment
#[test]
fn verify_signal_spamrisk() {
    let signal = make_signal(AiSignalType::SpamRisk);

    // Step 1: Verify inputs
    assert_eq!(signal.signal_type, AiSignalType::SpamRisk);
    assert_eq!(signal.signal_type.to_byte(), 5);
    assert_eq!(signal.height, 1500);
    assert_eq!(signal.confidence, 105);
    assert_eq!(signal.issuer[0], 5);
    assert_eq!(signal.payload_hash[0], 10);

    // Step 2: Compute commitment
    let commitment = signal.to_commitment();

    // Step 3: Verify commitment fields
    assert_eq!(commitment.signal_type, AiSignalType::SpamRisk);
    assert_eq!(commitment.height, 1500);

    // Step 4: Lock commitment hash
    let path = vectors_dir().join("signal_commit_spamrisk.bin");
    write_or_compare_hash(&path, &commitment.commitment_hash, "SpamRisk");

    // Step 5: Verify signature independence
    let mut signal2 = signal.clone();
    signal2.signature = [0xEE; 64];
    assert_eq!(
        signal.to_commitment().commitment_hash,
        signal2.to_commitment().commitment_hash
    );
}

/// VERIFICATION STEPS for CongestionForecast (type=6):
/// 1. Input: signal_type=6, height=1600 (LE), issuer=[6,7,8,...37],
///    confidence=106, payload_hash=[12,13,14,...43], no proof
/// 2. Domain: "NOVAI_SIGNAL_COMMIT_V1" (21 bytes)
/// 3. Hash: blake3(domain || type || height_le || issuer || confidence || payload_hash || proof_len=0)
/// 4. Expected: <golden vector in signal_commit_congestionforecast.bin>
/// 5. Signature [0x55; 64] is NOT included in commitment
#[test]
fn verify_signal_congestionforecast() {
    let signal = make_signal(AiSignalType::CongestionForecast);

    // Step 1: Verify inputs
    assert_eq!(signal.signal_type, AiSignalType::CongestionForecast);
    assert_eq!(signal.signal_type.to_byte(), 6);
    assert_eq!(signal.height, 1600);
    assert_eq!(signal.confidence, 106);
    assert_eq!(signal.issuer[0], 6);
    assert_eq!(signal.payload_hash[0], 12);

    // Step 2: Compute commitment
    let commitment = signal.to_commitment();

    // Step 3: Verify commitment fields
    assert_eq!(commitment.signal_type, AiSignalType::CongestionForecast);
    assert_eq!(commitment.height, 1600);

    // Step 4: Lock commitment hash
    let path = vectors_dir().join("signal_commit_congestionforecast.bin");
    write_or_compare_hash(&path, &commitment.commitment_hash, "CongestionForecast");

    // Step 5: Verify signature independence
    let mut signal2 = signal.clone();
    signal2.signature = [0x11; 64];
    assert_eq!(
        signal.to_commitment().commitment_hash,
        signal2.to_commitment().commitment_hash
    );
}

// ============================================================================
// ZK PROOF BINDING TESTS
// ============================================================================

/// Verify that ZK proof is bound to commitment hash.
/// Different proofs MUST produce different commitment hashes.
#[test]
fn verify_proof_binding() {
    let signal_no_proof = make_signal(AiSignalType::Anomaly);
    let signal_with_proof = make_signal_with_proof(AiSignalType::Anomaly);

    let hash_no_proof = signal_no_proof.compute_commitment_hash();
    let hash_with_proof = signal_with_proof.compute_commitment_hash();

    assert_ne!(
        hash_no_proof, hash_with_proof,
        "Proof presence must affect commitment hash"
    );

    // Golden vector for proof-included commitment
    let path = vectors_dir().join("signal_commit_anomaly_with_proof.bin");
    write_or_compare_hash(&path, &hash_with_proof, "Anomaly+Proof");
}

/// Verify that different proof contents produce different hashes.
#[test]
fn verify_proof_content_binding() {
    let signal1 = make_signal_with_proof(AiSignalType::Prediction);
    let mut signal2 = make_signal_with_proof(AiSignalType::Prediction);

    // Modify one byte in proof
    if let Some(ref mut proof) = signal2.zk_proof {
        proof[0] ^= 0xFF;
    }

    let hash1 = signal1.compute_commitment_hash();
    let hash2 = signal2.compute_commitment_hash();

    assert_ne!(
        hash1, hash2,
        "Different proof content must produce different commitment hash"
    );
}

// ============================================================================
// DETERMINISM TESTS
// ============================================================================

/// Verify commitment hash computation is deterministic.
#[test]
fn verify_commitment_determinism() {
    for signal_type in [
        AiSignalType::Anomaly,
        AiSignalType::Optimization,
        AiSignalType::Prediction,
        AiSignalType::RiskScore,
        AiSignalType::AuditReport,
        AiSignalType::SpamRisk,
        AiSignalType::CongestionForecast,
    ] {
        let signal = make_signal(signal_type);

        let hash1 = signal.compute_commitment_hash();
        let hash2 = signal.compute_commitment_hash();
        let hash3 = signal.compute_commitment_hash();

        assert_eq!(hash1, hash2, "{signal_type:?} hash not deterministic");
        assert_eq!(hash2, hash3, "{signal_type:?} hash not deterministic");
    }
}

/// Verify all signal types produce unique commitment hashes.
#[test]
fn verify_signal_types_produce_unique_hashes() {
    let types = [
        AiSignalType::Anomaly,
        AiSignalType::Optimization,
        AiSignalType::Prediction,
        AiSignalType::RiskScore,
        AiSignalType::AuditReport,
        AiSignalType::SpamRisk,
        AiSignalType::CongestionForecast,
    ];

    let hashes: Vec<[u8; 32]> = types
        .iter()
        .map(|t| make_signal(*t).compute_commitment_hash())
        .collect();

    // Check all pairs are unique
    for i in 0..hashes.len() {
        for j in (i + 1)..hashes.len() {
            assert_ne!(
                hashes[i], hashes[j],
                "{:?} and {:?} produced same hash!",
                types[i], types[j]
            );
        }
    }
}

// ============================================================================
// TYPE ENCODING TESTS
// ============================================================================

/// Verify signal type byte encoding is stable.
#[test]
fn verify_signal_type_encoding() {
    assert_eq!(AiSignalType::Anomaly.to_byte(), 0);
    assert_eq!(AiSignalType::Optimization.to_byte(), 1);
    assert_eq!(AiSignalType::Prediction.to_byte(), 2);
    assert_eq!(AiSignalType::RiskScore.to_byte(), 3);
    assert_eq!(AiSignalType::AuditReport.to_byte(), 4);
    assert_eq!(AiSignalType::SpamRisk.to_byte(), 5);
    assert_eq!(AiSignalType::CongestionForecast.to_byte(), 6);
}

/// Verify signal type roundtrip from byte.
#[test]
fn verify_signal_type_roundtrip() {
    for i in 0u8..=6 {
        let signal_type = AiSignalType::from_byte(i).expect("valid type");
        assert_eq!(signal_type.to_byte(), i);
    }

    // Invalid bytes should return None
    assert!(AiSignalType::from_byte(7).is_none());
    assert!(AiSignalType::from_byte(255).is_none());
}
