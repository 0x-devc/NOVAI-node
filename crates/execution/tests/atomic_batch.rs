//! Atomic batch tests: prove that DB failures during batch don't cause partial state.

use novai_execution::{apply_tx_v1_transfer, encode_transfer_payload_v1, TransferPayloadV1};
use novai_state::{
    account_key, encode_account_v1, encode_fee_pool_v1, AccountStateV1, FeePoolV1, Kv, KvBatch,
    WriteOp, KEY_FEE_POOL,
};
use novai_types::{Address, TxV1, TxVersion};
use std::sync::{Arc, Mutex};

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

type KvEntries = Vec<(Vec<u8>, Vec<u8>)>;

/// A wrapper KV that fails on the Nth operation in a batch.
#[derive(Clone)]
struct FaultyKv {
    inner: Arc<Mutex<KvEntries>>,
    fail_on_op: usize,
    op_count: Arc<Mutex<usize>>,
}

impl FaultyKv {
    fn new(fail_on_op: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
            fail_on_op,
            op_count: Arc::new(Mutex::new(0)),
        }
    }

    fn snapshot(&self) -> KvEntries {
        self.inner.lock().unwrap().clone()
    }
}

impl Kv for FaultyKv {
    type Error = String;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        let entries = self.inner.lock().unwrap();
        Ok(entries
            .iter()
            .find(|(k, _)| k.as_slice() == key)
            .map(|(_, v)| v.clone()))
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Self::Error> {
        let mut entries = self.inner.lock().unwrap();
        if let Some(idx) = entries.iter().position(|(k, _)| k.as_slice() == key) {
            entries[idx].1 = value.to_vec();
        } else {
            entries.push((key.to_vec(), value.to_vec()));
        }
        Ok(())
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), Self::Error> {
        let mut entries = self.inner.lock().unwrap();
        if let Some(idx) = entries.iter().position(|(k, _)| k.as_slice() == key) {
            entries.swap_remove(idx);
        }
        Ok(())
    }
}

impl KvBatch for FaultyKv {
    fn apply_batch(&mut self, ops: &[WriteOp]) -> Result<(), Self::Error> {
        let mut tmp = self.inner.lock().unwrap().clone();

        for (i, op) in ops.iter().enumerate() {
            // Simulate failure on specific operation
            let mut count = self.op_count.lock().unwrap();
            *count += 1;
            if *count == self.fail_on_op {
                // FAIL HERE - batch should be aborted, no changes applied
                return Err(format!("Simulated DB failure on operation {}", i));
            }

            match op {
                WriteOp::Put(key, value) => {
                    if let Some(idx) = tmp.iter().position(|(k, _)| k.as_slice() == key.as_slice())
                    {
                        tmp[idx].1 = value.clone();
                    } else {
                        tmp.push((key.clone(), value.clone()));
                    }
                }
                WriteOp::Delete(key) => {
                    if let Some(idx) = tmp.iter().position(|(k, _)| k.as_slice() == key.as_slice())
                    {
                        tmp.swap_remove(idx);
                    }
                }
            }
        }

        // If we got here, all ops succeeded - commit
        *self.inner.lock().unwrap() = tmp;
        Ok(())
    }
}

#[test]
fn batch_failure_leaves_state_unchanged() {
    let from = addr(1);
    let to = addr(2);

    // Setup initial state
    let mut db = FaultyKv::new(2); // Fail on 2nd operation in batch

    // Setup accounts
    let from_acct = AccountStateV1 {
        balance: 1000,
        nonce: 0,
    };
    db.put(&account_key(&from), &encode_account_v1(&from_acct))
        .unwrap();

    let fee_pool = FeePoolV1 { balance: 0 };
    db.put(KEY_FEE_POOL, &encode_fee_pool_v1(&fee_pool))
        .unwrap();

    // Snapshot state BEFORE transaction
    let snapshot_before = db.snapshot();

    // Try to apply transaction (will fail mid-batch)
    let t = tx(from, 0, 5, to, 100);
    let result = apply_tx_v1_transfer(&mut db, &t);

    // Transaction should fail
    assert!(result.is_err(), "Transaction should fail due to DB error");

    // State should be UNCHANGED (atomicity proof)
    let snapshot_after = db.snapshot();
    assert_eq!(
        snapshot_before, snapshot_after,
        "State must be unchanged after batch failure"
    );
}

#[test]
fn batch_success_applies_all_changes() {
    let from = addr(1);
    let to = addr(2);

    // Setup with NO failure
    let mut db = FaultyKv::new(999); // Won't fail

    let from_acct = AccountStateV1 {
        balance: 1000,
        nonce: 0,
    };
    db.put(&account_key(&from), &encode_account_v1(&from_acct))
        .unwrap();

    let fee_pool = FeePoolV1 { balance: 0 };
    db.put(KEY_FEE_POOL, &encode_fee_pool_v1(&fee_pool))
        .unwrap();

    // Apply transaction (should succeed)
    let t = tx(from, 0, 5, to, 100);
    apply_tx_v1_transfer(&mut db, &t).unwrap();

    // Verify ALL changes applied
    let from_bytes = db.get(&account_key(&from)).unwrap().unwrap();
    let from_after = novai_state::decode_account_v1(&from_bytes).unwrap();
    assert_eq!(from_after.balance, 1000 - 100 - 5); // Debited
    assert_eq!(from_after.nonce, 1); // Incremented

    let to_bytes = db.get(&account_key(&to)).unwrap().unwrap();
    let to_after = novai_state::decode_account_v1(&to_bytes).unwrap();
    assert_eq!(to_after.balance, 100); // Credited

    let pool_bytes = db.get(KEY_FEE_POOL).unwrap().unwrap();
    let pool_after = novai_state::decode_fee_pool_v1(&pool_bytes).unwrap();
    assert_eq!(pool_after.balance, 5); // Fee collected
}
