use novai_execution::{apply_tx_v1_transfer, encode_transfer_payload_v1, TransferPayloadV1};
use novai_smt::smt::{MemoryStore, Smt};
use novai_state::{
    account_key, decode_account_v1, decode_smt_root_v1, encode_account_v1, smt_key_for_state_key,
    AccountStateV1, Kv, MemKv, KEY_FEE_POOL, KEY_SMT_ROOT,
};
use novai_types::{Address, TxV1, TxVersion};

fn mk_addr(b: u8) -> Address {
    [b; 32]
}

fn mk_tx(from: Address, nonce: u64, fee: u64, to: Address, amount: u64) -> TxV1 {
    let payload = encode_transfer_payload_v1(&TransferPayloadV1 { to, amount }).to_vec();

    TxV1 {
        version: TxVersion::V1,
        from,
        nonce,
        fee,
        payload,
        // execution doesn't verify signatures (mempool/crypto does),
        // but TxV1 requires a 64-byte signature field.
        sig: [0u8; 64],
    }
}

#[test]
fn smt_root_matches_fresh_recompute_from_state() {
    let mut db = MemKv::default();

    let alice = mk_addr(0xA1);
    let bob = mk_addr(0xB2);

    // Seed Alice so the tx succeeds.
    db.put(
        &account_key(&alice),
        &encode_account_v1(&AccountStateV1 {
            balance: 1_000,
            nonce: 0,
        }),
    )
    .unwrap();

    // Apply tx (writes state + SMT nodes + SMT root atomically).
    let tx = mk_tx(alice, 0, 1, bob, 10);
    apply_tx_v1_transfer(&mut db, &tx).unwrap();

    // Read stored SMT root from DB.
    let stored_root = match db.get(KEY_SMT_ROOT).unwrap() {
        None => panic!("expected KEY_SMT_ROOT to be written"),
        Some(bytes) => decode_smt_root_v1(&bytes).unwrap(),
    };

    // Recompute root from scratch from the state keys we expect to exist.
    let mut smt = Smt::new(MemoryStore::default());

    // Alice account must exist.
    let alice_k = account_key(&alice);
    let alice_v = db.get(&alice_k).unwrap().unwrap();
    let _ = decode_account_v1(&alice_v).unwrap();
    smt.update(smt_key_for_state_key(&alice_k), &alice_v)
        .unwrap();

    // Bob account must exist.
    let bob_k = account_key(&bob);
    let bob_v = db.get(&bob_k).unwrap().unwrap();
    let _ = decode_account_v1(&bob_v).unwrap();
    smt.update(smt_key_for_state_key(&bob_k), &bob_v).unwrap();

    // Fee pool must exist (execution writes it on success).
    let fee_k = KEY_FEE_POOL.to_vec();
    let fee_v = db.get(&fee_k).unwrap().unwrap();
    smt.update(smt_key_for_state_key(&fee_k), &fee_v).unwrap();

    let recomputed_root = smt.root();

    assert_eq!(
        stored_root, recomputed_root,
        "stored SMT root must match fresh recomputation from state"
    );
}
