use novai_execution::{
    apply_tx_v1_transfer, encode_transfer_payload_v1, ExecError, TransferPayloadV1,
};
use novai_state::{
    account_key, decode_account_v1, decode_fee_pool_v1, encode_account_v1, encode_fee_pool_v1,
    AccountStateV1, FeePoolV1, Kv, MemKv, KEY_FEE_POOL,
};
use novai_types::{Address, TxV1, TxVersion};

fn addr(b: u8) -> Address {
    [b; 32]
}

fn tx(from: Address, nonce: u64, fee: u64, to: Address, amount: u64) -> TxV1 {
    let payload = encode_transfer_payload_v1(&TransferPayloadV1 { to, amount }).to_vec();
    TxV1 {
        version: TxVersion::V1,
        from,
        nonce,
        fee,
        payload,
        sig: [0u8; 64], // execution does not verify sig in Week 3
    }
}

fn put_account(db: &mut MemKv, who: &Address, balance: u128, nonce: u64) {
    let s = AccountStateV1 { balance, nonce };
    db.put(&account_key(who), &encode_account_v1(&s)).unwrap();
}

fn put_fee_pool(db: &mut MemKv, balance: u128) {
    let p = FeePoolV1 { balance };
    db.put(KEY_FEE_POOL, &encode_fee_pool_v1(&p)).unwrap();
}

fn get_account(db: &MemKv, who: &Address) -> AccountStateV1 {
    let bytes = db.get(&account_key(who)).unwrap().unwrap();
    decode_account_v1(&bytes).unwrap()
}

fn get_fee_pool(db: &MemKv) -> FeePoolV1 {
    let bytes = db.get(KEY_FEE_POOL).unwrap().unwrap();
    decode_fee_pool_v1(&bytes).unwrap()
}

#[test]
fn nonce_is_monotonic_for_successful_transfers() {
    let from = addr(1);
    let to = addr(2);

    let mut db = MemKv::new();
    put_account(&mut db, &from, 1_000, 0);
    put_fee_pool(&mut db, 0);

    // Apply 3 successful txs in sequence; nonce should strictly increase by 1 each time.
    let txs = [
        tx(from, 0, 1, to, 10),
        tx(from, 1, 1, to, 10),
        tx(from, 2, 1, to, 10),
    ];

    for (i, t) in txs.iter().enumerate() {
        apply_tx_v1_transfer(&mut db, t).unwrap();
        let a = get_account(&db, &from);
        assert_eq!(a.nonce, (i as u64) + 1);
    }
}

#[test]
fn failed_nonce_mismatch_does_not_mutate_state() {
    let from = addr(1);
    let to = addr(2);

    let mut db = MemKv::new();
    put_account(&mut db, &from, 500, 5);
    put_fee_pool(&mut db, 7);

    // Snapshot canonical bytes.
    let k_from = account_key(&from);
    let k_to = account_key(&to);

    let before_from = db.get(&k_from).unwrap();
    let before_to = db.get(&k_to).unwrap(); // may be None
    let before_pool = db.get(KEY_FEE_POOL).unwrap();

    let bad = tx(from, 4, 1, to, 1); // nonce mismatch (expected 5)
    let err = apply_tx_v1_transfer(&mut db, &bad).unwrap_err();
    assert!(matches!(
        err,
        ExecError::NonceMismatch {
            expected: 5,
            got: 4
        }
    ));

    // State must be identical after rejection.
    assert_eq!(db.get(&k_from).unwrap(), before_from);
    assert_eq!(db.get(&k_to).unwrap(), before_to);
    assert_eq!(db.get(KEY_FEE_POOL).unwrap(), before_pool);
}

#[test]
fn failed_insufficient_funds_does_not_mutate_state() {
    let from = addr(1);
    let to = addr(2);

    let mut db = MemKv::new();
    put_account(&mut db, &from, 10, 0);
    put_fee_pool(&mut db, 0);

    let k_from = account_key(&from);
    let k_to = account_key(&to);

    let before_from = db.get(&k_from).unwrap();
    let before_to = db.get(&k_to).unwrap();
    let before_pool = db.get(KEY_FEE_POOL).unwrap();

    // Needs 11, has 10.
    let bad = tx(from, 0, 9, to, 2);
    let err = apply_tx_v1_transfer(&mut db, &bad).unwrap_err();
    assert!(matches!(
        err,
        ExecError::InsufficientFunds {
            balance: 10,
            needed: 11
        }
    ));

    assert_eq!(db.get(&k_from).unwrap(), before_from);
    assert_eq!(db.get(&k_to).unwrap(), before_to);
    assert_eq!(db.get(KEY_FEE_POOL).unwrap(), before_pool);
}

#[test]
fn exact_balance_amount_plus_fee_succeeds_and_never_underflows() {
    let from = addr(1);
    let to = addr(2);

    let mut db = MemKv::new();
    // Exactly enough: amount 7 + fee 3 = 10
    put_account(&mut db, &from, 10, 0);
    put_fee_pool(&mut db, 0);

    let t = tx(from, 0, 3, to, 7);
    apply_tx_v1_transfer(&mut db, &t).unwrap();

    let from_after = get_account(&db, &from);
    let to_after = get_account(&db, &to);
    let pool_after = get_fee_pool(&db);

    assert_eq!(from_after.balance, 0);
    assert_eq!(from_after.nonce, 1);
    assert_eq!(to_after.balance, 7);
    assert_eq!(pool_after.balance, 3);
}

#[test]
fn below_exact_balance_fails_and_state_is_unchanged() {
    let from = addr(1);
    let to = addr(2);

    let mut db = MemKv::new();
    // One short: needs 10 but has 9
    put_account(&mut db, &from, 9, 0);
    put_fee_pool(&mut db, 0);

    let k_from = account_key(&from);
    let k_to = account_key(&to);

    let before_from = db.get(&k_from).unwrap();
    let before_to = db.get(&k_to).unwrap();
    let before_pool = db.get(KEY_FEE_POOL).unwrap();

    let bad = tx(from, 0, 3, to, 7);
    let err = apply_tx_v1_transfer(&mut db, &bad).unwrap_err();
    assert!(matches!(
        err,
        ExecError::InsufficientFunds {
            balance: 9,
            needed: 10
        }
    ));

    assert_eq!(db.get(&k_from).unwrap(), before_from);
    assert_eq!(db.get(&k_to).unwrap(), before_to);
    assert_eq!(db.get(KEY_FEE_POOL).unwrap(), before_pool);
}
