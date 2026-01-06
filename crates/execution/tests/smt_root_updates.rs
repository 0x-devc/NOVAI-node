use novai_execution::{apply_tx_v1_transfer, encode_transfer_payload_v1, TransferPayloadV1};
use novai_smt::hash::empty_hash_at_height;
use novai_state::{
    account_key, decode_account_v1, decode_smt_root_v1, encode_account_v1, AccountStateV1, Kv,
    MemKv, KEY_SMT_ROOT,
};
use novai_types::{TxV1, TxVersion};

fn mk_tx(from: [u8; 32], nonce: u64, fee: u64, to: [u8; 32], amount: u64) -> TxV1 {
    let payload = encode_transfer_payload_v1(&TransferPayloadV1 { to, amount }).to_vec();
    TxV1 {
        version: TxVersion::V1,
        from,
        nonce,
        fee,
        payload,
        sig: [0u8; 64], // execution doesn't validate sig (crypto does)
    }
}

fn read_root_or_empty(db: &MemKv) -> [u8; 32] {
    match db.get(KEY_SMT_ROOT).unwrap() {
        None => empty_hash_at_height(256),
        Some(b) => decode_smt_root_v1(&b).unwrap(),
    }
}

#[test]
fn smt_root_is_written_on_success() {
    let mut db = MemKv::default();

    let alice = [0xA1u8; 32];
    let bob = [0xB2u8; 32];

    // Seed Alice balance/nonce so tx succeeds.
    db.put(
        &account_key(&alice),
        &encode_account_v1(&AccountStateV1 {
            balance: 1_000,
            nonce: 0,
        }),
    )
    .unwrap();

    let root_before = read_root_or_empty(&db);
    assert_eq!(root_before, empty_hash_at_height(256));

    let tx = mk_tx(alice, 0, 1, bob, 10);
    apply_tx_v1_transfer(&mut db, &tx).unwrap();

    let root_after = read_root_or_empty(&db);
    assert_ne!(root_after, root_before);

    // Sanity: state actually changed
    let a_bytes = db.get(&account_key(&alice)).unwrap().unwrap();
    let a = decode_account_v1(&a_bytes).unwrap();
    assert_eq!(a.nonce, 1);
}

#[test]
fn smt_root_does_not_change_on_failed_tx() {
    let mut db = MemKv::default();

    let alice = [0xA1u8; 32];
    let bob = [0xB2u8; 32];

    // Seed Alice with too little balance -> tx fails.
    db.put(
        &account_key(&alice),
        &encode_account_v1(&AccountStateV1 {
            balance: 0,
            nonce: 0,
        }),
    )
    .unwrap();

    let root_before = read_root_or_empty(&db);

    let tx = mk_tx(alice, 0, 1, bob, 10);
    assert!(apply_tx_v1_transfer(&mut db, &tx).is_err());

    let root_after = read_root_or_empty(&db);
    assert_eq!(root_after, root_before);
}
