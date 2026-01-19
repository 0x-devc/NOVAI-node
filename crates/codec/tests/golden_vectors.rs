use std::fs;
use std::path::Path;

use novai_codec::{
    decode_block_header_v1, decode_tx_v1_signed, decode_tx_v1_unsigned, encode_block_header_v1,
    encode_tx_v1_signed, encode_tx_v1_unsigned,
};
use novai_types::{
    Address, BlockHeaderV1, BlockHeaderVersion, Hash32, SignatureBytes, TxV1, TxVersion,
};

fn write_or_compare(path: &Path, actual: &[u8]) {
    let update = std::env::var("UPDATE_VECTORS").ok().as_deref() == Some("1");

    if update {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create vectors dir");
        }
        fs::write(path, actual).expect("write vector file");
        return;
    }

    let expected = fs::read(path).unwrap_or_else(|_| {
        panic!("missing vector file: {path:?}. Run with UPDATE_VECTORS=1 to generate.")
    });

    assert_eq!(
        expected, actual,
        "golden vector mismatch for {path:?} (encoding drift?)"
    );
}

fn sample_tx() -> TxV1 {
    let from: Address = [0x11u8; 32];
    let pubkey: [u8; 32] = [0x33u8; 32];
    let sig: SignatureBytes = [0x22u8; 64];

    TxV1 {
        version: TxVersion::V1,
        from,
        pubkey,
        nonce: 42,
        fee: 7,
        payload: b"hello".to_vec(),
        sig,
    }
}

fn sample_header() -> BlockHeaderV1 {
    let prev_hash: Hash32 = [0xAAu8; 32];
    let state_root: Hash32 = [0xBBu8; 32];
    let tx_root: Hash32 = [0xCCu8; 32];
    let proposer: Address = [0xDDu8; 32];
    let qc_hash: Hash32 = [0xEEu8; 32];

    BlockHeaderV1 {
        version: BlockHeaderVersion::V1,
        height: 123,
        prev_hash,
        state_root,
        tx_root,
        proposer,
        qc_hash,
    }
}

#[test]
fn golden_vectors_tx_and_header_v1() {
    let tx = sample_tx();
    let header = sample_header();

    // --- Encode ---
    let unsigned = encode_tx_v1_unsigned(&tx).expect("encode unsigned");
    let signed = encode_tx_v1_signed(&tx).expect("encode signed");
    let header_bytes = encode_block_header_v1(&header).expect("encode header");

    // --- Decode checks ---
    let decoded_unsigned = decode_tx_v1_unsigned(&unsigned).expect("decode unsigned");
    assert_eq!(decoded_unsigned.version, tx.version);
    assert_eq!(decoded_unsigned.from, tx.from);
    assert_eq!(decoded_unsigned.nonce, tx.nonce);
    assert_eq!(decoded_unsigned.fee, tx.fee);
    assert_eq!(decoded_unsigned.payload, tx.payload);
    assert_eq!(
        decoded_unsigned.sig, [0u8; 64],
        "unsigned decode must zero sig"
    );

    let decoded_signed = decode_tx_v1_signed(&signed).expect("decode signed");
    assert_eq!(decoded_signed, tx);

    let decoded_header = decode_block_header_v1(&header_bytes).expect("decode header");
    assert_eq!(decoded_header, header);

    // --- Golden file paths (relative to crates/codec) ---
    let tx_unsigned_path = Path::new("tests/vectors/txv1_unsigned.bin");
    let tx_signed_path = Path::new("tests/vectors/txv1_signed.bin");
    let header_path = Path::new("tests/vectors/blockheader_v1.bin");

    // --- Compare (or generate) ---
    write_or_compare(tx_unsigned_path, &unsigned);
    write_or_compare(tx_signed_path, &signed);
    write_or_compare(header_path, &header_bytes);
}

