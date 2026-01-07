use novai_execution::{apply_tx_v1_transfer, encode_transfer_payload_v1, TransferPayloadV1};
use novai_smt::smt::{MemoryStore, Smt};
use novai_state::{
    account_key, decode_account_v1, decode_smt_root_v1, encode_account_v1, encode_fee_pool_v1,
    smt_key_for_state_key, AccountStateV1, FeePoolV1, Kv, MemKv, KEY_FEE_POOL, KEY_SMT_ROOT,
};
use novai_types::{Address, TxV1, TxVersion};

fn read_smt_root_from_db(db: &MemKv) -> [u8; 32] {
    match db.get(KEY_SMT_ROOT).unwrap() {
        None => [0u8; 32],
        Some(b) => decode_smt_root_v1(&b).unwrap(),
    }
}

fn must_get(db: &MemKv, key: &[u8]) -> Vec<u8> {
    db.get(key).unwrap().expect("missing expected key in db")
}

fn mk_addr(b: u8) -> Address {
    [b; 32]
}

/// Constructs a structurally-valid TxV1 for execution tests.
/// Execution does NOT validate signatures (mempool does), so sig is zeroed.
fn mk_transfer_tx(from: Address, nonce: u64, fee: u64, to: Address, amount: u64) -> TxV1 {
    let payload = encode_transfer_payload_v1(&TransferPayloadV1 { to, amount }).to_vec();

    TxV1 {
        version: TxVersion::V1,
        from,
        nonce,
        fee,
        payload,
        sig: [0u8; 64], // execution doesn't check sigs; mempool does
    }
}

#[test]
fn smt_root_stored_matches_fresh_rebuild_from_state() {
    // --- Setup MemKv ---
    let mut db = MemKv::default();

    // --- Seed Alice balance + nonce (and fee pool) ---
    let alice = mk_addr(0xA1);
    let bob = mk_addr(0xB2);

    let alice_state = AccountStateV1 {
        balance: 1_000u128,
        nonce: 0,
    };
    db.put(&account_key(&alice), &encode_account_v1(&alice_state))
        .unwrap();

    // Seed fee pool explicitly (optional, but makes the rebuild set explicit/stable).
    let fee_pool = FeePoolV1 { balance: 0 };
    db.put(KEY_FEE_POOL, &encode_fee_pool_v1(&fee_pool))
        .unwrap();

    // --- Apply apply_tx_v1_transfer ---
    let tx = mk_transfer_tx(alice, 0, 1, bob, 10);
    apply_tx_v1_transfer(&mut db, &tx).unwrap();

    // --- Read stored root from DB (KEY_SMT_ROOT -> decode_smt_root_v1) ---
    let stored_root = read_smt_root_from_db(&db);
    assert_ne!(stored_root, [0u8; 32], "expected smt/root to be written");

    // --- Rebuild fresh SMT from scratch using ONLY canonical state records ---
    //
    // IMPORTANT: We only insert the *state* keys (accounts + fee pool),
    // NOT smt/node/* and NOT smt/root itself, otherwise you'd hash the index into itself.
    let mut fresh = Smt::new(MemoryStore::default());

    let keys: Vec<Vec<u8>> = vec![
        account_key(&alice),
        account_key(&bob),
        KEY_FEE_POOL.to_vec(),
    ];

    for k in keys {
        let v = must_get(&db, &k).to_vec(); // explicit clone for clarity/determinism
        let k32 = smt_key_for_state_key(&k);
        fresh.update(k32, &v).unwrap();
    }

    // --- Compare fresh root to stored root ---
    assert_eq!(
        fresh.root(),
        stored_root,
        "stored SMT root must equal a clean rebuild from state keys"
    );

    // (Optional sanity) Ensure bob exists and decoded properly (makes failure mode obvious).
    let bob_bytes = must_get(&db, &account_key(&bob));
    let _bob_state = decode_account_v1(&bob_bytes).unwrap();
}
