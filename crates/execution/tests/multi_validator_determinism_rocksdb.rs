#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::format_push_string)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::redundant_clone)]
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::cloned_ref_to_slice_refs)]
#![allow(clippy::needless_pass_by_ref_mut)]

//! PURPOSE: RocksKv-backed four-validator determinism harness, companion
//! to `multi_validator_determinism.rs` (MemKv). Tests the same mechanism
//! at the RocksDB layer to surface any divergence that depends on
//! RocksDB-specific behavior (compaction, memtable flush, WAL ordering)
//! rather than the executor itself.
//!
//! INVARIANTS:
//! - 4 RocksKv instances driven by identical input must produce byte-equal
//!   state for every public-chain key after each commit.
//! - Forced compaction (`compact_range_default`) must not alter byte-level
//!   key/value contents in the default column family.
//!
//! FAILURE MODES:
//! - Tests use tempdirs; cleanup is handled by Drop on TempDir.
//! - RocksDB compaction is internally non-blocking in some configurations;
//!   `compact_range_default(None, None)` waits for completion of the full
//!   range. We use it directly to control compaction timing.
//! - The harness is single-threaded; it does NOT model concurrent
//!   per-validator I/O contention. A real per-validator compaction stall
//!   in production has wall-clock impact but identical correctness
//!   semantics for RocksDB; documented here for the operator's record.
//!
//! WORKLOADS:
//!   R1 baseline_rocks_determinism_no_compaction
//!   R2 register_then_transfer_no_compaction
//!   R3 manual_compaction_does_not_alter_state
//!   R4 manual_compaction_between_blocks_stays_deterministic
//!   R5 per_validator_uneven_compaction_does_not_introduce_divergence
//!   R6 register_then_compact_then_transfer_stays_deterministic
//!   R7 high_load_with_periodic_compaction_stays_deterministic

use novai_ai_entities::{AutonomyMode, Capabilities};
use novai_execution::{
    dispatch_tx, encode_register_ai_entity_with_key_payload_v1, encode_transfer_payload_v1,
    RegisterAiEntityWithKeyPayloadV1, TransferPayloadV1,
};
use novai_state::{account_key, encode_account_v1, AccountStateV1, Kv, KvBatch, RocksKv, WriteOp};
use novai_types::{Address, TxV1, TxVersion};
use tempfile::TempDir;

const VALIDATORS: usize = 4;

struct RocksQuad {
    _dirs: [TempDir; VALIDATORS],
    dbs: [RocksKv; VALIDATORS],
}

impl RocksQuad {
    fn new() -> Self {
        let dirs: [TempDir; VALIDATORS] = [
            TempDir::new().unwrap(),
            TempDir::new().unwrap(),
            TempDir::new().unwrap(),
            TempDir::new().unwrap(),
        ];
        let dbs: [RocksKv; VALIDATORS] = [
            RocksKv::open(dirs[0].path()).unwrap(),
            RocksKv::open(dirs[1].path()).unwrap(),
            RocksKv::open(dirs[2].path()).unwrap(),
            RocksKv::open(dirs[3].path()).unwrap(),
        ];
        Self { _dirs: dirs, dbs }
    }

    fn commit_block(&mut self, txs: &[TxV1], height: u64, label: &str) {
        for (idx, tx) in txs.iter().enumerate() {
            let outcomes: [Result<(), String>; VALIDATORS] = [
                dispatch_tx(&mut self.dbs[0], tx, height).map_err(|e| format!("{e:?}")),
                dispatch_tx(&mut self.dbs[1], tx, height).map_err(|e| format!("{e:?}")),
                dispatch_tx(&mut self.dbs[2], tx, height).map_err(|e| format!("{e:?}")),
                dispatch_tx(&mut self.dbs[3], tx, height).map_err(|e| format!("{e:?}")),
            ];
            for i in 1..VALIDATORS {
                assert!(
                    outcomes[0] == outcomes[i],
                    "[{label}] outcome divergence at tx_idx={idx} between v0 and v{i}: v0={:?} v{i}={:?}",
                    outcomes[0],
                    outcomes[i]
                );
            }
        }
        assert_states_equal(&self.dbs, label);
    }

