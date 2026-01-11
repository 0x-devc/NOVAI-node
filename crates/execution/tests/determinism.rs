//! Determinism tests: prove execution is reproducible across "machines" (fresh state).

use novai_execution::{apply_tx_v1_transfer, encode_transfer_payload_v1, TransferPayloadV1};
use novai_state::{
    account_key, encode_account_v1, encode_fee_pool_v1, AccountStateV1, FeePoolV1, Kv, MemKv,
    KEY_FEE_POOL,
};
use novai_types::{Address, TxV1, TxVersion};

fn addr(b: u8) -> Address {
    [b; 32]
}

fn tx(from: Address, nonce: u64, fee: u64, to: Address, amount: u64) -> TxV1 {
    let payload = encode_transfer_payload_v1(&TransferPayloadV1 { to, amount }).to_vec();
    TxV1 {
        version: novai_types::TxVersion::V1,
        from,
        pubkey: from, // Execution doesn't verify, so reuse from as dummy pubkey
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    }
}

fn setup_db(db: &mut MemKv) {
    // Account 1: balance 1000, nonce 0
    let a1 = AccountStateV1 {
        balance: 1000,
        nonce: 0,
    };
    db.put(&account_key(&addr(1)), &encode_account_v1(&a1))
        .unwrap();

    // Account 2: balance 500, nonce 0
    let a2 = AccountStateV1 {
        balance: 500,
        nonce: 0,
    };
    db.put(&account_key(&addr(2)), &encode_account_v1(&a2))
        .unwrap();

    // Fee pool: 0
    let pool = FeePoolV1 { balance: 0 };
    db.put(KEY_FEE_POOL, &encode_fee_pool_v1(&pool)).unwrap();
}

fn apply_txs(db: &mut MemKv) {
    let txs = [
        tx(addr(1), 0, 5, addr(2), 100), // 1 -> 2: 100 (fee 5)
        tx(addr(2), 0, 3, addr(1), 50),  // 2 -> 1: 50 (fee 3)
        tx(addr(1), 1, 2, addr(2), 30),  // 1 -> 2: 30 (fee 2)
    ];

    for t in &txs {
        apply_tx_v1_transfer(db, t).unwrap();
    }
}

fn snapshot_all_keys(db: &MemKv) -> Vec<(Vec<u8>, Vec<u8>)> {
    // Collect all KV pairs in deterministic order
    let mut pairs = Vec::new();

    // Account 1
    if let Some(v) = db.get(&account_key(&addr(1))).unwrap() {
        pairs.push((account_key(&addr(1)), v));
    }

    // Account 2
    if let Some(v) = db.get(&account_key(&addr(2))).unwrap() {
        pairs.push((account_key(&addr(2)), v));
    }

    // Fee pool
    if let Some(v) = db.get(KEY_FEE_POOL).unwrap() {
        pairs.push((KEY_FEE_POOL.to_vec(), v));
    }

    pairs
}

#[test]
fn deterministic_execution_across_runs() {
    // Run 1: Apply txs to fresh state
    let snapshot1 = {
        let mut db = MemKv::new();
        setup_db(&mut db);
        apply_txs(&mut db);
        snapshot_all_keys(&db)
    };

    // Run 2: Apply same txs to fresh state
    let snapshot2 = {
        let mut db = MemKv::new();
        setup_db(&mut db);
        apply_txs(&mut db);
        snapshot_all_keys(&db)
    };

    // Run 3: Apply same txs to fresh state
    let snapshot3 = {
        let mut db = MemKv::new();
        setup_db(&mut db);
        apply_txs(&mut db);
        snapshot_all_keys(&db)
    };

    // All snapshots must be byte-for-byte identical
    assert_eq!(snapshot1, snapshot2, "Run 1 vs Run 2 mismatch");
    assert_eq!(snapshot2, snapshot3, "Run 2 vs Run 3 mismatch");
}
