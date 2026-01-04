use novai_state::{
    decode_account_v1, decode_fee_pool_v1, encode_account_v1, encode_fee_pool_v1, AccountStateV1,
    FeePoolV1, STATE_CODEC_V1,
};

#[test]
fn account_v1_roundtrip_is_exact() {
    let a = AccountStateV1 {
        balance: 123456789012345678901234567890u128,
        nonce: 42u64,
    };
    let enc = encode_account_v1(&a);
    assert_eq!(enc[0], STATE_CODEC_V1);
    let dec = decode_account_v1(&enc).expect("decode");
    assert_eq!(dec, a);
}

#[test]
fn fee_pool_v1_roundtrip_is_exact() {
    let p = FeePoolV1 { balance: 7u128 };
    let enc = encode_fee_pool_v1(&p);
    assert_eq!(enc[0], STATE_CODEC_V1);
    let dec = decode_fee_pool_v1(&enc).expect("decode");
    assert_eq!(dec, p);
}

#[test]
fn decode_rejects_wrong_length() {
    let bytes = vec![STATE_CODEC_V1, 0, 1, 2];
    let err = decode_fee_pool_v1(&bytes).unwrap_err();
    assert!(matches!(
        err,
        novai_state::StateDecodeError::BadLength { .. }
    ));
}

#[test]
fn decode_rejects_wrong_version() {
    let a = AccountStateV1 {
        balance: 1,
        nonce: 2,
    };
    let mut enc = encode_account_v1(&a);
    enc[0] = 9;
    let err = decode_account_v1(&enc).unwrap_err();
    assert!(matches!(
        err,
        novai_state::StateDecodeError::BadVersion { .. }
    ));
}
