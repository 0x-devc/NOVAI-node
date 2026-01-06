use novai_state::{decode_smt_root_v1, encode_smt_root_v1, SMT_ROOT_CODEC_V1};

#[test]
fn smt_root_v1_roundtrip_is_exact() {
    let root = [0xABu8; 32];
    let enc = encode_smt_root_v1(&root);
    assert_eq!(enc[0], SMT_ROOT_CODEC_V1);
    let dec = decode_smt_root_v1(&enc).unwrap();
    assert_eq!(dec, root);
}

#[test]
fn smt_root_decode_rejects_wrong_length() {
    let bytes = vec![SMT_ROOT_CODEC_V1, 1, 2, 3]; // too short
    let err = decode_smt_root_v1(&bytes).unwrap_err();
    match err {
        novai_state::StateDecodeError::BadLength { expected, got } => {
            assert_eq!(expected, 33);
            assert_eq!(got, 4);
        }
        other => panic!("unexpected error: {:?}", other),
    }
}

#[test]
fn smt_root_decode_rejects_wrong_version() {
    let mut bytes = [0u8; 33];
    bytes[0] = 99; // wrong version
    let err = decode_smt_root_v1(&bytes).unwrap_err();
    match err {
        novai_state::StateDecodeError::BadVersion { expected, got } => {
            assert_eq!(expected, SMT_ROOT_CODEC_V1);
            assert_eq!(got, 99);
        }
        other => panic!("unexpected error: {:?}", other),
    }
}
