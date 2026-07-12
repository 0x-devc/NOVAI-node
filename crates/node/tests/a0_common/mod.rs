//! Shared fixtures for the A0 auditor gate tests (F4 execute gate, A0 scope).
//!
//! Everything here builds SYNTHETIC chain state inside throwaway RocksDB
//! directories under the system temp dir. No live node data directory is ever
//! touched. State writes go through the canonical execution path
//! (`append_smt_ops_for_state_ops`) so fixtures carry exactly the SMT
//! semantics the node produces: per-write atomic batches of flat puts, SMT
//! node puts, and the `smt/root` record.
//!
//! Fixture shape mirrors the verified identity from the F4 diagnosis:
//! `header(H).state_root` is the PRE-state of H, so `block_t` carries `r0`
//! (the root before block T's effect) and `block_t1` carries `r1` (the root
//! the audited copy stores).

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use ed25519_dalek::SigningKey;
use novai_consensus_types::codec::{encode_block_v1, encode_qc_v1, encode_vote_v1_unsigned, hash_block_v1};
use novai_consensus_types::{Block, Vote, QC};
use novai_crypto::{address_from_pubkey, sign_bytes};
use novai_execution::{append_smt_ops_for_state_ops, empty_smt_root};
use novai_state::{
    account_key, block_key, decode_smt_root_v1, encode_account_v1, qc_key, AccountStateV1, Kv,
    KvBatch, RocksKv, WriteOp, KEY_COMMITTED_HEIGHT, KEY_EXECUTED_HEIGHT, KEY_HIGHEST_QC,
    KEY_SMT_ROOT,
};

pub const DOMAIN_VOTE: &[u8] = b"NOVAI_VOTE_V1";

/// Throwaway directory under the system temp dir; removed on drop.
pub struct TmpDir(pub PathBuf);