use novai_ai_entities::{AiSignalType, AiSignalV1};
use novai_codec::{
    decode_ai_signal_v1, decode_signal_commitment_v1, encode_ai_signal_v1,
    encode_signal_commitment_v1,
};

fn sample_signal_no_proof() -> AiSignalV1 {
    AiSignalV1 {
        signal_type: AiSignalType::Prediction,
        height: 999,
        issuer: [0x44u8; 32],
        confidence: 128,
        payload_hash: [0x55u8; 32],
        zk_proof: None,
        signature: [0x66u8; 64],
    }
}

#[test]
fn golden_vectors_ai_signal_v1_and_commitment_v1() {
    let signal_no_proof = sample_signal_no_proof();
    let mut signal_with_proof = sample_signal_no_proof();
    signal_with_proof.zk_proof = Some(vec![0x99u8; 256]);

    // --- Encode ---
    let no_proof_bytes = encode_ai_signal_v1(&signal_no_proof).expect("encode signal (no proof)");
    let with_proof_bytes =
        encode_ai_signal_v1(&signal_with_proof).expect("encode signal (with proof)");

    let commitment = signal_no_proof.to_commitment();
    let commitment_bytes = encode_signal_commitment_v1(&commitment);

    // --- Decode checks / roundtrip ---
    let decoded_no_proof = decode_ai_signal_v1(&no_proof_bytes).expect("decode no-proof");
    assert_eq!(decoded_no_proof, signal_no_proof);

    let decoded_with_proof = decode_ai_signal_v1(&with_proof_bytes).expect("decode with-proof");
    assert_eq!(decoded_with_proof, signal_with_proof);

    let decoded_commitment =
        decode_signal_commitment_v1(&commitment_bytes).expect("decode commitment");
    assert_eq!(decoded_commitment, commitment);

    // --- Golden file paths (relative to crates/codec) ---
    let p1 = Path::new("tests/vectors/ai_signal_v1_no_proof.bin");
    let p2 = Path::new("tests/vectors/ai_signal_v1_with_proof.bin");
    let p3 = Path::new("tests/vectors/signal_commitment_v1.bin");

    // --- Compare (or generate) ---
    write_or_compare(p1, &no_proof_bytes);
    write_or_compare(p2, &with_proof_bytes);
    write_or_compare(p3, &commitment_bytes);
}

// ============================================================================
// Gate Golden Vectors (Week 13)
// ============================================================================

use novai_ai_entities::gates::{ApprovalGate, GateType};
use novai_codec::{decode_approval_gate_v1, encode_approval_gate_v1};

/// Standard multisig gate for golden vector testing.
fn sample_multisig_gate() -> ApprovalGate {
    // Use deterministic approver addresses
    let approvers = vec![[0x01u8; 32], [0x02u8; 32], [0x03u8; 32]];

    ApprovalGate::new(
        GateType::Multisig,
        approvers,
        2,     // threshold
        100,   // timelock_blocks
        1000,  // expiry_blocks
        false, // veto_enabled
        false, // freeze_enabled
    )
    .expect("valid multisig gate")
}

/// Standard threshold gate for golden vector testing.
fn sample_threshold_gate() -> ApprovalGate {
    let approvers = vec![[0x11u8; 32], [0x12u8; 32], [0x13u8; 32], [0x14u8; 32]];

    ApprovalGate::new(
        GateType::Threshold,
        approvers,
        3,    // threshold
        200,  // timelock_blocks
        2000, // expiry_blocks
        true, // veto_enabled
        true, // freeze_enabled
    )
    .expect("valid threshold gate")
}

/// Standard timelock-only gate for golden vector testing.
fn sample_timelock_only_gate() -> ApprovalGate {
    ApprovalGate::new(
        GateType::TimelockOnly,
        vec![], // No approvers
        0,      // threshold
        500,    // timelock_blocks
        5000,   // expiry_blocks
        false,  // veto_enabled
        true,   // freeze_enabled
    )
    .expect("valid timelock-only gate")
}

