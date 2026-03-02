use mempool::{NonceProvider, TxMempool};
use novai_codec::txid_v1;
use novai_crypto::{address_from_pubkey, sign_tx_v1};
use novai_types::{Address, TxV1, TxVersion};

use ed25519_dalek::{SigningKey, VerifyingKey};

#[derive(Default)]
struct TestNonceProvider {
    map: std::collections::HashMap<Address, u64>,
}

impl TestNonceProvider {
    fn set(&mut self, from: Address, nonce: u64) {
        self.map.insert(from, nonce);
    }
}

impl NonceProvider for TestNonceProvider {
    fn expected_nonce(&self, from: &Address) -> u64 {
        *self.map.get(from).unwrap_or(&0)
    }
}

fn test_keypair(seed: u8) -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let vk: VerifyingKey = sk.verifying_key();
    (sk, vk)
}

fn make_signed_tx(
    from_sk: &SigningKey,
    from_vk: &VerifyingKey,
    nonce: u64,
    fee: u64,
    payload: &[u8],
) -> TxV1 {
    let from_addr = address_from_pubkey(from_vk);

    let mut tx = TxV1 {
        version: TxVersion::V1,
        from: from_addr,
        pubkey: from_vk.to_bytes(),
        nonce,
        fee,
        payload: payload.to_vec(),
        sig: [0u8; 64],
    };

    sign_tx_v1(from_sk, &mut tx).expect("sign_tx_v1");
    tx
}

#[test]
fn week2_submit_tx_inserts_and_returns_txid() {
    let (sk, vk) = test_keypair(42);
    let from: Address = address_from_pubkey(&vk);

    let mut np = TestNonceProvider::default();
    np.set(from, 0);

    let mut mp = TxMempool::new(1, 1000);
    let tx = make_signed_tx(&sk, &vk, 0, 5, b"hello");

    let id = mp.insert(tx.clone(), &np).expect("insert ok");
    assert!(mp.contains(&id));
    assert_eq!(mp.len(), 1);

    // txid should match deterministic txid_v1() helper
    let expected = txid_v1(&tx).expect("txid");
    assert_eq!(id, expected);
}

#[test]
fn week2_drain_mempool_is_fee_priority_deterministic() {
    let (sk, vk) = test_keypair(7);
    let from: Address = address_from_pubkey(&vk);

    let mut np = TestNonceProvider::default();
    np.set(from, 0);

    // fairness cap large so it doesn't interfere with ordering
    let mut mp = TxMempool::new(1, 1000);

    // Same nonce (ready), different fees and payloads -> should drain by fee DESC then txid ASC.
    let tx_low = make_signed_tx(&sk, &vk, 0, 1, b"a");
    let tx_mid = make_signed_tx(&sk, &vk, 0, 2, b"b");
    let tx_high = make_signed_tx(&sk, &vk, 0, 3, b"c");

    let id_low = mp.insert(tx_low.clone(), &np).expect("insert low");
    let id_mid = mp.insert(tx_mid.clone(), &np).expect("insert mid");
    let id_high = mp.insert(tx_high.clone(), &np).expect("insert high");

    let drained = mp.drain_ready(10, &np);
    assert_eq!(drained.len(), 3);

    // Primary ordering: by fee descending
    let fees: Vec<u64> = drained.iter().map(|t| t.fee).collect();
    assert_eq!(fees, vec![3, 2, 1]);

    // And ids correspond (not strictly required, but nice sanity check)
    let drained_ids: Vec<[u8; 32]> = drained.iter().map(|t| txid_v1(t).expect("txid")).collect();
    assert_eq!(drained_ids, vec![id_high, id_mid, id_low]);
}