    fn compact_all(&mut self) {
        for db in &self.dbs {
            db.compact_range_default(None, None);
        }
    }

    fn compact_one(&mut self, idx: usize) {
        self.dbs[idx].compact_range_default(None, None);
    }
}

/// Snapshot the public-chain keys we care about from a RocksKv instance.
/// We use a fixed list of prefixes covering account, fee_pool, entity
/// records, reverse-index, SMT root and nodes, signal commitments, and
/// oracle anchors. This is the set the bug touches.
fn snapshot_default_keys(db: &RocksKv) -> Vec<(Vec<u8>, Vec<u8>)> {
    let prefixes: &[&[u8]] = &[
        b"accounts/",
        b"fee_pool",
        b"smt/",
        b"ai/entities/",
        b"ai/entities_by_addr/",
        b"ai/signals/",
        b"ai/memory_objects/",
        b"ai/memory_by_type/",
        b"ai/memory_count/",
        b"ai/oracle_anchors/",
        b"ai/payment_records/",
        b"ai/payment_splits/",
        b"ai/payment_conditions/",
        b"ai/entity_upgrades/",
    ];
    let mut out = Vec::new();
    for p in prefixes {
        let mut pairs = db.scan_prefix(p).unwrap();
        out.append(&mut pairs);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn assert_states_equal(dbs: &[RocksKv; VALIDATORS], label: &str) {
    let s0 = snapshot_default_keys(&dbs[0]);
    for i in 1..VALIDATORS {
        let si = snapshot_default_keys(&dbs[i]);
        if s0 != si {
            // Compute a tiny diff summary.
            let mut diff_lines = Vec::new();
            let max_len = s0.len().max(si.len());
            for j in 0..max_len {
                let a = s0.get(j);
                let b = si.get(j);
                match (a, b) {
                    (Some(av), Some(bv)) if av != bv => {
                        diff_lines.push(format!(
                            "  pos {j}: v0 key_len={}, v{i} key_len={}, v0 val_len={}, v{i} val_len={}",
                            av.0.len(),
                            bv.0.len(),
                            av.1.len(),
                            bv.1.len(),
                        ));
                    }
                    (Some(av), None) => {
                        diff_lines.push(format!("  pos {j}: only v0 has key_len={}", av.0.len()));
                    }
                    (None, Some(bv)) => {
                        diff_lines.push(format!("  pos {j}: only v{i} has key_len={}", bv.0.len()));
                    }
                    _ => {}
                }
                if diff_lines.len() >= 8 {
                    break;
                }
            }
            panic!(
                "[{label}] RocksKv state divergence v0 vs v{i}: v0 entries={}, v{i} entries={}\n{}",
                s0.len(),
                si.len(),
                diff_lines.join("\n")
            );
        }
    }
}

// ============================================================================
// TX BUILDERS (same as MemKv harness)
// ============================================================================

fn mk_tx(from: Address, nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
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

fn mk_transfer_tx(from: Address, nonce: u64, fee: u64, to: Address, amount: u64) -> TxV1 {
    let payload = encode_transfer_payload_v1(&TransferPayloadV1 { to, amount }).to_vec();
    mk_tx(from, nonce, fee, payload)
}

fn mk_register_type10_tx(
    creator: Address,
    nonce: u64,
    fee: u64,
    code_hash: [u8; 32],
    entity_pubkey: [u8; 32],
    initial_balance: u128,
) -> TxV1 {
    let payload =
        encode_register_ai_entity_with_key_payload_v1(&RegisterAiEntityWithKeyPayloadV1 {
            code_hash,
            pubkey: entity_pubkey,
            autonomy_mode: AutonomyMode::Gated,
            capabilities: Capabilities::oracle(),
            initial_balance,
        })
        .to_vec();
    mk_tx(creator, nonce, fee, payload)
}

fn address_from_pubkey_bytes(pubkey: &[u8; 32]) -> Address {
    let mut h = blake3::Hasher::new();
    h.update(b"NOVAI_ADDRESS_V1");
    h.update(pubkey);
    *h.finalize().as_bytes()
}

fn seed_account_all(quad: &mut RocksQuad, addr: &Address, balance: u128, nonce: u64) {
    let acct = AccountStateV1 { balance, nonce };
    let op = WriteOp::Put(account_key(addr), encode_account_v1(&acct).to_vec());
    for db in &mut quad.dbs {
        db.apply_batch(&[op.clone()]).unwrap();
    }
}

// ============================================================================
// R1: baseline RocksKv determinism (no compaction)
// ============================================================================

#[test]
fn r1_baseline_rocks_determinism_no_compaction() {
    let mut quad = RocksQuad::new();
    let a = [0x01u8; 32];
    let b = [0x02u8; 32];
    seed_account_all(&mut quad, &a, 1_000_000, 0);
    seed_account_all(&mut quad, &b, 1_000_000, 0);

    let block = vec![
        mk_transfer_tx(a, 0, 100, b, 5_000),
        mk_transfer_tx(b, 0, 100, a, 2_500),
        mk_transfer_tx(a, 1, 100, b, 1_000),
    ];
    quad.commit_block(&block, 1, "r1");
}

// ============================================================================
// R2: register then transfer (no compaction)
// ============================================================================

#[test]
fn r2_register_then_transfer_no_compaction() {
    let mut quad = RocksQuad::new();
    let creator = [0x10u8; 32];
    let recipient = [0x20u8; 32];
    let entity_pubkey = [0xBBu8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xAA; 32],
            entity_pubkey,
            500_000,
        )],
        1,
        "r2/register",
    );
    quad.commit_block(
        &[mk_transfer_tx(entity_addr, 0, 100, recipient, 5_000)],
        2,
        "r2/transfer",
    );
}

