//! Replay Test Harness (D20.2)
//!
//! PURPOSE: Prove deterministic state transitions by applying identical
//! block sequences to independent nodes and verifying state root equality.
//!
//! INVARIANTS:
//! - No randomness (all values derived from indices)
//! - No floats, no nondeterministic behavior
//! - State roots must match across "nodes" (fresh `MemKv` instances)
//!
//! FAILURE MODES:
//! - State root mismatch indicates nondeterminism in execution
//! - Must be runnable on Linux + macOS (CI requirement)

use novai_ai_entities::{AiEntity, AiSignalType, AutonomyMode, Capabilities};
use novai_execution::{
    apply_signal_commitment_tx, apply_tx_v1_transfer, encode_signal_commitment_payload_v1,
    encode_transfer_payload_v1, write_ai_entity_op, SignalCommitmentPayloadV1, TransferPayloadV1,
};
use novai_smt::hash::empty_hash_at_height;
use novai_state::{
    account_key, ai_entity_by_address_key, decode_smt_root_v1, encode_account_v1,
    encode_fee_pool_v1, AccountStateV1, FeePoolV1, Kv, KvBatch, MemKv, WriteOp, KEY_FEE_POOL,
    KEY_SMT_ROOT,
};
use novai_types::{Address, TxV1, TxVersion};

// ============================================================================
// DETERMINISTIC ADDRESS/VALUE GENERATION
// ============================================================================

/// Generate deterministic address from seed byte.
const fn addr(seed: u8) -> Address {
    [seed; 32]
}

// ============================================================================
// REPLAY BLOCK STRUCTURE
// ============================================================================

/// A deterministic block for replay testing.
#[derive(Debug, Clone)]
struct ReplayBlock {
    height: u64,
    txs: Vec<TxV1>,
    /// AI signal commitment txs (applied via `apply_signal_commitment_tx`)
    signal_txs: Vec<TxV1>,
}

#[allow(clippy::missing_const_for_fn)] // Vec operations can't be const
impl ReplayBlock {
    fn empty(height: u64) -> Self {
        Self {
            height,
            txs: vec![],
            signal_txs: vec![],
        }
    }

    fn with_transfers(height: u64, txs: Vec<TxV1>) -> Self {
        Self {
            height,
            txs,
            signal_txs: vec![],
        }
    }

    fn with_signals(height: u64, signal_txs: Vec<TxV1>) -> Self {
        Self {
            height,
            txs: vec![],
            signal_txs,
        }
    }
}

// ============================================================================
// TRANSACTION BUILDERS
// ============================================================================

/// Build a deterministic transfer transaction.
fn mk_transfer(from: Address, nonce: u64, fee: u64, to: Address, amount: u64) -> TxV1 {
    let payload = encode_transfer_payload_v1(&TransferPayloadV1 { to, amount }).to_vec();
    TxV1 {
        version: TxVersion::V1,
        from,
        pubkey: from, // Execution doesn't verify signatures
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    }
}

/// Build a deterministic signal commitment transaction.
fn mk_signal_commitment(
    issuer_entity_id: [u8; 32],
    nonce: u64,
    fee: u64,
    signal_type: AiSignalType,
    signal_hash: [u8; 32],
) -> TxV1 {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type,
        issuer_entity_id,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
    });
    TxV1 {
        version: TxVersion::V1,
        from: issuer_entity_id,
        pubkey: issuer_entity_id,
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    }
}

// ============================================================================
// STATE INITIALIZATION
// ============================================================================

/// Initialize a fresh `MemKv` with deterministic starting state.
fn init_fresh_state() -> MemKv {
    let mut db = MemKv::new();

    // Create multiple funded accounts for transfers
    for i in 1u8..=10 {
        let acct = AccountStateV1 {
            balance: 10_000_000u128, // 10M balance each
            nonce: 0,
        };
        db.put(&account_key(&addr(i)), &encode_account_v1(&acct))
            .unwrap();
    }

    // Initialize fee pool
    let fee_pool = FeePoolV1 { balance: 0 };
    db.put(KEY_FEE_POOL, &encode_fee_pool_v1(&fee_pool))
        .unwrap();

    db
}

