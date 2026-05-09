#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Integration tests for the AI Signal Marketplace.
//!
//! Covers:
//! - Happy path: buyer pays seller, treasury accrues 2 percent fee.
//! - Seller-side failures: not found, deactivated.
//! - Catalog-side failures: missing catalog, missing offering, inactive offering.
//! - Buyer-side failures: price ceiling exceeded, insufficient balance.
//! - Self-purchase prohibition.
//! - total_transactions bumped on both buyer and seller.
//! - Catalog binary roundtrip and 10-offering capacity.
//! - Free signals (price = 0) succeed without touching the treasury.
//! - Existing non-purchase signals continue to work (regression).
//! - SignalPurchase payload byte-length is 107 with stable field offsets.

use novai_ai_entities::{
    encode_memory_object_v1, AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObject,
    MemoryObjectType, SignalCatalogData, SignalCatalogEntry, MAX_CATALOG_OFFERINGS,
    SIGNAL_CATALOG_ENTRY_SIZE,
};
use novai_execution::{
    apply_signal_commitment_tx, encode_signal_commitment_payload_v1, read_ai_entity,
    write_ai_entity_op, ExecError, SignalCommitmentPayloadV1, SignalPurchaseExtraV1,
    BPS_DENOMINATOR, KEY_MARKETPLACE_TREASURY, MARKETPLACE_FEE_BPS,
    SIGNAL_COMMITMENT_PAYLOAD_V1_PURCHASE_LEN,
};
use novai_state::{
    ai_entity_by_address_key, ai_memory_by_type_key, ai_memory_object_key, decode_fee_pool_v1, Kv,
    KvBatch, MemKv, WriteOp,
};
use novai_types::{TxV1, TxVersion};

const BUYER_BALANCE: u128 = 1_000_000;
const SELLER_BALANCE: u128 = 100;
const SIGNAL_FEE: u64 = 1_000;
const PURCHASE_HEIGHT: u64 = 100;

// ============================================================================
// Helpers
// ============================================================================

fn marketplace_caps() -> Capabilities {
    // emit_proposals is the dispatch gate for SignalCommitment, which carries
    // SignalPurchase. read_memory_objects keeps the role plausible for entities
    // that also browse offerings; nothing in the apply path requires it.
    Capabilities {
        read_public_chain: true,
        read_memory_objects: true,
        emit_proposals: true,
        request_execution: false,
        read_nnpx_derived: false,
        submit_reputation_updates: false,
        _reserved: [false; 2],
    }
}

fn build_entity(code_hash: [u8; 32], creator: [u8; 32], caps: Capabilities) -> AiEntity {
    AiEntity::new(code_hash, creator, AutonomyMode::Gated, caps, 1000)
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[
        write_ai_entity_op(entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
}

fn make_buyer(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut buyer = build_entity(code_hash, creator, marketplace_caps());
    buyer.economic_balance = BUYER_BALANCE;
    store_entity(db, &buyer);
    buyer
}

fn make_seller(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut seller = build_entity(code_hash, creator, marketplace_caps());
    seller.economic_balance = SELLER_BALANCE;
    store_entity(db, &seller);
    seller
}

/// Persist a SignalCatalog memory object directly to state, skipping the
/// CreateMemoryObject tx flow. This is enough for the apply branch because
/// `get_memory_objects_by_entity_and_type` only reads the object record and
/// the `ai/memory_by_type` presence index.
fn seed_catalog(db: &mut MemKv, seller: &AiEntity, catalog: &SignalCatalogData) -> [u8; 32] {
    let data = catalog.encode();
    let obj = MemoryObject::new(
        seller.id,
        MemoryObjectType::SignalCatalog,
        PURCHASE_HEIGHT - 1,
        data,
    );
    let object_id = obj.object_id;
    let encoded = encode_memory_object_v1(&obj);

    db.apply_batch(&[
        WriteOp::Put(ai_memory_object_key(&seller.id, &object_id), encoded),
        WriteOp::Put(
            ai_memory_by_type_key(
                MemoryObjectType::SignalCatalog.to_byte(),
                &seller.id,
                &object_id,
            ),
            Vec::new(),
        ),
    ])
    .unwrap();

    object_id
}

fn build_purchase_payload(
    buyer: [u8; 32],
    seller: [u8; 32],
    purchased_signal_type: u8,
    max_price: u64,
) -> Vec<u8> {
    encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xCDu8; 32],
        signal_type: AiSignalType::SignalPurchase,
        issuer_entity_id: buyer,
        reputation: None,
        purchase: Some(SignalPurchaseExtraV1 {
            seller_entity_id: seller,
            purchased_signal_type,
            max_price,
        }),
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
    })
}