// ============================================================================
// R3: manual compaction is a no-op for state contents
// ============================================================================

/// Seed accounts and run a transfer, then call compact_range_default on
/// all four. State must be byte-equal before and after compaction.
#[test]
fn r3_manual_compaction_does_not_alter_state() {
    let mut quad = RocksQuad::new();
    let a = [0x01u8; 32];
    let b = [0x02u8; 32];
    seed_account_all(&mut quad, &a, 1_000_000, 0);
    seed_account_all(&mut quad, &b, 1_000_000, 0);

    quad.commit_block(&[mk_transfer_tx(a, 0, 100, b, 5_000)], 1, "r3/pre-compact");

    let pre = snapshot_default_keys(&quad.dbs[0]);
    quad.compact_all();
    let post = snapshot_default_keys(&quad.dbs[0]);
    assert_eq!(
        pre, post,
        "compact_range_default(None, None) must not alter default-CF state"
    );
    assert_states_equal(&quad.dbs, "r3/post-compact");
}

// ============================================================================
// R4: compaction between blocks does not introduce divergence
// ============================================================================

#[test]
fn r4_manual_compaction_between_blocks_stays_deterministic() {
    let mut quad = RocksQuad::new();
    let a = [0x01u8; 32];
    let b = [0x02u8; 32];
    seed_account_all(&mut quad, &a, 1_000_000, 0);
    seed_account_all(&mut quad, &b, 1_000_000, 0);

    quad.commit_block(&[mk_transfer_tx(a, 0, 100, b, 5_000)], 1, "r4/b1");
    quad.compact_all();
    quad.commit_block(&[mk_transfer_tx(a, 1, 100, b, 5_000)], 2, "r4/b2");
    quad.compact_all();
    quad.commit_block(&[mk_transfer_tx(a, 2, 100, b, 5_000)], 3, "r4/b3");
}