#[test]
fn golden_vectors_approval_gate_v1() {
    let multisig = sample_multisig_gate();
    let threshold = sample_threshold_gate();
    let timelock_only = sample_timelock_only_gate();

    // --- Encode ---
    let multisig_bytes = encode_approval_gate_v1(&multisig);
    let threshold_bytes = encode_approval_gate_v1(&threshold);
    let timelock_only_bytes = encode_approval_gate_v1(&timelock_only);

    // --- Verify encoding is deterministic ---
    assert_eq!(
        encode_approval_gate_v1(&multisig),
        multisig_bytes,
        "Multisig encoding must be deterministic"
    );
    assert_eq!(
        encode_approval_gate_v1(&threshold),
        threshold_bytes,
        "Threshold encoding must be deterministic"
    );
    assert_eq!(
        encode_approval_gate_v1(&timelock_only),
        timelock_only_bytes,
        "TimelockOnly encoding must be deterministic"
    );

    // --- Decode and roundtrip verification ---
    let decoded_multisig = decode_approval_gate_v1(&multisig_bytes).expect("decode multisig");
    assert_eq!(multisig.gate_id, decoded_multisig.gate_id);
    assert_eq!(multisig.gate_type, decoded_multisig.gate_type);
    assert_eq!(
        multisig.required_approvers,
        decoded_multisig.required_approvers
    );
    assert_eq!(multisig.threshold, decoded_multisig.threshold);
    assert_eq!(multisig.timelock_blocks, decoded_multisig.timelock_blocks);
    assert_eq!(multisig.expiry_blocks, decoded_multisig.expiry_blocks);
    assert_eq!(multisig.veto_enabled, decoded_multisig.veto_enabled);
    assert_eq!(multisig.freeze_enabled, decoded_multisig.freeze_enabled);

    let decoded_threshold = decode_approval_gate_v1(&threshold_bytes).expect("decode threshold");
    assert_eq!(threshold.gate_id, decoded_threshold.gate_id);
    assert_eq!(threshold.gate_type, decoded_threshold.gate_type);
    assert_eq!(threshold.veto_enabled, decoded_threshold.veto_enabled);
    assert_eq!(threshold.freeze_enabled, decoded_threshold.freeze_enabled);

    let decoded_timelock = decode_approval_gate_v1(&timelock_only_bytes).expect("decode timelock");
    assert_eq!(timelock_only.gate_id, decoded_timelock.gate_id);
    assert_eq!(timelock_only.gate_type, decoded_timelock.gate_type);
    assert!(decoded_timelock.required_approvers.is_empty());

    // --- Golden file paths (relative to crates/codec) ---
    let p_multisig = Path::new("tests/vectors/approval_gate_v1_multisig.bin");
    let p_threshold = Path::new("tests/vectors/approval_gate_v1_threshold.bin");
    let p_timelock = Path::new("tests/vectors/approval_gate_v1_timelock_only.bin");

    // --- Compare (or generate) ---
    write_or_compare(p_multisig, &multisig_bytes);
    write_or_compare(p_threshold, &threshold_bytes);
    write_or_compare(p_timelock, &timelock_only_bytes);
}

#[test]
fn gate_encoding_byte_roundtrip() {
    // Test that encode(decode(bytes)) == bytes for all gate types
    let gates = [
        sample_multisig_gate(),
        sample_threshold_gate(),
        sample_timelock_only_gate(),
    ];

    for gate in gates {
        let encoded = encode_approval_gate_v1(&gate);
        let decoded = decode_approval_gate_v1(&encoded).expect("decode should succeed");
        let re_encoded = encode_approval_gate_v1(&decoded);

        assert_eq!(
            encoded, re_encoded,
            "encode(decode(bytes)) must equal original bytes for {:?}",
            gate.gate_type
        );
    }
}