fn make_tx(from: [u8; 32], nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from,
        pubkey: from,
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    }
}

fn read_treasury(db: &MemKv) -> u128 {
    db.get(KEY_MARKETPLACE_TREASURY)
        .unwrap()
        .map_or(0, |bytes| decode_fee_pool_v1(&bytes).unwrap().balance)
}

fn single_offering_catalog(signal_type: u8, price: u64, is_active: bool) -> SignalCatalogData {
    SignalCatalogData {
        entries: vec![SignalCatalogEntry {
            signal_type,
            price_per_signal: price,
            is_active,
        }],
    }
}

// ============================================================================
// 1. Happy path
// ============================================================================

#[test]
fn purchase_signal_basic() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let price: u64 = 10_000;
    let cat = single_offering_catalog(AiSignalType::Anomaly.to_byte(), price, true);
    seed_catalog(&mut db, &seller, &cat);

    let payload =
        build_purchase_payload(buyer.id, seller.id, AiSignalType::Anomaly.to_byte(), price);
    let tx = make_tx(buyer.id, 0, SIGNAL_FEE, payload);

    apply_signal_commitment_tx(&mut db, &tx, PURCHASE_HEIGHT).expect("purchase succeeds");

    let buyer_after = read_ai_entity(&db, &buyer.id).unwrap().unwrap();
    let seller_after = read_ai_entity(&db, &seller.id).unwrap().unwrap();

    let expected_fee = u128::from(price) * MARKETPLACE_FEE_BPS / BPS_DENOMINATOR;
    let total_debit = u128::from(price) + expected_fee + u128::from(SIGNAL_FEE);

    assert_eq!(
        buyer_after.economic_balance,
        BUYER_BALANCE - total_debit,
        "buyer pays price + service_fee + tx_fee"
    );
    assert_eq!(
        seller_after.economic_balance,
        SELLER_BALANCE + u128::from(price),
        "seller receives full price (no fee withheld from seller)"
    );
    assert_eq!(
        read_treasury(&db),
        expected_fee,
        "treasury accrues 2 percent"
    );
}

// ============================================================================
// 2. Seller not found
// ============================================================================

#[test]
fn purchase_rejected_seller_not_found() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    // No seller registered.

    let bogus_seller = [0xDEu8; 32];
    let payload =
        build_purchase_payload(buyer.id, bogus_seller, AiSignalType::Anomaly.to_byte(), 100);
    let tx = make_tx(buyer.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, PURCHASE_HEIGHT).expect_err("must fail");
    assert!(
        matches!(err, ExecError::SellerEntityNotFound),
        "got {err:?}"
    );

    let buyer_after = read_ai_entity(&db, &buyer.id).unwrap().unwrap();
    assert_eq!(
        buyer_after.economic_balance, BUYER_BALANCE,
        "rejected purchase must not move balance"
    );
}

// ============================================================================
// 3. Seller inactive (is_active = false)
// ============================================================================

#[test]
fn purchase_rejected_seller_inactive() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let mut seller = build_entity([0x12u8; 32], [0x22u8; 32], marketplace_caps());
    seller.economic_balance = SELLER_BALANCE;
    seller.is_active = false;
    store_entity(&mut db, &seller);

    let cat = single_offering_catalog(AiSignalType::Anomaly.to_byte(), 100, true);
    seed_catalog(&mut db, &seller, &cat);

    let payload = build_purchase_payload(buyer.id, seller.id, AiSignalType::Anomaly.to_byte(), 100);
    let tx = make_tx(buyer.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, PURCHASE_HEIGHT).expect_err("must fail");
    assert!(
        matches!(err, ExecError::SellerEntityNotActive),
        "got {err:?}"
    );
}

// ============================================================================
// 4. No catalog
// ============================================================================

#[test]
fn purchase_rejected_no_catalog() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32]);
    // No catalog seeded.

    let payload = build_purchase_payload(buyer.id, seller.id, AiSignalType::Anomaly.to_byte(), 100);
    let tx = make_tx(buyer.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, PURCHASE_HEIGHT).expect_err("must fail");
    assert!(
        matches!(err, ExecError::SignalCatalogNotFound),
        "got {err:?}"
    );
}

// ============================================================================
// 5. Offering not found in catalog
// ============================================================================

#[test]
fn purchase_rejected_offering_not_found() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32]);

    // Seller offers Anomaly; buyer asks for Prediction.
    let cat = single_offering_catalog(AiSignalType::Anomaly.to_byte(), 100, true);
    seed_catalog(&mut db, &seller, &cat);

    let payload =
        build_purchase_payload(buyer.id, seller.id, AiSignalType::Prediction.to_byte(), 100);
    let tx = make_tx(buyer.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, PURCHASE_HEIGHT).expect_err("must fail");
    let expected_byte = AiSignalType::Prediction.to_byte();
    assert!(
        matches!(err, ExecError::SignalOfferingNotFound { signal_type } if signal_type == expected_byte),
        "got {err:?}"
    );
}