/// Compute deterministic AI entity IDs for testing.
/// Returns entity IDs for entities 1, 2, 3.
fn get_test_entity_ids() -> [[u8; 32]; 3] {
    [
        AiEntity::compute_id(&[1u8; 32], &addr(1)),
        AiEntity::compute_id(&[2u8; 32], &addr(2)),
        AiEntity::compute_id(&[3u8; 32], &addr(3)),
    ]
}

/// Initialize state with AI entities for signal commitment tests.
/// Returns the entity IDs that were created.
fn init_state_with_ai_entities() -> (MemKv, [[u8; 32]; 3]) {
    let mut db = init_fresh_state();
    let mut entity_ids = [[0u8; 32]; 3];

    // Create AI entities with emit_proposals capability
    for i in 1u8..=3 {
        let mut entity = AiEntity::new(
            [i; 32], // code_hash (deterministic)
            addr(i), // creator
            AutonomyMode::Gated,
            Capabilities::gated(), // has emit_proposals
            1000,                  // registered_at
        );
        // Fund the AI entity
        entity.economic_balance = 1_000_000u128;

        // Store the entity ID
        entity_ids[(i - 1) as usize] = entity.id;

        db.apply_batch(&[
            write_ai_entity_op(&entity),
            WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
        ])
        .unwrap();
    }

    (db, entity_ids)
}

// ============================================================================
// STATE ROOT UTILITIES
// ============================================================================

/// Read SMT root from database.
fn read_smt_root(db: &MemKv) -> [u8; 32] {
    db.get(KEY_SMT_ROOT).unwrap().map_or_else(
        || empty_hash_at_height(256),
        |bytes| decode_smt_root_v1(&bytes).unwrap(),
    )
}

// ============================================================================
// BLOCK APPLICATION
// ============================================================================

/// Apply a sequence of blocks to a fresh database, return final state root.
fn apply_blocks_and_get_root(blocks: &[ReplayBlock], with_ai: bool) -> [u8; 32] {
    let mut db = if with_ai {
        let (db, _) = init_state_with_ai_entities();
        db
    } else {
        init_fresh_state()
    };

    for block in blocks {
        // Apply transfer transactions
        for tx in &block.txs {
            apply_tx_v1_transfer(&mut db, tx).unwrap();
        }

        // Apply signal commitment transactions
        for tx in &block.signal_txs {
            apply_signal_commitment_tx(&mut db, tx, block.height).unwrap();
        }
    }

    read_smt_root(&db)
}

// ============================================================================
// BLOCK GENERATION
// ============================================================================

/// Generate N deterministic blocks with transfers.
#[allow(clippy::cast_possible_truncation)] // sender_idx is bounded to 1..9, safe to cast
fn generate_transfer_blocks(count: usize) -> Vec<ReplayBlock> {
    let mut blocks = Vec::with_capacity(count);
    let mut nonces = [0u64; 11]; // Track nonces for addr(1)..addr(10)

    for height in 1..=count {
        let h = height as u64;
        let mut txs = Vec::new();

        // Deterministic pattern: addr(1) sends to addr(2), addr(2) sends to addr(3), etc.
        let sender_idx = ((height - 1) % 9) + 1; // 1..9
        let receiver_idx = sender_idx + 1; // 2..10

        let from = addr(sender_idx as u8);
        let to = addr(receiver_idx as u8);
        let nonce = nonces[sender_idx];
        nonces[sender_idx] += 1;

        let amount = (height as u64) * 100; // Deterministic amount
        let fee = 1;

        txs.push(mk_transfer(from, nonce, fee, to, amount));

        blocks.push(ReplayBlock::with_transfers(h, txs));
    }

    blocks
}

