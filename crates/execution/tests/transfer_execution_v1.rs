use novai_execution::{
    apply_tx_v1_transfer, encode_transfer_payload_v1, ExecError, TransferPayloadV1,
};
use novai_state::{
    encode_account_v1, encode_fee_pool_v1, AccountStateV1, FeePoolV1, Kv, MemKv, KEY_FEE_POOL,
};
use novai_types::{Address, TxV1};

const fn addr(b: u8) -> Address {
    [b; 32]
}

/// Pre-create an account in the DB to satisfy M-06 minimum balance requirement.
fn seed_recipient(db: &mut MemKv, who: &Address) {
    let s = AccountStateV1 {
        balance: 10_000,
        nonce: 0,
    };
    db.put(&novai_state::account_key(who), &encode_account_v1(&s))
        .unwrap();
}

fn tx(from: Address, nonce: u64, fee: u64, to: Address, amount: u64) -> TxV1 {
    let payload = encode_transfer_payload_v1(&TransferPayloadV1 { to, amount }).to_vec();
    TxV1 {
        version: novai_types::TxVersion::V1,
        from,
        pubkey: from,
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    }
}

#[test]
fn transfer_happy_path_updates_balances_nonce_and_fee_pool() {
    let from = addr(1);
    let to = addr(2);

    let mut db = MemKv::new();

    // initial state
    let from_state = AccountStateV1 {
        balance: 1000,
        nonce: 0,
    };
    db.put(
        &novai_state::account_key(&from),
        &encode_account_v1(&from_state),
    )
    .unwrap();

    let fee_pool = FeePoolV1 { balance: 0 };
    db.put(KEY_FEE_POOL, &encode_fee_pool_v1(&fee_pool))
        .unwrap();
    seed_recipient(&mut db, &to);

    let t = tx(from, 0, 7, to, 100);
    apply_tx_v1_transfer(&mut db, &t).unwrap();

    let from_bytes = db
        .get(&novai_state::account_key(&addr(1)))
        .unwrap()
        .unwrap();
    let to_bytes = db
        .get(&novai_state::account_key(&addr(2)))
        .unwrap()
        .unwrap();
    let pool_bytes = db.get(KEY_FEE_POOL).unwrap().unwrap();

    let from_after = novai_state::decode_account_v1(&from_bytes).unwrap();
    let to_after = novai_state::decode_account_v1(&to_bytes).unwrap();
    let pool_after = novai_state::decode_fee_pool_v1(&pool_bytes).unwrap();

    assert_eq!(from_after.nonce, 1);
    assert_eq!(from_after.balance, 1000 - 100 - 7);
    assert_eq!(to_after.balance, 10_000 + 100); // Pre-seeded with 10_000 (M-06)
    assert_eq!(to_after.nonce, 0);
    assert_eq!(pool_after.balance, 7);
}

#[test]
fn nonce_must_match_exactly() {
    let from = addr(1);
    let to = addr(2);

    let mut db = MemKv::new();
    let from_state = AccountStateV1 {
        balance: 1000,
        nonce: 5,
    };
    db.put(
        &novai_state::account_key(&from),
        &encode_account_v1(&from_state),
    )
    .unwrap();
    seed_recipient(&mut db, &to);

    let t = tx(from, 4, 1, to, 1);
    let err = apply_tx_v1_transfer(&mut db, &t).unwrap_err();
    assert!(matches!(
        err,
        ExecError::NonceMismatch {
            expected: 5,
            got: 4
        }
    ));
}

#[test]
fn balance_must_cover_amount_plus_fee() {
    let from = addr(1);
    let to = addr(2);

    let mut db = MemKv::new();
    let from_state = AccountStateV1 {
        balance: 10,
        nonce: 0,
    };
    db.put(
        &novai_state::account_key(&from),
        &encode_account_v1(&from_state),
    )
    .unwrap();
    seed_recipient(&mut db, &to);

    let t = tx(from, 0, 9, to, 2); // needs 11
    let err = apply_tx_v1_transfer(&mut db, &t).unwrap_err();
    assert!(matches!(
        err,
        ExecError::InsufficientFunds {
            balance: 10,
            needed: 11
        }
    ));
}

#[test]
fn overflow_is_rejected_deterministically() {
    let from = addr(1);
    let to = addr(2);

    let mut db = MemKv::new();

    // from has max balance so subtraction itself won't overflow, but receiver add will.
    let from_state = AccountStateV1 {
        balance: u128::MAX,
        nonce: 0,
    };
    let to_state = AccountStateV1 {
        balance: u128::MAX,
        nonce: 0,
    };
    db.put(
        &novai_state::account_key(&from),
        &encode_account_v1(&from_state),
    )
    .unwrap();
    db.put(
        &novai_state::account_key(&to),
        &encode_account_v1(&to_state),
    )
    .unwrap();

    let t = tx(from, 0, 0, to, 1); // credit would overflow u128::MAX + 1
    let err = apply_tx_v1_transfer(&mut db, &t).unwrap_err();
    assert!(matches!(err, ExecError::Overflow));
}

#[test]
fn determinism_same_initial_state_same_txs_same_final_state() {
    let from = addr(1);
    let to = addr(2);

    let mut db1 = MemKv::new();
    let mut db2 = MemKv::new();

    let from_state = AccountStateV1 {
        balance: 500,
        nonce: 0,
    };
    db1.put(
        &novai_state::account_key(&from),
        &encode_account_v1(&from_state),
    )
    .unwrap();
    db2.put(
        &novai_state::account_key(&from),
        &encode_account_v1(&from_state),
    )
    .unwrap();
    seed_recipient(&mut db1, &to);
    seed_recipient(&mut db2, &to);

    let txs = vec![
        tx(from, 0, 1, to, 10),
        tx(from, 1, 2, to, 20),
        tx(from, 2, 3, to, 30),
    ];

    for t in &txs {
        apply_tx_v1_transfer(&mut db1, t).unwrap();
        apply_tx_v1_transfer(&mut db2, t).unwrap();
    }

    // Compare canonical bytes for keys we care about.
    let k_from = novai_state::account_key(&from);
    let k_to = novai_state::account_key(&to);

    assert_eq!(db1.get(&k_from).unwrap(), db2.get(&k_from).unwrap());
    assert_eq!(db1.get(&k_to).unwrap(), db2.get(&k_to).unwrap());
    assert_eq!(
        db1.get(KEY_FEE_POOL).unwrap(),
        db2.get(KEY_FEE_POOL).unwrap()
    );
}