// ============================================================================
// 6. Offering inactive
// ============================================================================

#[test]
fn purchase_rejected_offering_inactive() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let cat = single_offering_catalog(AiSignalType::Anomaly.to_byte(), 100, false);
    seed_catalog(&mut db, &seller, &cat);

    let payload = build_purchase_payload(buyer.id, seller.id, AiSignalType::Anomaly.to_byte(), 100);
    let tx = make_tx(buyer.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, PURCHASE_HEIGHT).expect_err("must fail");
    assert!(
        matches!(err, ExecError::SignalOfferingInactive),
        "got {err:?}"
    );
}

// ============================================================================
// 7. Price exceeds buyer's max
// ============================================================================

#[test]
fn purchase_rejected_price_exceeds_max() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let cat = single_offering_catalog(AiSignalType::Anomaly.to_byte(), 500, true);
    seed_catalog(&mut db, &seller, &cat);

    // Buyer caps at 100 but seller charges 500.
    let payload = build_purchase_payload(buyer.id, seller.id, AiSignalType::Anomaly.to_byte(), 100);
    let tx = make_tx(buyer.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, PURCHASE_HEIGHT).expect_err("must fail");
    assert!(
        matches!(err, ExecError::PriceExceedsMaxPrice { offered, max } if offered == 500 && max == 100),
        "got {err:?}"
    );
}

// ============================================================================
// 8. Insufficient buyer balance
// ============================================================================

#[test]
fn purchase_rejected_insufficient_balance() {
    let mut db = MemKv::new();
    // Buyer with just enough for tx_fee but not the purchase.
    let mut buyer = build_entity([0x11u8; 32], [0x21u8; 32], marketplace_caps());
    buyer.economic_balance = u128::from(SIGNAL_FEE) + 50;
    store_entity(&mut db, &buyer);

    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let cat = single_offering_catalog(AiSignalType::Anomaly.to_byte(), 1_000, true);
    seed_catalog(&mut db, &seller, &cat);

    let payload =
        build_purchase_payload(buyer.id, seller.id, AiSignalType::Anomaly.to_byte(), 1_000);
    let tx = make_tx(buyer.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, PURCHASE_HEIGHT).expect_err("must fail");
    assert!(
        matches!(err, ExecError::InsufficientEntityBalance { .. }),
        "got {err:?}"
    );
}

// ============================================================================
// 9. Self-purchase
// ============================================================================

#[test]
fn purchase_rejected_self_purchase() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let cat = single_offering_catalog(AiSignalType::Anomaly.to_byte(), 100, true);
    seed_catalog(&mut db, &buyer, &cat);

    let payload = build_purchase_payload(buyer.id, buyer.id, AiSignalType::Anomaly.to_byte(), 100);
    let tx = make_tx(buyer.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, PURCHASE_HEIGHT).expect_err("must fail");
    assert!(matches!(err, ExecError::SellerIsBuyer), "got {err:?}");
}

// ============================================================================
// 10. total_transactions on both parties
// ============================================================================

#[test]
fn purchase_updates_total_transactions_both_parties() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let cat = single_offering_catalog(AiSignalType::Anomaly.to_byte(), 50, true);
    seed_catalog(&mut db, &seller, &cat);

    let payload = build_purchase_payload(buyer.id, seller.id, AiSignalType::Anomaly.to_byte(), 50);
    let tx = make_tx(buyer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PURCHASE_HEIGHT).unwrap();

    let buyer_after = read_ai_entity(&db, &buyer.id).unwrap().unwrap();
    let seller_after = read_ai_entity(&db, &seller.id).unwrap().unwrap();
    assert_eq!(buyer_after.total_transactions, 1);
    assert_eq!(seller_after.total_transactions, 1);
}

// ============================================================================
// 11. SignalCatalogData encode/decode roundtrip via memory object
// ============================================================================

#[test]
fn catalog_create_and_decode_roundtrip() {
    let mut db = MemKv::new();
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let cat = SignalCatalogData {
        entries: vec![
            SignalCatalogEntry {
                signal_type: AiSignalType::Anomaly.to_byte(),
                price_per_signal: 250,
                is_active: true,
            },
            SignalCatalogEntry {
                signal_type: AiSignalType::Prediction.to_byte(),
                price_per_signal: 5_000,
                is_active: false,
            },
        ],
    };
    let object_id = seed_catalog(&mut db, &seller, &cat);

    let stored = db
        .get(&ai_memory_object_key(&seller.id, &object_id))
        .unwrap()
        .expect("memory object stored");
    // Decode the wrapping MemoryObject, then the SignalCatalogData payload.
    let memobj = novai_ai_entities::decode_memory_object_v1(&stored).unwrap();
    assert_eq!(memobj.object_type, MemoryObjectType::SignalCatalog);
    assert_eq!(memobj.owner_entity, seller.id);

    let decoded = SignalCatalogData::decode(&memobj.data).expect("catalog decodes");
    assert_eq!(decoded, cat);
}