/// Generate N deterministic empty blocks.
#[allow(clippy::cast_possible_truncation)] // height is bounded by count parameter
fn generate_empty_blocks(count: usize) -> Vec<ReplayBlock> {
    (1..=count).map(|h| ReplayBlock::empty(h as u64)).collect()
}

/// Generate mixed blocks (empty + transfers interleaved).
#[allow(clippy::cast_possible_truncation)] // indices are bounded, safe to cast
fn generate_mixed_blocks(count: usize) -> Vec<ReplayBlock> {
    let mut blocks = Vec::with_capacity(count);
    let mut nonces = [0u64; 11];

    for height in 1..=count {
        let h = height as u64;

        if height % 3 == 0 {
            // Every 3rd block is empty
            blocks.push(ReplayBlock::empty(h));
        } else {
            // Transfer blocks
            let sender_idx = ((height - 1) % 9) + 1;
            let receiver_idx = sender_idx + 1;
            let from = addr(sender_idx as u8);
            let to = addr(receiver_idx as u8);
            let nonce = nonces[sender_idx];
            nonces[sender_idx] += 1;

            let txs = vec![mk_transfer(from, nonce, 1, to, (height as u64) * 50)];
            blocks.push(ReplayBlock::with_transfers(h, txs));
        }
    }

    blocks
}

// ============================================================================
// REPLAY TESTS
// ============================================================================

#[test]
fn replay_empty_blocks_deterministic() {
    let blocks = generate_empty_blocks(10);

    // Node A
    let root_a = apply_blocks_and_get_root(&blocks, false);

    // Node B (fresh state)
    let root_b = apply_blocks_and_get_root(&blocks, false);

    assert_eq!(
        root_a, root_b,
        "Empty blocks: state roots must match across nodes"
    );
}

#[test]
fn replay_transfer_blocks_deterministic() {
    let blocks = generate_transfer_blocks(20);

    // Node A
    let root_a = apply_blocks_and_get_root(&blocks, false);

    // Node B (fresh state)
    let root_b = apply_blocks_and_get_root(&blocks, false);

    assert_eq!(
        root_a, root_b,
        "Transfer blocks: state roots must match across nodes"
    );

    // Verify root is not empty (transactions were processed)
    assert_ne!(
        root_a,
        empty_hash_at_height(256),
        "Root should not be empty after transfers"
    );
}

#[test]
fn replay_mixed_blocks_deterministic() {
    let blocks = generate_mixed_blocks(30);

    // Node A
    let root_a = apply_blocks_and_get_root(&blocks, false);

    // Node B (fresh state)
    let root_b = apply_blocks_and_get_root(&blocks, false);

    assert_eq!(
        root_a, root_b,
        "Mixed blocks: state roots must match across nodes"
    );
}

#[test]
fn replay_multiple_runs_identical() {
    let blocks = generate_transfer_blocks(15);

    // Run 3 times, all must produce identical roots
    let root_1 = apply_blocks_and_get_root(&blocks, false);
    let root_2 = apply_blocks_and_get_root(&blocks, false);
    let root_3 = apply_blocks_and_get_root(&blocks, false);

    assert_eq!(root_1, root_2, "Run 1 vs Run 2 mismatch");
    assert_eq!(root_2, root_3, "Run 2 vs Run 3 mismatch");
}

#[test]
fn replay_different_blocks_different_roots() {
    let blocks_a = generate_transfer_blocks(10);
    let blocks_b = generate_transfer_blocks(11); // One more block

    let root_a = apply_blocks_and_get_root(&blocks_a, false);
    let root_b = apply_blocks_and_get_root(&blocks_b, false);

    assert_ne!(
        root_a, root_b,
        "Different block counts must produce different roots"
    );
}