impl TmpDir {
    pub fn new(tag: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "novai_a0_gate_{}_{}_{}",
            std::process::id(),
            tag,
            n
        ));
        std::fs::create_dir_all(&path).expect("create fixture tmp dir");
        Self(path)
    }

    pub fn path_str(&self) -> String {
        self.0.to_str().expect("utf8 tmp path").to_string()
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The four dev-keys signing keys, exactly as main.rs:1002-1006 derives them.
pub fn dev_signing_keys() -> Vec<SigningKey> {
    (0..4u8)
        .map(|i| SigningKey::from_bytes(&[i; 32]))
        .collect()
}

/// The four dev validator addresses, exactly as main.rs:1008-1011 derives them.
pub fn dev_addresses() -> Vec<[u8; 32]> {
    dev_signing_keys()
        .iter()
        .map(|sk| address_from_pubkey(&sk.verifying_key()))
        .collect()
}

/// Sign a vote the way the consensus vote path does (consensus lib create_vote):
/// domain tag then the unsigned vote encoding.
pub fn sign_vote(sk: &SigningKey, height: u64, round: u64, block_hash: [u8; 32]) -> Vote {
    let voter = address_from_pubkey(&sk.verifying_key());
    let unsigned = Vote {
        height,
        round,
        block_hash,
        voter,
        signature: [0u8; 64],
        ai_signal_commitment: None,
    };
    let unsigned_bytes = encode_vote_v1_unsigned(&unsigned);
    let mut to_sign = Vec::new();
    to_sign.extend_from_slice(DOMAIN_VOTE);
    to_sign.extend_from_slice(&unsigned_bytes);
    let signature = sign_bytes(sk, &to_sign);
    Vote {
        signature,
        ..unsigned
    }
}

/// QC over `block` signed by the dev validators at `voter_indices`.
pub fn make_qc(block: &Block, voter_indices: &[usize]) -> QC {
    let keys = dev_signing_keys();
    let refs: Vec<&SigningKey> = voter_indices.iter().map(|&i| &keys[i]).collect();
    make_qc_with_keys(block, &refs)
}

/// QC over `block` signed by arbitrary keys (for unknown-voter fixtures).
pub fn make_qc_with_keys(block: &Block, keys: &[&SigningKey]) -> QC {
    let block_hash = hash_block_v1(block).expect("hash block");
    let votes = keys
        .iter()
        .map(|sk| sign_vote(sk, block.height, block.round, block_hash))
        .collect();
    QC {
        height: block.height,
        round: block.round,
        block_hash,
        votes,
    }
}

/// Read the stored root, defaulting to the canonical empty root when absent,
/// mirroring every consensus read site.
pub fn read_root(db: &RocksKv) -> [u8; 32] {
    match db.get(KEY_SMT_ROOT).expect("get smt root") {
        Some(bytes) => decode_smt_root_v1(&bytes).expect("decode smt root"),
        None => empty_smt_root(),
    }
}

/// Apply state pairs one at a time through the canonical execution path,
/// matching the node's per-transaction batching. Returns the resulting root.
pub fn apply_state_chunked(db: &mut RocksKv, pairs: &[(Vec<u8>, Vec<u8>)]) -> [u8; 32] {
    for (k, v) in pairs {
        let state_ops = vec![WriteOp::Put(k.clone(), v.clone())];
        let mut all_ops = state_ops.clone();
        append_smt_ops_for_state_ops(db, &state_ops, &mut all_ops).expect("smt ops");
        db.apply_batch(&all_ops).expect("apply fixture batch");
    }
    read_root(db)
}

/// Apply all state pairs in a single walk and batch, matching the genesis
/// precedent (main.rs dev genesis, genesis crate). Returns the resulting root.
pub fn apply_state_oneshot(db: &mut RocksKv, pairs: &[(Vec<u8>, Vec<u8>)]) -> [u8; 32] {
    if pairs.is_empty() {
        return read_root(db);
    }
    let state_ops: Vec<WriteOp> = pairs
        .iter()
        .map(|(k, v)| WriteOp::Put(k.clone(), v.clone()))
        .collect();
    let mut all_ops = state_ops.clone();
    append_smt_ops_for_state_ops(db, &state_ops, &mut all_ops).expect("smt ops");
    db.apply_batch(&all_ops).expect("apply fixture batch");
    read_root(db)
}

/// Account state pair helper.
pub fn acct(tag: u8, balance: u128, nonce: u64) -> (Vec<u8>, Vec<u8>) {
    (
        account_key(&[tag; 32]),
        encode_account_v1(&AccountStateV1 { balance, nonce }).to_vec(),
    )
}

/// The default pre-state pairs (state as of the end of height T-1).
pub fn default_pre_state() -> Vec<(Vec<u8>, Vec<u8>)> {
    vec![acct(0xA1, 1_000, 0), acct(0xB2, 500, 3), acct(0xC3, 42, 1)]
}

/// The default block-T effect (one new account).
pub fn default_step_state() -> Vec<(Vec<u8>, Vec<u8>)> {
    vec![acct(0xD4, 9_999, 0)]
}

/// How the fixture provides certification evidence for the current root.
pub enum Evidence {
    /// Dense rows: block(T+1) plus the qc(T+1) row (installed-dir shape).
    QcRow,
    /// Pipeline shape of a fresh healthy-node copy: blocks T+1 and T+2 stored
    /// at receipt, KEY_HIGHEST_QC certifying T+2, no dense qc rows above T.
    HqcDescent,
}

pub struct FixtureSpec {
    pub t: u64,
    pub pre_state: Vec<(Vec<u8>, Vec<u8>)>,
    pub step_state: Vec<(Vec<u8>, Vec<u8>)>,
    pub oneshot: bool,
    pub evidence: Evidence,
    pub voters: Vec<usize>,
}

impl Default for FixtureSpec {
    fn default() -> Self {
        Self {
            t: 7,
            pre_state: default_pre_state(),
            step_state: default_step_state(),
            oneshot: false,
            evidence: Evidence::QcRow,
            voters: vec![0, 1, 3],
        }
    }
}

pub struct Fixture {
    pub tmp: TmpDir,
    pub t: u64,
    pub r0: [u8; 32],
    pub r1: [u8; 32],
    pub block_t: Block,
    pub block_t1: Block,
}

impl Fixture {
    pub fn db_arg(&self) -> String {
        self.tmp.path_str()
    }

    /// Reopen the fixture DB for raw post-build mutations. Callers must drop
    /// the handle before invoking the a0 binary (RocksDB holds a LOCK file).
    pub fn reopen(&self) -> RocksKv {
        RocksKv::open(&self.tmp.0).expect("reopen fixture db")
    }
}

/// Build a synthetic audited-copy fixture:
/// state through height T (root r1 stored), cursors committed=executed=T,
/// block rows for T and T+1 (T+1 carries r1 as its pre-state), and
/// certification evidence per `spec.evidence`.
pub fn build_fixture(tag: &str, spec: FixtureSpec) -> Fixture {
    let tmp = TmpDir::new(tag);
    let mut db = RocksKv::open(&tmp.0).expect("open fixture db");

    let r0 = if spec.oneshot {
        apply_state_oneshot(&mut db, &spec.pre_state)
    } else {
        apply_state_chunked(&mut db, &spec.pre_state)
    };

    let block_t = Block {
        height: spec.t,
        round: 0,
        parent_hash: [0x55; 32],
        state_root: r0,
        txs: vec![],
    };

    let r1 = if spec.oneshot {
        apply_state_oneshot(&mut db, &spec.step_state)
    } else {
        apply_state_chunked(&mut db, &spec.step_state)
    };

    let block_t1 = Block {
        height: spec.t + 1,
        round: 0,
        parent_hash: hash_block_v1(&block_t).expect("hash block t"),
        state_root: r1,
        txs: vec![],
    };

    db.put(
        &block_key(spec.t),
        &encode_block_v1(&block_t).expect("encode block t"),
    )
    .expect("put block t");
    db.put(
        &block_key(spec.t + 1),
        &encode_block_v1(&block_t1).expect("encode block t1"),
    )
    .expect("put block t1");

    match spec.evidence {
        Evidence::QcRow => {
            let qc1 = make_qc(&block_t1, &spec.voters);
            let qc1_bytes = encode_qc_v1(&qc1).expect("encode qc1");
            db.put(&qc_key(spec.t + 1), &qc1_bytes).expect("put qc row");
            db.put(KEY_HIGHEST_QC, &qc1_bytes).expect("put highest qc");
        }
        Evidence::HqcDescent => {
            let block_t2 = Block {
                height: spec.t + 2,
                round: 0,
                parent_hash: hash_block_v1(&block_t1).expect("hash block t1"),
                state_root: r1,
                txs: vec![],
            };
            db.put(
                &block_key(spec.t + 2),
                &encode_block_v1(&block_t2).expect("encode block t2"),
            )
            .expect("put block t2");
            let qc2 = make_qc(&block_t2, &spec.voters);
            db.put(KEY_HIGHEST_QC, &encode_qc_v1(&qc2).expect("encode qc2"))
                .expect("put highest qc");
        }
    }

    db.put(KEY_COMMITTED_HEIGHT, &spec.t.to_be_bytes())
        .expect("put committed");
    db.put(KEY_EXECUTED_HEIGHT, &spec.t.to_be_bytes())
        .expect("put executed");

    drop(db);

    Fixture {
        tmp,
        t: spec.t,
        r0,
        r1,
        block_t,
        block_t1,
    }
}

/// Run the a0 binary with the given args; returns (exit_code, stdout, stderr).
pub fn run_a0(args: &[&str]) -> (i32, String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_a0"))
        .args(args)
        .output()
        .expect("spawn a0");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Extract the root hex from the `RESULT PASS height=.. root=<hex>` line.
pub fn parse_result_root(stdout: &str) -> String {
    for line in stdout.lines() {
        if line.starts_with("RESULT PASS") {
            if let Some(idx) = line.find("root=") {
                return line[idx + 5..].trim().to_string();
            }
        }
    }
    panic!("no RESULT PASS root= line in stdout:\n{stdout}");
}