// ============================================================================
// 12. Catalog at MAX_CATALOG_OFFERINGS encodes to 101 bytes and decodes
// ============================================================================

#[test]
fn catalog_max_10_offerings() {
    let entries: Vec<SignalCatalogEntry> = (0..MAX_CATALOG_OFFERINGS)
        .map(|i| SignalCatalogEntry {
            signal_type: i as u8,
            price_per_signal: 1_000 * (i as u64 + 1),
            is_active: true,
        })
        .collect();
    let cat = SignalCatalogData { entries };
    let encoded = cat.encode();
    assert_eq!(
        encoded.len(),
        1 + MAX_CATALOG_OFFERINGS * SIGNAL_CATALOG_ENTRY_SIZE
    );
    assert_eq!(encoded.len(), 101);

    let decoded = SignalCatalogData::decode(&encoded).expect("decode");
    assert_eq!(decoded, cat);
    assert_eq!(decoded.entries.len(), MAX_CATALOG_OFFERINGS);
}

// ============================================================================
// 13. Existing non-purchase signals continue to work (regression)
// ============================================================================

#[test]
fn non_purchase_signals_still_work() {
    let mut db = MemKv::new();
    let issuer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: AiSignalType::Anomaly,
        issuer_entity_id: issuer.id,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
    });
    let tx = make_tx(issuer.id, 0, SIGNAL_FEE, payload);

    apply_signal_commitment_tx(&mut db, &tx, PURCHASE_HEIGHT)
        .expect("base anomaly signal still applies");

    let after = read_ai_entity(&db, &issuer.id).unwrap().unwrap();
    assert_eq!(
        after.total_transactions, 0,
        "non-purchase signals do not bump total_transactions"
    );
    assert_eq!(
        read_treasury(&db),
        0,
        "non-purchase signals do not credit marketplace treasury"
    );
}

// ============================================================================
// 14. Zero-price purchase
// ============================================================================

#[test]
fn purchase_with_zero_price_offering() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let cat = single_offering_catalog(AiSignalType::Anomaly.to_byte(), 0, true);
    seed_catalog(&mut db, &seller, &cat);

    let payload = build_purchase_payload(buyer.id, seller.id, AiSignalType::Anomaly.to_byte(), 0);
    let tx = make_tx(buyer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PURCHASE_HEIGHT)
        .expect("zero-price purchase succeeds");

    let buyer_after = read_ai_entity(&db, &buyer.id).unwrap().unwrap();
    let seller_after = read_ai_entity(&db, &seller.id).unwrap().unwrap();

    assert_eq!(
        buyer_after.economic_balance,
        BUYER_BALANCE - u128::from(SIGNAL_FEE),
        "zero-price: only tx_fee debited from buyer"
    );
    assert_eq!(
        seller_after.economic_balance, SELLER_BALANCE,
        "zero-price: seller balance unchanged"
    );
    assert_eq!(buyer_after.total_transactions, 1);
    assert_eq!(seller_after.total_transactions, 1);
    assert_eq!(
        read_treasury(&db),
        0,
        "zero-price: treasury record never written"
    );
}

// ============================================================================
// 15. Golden: SignalPurchase payload byte layout
// ============================================================================

#[test]
fn golden_vector_signal_purchase_payload_107_bytes() {
    let buyer = [0x22u8; 32];
    let seller = [0x33u8; 32];
    let payload = build_purchase_payload(buyer, seller, 4, 0x0102_0304_0506_0708);

    assert_eq!(payload.len(), 107);
    assert_eq!(payload.len(), SIGNAL_COMMITMENT_PAYLOAD_V1_PURCHASE_LEN);
    assert_eq!(payload[33], AiSignalType::SignalPurchase.to_byte());
    assert_eq!(&payload[34..66], &buyer, "issuer_entity_id at 34..66");
    assert_eq!(&payload[66..98], &seller, "seller_entity_id at 66..98");
    assert_eq!(payload[98], 4, "purchased_signal_type at 98");
    assert_eq!(
        &payload[99..107],
        &0x0102_0304_0506_0708u64.to_be_bytes(),
        "max_price_be at 99..107"
    );
}