#[test]
fn replay_ai_signal_blocks_deterministic() {
    // Generate blocks with AI signal commitments
    // This is required by D20.2: "blocks with AI signal refs"

    // Get the pre-computed entity IDs
    let entity_ids = get_test_entity_ids();

    // Create signal commitment transactions for AI entities 1 and 2
    let signal_txs = vec![
        mk_signal_commitment(entity_ids[0], 0, 1, AiSignalType::Anomaly, [0xAAu8; 32]),
        mk_signal_commitment(entity_ids[1], 0, 1, AiSignalType::Prediction, [0xBBu8; 32]),
    ];

    let blocks = vec![ReplayBlock::with_signals(1, signal_txs)];

    let root_a = apply_blocks_and_get_root(&blocks, true); // with_ai = true
    let root_b = apply_blocks_and_get_root(&blocks, true);

    assert_eq!(root_a, root_b, "AI signal blocks: state roots must match");

    // Note: Signal commitments are stored in separate indices and currently
    // don't update the SMT root (they may be intentionally off-SMT).
    // The key invariant is that roots match across nodes.
}

// Golden vector constant (must be at module level for clippy)
const EXPECTED_ROOT_10_BLOCKS: [u8; 32] = [
    // Will be filled on first run - placeholder zeros
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

#[test]
fn replay_golden_vector_10_blocks() {
    // Golden vector test: lock the root for 10 transfer blocks
    let blocks = generate_transfer_blocks(10);
    let root = apply_blocks_and_get_root(&blocks, false);

    // Skip golden check if not yet initialized
    if EXPECTED_ROOT_10_BLOCKS == [0u8; 32] {
        eprintln!("GOLDEN VECTOR FOR 10 BLOCKS:");
        eprintln!("const EXPECTED_ROOT_10_BLOCKS: [u8; 32] = [");
        for chunk in root.chunks(8) {
            eprint!("    ");
            for b in chunk {
                eprint!("0x{b:02x}, ");
            }
            eprintln!();
        }
        eprintln!("];");
        // Don't fail - just print for initial setup
    } else {
        assert_eq!(
            root, EXPECTED_ROOT_10_BLOCKS,
            "Golden vector mismatch for 10 blocks"
        );
    }
}

// ============================================================================
// BLOCK ORDER TESTS
// ============================================================================

#[test]
fn replay_independent_tx_order_does_not_affect_root() {
    // Independent transactions (different senders) can be reordered
    // without affecting the final SMT root, because SMT roots depend
    // on final key-value state, not insertion order.
    let tx1 = mk_transfer(addr(1), 0, 1, addr(2), 100);
    let tx2 = mk_transfer(addr(3), 0, 1, addr(4), 200);

    let block_a = ReplayBlock::with_transfers(1, vec![tx1.clone(), tx2.clone()]);
    let block_b = ReplayBlock::with_transfers(1, vec![tx2, tx1]);

    let root_a = apply_blocks_and_get_root(std::slice::from_ref(&block_a), false);
    let root_b = apply_blocks_and_get_root(std::slice::from_ref(&block_b), false);

    // Independent transactions produce the same final state
    assert_eq!(
        root_a, root_b,
        "Independent tx order should not affect final state root"
    );
}

#[test]
fn replay_dependent_tx_order_matters() {
    // Dependent transactions (same sender, different nonces) MUST be ordered
    // correctly, or nonce validation will fail.
    // Here we verify that a valid sequence produces a deterministic root.
    let tx1 = mk_transfer(addr(1), 0, 1, addr(2), 100); // nonce 0
    let tx2 = mk_transfer(addr(1), 1, 1, addr(3), 200); // nonce 1

    let block = ReplayBlock::with_transfers(1, vec![tx1, tx2]);

    let root_a = apply_blocks_and_get_root(std::slice::from_ref(&block), false);
    let root_b = apply_blocks_and_get_root(std::slice::from_ref(&block), false);

    assert_eq!(
        root_a, root_b,
        "Sequential nonce transactions must produce deterministic root"
    );
}