// ============================================================================
// R5: per-validator uneven compaction
// ============================================================================

/// Compact only validator 3 (mimicking the production scenario where @3
/// stalled on compaction while others did not). State must remain
/// byte-equal across all four after compaction.
#[test]
fn r5_per_validator_uneven_compaction_does_not_introduce_divergence() {
    let mut quad = RocksQuad::new();
    let creator = [0x11u8; 32];
    let recipient = [0x21u8; 32];
    let entity_pubkey = [0xCCu8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xDD; 32],
            entity_pubkey,
            500_000,
        )],
        1,
        "r5/register",
    );

    // Only v3 compacts. State must still match across all 4.
    quad.compact_one(3);
    assert_states_equal(&quad.dbs, "r5/after-v3-compact");

    quad.commit_block(
        &[mk_transfer_tx(entity_addr, 0, 100, recipient, 5_000)],
        2,
        "r5/transfer",
    );
}

// ============================================================================
// R6: register, compact (all four), then transfer FROM creator-driven entity
//     This is the production-relevant scenario: forced compaction
//     immediately after a register and before the next Transfer.
// ============================================================================

#[test]
fn r6_register_then_compact_then_transfer_stays_deterministic() {
    let mut quad = RocksQuad::new();
    let creator = [0x12u8; 32];
    let recipient = [0x22u8; 32];
    let entity_pubkey = [0xEEu8; 32];
    let entity_addr = address_from_pubkey_bytes(&entity_pubkey);

    seed_account_all(&mut quad, &creator, 10_000_000, 0);
    seed_account_all(&mut quad, &recipient, 10_000, 0);

    // Block 1: register Type-10 entity (writes reverse-index + entity).
    quad.commit_block(
        &[mk_register_type10_tx(
            creator,
            0,
            5_000,
            [0xF1; 32],
            entity_pubkey,
            500_000,
        )],
        1,
        "r6/register",
    );

    // Forced compaction across the boundary, on all 4. If the production
    // bug is caused by compaction silently dropping the reverse-index Put,
    // the next Transfer FROM the entity_addr would diverge.
    quad.compact_all();

    // Block 2: Transfer FROM the entity address. Should route through the
    // AI-sender branch on all 4 if the reverse-index is intact.
    quad.commit_block(
        &[mk_transfer_tx(entity_addr, 0, 100, recipient, 5_000)],
        2,
        "r6/transfer",
    );

    // Also assert each validator can still look up the reverse-index.
    let rev_key = novai_state::ai_entity_by_address_key(&entity_addr);
    for (i, db) in quad.dbs.iter().enumerate() {
        let v = db
            .get(&rev_key)
            .unwrap()
            .expect("reverse-index present after compaction");
        assert_eq!(
            v.len(),
            32,
            "v{i} reverse-index value is the 32-byte entity_id"
        );
    }
}

// ============================================================================
// R7: high-load with periodic compaction
// ============================================================================

#[test]
fn r7_high_load_with_periodic_compaction_stays_deterministic() {
    let mut quad = RocksQuad::new();
    let mut addrs: Vec<Address> = Vec::new();
    for i in 0..6u8 {
        let mut a = [0u8; 32];
        a[0] = 0x30;
        a[1] = i;
        addrs.push(a);
        seed_account_all(&mut quad, &a, 1_000_000, 0);
    }

    // 10 rounds of transfers, compact every 3 rounds.
    for round in 0..10u64 {
        let mut block = Vec::new();
        for (i, addr) in addrs.iter().enumerate() {
            let to = addrs[(i + 1) % addrs.len()];
            block.push(mk_transfer_tx(*addr, round, 100, to, 100 + i as u64));
        }
        quad.commit_block(&block, round + 1, &format!("r7/round{round}"));
        if round % 3 == 2 {
            quad.compact_all();
        }
    }
}
