//! Fee pool accumulation tests.

use novai_execution::{apply_tx_v1_transfer, encode_transfer_payload_v1, TransferPayloadV1};
use novai_state::{
    account_key, decode_fee_pool_v1, encode_account_v1, encode_fee_pool_v1, AccountStateV1,
    FeePoolV1, Kv, MemKv, KEY_FEE_POOL,
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
        sig: [0u8; 64],
    }
}

#[test]
fn fee_pool_accumulates_correctly() {
    let mut db = MemKv::new();

    // Setup 3 accounts, each with balance 1000, nonce 0
    for i in 1..=3 {
        let a = AccountStateV1 {
            balance: 1000,
            nonce: 0,
        };
        db.put(&account_key(&addr(i)), &encode_account_v1(&a))
            .unwrap();
    }

    // Fee pool starts at 0
    let pool = FeePoolV1 { balance: 0 };
    db.put(KEY_FEE_POOL, &encode_fee_pool_v1(&pool)).unwrap();

    // Each account sends 1 tx with different fees
    let txs = [
        tx(addr(1), 0, 5, addr(99), 10), // fee = 5
        tx(addr(2), 0, 7, addr(99), 10), // fee = 7
        tx(addr(3), 0, 3, addr(99), 10), // fee = 3
    ];

    for t in &txs {
        apply_tx_v1_transfer(&mut db, t).unwrap();
    }

    // Fee pool should have accumulated: 5 + 7 + 3 = 15
    let pool_bytes = db.get(KEY_FEE_POOL).unwrap().unwrap();
    let pool = decode_fee_pool_v1(&pool_bytes).unwrap();

    assert_eq!(pool.balance, 15, "Fee pool should accumulate all fees");
}

#[test]
fn fee_pool_starts_from_nonzero() {
    let mut db = MemKv::new();

    // Account with balance 100
    let a = AccountStateV1 {
        balance: 100,
        nonce: 0,
    };
    db.put(&account_key(&addr(1)), &encode_account_v1(&a))
        .unwrap();

    // Fee pool starts at 50
    let pool = FeePoolV1 { balance: 50 };
    db.put(KEY_FEE_POOL, &encode_fee_pool_v1(&pool)).unwrap();

    // Send tx with fee 10
    let t = tx(addr(1), 0, 10, addr(2), 20);
    apply_tx_v1_transfer(&mut db, &t).unwrap();

    // Fee pool should now be 50 + 10 = 60
    let pool_bytes = db.get(KEY_FEE_POOL).unwrap().unwrap();
    let pool = decode_fee_pool_v1(&pool_bytes).unwrap();

    assert_eq!(
        pool.balance, 60,
        "Fee pool should accumulate on top of existing balance"
    );
}
