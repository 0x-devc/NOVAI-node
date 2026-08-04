//! Consensus engine for NOVAI v1.
//!
//! Week 6: Propose → Vote → QC formation (no commit yet).

#![forbid(unsafe_code)]

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus_types::codec::{
    decode_block_v1, decode_qc_v1, decode_voted_view_v1, encode_block_v1, encode_qc_v1,
    encode_voted_view_v1,
};
use novai_consensus_types::{Block, Timeout, Vote, QC};
use novai_state::{
    block_key, qc_key, Kv, KvBatch, KEY_COMMITTED_HEIGHT, KEY_HIGHEST_QC, KEY_LOCKED_QC,
    KEY_VOTED_VIEW,
};
use novai_types::{Address, TxV1};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// ========== WEEK 8: TIMEOUT CONFIGURATION ==========

/// Base timeout duration in milliseconds.
/// This is the timeout for round 0.
/// NOTE: 1 second allows fast recovery from missed proposals while still
/// giving enough time for vote collection on a local network.
pub const BASE_TIMEOUT_MS: u64 = 1000; // 1 second

/// Timeout multiplier for exponential backoff.
/// Each round doubles the timeout.
pub const TIMEOUT_MULTIPLIER: u64 = 2;

/// Maximum timeout duration in milliseconds.
/// Prevents unbounded timeout growth.
pub const MAX_TIMEOUT_MS: u64 = 60_000; // 60 seconds

/// Number of committed blocks to retain in memory caches.
/// Provides safety margin for the 3-chain commit rule and sync requests.
/// The 3-chain commit rule strictly only needs the last 3 blocks; this
/// margin keeps a small buffer for in-flight work without retaining
/// excess state on memory-constrained deployments.
/// Blocks older than `committed_height - CACHE_RETAIN_DEPTH` are evicted
/// from in-memory caches only (DB is never touched).
pub const CACHE_RETAIN_DEPTH: u64 = 5;

/// Number of committed blocks to retain on disk (RocksDB).
/// When a new block is committed, blocks and QCs older than
/// `committed_height - PRUNE_RETAIN_BLOCKS` are deleted from disk
/// as part of the atomic commit batch.
///
/// The 3-chain commit rule only requires the last 3 blocks; this depth
/// gives headroom for catch-up sync while bounding disk and compaction
/// cost. Late-joining nodes outside this window must use chunked sync.
///
/// LOAD-BEARING FOR DISASTER RECOVERY (WEDGE-20260718): the deletion
/// floor is measured from COMMITTED height and the deletes ride the
/// atomic commit batch (`persist_commit_atomic` step 6). Because of that
/// coupling, the 20260718 commit freeze froze pruning with it, and the
/// committed window plus the floor QC row survived five days of frontier
/// runaway, which is what made offline recovery possible. Any future
/// refactor that decouples pruning from the commit batch (a background
/// GC, a startup sweeper, an off-thread retention task) or measures the
/// floor from the consensus/QC clock MUST carry a commit-stall halt with
/// it. tests/gate_prune_commit_coupling.rs pins both properties.
pub const PRUNE_RETAIN_BLOCKS: u64 = 50_000;

/// Commit-window rule (incident WEDGE-20260718): the maximum number of
/// heights this node will propose or vote ABOVE its own committed height.
///
/// In the 20260718 incident a host resource event froze commits while
/// consensus kept certifying new heights for five days: the frontier ended
/// 818,258 heights above the committed floor, the durable vote marks were
/// poisoned all the way up, the fleet left its own retention arithmetic,
/// and recovery required fleet-wide offline surgery. Nothing bounded the
/// climb. This constant is that bound.
///
/// Sizing: the healthy pipeline depth under the 3-chain rule is 2 to 3
/// heights, and catch-up sync commits as it goes, so the window slides and
/// never throttles a healthy or syncing fleet. 1024 gives two orders of
/// magnitude of headroom over the healthy depth (and two 500-block sync
/// chunks) while staying far inside PRUNE_RETAIN_BLOCKS (50,000), so a
/// parked frontier is always within every peer's retention window and a
/// parked node recovers by plain restart plus sync. At the fleet's
/// measured cadence (about 4 blocks/s) a fleet-wide commit freeze parks
/// consensus in about four minutes instead of climbing for days.
///
/// Enforcement points: `propose_block_with_budget` (refuse to build),
/// `verify_block` (refuse to vote), `note_self_vote` (backstop: no self
/// vote above the bound can ever be recorded), and the node's proposer
/// intent check. QC ADOPTION is deliberately NOT gated: a quorum
/// certificate is proof the fleet accepted those votes, and refusing to
/// adopt it would strand a behind node without a sync target. The fleet
/// property comes from every correct node refusing to CAST votes above its
/// own bound: with n=4 and quorum 3, two correct parked nodes make
/// certifying past committed + COMMIT_WINDOW impossible.
pub const COMMIT_WINDOW: u64 = 1_024;

/// Calculate timeout duration for a given round using the default base timeout.
///
/// Uses exponential backoff: `min(BASE_TIMEOUT_MS * 2^round, MAX_TIMEOUT_MS)`
#[must_use]
pub fn timeout_for_round(round: u64) -> u64 {
    timeout_for_round_with_base(round, BASE_TIMEOUT_MS)
}

/// Calculate timeout duration for a given round with a configurable base timeout.
///
/// Uses exponential backoff: `min(base_ms * 2^round, MAX_TIMEOUT_MS)`
///
/// # Examples (with base_ms=1000)
/// - Round 0: 1000ms (1s)
/// - Round 1: 2000ms (2s)
/// - Round 2: 4000ms (4s)
/// - Round 5: 32000ms (32s)
/// - Round 6+: 60000ms (60s, capped)
#[must_use]
pub fn timeout_for_round_with_base(round: u64, base_ms: u64) -> u64 {
    timeout_for_round_capped(round, base_ms, MAX_TIMEOUT_MS)
}

/// L-04: Configurable variant that accepts a custom max timeout cap.
/// WAN deployments may need higher caps (e.g., 300_000ms) to avoid
/// consensus livelock under high latency.
#[must_use]
pub fn timeout_for_round_capped(round: u64, base_ms: u64, max_timeout_ms: u64) -> u64 {
    // Prevent overflow: cap the shift at a reasonable value
    // 2^16 * 2000 = 131_072_000 which is > MAX_TIMEOUT_MS
    let effective_round = round.min(16);

    let timeout = base_ms.saturating_mul(TIMEOUT_MULTIPLIER.saturating_pow(effective_round as u32));
    timeout.min(max_timeout_ms)
}

/// Trait for processing AI state updates during block commit.
/// Implementations will be provided in later weeks (Week 17+).
pub trait AiCommitHook {
    /// Called when blocks are committed. Returns AI-related WriteOps
    /// that must be atomically persisted with the block commit.
    fn on_commit(&self, blocks: &[Block]) -> Vec<novai_state::WriteOp>;
}

/// No-op implementation for current phase (no AI processing yet).
pub struct NoopAiHook;

impl AiCommitHook for NoopAiHook {
    fn on_commit(&self, _blocks: &[Block]) -> Vec<novai_state::WriteOp> {
        Vec::new()
    }
}

/// Consensus engine errors.
#[derive(Debug)]
pub enum ConsensusError {
    /// Invalid block (verification failed).
    InvalidBlock(String),
    /// Invalid vote (signature or format).
    InvalidVote(String),
    /// QC formation failed.
    QcFormationFailed(String),
    /// State error.
    StateError(String),
    /// Codec error.
    CodecError(String),
    /// Crypto error.
    CryptoError(String),
    /// Not leader for this height/round.
    NotLeader,
    /// Already proposed for this height/round.
    AlreadyProposed,
    /// Commit-window rule (WEDGE-20260718): the intended height is more
    /// than COMMIT_WINDOW heights above the committed height. Commits have
    /// stalled and the frontier is parked; proposing and self-voting are
    /// refused until commits advance.
    CommitWindowExceeded {
        /// The refused proposal or vote height.
        height: u64,
        /// This node's committed height at refusal time.
        committed_height: u64,
    },
}

/// Consensus state for a single node.
pub struct ConsensusState {
    /// Current consensus height (last committed + 1).
    pub height: u64,
    /// Current round within this height.
    pub round: u64,
    /// Highest QC seen.
    pub highest_qc: Option<QC>,
    /// Pending votes by block hash.
    pub pending_votes: HashMap<[u8; 32], Vec<Vote>>,
    /// Our validator address.
    pub our_address: Address,
    /// Last proposed (height, round) to prevent spam.
    pub last_proposed: Option<(u64, u64)>,
    /// Voters in current round (deduplication).
    pub voted_in_round: HashSet<Address>,
    /// Highest committed height.
    pub committed_height: u64,
    /// Block cache by height (for commit rule). Uses Arc to avoid
    /// cloning full blocks (50-100KB) on every proposal.
    pub block_cache: HashMap<u64, Arc<Block>>,
    /// QC cache by height (for commit rule).
    pub qc_cache: HashMap<u64, QC>,
    /// Block cache by hash (for chain-following in commit rule).
    pub block_by_hash: HashMap<[u8; 32], Arc<Block>>,
    /// Pending timeouts by (height, round).
    pub pending_timeouts: HashMap<(u64, u64), Vec<Timeout>>,
    /// Addresses that already sent timeout in current round (deduplication).
    pub timed_out_in_round: HashSet<Address>,
    /// Total view changes (round advances due to timeouts) since node start.
    pub view_changes_total: u64,
    /// Txs from the last block we proposed. Recovered to mempool if the
    /// block is abandoned (round change / view change before commit).
    pub last_proposed_txs: Vec<TxV1>,
    /// Hash of the last block we proposed. Used by `apply_commits` to detect
    /// whether our proposal was committed (clear buffered txs) or orphaned by
    /// a sibling block (keep buffered txs so `take_abandoned_txs` can recover).
    pub last_proposed_block_hash: Option<[u8; 32]>,
    /// Locked QC: the highest QC this node has adopted on its own branch
    /// (535004 Layer 4 safety lock). Advances monotonically in height at QC
    /// adoption (the 1-chain) and is NEVER cleared by round / commit / view
    /// resets. Gates voting and QC migration via `safe_to_extend`, so a node
    /// cannot adopt or vote a conflicting same-height branch.
    pub locked_qc: Option<QC>,
    /// Durable vote high-water mark (gate 9): the highest (height, round) this
    /// node has voted at. Advances monotonically, is NEVER cleared by round /
    /// commit / view resets, and is force-fsynced before a vote is observable on
    /// the network and restored on recovery, so a restarted node never votes
    /// twice at one view. Keyed by (height, round), not height, so a legitimate
    /// higher-round re-proposal after a view change is still admitted.
    pub voted_view: Option<(u64, u64)>,
    /// Speculative execution results for uncommitted (pending) blocks, keyed by
    /// block hash (gate wedge-276272). The propose and vote paths use these to
    /// resolve a parent state view without re-executing ancestors on every call.
    /// Populated on verified execution, evicted at commit. Empty until the
    /// execute-before-vote paths are wired, so it is inert until then.
    pub pending_exec: HashMap<[u8; 32], PendingExec>,
}

/// Speculative execution result for one uncommitted (pending) block. Keyed by
/// block hash in [`ConsensusState::pending_exec`]. `write_set` is dropped to
/// `None` for entries beyond the newest few pending heights (a memory bound);
/// those are recomputed on demand from the stored blocks, so the entry is always
/// reconstructable. `post_root` is retained for every entry.
///
/// Stage B (gate ACCEL): `outcomes` carries the per-tx applied/skipped record
/// so the commit path keeps log parity without re-deriving it, and
/// `write_set_checksum` is a blake3 digest of the write set recorded at cache
/// time and re-verified at take time, so a corrupted entry degrades to a
/// re-execution cache miss instead of being applied.
#[derive(Debug, Clone)]
pub struct PendingExec {
    pub height: u64,
    pub post_root: [u8; 32],
    pub write_set: Option<std::collections::BTreeMap<Vec<u8>, Option<Vec<u8>>>>,
    pub outcomes: Vec<novai_execution::TxOutcome>,
    pub write_set_checksum: [u8; 32],
}

/// Deterministic blake3 digest of a pending write set: length-prefixed key,
/// then a put/delete tag, then the length-prefixed value for puts. BTreeMap
/// iteration is sorted, so the digest is a pure function of the set's content.
fn pending_write_set_checksum(
    write_set: &std::collections::BTreeMap<Vec<u8>, Option<Vec<u8>>>,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for (key, value) in write_set {
        hasher.update(&(key.len() as u64).to_le_bytes());
        hasher.update(key);
        match value {
            Some(v) => {
                hasher.update(&[1]);
                hasher.update(&(v.len() as u64).to_le_bytes());
                hasher.update(v);
            }
            None => {
                hasher.update(&[0]);
            }
        }
    }
    *hasher.finalize().as_bytes()
}

impl ConsensusState {
    /// Create new consensus state.
    pub fn new(our_address: Address) -> Self {
        Self {
            height: 0,
            round: 0,
            highest_qc: None,
            pending_votes: HashMap::new(),
            our_address,
            last_proposed: None,
            voted_in_round: HashSet::new(),
            committed_height: 0,
            block_cache: HashMap::new(),
            qc_cache: HashMap::new(),
            block_by_hash: HashMap::new(),
            pending_timeouts: HashMap::new(),
            timed_out_in_round: HashSet::new(),
            view_changes_total: 0,
            last_proposed_txs: Vec::new(),
            last_proposed_block_hash: None,
            locked_qc: None,
            voted_view: None,
            pending_exec: HashMap::new(),
        }
    }

    /// Propose a block (leader only) with the full MAX_BLOCK_SIZE tx budget.
    ///
    /// # Errors
    /// Returns error if not leader or block building fails.
    pub fn propose_block<K>(
        &mut self,
        mempool: &mut mempool::TxMempool,
        nonce_provider: &impl mempool::NonceProvider,
        state_db: &K,
        validator_set: &[Address],
    ) -> Result<Block, ConsensusError>
    where
        K: novai_state::Kv,
        K::Error: std::fmt::Debug,
    {
        self.propose_block_with_budget(
            mempool,
            nonce_provider,
            state_db,
            validator_set,
            novai_types::MAX_BLOCK_SIZE,
        )
    }

    /// Propose a block (leader only) with an explicit tx-byte budget.
    ///
    /// F3 proposer guard Layer 1: the node passes a budget derived from
    /// the runtime wire send cap minus the measured proposal envelope
    /// (SignedProposal wrapper + block header + justify QC + wire bytes),
    /// so the assembled envelope always encodes under the cap instead of
    /// dying at broadcast after `last_proposed` is irreversibly set. The
    /// budget is clamped to MAX_BLOCK_SIZE; the verifier's block-size
    /// rules (verify_block) are untouched, so a smaller proposer budget
    /// is always compatible.
    ///
    /// # Errors
    /// Returns error if not leader or block building fails.
    pub fn propose_block_with_budget<K>(
        &mut self,
        mempool: &mut mempool::TxMempool,
        nonce_provider: &impl mempool::NonceProvider,
        state_db: &K,
        validator_set: &[Address],
        tx_byte_budget: usize,
    ) -> Result<Block, ConsensusError>
    where
        K: novai_state::Kv,
        K::Error: std::fmt::Debug,
    {
        let tx_budget = tx_byte_budget.min(novai_types::MAX_BLOCK_SIZE);
        // Block height should be max(committed_height, highest_qc_height) + 1
        // This ensures we don't propose conflicting blocks after a QC forms
        let next_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        // Commit-window rule (WEDGE-20260718): refuse to build a block more
        // than COMMIT_WINDOW heights above committed. Checked before every
        // side effect (mempool drain, last_proposed) so a parked leader
        // ticks quietly and loses nothing.
        if !self.within_commit_window(next_height) {
            return Err(ConsensusError::CommitWindowExceeded {
                height: next_height,
                committed_height: self.committed_height,
            });
        }

        // Check if already proposed for this height/round
        let proposed_key = (next_height, self.round);
        if self.last_proposed == Some(proposed_key) {
            return Err(ConsensusError::AlreadyProposed);
        }

        // Check if we're the leader
        let leader = self.compute_leader(validator_set)?;
        if leader != self.our_address {
            return Err(ConsensusError::NotLeader);
        }

        // Drain ready transactions from mempool with size-aware filtering.
        // Drain up to MAX_TXS_PER_BLOCK candidates, then filter by cumulative
        // block size. Txs that don't fit are returned to the mempool.
        let mempool_size_before = mempool.len();
        let mut candidates = mempool.drain_ready(novai_types::MAX_TXS_PER_BLOCK, nonce_provider);
        tracing::debug!(
            tx_count = candidates.len(),
            mempool_size_before,
            mempool_remaining = mempool.len(),
            "drain_ready returned"
        );
        let mut txs = Vec::new();
        let mut block_bytes = 0usize;
        let mut overflow = Vec::new();
        for tx in candidates.drain(..) {
            let size = novai_codec::tx_encoded_size(&tx);
            if block_bytes + size > tx_budget {
                overflow.push(tx);
            } else {
                block_bytes += size;
                txs.push(tx);
            }
        }
        // Re-insert overflow txs that didn't fit in this block.
        // NOTE: re-inserted overflow txs lose original FIFO ordering. Acceptable
        // for now; a size-aware drain_ready() would preserve ordering.
        for tx in overflow {
            let _ = mempool.reinsert_unchecked(tx);
        }

        // Compute parent hash (from highest_qc if exists, else genesis)
        let parent_hash = if let Some(ref qc) = self.highest_qc {
            qc.block_hash
        } else {
            [0u8; 32] // Genesis parent
        };

        // Execute the drained txs against the resolved parent post-state in a
        // non-persisting overlay and stamp the POST-execution root (gate
        // wedge-276272): the header state_root now means post-state(this height).
        // If the parent state is unresolvable (its body not yet available), skip
        // quietly like NotLeader and let the missing-block sync fire; a leader that
        // cannot resolve its parent could only have proposed a stale root anyway.
        let exec = {
            let parent_view = self
                .resolve_parent_state(parent_hash, next_height - 1, state_db)
                .map_err(|_| ConsensusError::NotLeader)?;
            novai_execution::execute_block_to_root(&parent_view, &txs, next_height)
                .map_err(|e| ConsensusError::StateError(format!("propose execute: {e:?}")))?
        };
        let state_root = exec.post_root;

        // Build block
        let block = Block {
            height: next_height,
            round: self.round,
            parent_hash,
            state_root,
            txs,
        };

        // Mark as proposed and save txs (plus block hash) for abandonment-aware
        // recovery. The hash lets `apply_commits` distinguish "our block was
        // committed" (clear buffer) from "a sibling committed at our height,
        // we were orphaned" (keep buffer for `take_abandoned_txs`).
        let block_hash = novai_consensus_types::codec::hash_block_v1(&block)
            .map_err(|e| ConsensusError::CodecError(format!("{e:?}")))?;
        self.last_proposed = Some((block.height, block.round));
        self.last_proposed_txs = block.txs.clone();
        self.last_proposed_block_hash = Some(block_hash);

        // Cache this block's speculative execution so our next proposals and the
        // followers' votes can resolve it as a parent (gate wedge-276272).
        self.note_pending_exec(
            block_hash,
            block.height,
            exec.post_root,
            exec.write_set,
            exec.outcomes,
        );

        Ok(block)
    }

    /// Take txs from the last *abandoned* proposal for mempool recovery.
    ///
    /// Only returns txs when the proposal is provably abandoned:
    ///   - The round has advanced past our proposed round (view change), or
    ///   - `apply_commits` cleared `last_proposed` without matching our hash
    ///     (sibling block committed at our height — we were orphaned).
    ///
    /// While the proposal is still in flight (no commit, no view change),
    /// returns an empty Vec. Recovering in-flight txs would cause duplicate
    /// inclusion across consecutive proposals during the 3-chain commit delay.
    ///
    /// The caller should still nonce-filter (reinsert iff
    /// `tx.nonce >= expected_nonce`) as a defence-in-depth check.
    pub fn take_abandoned_txs(&mut self) -> Vec<TxV1> {
        if self.last_proposed_txs.is_empty() {
            return Vec::new();
        }

        // In-flight: same round as when we proposed and `apply_commits` has
        // not run since (it would have cleared `last_proposed` or our hash).
        // Returning txs here would re-include them in the next proposal while
        // the original is still travelling toward commit.
        if let Some((_, proposed_round)) = self.last_proposed {
            if self.round == proposed_round {
                return Vec::new();
            }
        }

        // Either round advanced (view change) or `last_proposed` was cleared
        // by `apply_commits` and our hash was not in the committed batch
        // (orphan). Both are real abandonment — return the txs.
        self.last_proposed_block_hash = None;
        std::mem::take(&mut self.last_proposed_txs)
    }

    /// Verify a proposed block and return its speculative execution result.
    ///
    /// The header `state_root` is compared against the POST-execution root computed
    /// over the resolved parent state (gate wedge-276272); signatures are verified
    /// before execution. A refusal returns `Err` and records nothing (the caller's
    /// gate-9 and persist steps run only on `Ok`). The returned `BlockExecution`
    /// lets the caller cache the pending state without re-executing.
    ///
    /// # Errors
    /// Returns error if the block is invalid, the parent is unresolvable, or the
    /// computed post-root does not match the header.
    pub fn verify_block_execute<K>(
        &self,
        block: &Block,
        state_db: &K,
    ) -> Result<novai_execution::BlockExecution, ConsensusError>
    where
        K: novai_state::Kv,
        K::Error: std::fmt::Debug,
    {
        // --- Size limit enforcement (consensus-critical) ---
        // Uses the same tx_encoded_size() as the block proposer. Any divergence
        // between these checks would cause a consensus split.
        if block.txs.len() > novai_types::MAX_TXS_PER_BLOCK {
            return Err(ConsensusError::InvalidBlock(format!(
                "block has {} txs, exceeds limit of {}",
                block.txs.len(),
                novai_types::MAX_TXS_PER_BLOCK
            )));
        }

        let mut block_tx_bytes = 0usize;
        for tx in &block.txs {
            let size = novai_codec::tx_encoded_size(tx);
            if size > novai_types::MAX_TX_SIZE {
                return Err(ConsensusError::InvalidBlock(format!(
                    "tx encoded size {} exceeds limit of {}",
                    size,
                    novai_types::MAX_TX_SIZE
                )));
            }
            block_tx_bytes += size;
        }

        if block_tx_bytes > novai_types::MAX_BLOCK_SIZE {
            return Err(ConsensusError::InvalidBlock(format!(
                "block payload {} bytes exceeds limit of {}",
                block_tx_bytes,
                novai_types::MAX_BLOCK_SIZE
            )));
        }

        // Expected height is max(committed_height, highest_qc_height) + 1
        let expected_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        // Check height is next
        if block.height != expected_height {
            return Err(ConsensusError::InvalidBlock(format!(
                "Height mismatch: expected {}, got {}",
                expected_height, block.height
            )));
        }

        // Commit-window rule (WEDGE-20260718): refuse to vote for a block
        // more than COMMIT_WINDOW heights above our committed height. While
        // commits stall the frontier parks at committed + COMMIT_WINDOW
        // instead of climbing unbounded; a legitimately behind node
        // re-admits the frontier as its own sync commits slide the window
        // forward. Sync itself never passes through here.
        if !self.within_commit_window(block.height) {
            return Err(ConsensusError::InvalidBlock(format!(
                "commit window: refusing to vote for block at height {} while committed height is {} (bound = committed + {})",
                block.height, self.committed_height, COMMIT_WINDOW
            )));
        }

        // Check parent hash matches highest QC
        let expected_parent = if let Some(ref qc) = self.highest_qc {
            qc.block_hash
        } else {
            [0u8; 32] // Genesis
        };

        if block.parent_hash != expected_parent {
            return Err(ConsensusError::InvalidBlock(
                "Parent hash mismatch".to_string(),
            ));
        }

        // 535004 Layer 4 voting gate: refuse to vote a block whose certifying
        // QC (our highest_qc, which the parent check above ties this block to)
        // is not safe to extend under the lock. The migration gate keeps
        // highest_qc on the locked branch so this is normally satisfied; it is
        // the explicit safety-rule statement and defends any path that set
        // highest_qc without the gate (for example a reload).
        if let Some(ref hqc) = self.highest_qc {
            if !self.safe_to_extend(hqc) {
                return Err(ConsensusError::InvalidBlock(
                    "block extends a QC that conflicts with the locked QC (535004 lock)"
                        .to_string(),
                ));
            }
        }

        // Verify all transaction signatures BEFORE execution: execution must never
        // run on unauthenticated payloads (gate wedge-276272 moves signatures ahead
        // of the root/execution step, which today's one memcmp made harmless).
        for tx in &block.txs {
            // 1. Verify address matches pubkey
            let pubkey = novai_crypto::pubkey_from_bytes(&tx.pubkey)
                .map_err(|e| ConsensusError::CryptoError(format!("{e:?}")))?;

            let expected_addr = novai_crypto::address_from_pubkey(&pubkey);
            if tx.from != expected_addr {
                return Err(ConsensusError::InvalidBlock(format!(
                    "Address mismatch: from={:?} but pubkey hashes to {:?}",
                    tx.from, expected_addr
                )));
            }

            // 2. Verify signature
            if !novai_crypto::verify_tx_v1(&pubkey, tx)
                .map_err(|e| ConsensusError::CryptoError(format!("{e:?}")))?
            {
                return Err(ConsensusError::InvalidBlock(format!(
                    "Invalid transaction signature for tx from {:?}",
                    tx.from
                )));
            }
        }

        // Execute against the resolved parent post-state and compare the header
        // state_root to the computed POST-execution root (gate wedge-276272). A
        // mismatch or an unresolvable parent refuses the block WITHOUT recording.
        let parent_view = self
            .resolve_parent_state(block.parent_hash, block.height - 1, state_db)
            .map_err(|e| {
                ConsensusError::InvalidBlock(format!("parent state unresolvable: {e:?}"))
            })?;
        let exec = novai_execution::execute_block_to_root(&parent_view, &block.txs, block.height)
            .map_err(|e| ConsensusError::StateError(format!("verify execute: {e:?}")))?;
        if block.state_root != exec.post_root {
            return Err(ConsensusError::InvalidBlock(format!(
                "state root mismatch: header {:02x?} computed {:02x?}",
                &block.state_root[..8],
                &exec.post_root[..8]
            )));
        }

        Ok(exec)
    }

    /// Verify a proposed block, discarding the execution result. Thin wrapper over
    /// `verify_block_execute` for callers that need only validity.
    ///
    /// # Errors
    /// Returns error if the block is invalid (see `verify_block_execute`).
    pub fn verify_block<K>(&self, block: &Block, state_db: &K) -> Result<(), ConsensusError>
    where
        K: novai_state::Kv,
        K::Error: std::fmt::Debug,
    {
        self.verify_block_execute(block, state_db).map(|_| ())
    }

    /// Commit-window rule (WEDGE-20260718): may this node extend the
    /// frontier at `height`? True iff `height` is no more than
    /// COMMIT_WINDOW heights above this node's committed height. Gates
    /// proposing and self-voting only; QC adoption and sync are
    /// deliberately ungated (see the COMMIT_WINDOW doc).
    #[must_use]
    pub fn within_commit_window(&self, height: u64) -> bool {
        height <= self.committed_height.saturating_add(COMMIT_WINDOW)
    }

    /// gate 9: may this node cast a vote at `(height, round)`? Only if it is
    /// strictly ahead of the highest view already durably voted. `None` means
    /// this node has never voted, so the first vote is allowed. The strict
    /// lexicographic compare admits a legitimate higher-round re-proposal after a
    /// view change (`(H, R+1) > (H, R)`) and a new height (`(H+1, 0) > (H, R)`),
    /// while refusing a duplicate or a regress; it never reintroduces the
    /// height-only re-proposal halt that the removed `voted_at_height` caused.
    #[must_use]
    pub fn may_vote(&self, height: u64, round: u64) -> bool {
        self.voted_view.is_none_or(|hwm| (height, round) > hwm)
    }

    /// gate 9: advance the durable vote high-water mark. Monotonic; never
    /// regresses, so it is safe to call on any accepted self-vote.
    fn record_vote(&mut self, height: u64, round: u64) {
        let candidate = (height, round);
        if self.voted_view.is_none_or(|hwm| candidate > hwm) {
            self.voted_view = Some(candidate);
        }
    }

    /// gate 9: gate and record THIS node's own vote at `(height, round)`.
    ///
    /// Refuses (without recording) a vote at a view this node already voted at
    /// or higher; otherwise advances the high-water mark. The caller is
    /// responsible for the synced persist before the vote is broadcast.
    ///
    /// # Errors
    /// Returns `InvalidVote` if this node already voted at this view or higher.
    pub fn note_self_vote(&mut self, height: u64, round: u64) -> Result<(), ConsensusError> {
        // Commit-window backstop (WEDGE-20260718): every self vote passes
        // through here (the leader via add_vote, the follower via the node
        // vote path), so refusing above the bound makes "no own vote above
        // committed + COMMIT_WINDOW" an engine invariant no caller can
        // bypass. Refuses WITHOUT recording, like the gate 9 refusal below,
        // so the durable mark stays within the window and a stalled fleet
        // stays restart recoverable.
        if !self.within_commit_window(height) {
            return Err(ConsensusError::CommitWindowExceeded {
                height,
                committed_height: self.committed_height,
            });
        }
        if !self.may_vote(height, round) {
            return Err(ConsensusError::InvalidVote(
                "durable vote guard: already voted at this (height, round) or higher".to_string(),
            ));
        }
        self.record_vote(height, round);
        Ok(())
    }

    /// Create a vote for a block.
    ///
    /// # Errors
    /// Returns error if block hashing or signing fails.
    pub fn create_vote(
        &self,
        block: &Block,
        signing_key: &SigningKey,
    ) -> Result<Vote, ConsensusError> {
        // Compute block hash
        let block_hash = novai_consensus_types::codec::hash_block_v1(block)
            .map_err(|e| ConsensusError::CodecError(format!("{e:?}")))?;

        // Create unsigned vote struct
        let unsigned_vote = Vote {
            height: block.height,
            round: block.round,
            block_hash,
            voter: self.our_address,
            signature: [0u8; 64],
            ai_signal_commitment: None,
        };

        // Encode unsigned bytes
        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned_vote);

        // Sign with domain separation
        let domain_tag = b"NOVAI_VOTE_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);

        let signature = novai_crypto::sign_bytes(signing_key, &to_sign);

        // Build final vote with signature
        let vote = Vote {
            height: block.height,
            round: block.round,
            block_hash,
            voter: self.our_address,
            signature,
            ai_signal_commitment: None,
        };

        Ok(vote)
    }

    /// Add a vote to pending votes.
    ///
    /// # Errors
    /// Returns error if vote is invalid.
    pub fn add_vote(
        &mut self,
        vote: Vote,
        validator_pubkeys: &[(Address, VerifyingKey)],
    ) -> Result<(), ConsensusError> {
        // Expected vote height is max(committed_height, highest_qc_height) + 1
        let expected_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        // Accept votes for expected height only. Votes exactly 1 behind are
        // expected stragglers (peer voted before seeing our latest QC) — drop
        // them silently since QC already formed for that height.
        if vote.height != expected_height {
            if vote.height + 1 == expected_height {
                tracing::debug!(
                    vote_height = vote.height,
                    expected_height,
                    voter = ?&vote.voter[..4],
                    "VOTE_DIAG: stale vote (1 behind), dropping"
                );
                return Ok(()); // Stale vote, silently ignore
            }
            tracing::debug!(
                vote_height = vote.height,
                expected_height,
                voter = ?&vote.voter[..4],
                vote_round = vote.round,
                "VOTE_DIAG: vote REJECTED (height mismatch)"
            );
            return Err(ConsensusError::InvalidVote(format!(
                "Vote height mismatch: expected {}, got {}",
                expected_height, vote.height
            )));
        }

        // Find voter's public key in validator set
        let pubkey = validator_pubkeys
            .iter()
            .find(|(addr, _)| *addr == vote.voter)
            .map(|(_, pk)| pk)
            .ok_or_else(|| ConsensusError::InvalidVote("Voter not in validator set".to_string()))?;

        // Check for duplicate vote from same voter in this round (BEFORE expensive signature check)
        if self.voted_in_round.contains(&vote.voter) {
            return Err(ConsensusError::InvalidVote(
                "Duplicate vote from same voter in current round (equivocation)".to_string(),
            ));
        }

        // Create unsigned vote for verification
        let unsigned_vote = Vote {
            height: vote.height,
            round: vote.round,
            block_hash: vote.block_hash,
            voter: vote.voter,
            signature: [0u8; 64],
            ai_signal_commitment: vote.ai_signal_commitment,
        };

        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned_vote);

        // Verify signature
        let domain_tag = b"NOVAI_VOTE_V1";
        let mut to_verify = Vec::new();
        to_verify.extend_from_slice(domain_tag);
        to_verify.extend_from_slice(&unsigned_bytes);

        if !novai_crypto::verify_bytes(pubkey, &to_verify, &vote.signature) {
            return Err(ConsensusError::InvalidVote("Invalid signature".to_string()));
        }

        // gate 9: durable equivocation guard for THIS node's own vote. The
        // signature is verified above, so a vote whose voter is our address is
        // genuinely ours; a peer cannot forge it. Peer votes are unaffected.
        if vote.voter == self.our_address {
            self.note_self_vote(vote.height, vote.round)?;
        }

        // Advisory AI signal logging (does NOT affect vote validity)
        if let Some(commitment) = vote.ai_signal_commitment {
            tracing::debug!(?commitment, "Vote includes AI signal");
        }

        // Mark this voter as having voted in this round
        self.voted_in_round.insert(vote.voter);

        // Add vote to pending votes (capped to prevent unbounded memory from
        // Byzantine vote spam — each block hash stores at most validator_count + 5 votes)
        let max_per_hash = validator_pubkeys.len() + 5;
        let votes_for_hash = self.pending_votes.entry(vote.block_hash).or_default();
        if votes_for_hash.len() >= max_per_hash {
            return Ok(()); // Silently drop excess votes
        }
        votes_for_hash.push(vote);

        Ok(())
    }

    /// H-11: Add a vote whose signature has already been verified by the caller.
    ///
    /// This allows `handle_vote()` in the node layer to verify signatures
    /// BEFORE acquiring the state lock, reducing lock contention.
    /// All other checks (height, round, duplicates, caps) still apply.
    ///
    /// # Errors
    /// Returns error if vote fails non-signature checks.
    pub fn add_vote_verified(
        &mut self,
        vote: Vote,
        validator_pubkeys: &[(Address, VerifyingKey)],
    ) -> Result<(), ConsensusError> {
        // Height check
        let expected_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        if vote.height != expected_height {
            if vote.height + 1 == expected_height {
                return Ok(());
            }
            return Err(ConsensusError::InvalidVote(format!(
                "Vote height mismatch: expected {}, got {}",
                expected_height, vote.height
            )));
        }

        // Voter must be in validator set
        if !validator_pubkeys
            .iter()
            .any(|(addr, _)| *addr == vote.voter)
        {
            return Err(ConsensusError::InvalidVote(format!(
                "Unknown voter {:?}",
                &vote.voter[..4]
            )));
        }

        // gate 9: durable equivocation guard for THIS node's own vote. The
        // caller verified the signature (this method's contract), so a vote whose
        // voter is our address is genuinely ours. Peer votes are unaffected.
        if vote.voter == self.our_address {
            self.note_self_vote(vote.height, vote.round)?;
        }

        // Duplicate check
        if self.voted_in_round.contains(&vote.voter) {
            return Err(ConsensusError::InvalidVote(
                "Duplicate vote from same voter in current round (equivocation)".to_string(),
            ));
        }

        // Advisory AI signal logging
        if let Some(commitment) = vote.ai_signal_commitment {
            tracing::debug!(?commitment, "Vote includes AI signal");
        }

        // Mark voted
        self.voted_in_round.insert(vote.voter);

        // Add vote (capped)
        let max_per_hash = validator_pubkeys.len() + 5;
        let votes_for_hash = self.pending_votes.entry(vote.block_hash).or_default();
        // Fix B (gate-equivocation-535004): refuse a second vote from the
        // same voter for the same block. voted_in_round catches duplicates
        // within a round, but it is cleared on round advance, so without
        // this scan the same voter's vote can land in pending_votes twice
        // across a round boundary. try_form_qc now dedups too, but keeping
        // the duplicate out of pending_votes is the cheaper first line and
        // bounds memory. It scans pending_votes directly rather than a
        // separate per-voter map, so it does not depend on any state that a
        // round advance clears. Idempotent: a duplicate is a silent no-op,
        // the same contract as the cap below.
        if votes_for_hash.iter().any(|v| v.voter == vote.voter) {
            return Ok(());
        }
        if votes_for_hash.len() >= max_per_hash {
            return Ok(());
        }
        votes_for_hash.push(vote);

        Ok(())
    }

    /// Verify that a QC received from an untrusted source is well-formed.
    ///
    /// This is the single definition of QC well-formedness for QCs that
    /// arrive from a peer: a timeout-embedded `highest_qc` (add_timeout)
    /// and a synced block's certifying QC (the Stage 2 Fix A2 sync check).
    /// It enforces, in order: no duplicate voters and no over-cap vote
    /// count (via `encode_qc_v1`, the canonical encoder), at least `quorum`
    /// DISTINCT voters, every voter present in the validator set, every
    /// vote bound to this QC's height and block hash, and every vote
    /// signature valid under the domain-separated vote encoding.
    ///
    /// Formation (`try_form_qc`) and the commit-path install
    /// (`cache_qc_and_check_commit`) deliberately do NOT call this: they
    /// validate duplicate voters via `encode_qc_v1` directly, because
    /// sub-quorum and empty-vote QCs (the genesis `justify_qc`) legitimately
    /// reach those sites and their votes are either already verified on the
    /// way in or out of scope to verify here.
    ///
    /// # Errors
    /// Returns `InvalidVote` if the QC has duplicate voters, fewer than
    /// `quorum` distinct voters, a voter outside the set, a vote bound to a
    /// different height or block, or an invalid signature.
    pub fn verify_qc_well_formed(
        qc: &QC,
        validator_pubkeys: &[(Address, VerifyingKey)],
        quorum: usize,
    ) -> Result<(), ConsensusError> {
        // 1. Canonical well-formedness: encode_qc_v1 rejects duplicate
        //    voters and an over-cap vote count. A QC that survives this has
        //    a set of DISTINCT voters, so votes.len() below is the distinct
        //    count, not a raw entry count (the Layer 2 bug was counting
        //    raw entries).
        encode_qc_v1(qc)
            .map_err(|e| ConsensusError::InvalidVote(format!("malformed QC: {e:?}")))?;

        // 2. Quorum of distinct voters.
        if qc.votes.len() < quorum {
            return Err(ConsensusError::InvalidVote(format!(
                "QC has {} distinct voters, below quorum {}",
                qc.votes.len(),
                quorum
            )));
        }

        // 3. Each voter in the set, each vote bound to this QC, each
        //    signature valid.
        for vote in &qc.votes {
            let pubkey = validator_pubkeys
                .iter()
                .find(|(addr, _)| *addr == vote.voter)
                .map(|(_, pk)| pk)
                .ok_or_else(|| {
                    ConsensusError::InvalidVote(format!(
                        "QC vote from unknown validator {:?}",
                        &vote.voter[..4]
                    ))
                })?;

            if vote.height != qc.height || vote.block_hash != qc.block_hash {
                return Err(ConsensusError::InvalidVote(
                    "QC vote not bound to QC height/block".to_string(),
                ));
            }

            let unsigned_vote = Vote {
                height: vote.height,
                round: vote.round,
                block_hash: vote.block_hash,
                voter: vote.voter,
                signature: [0u8; 64],
                ai_signal_commitment: vote.ai_signal_commitment,
            };
            let unsigned_bytes =
                novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned_vote);
            let domain_tag = b"NOVAI_VOTE_V1";
            let mut to_verify = Vec::new();
            to_verify.extend_from_slice(domain_tag);
            to_verify.extend_from_slice(&unsigned_bytes);
            if !novai_crypto::verify_bytes(pubkey, &to_verify, &vote.signature) {
                return Err(ConsensusError::InvalidVote(
                    "QC contains invalid vote signature".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Try to form a QC for a given block hash.
    ///
    /// # Errors
    /// Returns error if QC formation fails.
    pub fn try_form_qc(
        &mut self,
        block_hash: &[u8; 32],
        validator_set: &[Address],
    ) -> Result<Option<QC>, ConsensusError> {
        let votes = match self.pending_votes.get(block_hash) {
            Some(v) => v,
            None => return Ok(None),
        };

        // Check if we have quorum: 2f+1 where n = 3f+1
        let n = validator_set.len();
        let f = (n - 1) / 3;
        let quorum = 2 * f + 1;

        // Fix B (gate-equivocation-535004): dedup votes by voter BEFORE the
        // quorum count and slice. pending_votes can hold the same voter more
        // than once when a vote arrives twice across a round boundary
        // (voted_in_round is cleared on round advance), and the previous
        // votes.len() count let a 2-distinct-signer QC form at quorum 3.
        // Keeping the first occurrence per voter is deterministic in Vec
        // order, so all honest nodes select the same vote set.
        let mut seen = HashSet::new();
        let mut distinct_votes: Vec<Vote> = Vec::new();
        for vote in votes {
            if seen.insert(vote.voter) {
                distinct_votes.push(vote.clone());
            }
        }

        if distinct_votes.len() < quorum {
            return Ok(None);
        }

        // Form QC with exactly quorum DISTINCT votes.
        let qc_votes: Vec<Vote> = distinct_votes.into_iter().take(quorum).collect();

        // QC height is the view height we're forming consensus for
        let qc_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        let qc = QC {
            height: qc_height,
            round: self.round,
            block_hash: *block_hash,
            votes: qc_votes,
        };

        // Encode-validate the formed QC: encode_qc_v1 rejects duplicate
        // voters. After the dedup above this always passes, but it is the
        // canonical formation gate the spec calls for and guards against any
        // future change that reintroduces a duplicate into the formed QC.
        encode_qc_v1(&qc).map_err(|e| {
            ConsensusError::QcFormationFailed(format!("formed QC malformed: {e:?}"))
        })?;

        Ok(Some(qc))
    }

    /// Compute leader for a given view (height, round).
    /// This is the canonical leader selection function used everywhere.
    ///
    /// # Leader Selection Rule
    /// Leader index = (view_height + round) % validator_set.len()
    /// where view_height is the height we're building consensus AT (not FOR).
    ///
    /// # Examples
    /// - To propose for height=1, we're at view_height=0, so leader_idx = (0+0) % n
    /// - To vote for a block at height=1, we compute leader for view_height=0
    pub fn compute_leader_for_view(
        view_height: u64,
        round: u64,
        validator_set: &[Address],
    ) -> Result<Address, ConsensusError> {
        if validator_set.is_empty() {
            return Err(ConsensusError::InvalidBlock(
                "Empty validator set".to_string(),
            ));
        }
        let idx = (view_height.wrapping_add(round) as usize) % validator_set.len();
        Ok(validator_set[idx])
    }

    /// Compute leader for current view (convenience wrapper).
    /// Uses view_height = max(committed_height, highest_qc_height) for leader selection.
    fn compute_leader(&self, validator_set: &[Address]) -> Result<Address, ConsensusError> {
        let view_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height),
            None => self.height,
        };
        Self::compute_leader_for_view(view_height, self.round, validator_set)
    }

    // ========== WEEK 8: TIMEOUT & ROUND ADVANCE ==========

    /// Create a timeout message for the current (height, round).
    ///
    /// # Errors
    /// Returns error if signing fails.
    pub fn create_timeout(&self, signing_key: &SigningKey) -> Result<Timeout, ConsensusError> {
        // Timeout height is max(committed_height, highest_qc_height) + 1
        let timeout_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        // Create unsigned timeout struct
        let unsigned_timeout = Timeout {
            height: timeout_height,
            round: self.round,
            voter: self.our_address,
            highest_qc: self.highest_qc.clone(),
            signature: [0u8; 64],
        };

        // Encode unsigned bytes
        let unsigned_bytes =
            novai_consensus_types::codec::encode_timeout_v1_unsigned(&unsigned_timeout)
                .map_err(|e| ConsensusError::CodecError(format!("{e:?}")))?;

        // Sign with domain separation
        let domain_tag = b"NOVAI_TIMEOUT_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);

        let signature = novai_crypto::sign_bytes(signing_key, &to_sign);

        // Build final timeout with signature
        let timeout = Timeout {
            height: timeout_height,
            round: self.round,
            voter: self.our_address,
            highest_qc: self.highest_qc.clone(),
            signature,
        };

        Ok(timeout)
    }

    /// Add a timeout message from another validator.
    ///
    /// # Errors
    /// Returns error if timeout is invalid or signature verification fails.
    pub fn add_timeout(
        &mut self,
        timeout: Timeout,
        validator_pubkeys: &[(Address, VerifyingKey)],
    ) -> Result<(), ConsensusError> {
        // Fix D (gate-equivocation-535004): adopt a dominating QC from this
        // timeout BEFORE the height gate below, so a node stuck at a wrong
        // view can self-heal. Without this, a height-mismatched timeout is
        // rejected at the gate and the node never learns the dominating QC
        // that would let it catch up. The embedded QC is fully verified here
        // via verify_qc_well_formed (quorum distinct voters, each in the set,
        // each signature valid), so it is trustworthy independent of this
        // timeout wrapper's own signature, which is not checked until after
        // the gate. This is best-effort: a malformed embedded QC is ignored
        // here, not an error, and the gate below still decides the timeout's
        // own fate. The post-gate H-01 adoption is duplicated, not moved, so
        // the in-view path is unchanged (it becomes a no-op once this adopts).
        if let Some(ref qc) = timeout.highest_qc {
            let dominated = match &self.highest_qc {
                None => true,
                Some(existing) => {
                    qc.height > existing.height
                        || (qc.height == existing.height && qc.round > existing.round)
                }
            };
            if dominated {
                let n = validator_pubkeys.len();
                let f = (n - 1) / 3;
                let quorum = 2 * f + 1;
                if Self::verify_qc_well_formed(qc, validator_pubkeys, quorum).is_ok()
                    && self.safe_to_extend(qc)
                {
                    // 535004 Layer 4 migration gate + SET (1-chain): adopt and
                    // lock only when the lock permits this branch.
                    self.highest_qc = Some(qc.clone());
                    self.locked_qc = Some(qc.clone());
                }
            }
        }

        // Verify timeout is for next height. Computed AFTER the early adoption
        // above so the gate reflects a freshly self-healed view.
        // Expected timeout height is max(committed_height, highest_qc_height) + 1
        let expected_timeout_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        if timeout.height != expected_timeout_height {
            return Err(ConsensusError::InvalidVote(format!(
                "Timeout height mismatch: expected {}, got {}",
                expected_timeout_height, timeout.height
            )));
        }

        // Accept timeouts for any round at the correct height.
        // Timeouts for past rounds are harmless (won't form quorum since we've moved on).
        // Timeouts for future rounds are buffered so quorum can form when we catch up.
        // try_advance_round only checks rounds >= self.round for quorum.

        // Find voter's public key in validator set
        let pubkey = validator_pubkeys
            .iter()
            .find(|(addr, _)| *addr == timeout.voter)
            .map(|(_, pk)| pk)
            .ok_or_else(|| {
                ConsensusError::InvalidVote("Timeout voter not in validator set".to_string())
            })?;

        // Check for duplicate timeout from same voter in this specific round
        let key = (timeout.height, timeout.round);
        if let Some(existing) = self.pending_timeouts.get(&key) {
            if existing.iter().any(|t| t.voter == timeout.voter) {
                return Err(ConsensusError::InvalidVote(
                    "Duplicate timeout from same voter in this round".to_string(),
                ));
            }
        }

        // Create unsigned timeout for verification
        let unsigned_timeout = Timeout {
            height: timeout.height,
            round: timeout.round,
            voter: timeout.voter,
            highest_qc: timeout.highest_qc.clone(),
            signature: [0u8; 64],
        };

        let unsigned_bytes =
            novai_consensus_types::codec::encode_timeout_v1_unsigned(&unsigned_timeout)
                .map_err(|e| ConsensusError::CodecError(format!("{e:?}")))?;

        // Verify signature
        let domain_tag = b"NOVAI_TIMEOUT_V1";
        let mut to_verify = Vec::new();
        to_verify.extend_from_slice(domain_tag);
        to_verify.extend_from_slice(&unsigned_bytes);

        if !novai_crypto::verify_bytes(pubkey, &to_verify, &timeout.signature) {
            return Err(ConsensusError::InvalidVote(
                "Invalid timeout signature".to_string(),
            ));
        }

        // H-01 / Fix B (gate-equivocation-535004): update highest_qc if the
        // timeout carries a dominating QC, but ONLY after full
        // well-formedness verification. The previous inline check counted
        // qc.votes.len() rather than DISTINCT voters, so a duplicate-voter
        // QC could be adopted as a quorum certificate (Finding 2): three
        // copies of one voter's vote passed votes.len() == quorum and each
        // identical signature verified. Routing through verify_qc_well_formed
        // closes that, because encode_qc_v1 inside it rejects duplicate
        // voters before the quorum count, and it still checks voter
        // membership and every signature as before.
        if let Some(ref qc) = timeout.highest_qc {
            let dominated = match &self.highest_qc {
                None => true,
                Some(existing) => {
                    qc.height > existing.height
                        || (qc.height == existing.height && qc.round > existing.round)
                }
            };
            if dominated {
                let n = validator_pubkeys.len();
                let f = (n - 1) / 3;
                let quorum = 2 * f + 1;
                Self::verify_qc_well_formed(qc, validator_pubkeys, quorum)?;
                // 535004 Layer 4 migration gate + SET (1-chain): adopt and lock
                // only when the lock permits. The well-formedness check above
                // still errors on a malformed QC regardless of the lock.
                if self.safe_to_extend(qc) {
                    self.highest_qc = Some(qc.clone());
                    self.locked_qc = Some(qc.clone());
                }
            }
        }

        // Round sync: if this valid timeout is for a higher round than ours,
        // fast-forward to match. This allows restarted nodes (round 0) to
        // adopt the higher round from surviving nodes after quorum loss,
        // enabling all nodes to converge on the same round and form a TC.
        // Safe because: advancing to a higher round cannot violate safety
        // (safety depends on QC chain, not round number).
        if timeout.round > self.round {
            tracing::info!(
                old_round = self.round,
                new_round = timeout.round,
                peer = ?&timeout.voter[..4],
                "Round sync: fast-forwarding to peer's round"
            );
            self.round = timeout.round;
            self.voted_in_round.clear();
            self.timed_out_in_round.clear();
            self.last_proposed = None;
        }

        // H-01: Hard cap on pending_timeouts to prevent memory exhaustion.
        // 10,000 entries is ~2MB and far beyond normal operation.
        let total_entries: usize = self.pending_timeouts.values().map(std::vec::Vec::len).sum();
        if total_entries >= 10_000 {
            return Err(ConsensusError::InvalidVote(
                "pending_timeouts at capacity".to_string(),
            ));
        }

        // Add timeout to pending timeouts (dedup already checked above)
        self.pending_timeouts.entry(key).or_default().push(timeout);

        Ok(())
    }

    /// Try to advance to next round if we have 2f+1 timeouts.
    ///
    /// Returns true if round was advanced, false otherwise.
    pub fn try_advance_round(&mut self, validator_set: &[Address]) -> bool {
        let expected_height = match &self.highest_qc {
            Some(qc) => std::cmp::max(self.height, qc.height) + 1,
            None => self.height + 1,
        };

        let n = validator_set.len();
        let f = (n - 1) / 3;
        let quorum = 2 * f + 1;

        // Check current round and any future rounds for quorum.
        // This handles the case where we buffered timeouts for rounds ahead of us.
        let mut best_round = None;
        for &(h, r) in self.pending_timeouts.keys() {
            if h == expected_height && r >= self.round {
                if let Some(timeouts) = self.pending_timeouts.get(&(h, r)) {
                    if timeouts.len() >= quorum {
                        match best_round {
                            None => best_round = Some(r),
                            Some(prev) if r > prev => best_round = Some(r),
                            _ => {}
                        }
                    }
                }
            }
        }

        let target_round = match best_round {
            Some(r) => r,
            None => return false,
        };

        // Advance to target_round + 1
        self.round = target_round + 1;
        self.view_changes_total += 1;

        // Clear round-specific state EXCEPT pending_votes.
        // Votes are keyed by block_hash (unique per proposal). Keeping them
        // across round advances allows QCs to form even if the proposer's
        // round advanced before all votes arrived. Without this, the timeout
        // spiral becomes unrecoverable: votes accumulate, get cleared by round
        // advance, accumulate again, get cleared again — QC never forms.
        self.voted_in_round.clear();
        self.timed_out_in_round.clear();
        self.last_proposed = None;

        // H-01: Prune old pending_timeouts to prevent unbounded memory growth.
        // Keep timeouts for recent rounds only (current_round - 5 as margin).
        // The same prune_below_round is reused for block_by_hash and
        // pending_votes pruning below.
        let prune_below_round = self.round.saturating_sub(5);
        let before = self.pending_timeouts.len();
        self.pending_timeouts
            .retain(|&(_, r), _| r >= prune_below_round);
        let pruned = before - self.pending_timeouts.len();
        if pruned > 0 {
            tracing::debug!(
                pruned,
                remaining = self.pending_timeouts.len(),
                "Pruned old pending_timeouts"
            );
        }

        // H-02: Prune stale proposals and votes from old rounds to prevent
        // unbounded memory growth during round escalation (timeout spirals).
        // block_by_hash grows by 1 entry per round (each round proposes a
        // different block hash at the same height). pending_votes similarly
        // accumulates votes keyed by those stale hashes.
        // Keep blocks/votes from recent rounds only; older ones can never
        // form a QC since the proposer changes each round.
        // M-11: HashMap iteration order in retain() is non-deterministic, but this
        // is SAFE because pruning only affects local in-memory caches — NOT committed
        // state. Each validator prunes independently; the same SET of entries is removed
        // regardless of iteration order. No consensus property depends on prune order.
        {
            let bbh_before = self.block_by_hash.len();
            self.block_by_hash.retain(|_, b| {
                // Keep committed/near-committed blocks, prune stale proposals
                b.height < expected_height || b.round >= prune_below_round
            });
            let bbh_pruned = bbh_before - self.block_by_hash.len();

            // Collect surviving block hashes to filter pending_votes
            let live_hashes: HashSet<[u8; 32]> = self.block_by_hash.keys().copied().collect();
            let pv_before = self.pending_votes.len();
            self.pending_votes
                .retain(|hash, _| live_hashes.contains(hash));
            let pv_pruned = pv_before - self.pending_votes.len();

            if bbh_pruned > 0 || pv_pruned > 0 {
                tracing::debug!(
                    bbh_pruned,
                    bbh_remaining = self.block_by_hash.len(),
                    pv_pruned,
                    pv_remaining = self.pending_votes.len(),
                    "Pruned stale proposals/votes on round advance"
                );
            }
        }

        tracing::info!(
            round = self.round,
            height = expected_height,
            quorum_round = target_round,
            "ROUND ADVANCED"
        );

        true
    }

    // ========== WEEK 7: COMMIT PIPELINE ==========

    /// Cache a block for commit rule tracking.
    ///
    /// Stores block by both height and hash for chain-following.
    ///
    /// # Errors
    /// Returns error if the block cannot be encoded for hashing.
    pub fn cache_block(&mut self, block: Block) -> Result<(), ConsensusError> {
        let hash = novai_consensus_types::codec::hash_block_v1(&block)
            .map_err(|e| ConsensusError::CodecError(format!("block hash failed: {e:?}")))?;
        tracing::debug!(
            height = block.height,
            round = block.round,
            tx_count = block.txs.len(),
            hash = ?&hash[..4],
            "cache_block"
        );
        let arc = Arc::new(block);
        self.block_cache.insert(arc.height, Arc::clone(&arc));
        self.block_by_hash.insert(hash, arc);
        Ok(())
    }

    /// Number of pending write sets kept in memory. Deeper ancestors are
    /// recomputed on demand from the stored blocks (see `resolve_parent_state`),
    /// so this bounds memory only, never correctness.
    const PENDING_WRITE_SET_KEEP: usize = 8;

    /// Record a block's speculative execution (gate wedge-276272). Called on
    /// verified execution in the propose and vote paths; execution validity is
    /// independent of vote eligibility, so this runs even if gate 9 then refuses
    /// the vote. Post roots are kept for every entry; write sets are bounded.
    /// The write-set checksum is recorded here and re-verified when the commit
    /// path takes the entry (gate ACCEL Stage B).
    pub fn note_pending_exec(
        &mut self,
        block_hash: [u8; 32],
        height: u64,
        post_root: [u8; 32],
        write_set: std::collections::BTreeMap<Vec<u8>, Option<Vec<u8>>>,
        outcomes: Vec<novai_execution::TxOutcome>,
    ) {
        let write_set_checksum = pending_write_set_checksum(&write_set);
        self.pending_exec.insert(
            block_hash,
            PendingExec {
                height,
                post_root,
                write_set: Some(write_set),
                outcomes,
                write_set_checksum,
            },
        );
        self.enforce_pending_write_set_bound();
    }

    /// Keep write sets for only the newest `PENDING_WRITE_SET_KEEP` pending
    /// heights; drop older ones to post-root-only (recomputed on demand). This
    /// bounds memory in the degraded regime where commits stall while votes
    /// continue; the healthy pipeline depth (2 to 3) is always within the bound.
    fn enforce_pending_write_set_bound(&mut self) {
        let mut heights: Vec<u64> = self
            .pending_exec
            .values()
            .filter(|pe| pe.write_set.is_some())
            .map(|pe| pe.height)
            .collect();
        if heights.len() <= Self::PENDING_WRITE_SET_KEEP {
            return;
        }
        heights.sort_unstable();
        let cutoff = heights[heights.len() - Self::PENDING_WRITE_SET_KEEP];
        for pe in self.pending_exec.values_mut() {
            if pe.height < cutoff {
                pe.write_set = None;
            }
        }
    }

    /// Drop pending-exec entries at or below the committed height (committed
    /// blocks and abandoned siblings of committed heights). Called from
    /// `apply_commits`, so it fires at every commit site.
    pub fn evict_pending_exec(&mut self) {
        let committed = self.committed_height;
        self.pending_exec.retain(|_, pe| pe.height > committed);
    }

    /// Remove and return the cached speculative execution for each block about
    /// to be committed (gate ACCEL Stage B). Called at every commit site while
    /// the state lock is held, BEFORE `apply_commits` evicts the entries, so
    /// the commit path can apply the vote-time write set as one batch instead
    /// of re-executing.
    ///
    /// A block's entry is returned only when every binding holds: an entry
    /// exists under the block's hash, its write set is still retained (the
    /// memory bound drops old ones to post-root-only), its cached post root
    /// equals the block's header state root, and the write-set checksum
    /// recorded at cache time still verifies. Anything else is a cache miss
    /// (`None`): the commit path falls back to one re-execution, so
    /// correctness never depends on this cache. A post-root mismatch warns
    /// loudly (both populate paths bind root to header, so it indicates a
    /// logic bug upstream); a checksum mismatch removes the corrupt entry so
    /// nothing can reuse it. Unmatched entries stay for `evict_pending_exec`.
    pub fn take_pending_execs(&mut self, blocks: &[Block]) -> Vec<Option<PendingExec>> {
        blocks
            .iter()
            .map(|block| {
                let bh = novai_consensus_types::block_hash(block);
                let entry = self.pending_exec.get(&bh)?;
                let Some(ws) = entry.write_set.as_ref() else {
                    // Memory bound dropped the write set: a normal miss.
                    return None;
                };
                if entry.post_root != block.state_root {
                    tracing::warn!(
                        height = block.height,
                        "pending-exec post root does not match the committed header; \
                         falling back to re-execution"
                    );
                    return None;
                }
                if pending_write_set_checksum(ws) != entry.write_set_checksum {
                    tracing::warn!(
                        height = block.height,
                        "pending-exec write-set checksum mismatch; dropping the corrupt \
                         entry and falling back to re-execution"
                    );
                    self.pending_exec.remove(&bh);
                    return None;
                }
                self.pending_exec.remove(&bh)
            })
            .collect()
    }

    /// Read the committed SMT root the way the propose and vote paths do (an
    /// absent root defaults to the canonical empty root).
    fn read_committed_smt_root<K>(db: &K) -> Result<[u8; 32], ConsensusError>
    where
        K: novai_state::Kv,
        K::Error: std::fmt::Debug,
    {
        match db.get(novai_state::KEY_SMT_ROOT) {
            Ok(Some(bytes)) => novai_state::decode_smt_root_v1(&bytes)
                .map_err(|e| ConsensusError::StateError(format!("decode smt root: {e:?}"))),
            Ok(None) => Ok(novai_execution::empty_smt_root()),
            Err(e) => Err(ConsensusError::StateError(format!("read smt root: {e:?}"))),
        }
    }

    /// Recompute one block's write set by re-executing it over the committed DB
    /// plus the write sets already merged for its ancestors. When a cached post
    /// root is known, the recomputed root must match it (a determinism guard).
    fn recompute_pending_write_set<K>(
        db: &K,
        merged: &std::collections::BTreeMap<Vec<u8>, Option<Vec<u8>>>,
        block: &Block,
        expected_post_root: Option<[u8; 32]>,
    ) -> Result<std::collections::BTreeMap<Vec<u8>, Option<Vec<u8>>>, ConsensusError>
    where
        K: novai_state::Kv,
        K::Error: std::fmt::Debug,
    {
        let view = novai_execution::BlockOverlay::with_write_set(db, merged.clone());
        let exec = novai_execution::execute_block_to_root(&view, &block.txs, block.height)
            .map_err(|e| {
                ConsensusError::StateError(format!("resolve_parent_state recompute failed: {e:?}"))
            })?;
        if let Some(expected) = expected_post_root {
            if exec.post_root != expected {
                return Err(ConsensusError::StateError(format!(
                    "CONSENSUS SAFETY HALT: recomputed post root {:02x?} != cached post root \
                     {:02x?} at height {}",
                    &exec.post_root[..8],
                    &expected[..8],
                    block.height
                )));
            }
        }
        Ok(exec.write_set)
    }

    /// Resolve a `Kv` view of the POST-execution state of the parent block
    /// (`parent_hash` at `parent_height`), for executing a child block at propose
    /// or vote time (gate wedge-276272). The view layers the pending ancestors'
    /// write sets over the committed database, reconstructing post-state(parent)
    /// without persisting anything.
    ///
    /// Ancestors are walked by hash with a DB fallback by height that verifies
    /// the hash, exactly as the commit chain walk does, so a restarted node with
    /// an empty in-memory cache rebuilds from the stored blocks. Write sets come
    /// from `pending_exec` when present and are recomputed by re-execution
    /// otherwise.
    ///
    /// Two base self-consistency checks make a locally-diverged node refuse
    /// rather than build on bad state: the walk must terminate exactly at the
    /// committed tip (self-consistency 1), and the local SMT root must equal the
    /// committed tip's header state root (self-consistency 2, the post-state
    /// convention). Either failure is local divergence and returns an error in
    /// the same class as the catch-up commit halt.
    ///
    /// # Errors
    /// Returns an error if an ancestor is missing or mis-hashed, if the walk does
    /// not reach the committed tip, or if the local root has diverged.
    pub fn resolve_parent_state<'a, K>(
        &self,
        parent_hash: [u8; 32],
        parent_height: u64,
        db: &'a K,
    ) -> Result<novai_execution::BlockOverlay<'a, K>, ConsensusError>
    where
        K: novai_state::Kv,
        K::Error: std::fmt::Debug,
    {
        let committed = self.committed_height;
        if parent_height < committed {
            return Err(ConsensusError::StateError(format!(
                "resolve_parent_state: parent height {parent_height} below committed {committed}"
            )));
        }

        let committed_tip_hash = if committed == 0 {
            [0u8; 32]
        } else {
            let tip = Self::load_block(db, committed)?.ok_or_else(|| {
                ConsensusError::StateError(format!("committed tip block {committed} missing"))
            })?;
            novai_consensus_types::block_hash(&tip)
        };

        // Walk parent -> committed frontier, newest first, by hash with a DB
        // fallback by height that verifies the hash (mirrors the commit walk).
        let mut chain: Vec<Block> = Vec::new();
        let mut current_hash = parent_hash;
        for h in (committed + 1..=parent_height).rev() {
            let block = if let Some(b) = self.block_by_hash.get(&current_hash) {
                Block::clone(b)
            } else {
                let loaded = Self::load_block(db, h)?.ok_or_else(|| {
                    ConsensusError::StateError(format!(
                        "resolve_parent_state: missing block at height {h}"
                    ))
                })?;
                if novai_consensus_types::block_hash(&loaded) != current_hash {
                    return Err(ConsensusError::StateError(format!(
                        "resolve_parent_state: DB block at height {h} has the wrong hash for the parent chain"
                    )));
                }
                loaded
            };
            if block.height != h {
                return Err(ConsensusError::InvalidBlock(format!(
                    "resolve_parent_state: chain height mismatch: expected {h}, got {}",
                    block.height
                )));
            }
            current_hash = block.parent_hash;
            chain.push(block);
        }

        // Self-consistency 1: the walk must connect to the committed tip.
        if current_hash != committed_tip_hash {
            return Err(ConsensusError::StateError(
                "CONSENSUS SAFETY HALT: parent chain does not connect to the committed tip \
                 (local divergence); refusing to build on it"
                    .to_string(),
            ));
        }

        // Self-consistency 2: the local executed root must equal the committed
        // tip's header state root (post-state convention). A mismatch is local
        // divergence; refuse in the same class as the commit halt.
        if committed > 0 {
            let tip = Self::load_block(db, committed)?.ok_or_else(|| {
                ConsensusError::StateError(format!("committed tip block {committed} missing"))
            })?;
            let local_root = Self::read_committed_smt_root(db)?;
            if local_root != tip.state_root {
                return Err(ConsensusError::StateError(format!(
                    "CONSENSUS SAFETY HALT: local executed root {:02x?} diverged from committed \
                     tip header {:02x?} at height {committed}; refusing to build on it",
                    &local_root[..8],
                    &tip.state_root[..8]
                )));
            }
        }

        // Execute the ancestors forward (oldest first), layering write sets.
        chain.reverse();
        let mut merged: std::collections::BTreeMap<Vec<u8>, Option<Vec<u8>>> =
            std::collections::BTreeMap::new();
        for block in &chain {
            let bh = novai_consensus_types::block_hash(block);
            let wset = match self.pending_exec.get(&bh) {
                Some(PendingExec {
                    write_set: Some(ws),
                    ..
                }) => ws.clone(),
                Some(PendingExec { post_root, .. }) => {
                    Self::recompute_pending_write_set(db, &merged, block, Some(*post_root))?
                }
                None => Self::recompute_pending_write_set(db, &merged, block, None)?,
            };
            for (k, v) in wset {
                merged.insert(k, v);
            }
        }

        Ok(novai_execution::BlockOverlay::with_write_set(db, merged))
    }

    /// 535004 Layer 4 safety predicate. Returns whether it is safe to adopt
    /// `candidate` as `highest_qc`, or to vote a block that extends it, given
    /// this node's lock. Safe iff: this node is not locked yet (no QC adopted),
    /// the candidate certifies the same block as the lock, or the candidate is
    /// at a strictly greater height than the lock. The strict-height arm is the
    /// unlock: a genuinely higher branch is always adoptable, while a
    /// conflicting same-height (or lower) QC is refused. A conflicting branch
    /// can never out-height an honest node's lock (the overlap honest voter is
    /// locked), so the unlock is only ever satisfied by a legitimate higher
    /// branch.
    fn safe_to_extend(&self, candidate: &QC) -> bool {
        match &self.locked_qc {
            None => true,
            Some(locked) => {
                candidate.block_hash == locked.block_hash || candidate.height > locked.height
            }
        }
    }

    /// Cache a QC and check if commit rule triggers.
    ///
    /// # 3-Chain Commit Rule
    /// When QC at height H is observed, commit block at height H-2.
    /// **Verifies parent-chain linkage before committing.**
    ///
    /// Visual:
    /// ```text
    /// B(h) --QC(h)--> B(h+1) --QC(h+1)--> B(h+2) --QC(h+2)
    ///  ^                                            |
    ///  |____________________________________________|
    ///                    COMMIT (verified via parent pointers)
    /// ```
    ///
    /// # Returns
    /// - `Ok(blocks)`: List of blocks to commit (oldest first), or empty if no commit.
    /// - `Err`: Chain linkage broken or required blocks missing.
    ///
    /// # Errors
    /// Returns error if:
    /// - Certified block missing from cache
    /// - Parent chain has gaps or height mismatches
    /// - Required blocks for commit are missing
    pub fn cache_qc_and_check_commit<K>(
        &mut self,
        qc: QC,
        db: &K,
    ) -> Result<Vec<Block>, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        let qc_height = qc.height;

        // Update highest QC if this one dominates
        let dominated = match &self.highest_qc {
            None => true,
            Some(existing) => {
                qc_height > existing.height
                    || (qc_height == existing.height && qc.round > existing.round)
            }
        };
        if dominated {
            // Fix B (gate-equivocation-535004): encode-validate before
            // installing as highest_qc. encode_qc_v1 rejects duplicate
            // voters, so a duplicate-voter QC cannot become highest_qc via
            // the commit path. I deliberately use encode_qc_v1 here rather
            // than the full verify_qc_well_formed helper: a sub-quorum
            // genesis justify_qc (height 0, no votes) legitimately reaches
            // this install, and full signature verification of a justify_qc
            // is the proposal path's concern, out of scope for this fix.
            // Validating before the state clears below means a malformed QC
            // is rejected with no side effects.
            encode_qc_v1(&qc).map_err(|e| {
                ConsensusError::QcFormationFailed(format!(
                    "refusing to install malformed QC as highest_qc: {e:?}"
                ))
            })?;

            // Reset round to 0 when view height advances (new dominating QC)
            // This is critical for leader synchronization
            let old_view_height = self
                .highest_qc
                .as_ref()
                .map(|q| q.height)
                .unwrap_or(self.height);
            let new_view_height = qc.height;

            if new_view_height > old_view_height {
                let pending_vote_count: usize =
                    self.pending_votes.values().map(std::vec::Vec::len).sum();
                tracing::debug!(
                    old_view_height,
                    new_view_height,
                    pending_votes_cleared = pending_vote_count,
                    "VIEW_DIAG: view height advanced, clearing state"
                );
                self.round = 0;
                self.pending_votes.clear();
                self.voted_in_round.clear();
                self.timed_out_in_round.clear();
                self.pending_timeouts.clear();
                self.last_proposed = None;
                // Reclaim capacity after clear() — without this,
                // HashMap/HashSet backing arrays survive across every
                // view advance, accumulating high-watermark capacity
                // over millions of blocks.
                self.pending_votes.shrink_to_fit();
                self.voted_in_round.shrink_to_fit();
                self.timed_out_in_round.shrink_to_fit();
                self.pending_timeouts.shrink_to_fit();
            }

            // 535004 Layer 4 migration gate + SET (1-chain): adopt and lock only
            // when the lock permits. The only dominated-but-unsafe case is a
            // same-height higher-round conflicting QC, which never reaches the
            // view reset above (that fires only on a strict height advance), so
            // gating the swap here keeps highest_qc on the locked branch.
            // locked_qc tracks highest_qc and is NEVER cleared by the resets
            // (here, in apply_commits, or in the round-sync), only advanced.
            if self.safe_to_extend(&qc) {
                self.highest_qc = Some(qc.clone());
                self.locked_qc = Some(qc.clone());
            }
        }

        // Cache the QC
        self.qc_cache.insert(qc_height, qc.clone());

        // 3-chain rule: need QC at height >= 2
        if qc_height < 2 {
            return Ok(vec![]);
        }

        let commit_target = qc_height - 2;

        // Nothing to commit if already at or past this height
        if commit_target <= self.committed_height {
            return Ok(vec![]);
        }

        // === VERIFY CHAIN LINKAGE ===
        tracing::debug!(
            qc_height,
            commit_target,
            committed_height = self.committed_height,
            qc_hash = ?&qc.block_hash[..4],
            cache_size = self.block_by_hash.len(),
            "commit chain walk starting"
        );

        // 1. Find B_H (certified block) by QC's block_hash (with DB fallback)
        let block_h = if let Some(b) = self.block_by_hash.get(&qc.block_hash) {
            tracing::debug!(
                height = b.height,
                round = b.round,
                tx_count = b.txs.len(),
                "certified block from CACHE"
            );
            b.clone()
        } else {
            // DB fallback: load by expected height, verify hash matches
            let loaded = Self::load_block(db, qc_height)
                .map_err(|e| {
                    ConsensusError::StateError(format!(
                        "DB fallback failed for certified block at height {qc_height}: {e:?}"
                    ))
                })?
                .ok_or_else(|| {
                    ConsensusError::StateError(format!(
                        "Missing certified block for QC at height {qc_height}",
                    ))
                })?;
            let loaded_hash = novai_consensus_types::codec::hash_block_v1(&loaded)
                .map_err(|e| ConsensusError::CodecError(format!("hash failed: {e:?}")))?;
            if loaded_hash != qc.block_hash {
                return Err(ConsensusError::StateError(format!(
                    "DB block at height {qc_height} has wrong hash for QC"
                )));
            }
            self.cache_block(loaded.clone())?;
            Arc::new(loaded)
        };

        if block_h.height != qc_height {
            return Err(ConsensusError::InvalidBlock(format!(
                "QC height {} doesn't match certified block height {}",
                qc_height, block_h.height
            )));
        }

        // 2. Walk chain backwards from B_H to committed_height+1, verifying linkage
        let mut chain: Vec<Block> = Vec::new();
        let mut current_hash = qc.block_hash;

        for expected_height in (self.committed_height + 1..=qc_height).rev() {
            let (block, source) = if let Some(b) = self.block_by_hash.get(&current_hash) {
                (Block::clone(b), "cache")
            } else {
                // DB fallback: load by expected height, verify hash matches
                let loaded = Self::load_block(db, expected_height)
                    .map_err(|e| {
                        ConsensusError::StateError(format!(
                            "DB fallback at height {expected_height}: {e:?}"
                        ))
                    })?
                    .ok_or_else(|| {
                        ConsensusError::StateError(format!(
                            "Missing block at height {expected_height} (chain broken)"
                        ))
                    })?;
                let loaded_hash = novai_consensus_types::codec::hash_block_v1(&loaded)
                    .map_err(|e| ConsensusError::CodecError(format!("hash failed: {e:?}")))?;
                if loaded_hash != current_hash {
                    return Err(ConsensusError::StateError(format!(
                        "DB block at height {expected_height} has wrong hash"
                    )));
                }
                self.cache_block(loaded.clone())?;
                (loaded, "db")
            };

            tracing::debug!(
                expected_height,
                actual_height = block.height,
                round = block.round,
                tx_count = block.txs.len(),
                source,
                hash = ?&current_hash[..4],
                will_commit = expected_height <= commit_target,
                "chain walk block"
            );

            if block.height != expected_height {
                return Err(ConsensusError::InvalidBlock(format!(
                    "Chain height mismatch: expected {}, got {}",
                    expected_height, block.height
                )));
            }

            // Extract parent_hash before potential move
            current_hash = block.parent_hash;

            // Only include blocks up to commit_target (not the 2 confirmation blocks)
            if expected_height <= commit_target {
                chain.push(block);
            }
        }

        // Reverse to get oldest first
        chain.reverse();

        let total_commit_txs: usize = chain.iter().map(|b| b.txs.len()).sum();
        tracing::debug!(
            commit_blocks = chain.len(),
            total_commit_txs,
            "commit chain built"
        );

        // Verify contiguous commit (no gaps) - Fix D
        let expected_count = (commit_target - self.committed_height) as usize;
        if chain.len() != expected_count {
            return Err(ConsensusError::StateError(format!(
                "Incomplete commit chain: expected {} blocks (heights {}..={}), got {}",
                expected_count,
                self.committed_height + 1,
                commit_target,
                chain.len()
            )));
        }

        Ok(chain)
    }

    /// Mark blocks as committed and advance state.
    ///
    /// # Errors
    /// Returns error if a commit gap is detected (consensus safety violation).
    /// The caller should log evidence and halt the node gracefully.
    pub fn apply_commits(&mut self, blocks: &[Block]) -> Result<(), ConsensusError> {
        for block in blocks {
            // Safety check: no gaps in commit sequence
            let expected_height = self.committed_height + 1;
            if block.height != expected_height {
                tracing::error!(
                    expected_height,
                    actual_height = block.height,
                    committed_height = self.committed_height,
                    "CONSENSUS SAFETY VIOLATION: commit gap detected"
                );
                return Err(ConsensusError::StateError(format!(
                    "CONSENSUS SAFETY VIOLATION: commit gap! expected height {}, got {}",
                    expected_height, block.height
                )));
            }

            // Advance committed height
            self.committed_height = block.height;

            // Advance consensus height to match
            if self.height < block.height {
                self.height = block.height;
            }

            // Clear stale state for committed height
            self.block_cache.remove(&block.height);

            // Log commit
            tracing::info!(
                height = block.height,
                state_root = ?&block.state_root[..4],
                "COMMITTED block"
            );
        }

        // Disambiguate self-commit vs orphan for `last_proposed_txs`:
        // - If a committed block at our proposed height matches our hash, our
        //   block committed — clear the buffer (executor will advance nonces,
        //   nothing to recover).
        // - Otherwise (no committed block at our height, or hash mismatch),
        //   leave the buffer so `take_abandoned_txs` recovers the txs once
        //   `last_proposed` is cleared below.
        if let (Some((proposed_height, _)), Some(my_hash)) =
            (self.last_proposed, self.last_proposed_block_hash)
        {
            for block in blocks {
                if block.height == proposed_height {
                    let committed_hash = novai_consensus_types::codec::hash_block_v1(block)
                        .map_err(|e| ConsensusError::CodecError(format!("{e:?}")))?;
                    if committed_hash == my_hash {
                        self.last_proposed_txs.clear();
                        self.last_proposed_block_hash = None;
                    }
                    break;
                }
            }
        }

        // Clear pending votes and voted_in_round after commits. NOTE (535004
        // Layer 4): locked_qc is intentionally NOT cleared here; it is a safety
        // invariant that must survive commits and view changes, only advancing.
        if !blocks.is_empty() {
            self.pending_votes.clear();
            self.voted_in_round.clear();
            self.timed_out_in_round.clear();
            self.pending_timeouts.clear();
            self.last_proposed = None;

            // Reset round to 0 after successful commit
            self.round = 0;

            // Evict old blocks from in-memory caches to bound memory usage.
            self.prune_old_blocks();

            // Drop speculative pending-exec entries at or below the new committed
            // height (gate wedge-276272). Inert until the vote path populates it.
            self.evict_pending_exec();
        }

        Ok(())
    }

    /// Prune in-memory block and QC caches below the retention window.
    ///
    /// Keeps the last [`CACHE_RETAIN_DEPTH`] committed blocks as safety margin
    /// for the 3-chain commit rule and peer sync requests.
    ///
    /// **Only prunes in-memory caches. Never deletes from database.**
    /// Block sync serves historical blocks from DB.
    pub fn prune_old_blocks(&mut self) {
        if self.committed_height <= CACHE_RETAIN_DEPTH {
            return;
        }

        let prune_below = self.committed_height - CACHE_RETAIN_DEPTH;

        self.block_cache.retain(|&height, _| height >= prune_below);
        self.qc_cache.retain(|&height, _| height >= prune_below);
        self.block_by_hash
            .retain(|_, block| block.height >= prune_below);

        // MEMORY LEAK FIX: Prune pending_votes for long-committed blocks.
        // Keep votes only if ANY vote in the Vec has height >= prune_below.
        self.pending_votes
            .retain(|_, votes| votes.iter().any(|v| v.height >= prune_below));

        // MEMORY LEAK FIX: Prune pending_timeouts for old heights.
        self.pending_timeouts
            .retain(|&(height, _), _| height >= prune_below);

        // FRAGMENTATION FIX: After millions of insert/remove cycles, HashMap
        // internal capacity grows far beyond the number of live entries. The
        // allocator keeps the old backing array allocated even after retain()
        // removes entries. shrink_to_fit() releases that excess capacity.
        // Shrink every 64 heights (originally 1000, then 256) to release
        // excess capacity sooner on memory-constrained deployments.
        if self.committed_height & 0x3F == 0 {
            self.block_cache.shrink_to_fit();
            self.qc_cache.shrink_to_fit();
            self.block_by_hash.shrink_to_fit();
            self.pending_votes.shrink_to_fit();
            self.pending_timeouts.shrink_to_fit();
            self.voted_in_round.shrink_to_fit();
            self.timed_out_in_round.shrink_to_fit();
        }
    }

    /// Apply commits with AI hook integration.
    ///
    /// This is the version that should be used when AI hooks are available.
    /// It calls the AI hook to generate operations that will be persisted atomically.
    ///
    /// # Returns
    /// Returns the AI operations that should be passed to `persist_commit_atomic`.
    ///
    /// # Errors
    /// Returns error if `apply_commits` detects a consensus safety violation.
    pub fn apply_commits_with_ai_hook(
        &mut self,
        blocks: &[Block],
        ai_hook: &dyn AiCommitHook,
    ) -> Result<Vec<novai_state::WriteOp>, ConsensusError> {
        // First apply commits normally (updates in-memory state)
        self.apply_commits(blocks)?;

        // Then generate AI operations if blocks were committed
        if !blocks.is_empty() {
            Ok(ai_hook.on_commit(blocks))
        } else {
            Ok(Vec::new())
        }
    }

    /// Check for conflicting commits (fork detection).
    ///
    /// In HotStuff BFT, if a block doesn't get a QC, the next round's leader
    /// proposes a different block for the same height. This is normal behavior.
    /// A real fork would be COMMITTING two different blocks at the same height.
    ///
    /// # Errors
    /// Returns error if two different blocks conflict at or below committed_height.
    /// The caller should log the fork evidence and halt the node gracefully.
    pub fn check_no_fork(&self, block: &Block) -> Result<(), ConsensusError> {
        // Only check for forks at or below committed_height.
        // Heights above committed_height can have different proposals in different rounds.
        if block.height > self.committed_height {
            return Ok(());
        }

        if let Some(cached) = self.block_cache.get(&block.height) {
            let cached_hash = novai_consensus_types::codec::hash_block_v1(cached)
                .map_err(|e| ConsensusError::CodecError(format!("{e:?}")))?;
            let new_hash = novai_consensus_types::codec::hash_block_v1(block)
                .map_err(|e| ConsensusError::CodecError(format!("{e:?}")))?;

            if cached_hash != new_hash {
                tracing::error!(
                    height = block.height,
                    cached_hash = ?&cached_hash[..8],
                    new_hash = ?&new_hash[..8],
                    "CONSENSUS SAFETY VIOLATION: FORK DETECTED"
                );
                return Err(ConsensusError::InvalidBlock(format!(
                    "FORK DETECTED at height {}! cached={:?} new={:?}",
                    block.height,
                    &cached_hash[..8],
                    &new_hash[..8]
                )));
            }
        }

        Ok(())
    }

    /// Get committed height.
    pub fn committed_height(&self) -> u64 {
        self.committed_height
    }

    // ========== PERSISTENCE ==========

    /// Persist a block to the database.
    ///
    /// # Errors
    /// Returns error if encoding or database write fails.
    pub fn persist_block<K>(&self, db: &mut K, block: &Block) -> Result<(), ConsensusError>
    where
        K: KvBatch,
        K::Error: std::fmt::Debug,
    {
        let key = block_key(block.height);
        let value = encode_block_v1(block)
            .map_err(|e| ConsensusError::CodecError(format!("Failed to encode block: {e:?}")))?;
        db.put(&key, &value)
            .map_err(|e| ConsensusError::StateError(format!("Failed to persist block: {e:?}")))
    }

    /// Persist a QC to the database.
    ///
    /// # Errors
    /// Returns error if encoding or database write fails.
    pub fn persist_qc<K>(&self, db: &mut K, qc: &QC) -> Result<(), ConsensusError>
    where
        K: KvBatch,
        K::Error: std::fmt::Debug,
    {
        let key = qc_key(qc.height);
        let value = encode_qc_v1(qc)
            .map_err(|e| ConsensusError::CodecError(format!("Failed to encode QC: {e:?}")))?;
        db.put(&key, &value)
            .map_err(|e| ConsensusError::StateError(format!("Failed to persist QC: {e:?}")))
    }

    /// Persist committed height to the database.
    ///
    /// # Errors
    /// Returns error if database write fails.
    pub fn persist_committed_height<K>(&self, db: &mut K) -> Result<(), ConsensusError>
    where
        K: KvBatch,
        K::Error: std::fmt::Debug,
    {
        let value = self.committed_height.to_be_bytes().to_vec();
        db.put(KEY_COMMITTED_HEIGHT, &value).map_err(|e| {
            ConsensusError::StateError(format!("Failed to persist committed height: {e:?}"))
        })
    }

    /// Persist highest QC to the database.
    ///
    /// # Errors
    /// Returns error if encoding or database write fails.
    pub fn persist_highest_qc<K>(&self, db: &mut K) -> Result<(), ConsensusError>
    where
        K: KvBatch,
        K::Error: std::fmt::Debug,
    {
        if let Some(ref qc) = self.highest_qc {
            let value = encode_qc_v1(qc).map_err(|e| {
                ConsensusError::CodecError(format!("Failed to encode highest QC: {e:?}"))
            })?;
            db.put(KEY_HIGHEST_QC, &value).map_err(|e| {
                ConsensusError::StateError(format!("Failed to persist highest QC: {e:?}"))
            })?;
        }
        // 535004 Layer 4: co-persist the lock wherever highest_qc is persisted,
        // so a recovered node restores its lock and cannot vote a conflicting
        // block after a crash. locked_qc invariantly tracks highest_qc.
        if let Some(ref qc) = self.locked_qc {
            let value = encode_qc_v1(qc).map_err(|e| {
                ConsensusError::CodecError(format!("Failed to encode locked QC: {e:?}"))
            })?;
            db.put(KEY_LOCKED_QC, &value).map_err(|e| {
                ConsensusError::StateError(format!("Failed to persist locked QC: {e:?}"))
            })?;
        }
        // gate 9: co-persist the vote high-water mark wherever highest_qc is
        // persisted, mirroring locked_qc. This is a NON-synced freshness write;
        // the persist-before-broadcast guarantee is the synced persist_voted_view.
        if let Some((h, r)) = self.voted_view {
            let value = encode_voted_view_v1(h, r);
            db.put(KEY_VOTED_VIEW, &value).map_err(|e| {
                ConsensusError::StateError(format!("Failed to co-persist voted view: {e:?}"))
            })?;
        }
        Ok(())
    }

    /// Persist the durable vote high-water mark with a forced fsync (gate 9).
    ///
    /// This is the persist-before-broadcast safety guarantee: the synced write
    /// must return (WAL fsync complete) BEFORE this node's vote is observable on
    /// the network. Written once per (height, round), i.e. once per block, so the
    /// fsync cost is constant in block size and never scales with transactions.
    /// Distinct from the NON-synced co-persists in `persist_highest_qc` and
    /// `persist_commit_atomic`, which are freshness writes mirroring `locked_qc`
    /// and must NOT be relied on for the persist-before-broadcast property.
    ///
    /// # Errors
    /// Returns `StateError` if the synced write fails; the caller MUST abort the
    /// vote (not broadcast) when this errors.
    pub fn persist_voted_view<K>(&self, db: &mut K) -> Result<(), ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        if let Some((h, r)) = self.voted_view {
            let value = encode_voted_view_v1(h, r);
            db.put_synced(KEY_VOTED_VIEW, &value).map_err(|e| {
                ConsensusError::StateError(format!("Failed to persist voted view: {e:?}"))
            })?;
        }
        Ok(())
    }
    /// Persist commit state atomically (all-or-nothing).
    ///
    /// Writes blocks, QC, committed_height, and highest_qc in a single batch.
    /// If the node crashes, either all writes succeed or none do.
    ///
    /// # Errors
    /// Returns error if encoding fails or batch write fails.
    pub fn persist_commit_atomic<K>(
        &self,
        db: &mut K,
        blocks: &[Block],
        qc: &QC,
        new_committed_height: u64,
        ai_ops: Option<&[novai_state::WriteOp]>, // NEW: AI operations to commit atomically
    ) -> Result<(), ConsensusError>
    where
        K: KvBatch,
        K::Error: std::fmt::Debug,
    {
        use novai_state::WriteOp;

        let mut ops = Vec::new();

        // 1. Blocks
        for block in blocks {
            let key = block_key(block.height);
            let value = encode_block_v1(block).map_err(|e| {
                ConsensusError::CodecError(format!("Failed to encode block: {e:?}"))
            })?;
            ops.push(WriteOp::Put(key, value));
        }

        // 1b. Dense per-height certifying QCs. The triggering QC write
        // below (step 2) covers only qc.height; intermediate heights in a
        // multi-block batch, plus the genesis edge (a QC at height 1 or 2
        // never triggers a commit under the 3-chain rule), get their rows
        // here, sourced from qc_cache and verified against the committed
        // block's hash. Every failure mode degrades to an absent row plus
        // a log line, never a fabricated row and never a failed commit:
        // commit/accept decisions must be byte-for-byte unchanged by this
        // step. Dense rows ride the same PRUNE_RETAIN_BLOCKS prune as
        // blocks (step 6 deletes qc_key for every pruned height).
        for block in blocks {
            let Some(cqc) = self.qc_cache.get(&block.height) else {
                // Distinguish a benign miss (row already on disk from an
                // earlier trigger write, e.g. before a restart) from a
                // genuine gap (no certifying QC available anywhere, the
                // sync catch-up case until Stage 2 carries QCs on the wire).
                match db.get(&qc_key(block.height)) {
                    Ok(Some(_)) => tracing::debug!(
                        height = block.height,
                        "Dense QC persist: qc_cache miss but QC row already on disk"
                    ),
                    Ok(None) => tracing::warn!(
                        height = block.height,
                        "Dense QC persist: no certifying QC available for committed block, row left absent"
                    ),
                    Err(e) => tracing::warn!(
                        height = block.height,
                        error = ?e,
                        "Dense QC persist: QC row existence check failed"
                    ),
                }
                continue;
            };

            let block_hash = match novai_consensus_types::codec::hash_block_v1(block) {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!(
                        height = block.height,
                        error = ?e,
                        "Dense QC persist: block hash failed, row left absent"
                    );
                    continue;
                }
            };

            if cqc.block_hash != block_hash {
                tracing::warn!(
                    height = block.height,
                    qc_hash = ?&cqc.block_hash[..4],
                    block_hash = ?&block_hash[..4],
                    "Dense QC persist: cached QC does not certify committed block, row left absent"
                );
                continue;
            }

            match encode_qc_v1(cqc) {
                Ok(value) => ops.push(WriteOp::Put(qc_key(block.height), value)),
                Err(e) => tracing::warn!(
                    height = block.height,
                    error = ?e,
                    "Dense QC persist: certifying QC failed to encode, row left absent"
                ),
            }
        }

        // 2. QC that triggered commit
        let qc_k = qc_key(qc.height);
        let qc_v = encode_qc_v1(qc)
            .map_err(|e| ConsensusError::CodecError(format!("Failed to encode QC: {e:?}")))?;
        ops.push(WriteOp::Put(qc_k, qc_v));

        // 3. Committed height
        let ch_v = new_committed_height.to_be_bytes().to_vec();
        ops.push(WriteOp::Put(KEY_COMMITTED_HEIGHT.to_vec(), ch_v));

        // 4. Highest QC (if present)
        if let Some(ref hqc) = self.highest_qc {
            let hqc_v = encode_qc_v1(hqc).map_err(|e| {
                ConsensusError::CodecError(format!("Failed to encode highest QC: {e:?}"))
            })?;
            ops.push(WriteOp::Put(KEY_HIGHEST_QC.to_vec(), hqc_v));
        }

        // 4b. Locked QC (535004 Layer 4): co-persist the safety lock atomically
        // with highest_qc so recovery restores it.
        if let Some(ref lqc) = self.locked_qc {
            let lqc_v = encode_qc_v1(lqc).map_err(|e| {
                ConsensusError::CodecError(format!("Failed to encode locked QC: {e:?}"))
            })?;
            ops.push(WriteOp::Put(KEY_LOCKED_QC.to_vec(), lqc_v));
        }

        // 4c. gate 9: co-persist the vote high-water mark in the commit batch,
        // mirroring locked_qc. NON-synced freshness; the synced persist_voted_view
        // at vote time is the persist-before-broadcast guarantee.
        if let Some((h, r)) = self.voted_view {
            ops.push(WriteOp::Put(KEY_VOTED_VIEW.to_vec(), encode_voted_view_v1(h, r)));
        }

        // 5. AI operations (if provided)
        if let Some(ai_operations) = ai_ops {
            ops.extend_from_slice(ai_operations);
        }

        // 6. Prune old blocks and QCs from disk to cap DB size.
        // Delete block/QC data older than PRUNE_RETAIN_BLOCKS behind the
        // new committed height. This keeps RocksDB size bounded regardless
        // of chain height. Deletions are part of the atomic batch, so
        // pruning is crash-safe (either commit + prune both apply, or neither).
        //
        // LOAD-BEARING FOR DISASTER RECOVERY (WEDGE-20260718): this is the
        // ONLY deleter of the two consensus row families in the workspace,
        // its floor is the COMMITTED clock, and it cannot run without a
        // commit because it IS a clause of the commit write. That coupling
        // preserved the committed window through the incident's five-day
        // commit freeze. Do not move these deletes out of this batch and do
        // not measure the floor from the QC/consensus height;
        // tests/gate_prune_commit_coupling.rs fails on either change.
        if new_committed_height > PRUNE_RETAIN_BLOCKS {
            let prune_below = new_committed_height - PRUNE_RETAIN_BLOCKS;
            // Delete block and QC for each newly-prunable height.
            // Usually only 1 height per commit (blocks.len() == 1), but
            // batch commits may prune multiple heights.
            for block in blocks {
                let prune_height = block.height.saturating_sub(PRUNE_RETAIN_BLOCKS);
                if prune_height > 0 && prune_height <= prune_below {
                    ops.push(WriteOp::Delete(block_key(prune_height)));
                    ops.push(WriteOp::Delete(qc_key(prune_height)));
                }
            }
        }

        // Apply all writes atomically
        db.apply_batch(&ops)
            .map_err(|e| ConsensusError::StateError(format!("Atomic batch write failed: {e:?}")))?;

        Ok(())
    }

    /// Load committed height from the database.
    ///
    /// # Errors
    /// Returns error if database read fails.
    pub fn load_committed_height<K>(db: &K) -> Result<u64, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        match db.get(KEY_COMMITTED_HEIGHT) {
            Ok(Some(bytes)) => {
                if bytes.len() != 8 {
                    return Err(ConsensusError::StateError(
                        "Invalid committed height encoding".to_string(),
                    ));
                }
                let arr: [u8; 8] = bytes.try_into().expect("length verified as 8 above");
                Ok(u64::from_be_bytes(arr))
            }
            Ok(None) => Ok(0), // No committed height yet
            Err(e) => Err(ConsensusError::StateError(format!(
                "Failed to load committed height: {e:?}"
            ))),
        }
    }

    /// Load highest QC from the database.
    ///
    /// # Errors
    /// Returns error if database read or decoding fails.
    pub fn load_highest_qc<K>(db: &K) -> Result<Option<QC>, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        match db.get(KEY_HIGHEST_QC) {
            Ok(Some(bytes)) => {
                let qc = decode_qc_v1(&bytes).map_err(|e| {
                    ConsensusError::CodecError(format!("Failed to decode highest QC: {e:?}"))
                })?;
                Ok(Some(qc))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ConsensusError::StateError(format!(
                "Failed to load highest QC: {e:?}"
            ))),
        }
    }

    /// Load the locked QC (535004 Layer 4 safety lock) from the database.
    ///
    /// Returns `Ok(None)` if the row is absent (a node that never locked, or a
    /// database written before this fix). Recovery substitutes `highest_qc` in
    /// that case, so a restarted node is never briefly unlocked.
    ///
    /// # Errors
    /// Returns error if database read or decoding fails.
    pub fn load_locked_qc<K>(db: &K) -> Result<Option<QC>, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        match db.get(KEY_LOCKED_QC) {
            Ok(Some(bytes)) => {
                let qc = decode_qc_v1(&bytes).map_err(|e| {
                    ConsensusError::CodecError(format!("Failed to decode locked QC: {e:?}"))
                })?;
                Ok(Some(qc))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ConsensusError::StateError(format!(
                "Failed to load locked QC: {e:?}"
            ))),
        }
    }

    /// Load the durable vote high-water mark (gate 9) from the database.
    ///
    /// Returns `Ok(None)` if absent (a fresh node, or a DB written before this
    /// gate): absence means this node has never voted, so the first vote is
    /// allowed.
    ///
    /// # Errors
    /// Returns error if the database read or the decode fails.
    pub fn load_voted_view<K>(db: &K) -> Result<Option<(u64, u64)>, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        match db.get(KEY_VOTED_VIEW) {
            Ok(Some(bytes)) => {
                let (h, r) = decode_voted_view_v1(&bytes).map_err(|e| {
                    ConsensusError::CodecError(format!("Failed to decode voted view: {e:?}"))
                })?;
                Ok(Some((h, r)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ConsensusError::StateError(format!(
                "Failed to load voted view: {e:?}"
            ))),
        }
    }

    /// Load the certifying QC for a given height from the database.
    ///
    /// Returns `Ok(None)` if no QC row exists at that height. Absence is a
    /// meaningful result, not an error: a height can legitimately lack a QC
    /// row (pruned past the retention window, synced before QCs travel on
    /// the wire, or never observed by this node).
    ///
    /// # Errors
    /// Returns error if database read or decoding fails.
    pub fn load_qc_at_height<K>(db: &K, height: u64) -> Result<Option<QC>, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        let key = qc_key(height);
        match db.get(&key) {
            Ok(Some(bytes)) => {
                let qc = decode_qc_v1(&bytes).map_err(|e| {
                    ConsensusError::CodecError(format!("Failed to decode QC: {e:?}"))
                })?;
                Ok(Some(qc))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ConsensusError::StateError(format!(
                "Failed to load QC: {e:?}"
            ))),
        }
    }

    /// Load a block from the database.
    ///
    /// # Errors
    /// Returns error if database read or decoding fails.
    pub fn load_block<K>(db: &K, height: u64) -> Result<Option<Block>, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        let key = block_key(height);
        match db.get(&key) {
            Ok(Some(bytes)) => {
                let mut slice = bytes.as_slice();
                let block = decode_block_v1(&mut slice).map_err(|e| {
                    ConsensusError::CodecError(format!("Failed to decode block: {e:?}"))
                })?;
                Ok(Some(block))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ConsensusError::StateError(format!(
                "Failed to load block: {e:?}"
            ))),
        }
    }

    /// Load a range of blocks from the database.
    ///
    /// Returns blocks in order from start_height to end_height (inclusive).
    /// Missing blocks in the range will cause an error.
    ///
    /// # Errors
    /// Returns error if any block in the range is missing or decoding fails.
    pub fn load_blocks_range<K>(
        db: &K,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<Block>, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        if start_height > end_height {
            return Ok(vec![]);
        }

        let mut blocks = Vec::with_capacity((end_height - start_height + 1) as usize);

        for height in start_height..=end_height {
            let block = Self::load_block(db, height)?.ok_or_else(|| {
                ConsensusError::StateError(format!("Missing block at height {height}"))
            })?;
            blocks.push(block);
        }

        Ok(blocks)
    }

    /// Verify that a sequence of blocks forms a valid chain.
    ///
    /// Checks that each block's parent_hash matches the hash of the previous block.
    ///
    /// # Arguments
    /// * `blocks` - Blocks to verify, must be in ascending height order
    /// * `expected_first_parent` - Expected parent hash of the first block
    ///
    /// # Errors
    /// Returns error if chain linkage is broken or heights are not contiguous.
    pub fn verify_block_chain(
        blocks: &[Block],
        expected_first_parent: [u8; 32],
    ) -> Result<(), ConsensusError> {
        if blocks.is_empty() {
            return Ok(());
        }

        // Verify first block's parent
        if blocks[0].parent_hash != expected_first_parent {
            return Err(ConsensusError::InvalidBlock(format!(
                "First block parent mismatch: expected {:?}, got {:?}",
                &expected_first_parent[..8],
                &blocks[0].parent_hash[..8]
            )));
        }

        // Verify contiguous heights and parent chain
        for i in 1..blocks.len() {
            let prev = &blocks[i - 1];
            let curr = &blocks[i];

            // Heights must be contiguous
            if curr.height != prev.height + 1 {
                return Err(ConsensusError::InvalidBlock(format!(
                    "Non-contiguous heights: {} followed by {}",
                    prev.height, curr.height
                )));
            }

            // Parent hash must match previous block's hash
            let prev_hash = novai_consensus_types::codec::hash_block_v1(prev)
                .map_err(|e| ConsensusError::CodecError(format!("{e:?}")))?;

            if curr.parent_hash != prev_hash {
                return Err(ConsensusError::InvalidBlock(format!(
                    "Chain broken at height {}: parent_hash {:?} != prev_hash {:?}",
                    curr.height,
                    &curr.parent_hash[..8],
                    &prev_hash[..8]
                )));
            }
        }

        Ok(())
    }

    /// Catch up from current state to target height.
    ///
    /// Loads blocks from committed_height+1 to target_height, verifies chain
    /// integrity, and caches them for the commit rule.
    ///
    /// # Arguments
    /// * `db` - Database to load blocks from
    /// * `target_height` - Height to catch up to (must be >= committed_height)
    ///
    /// # Returns
    /// Number of blocks loaded and cached.
    ///
    /// # Errors
    /// Returns error if blocks are missing, chain is broken, or state mismatch.
    pub fn catch_up_to<K>(&mut self, db: &K, target_height: u64) -> Result<usize, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        // Nothing to do if already caught up
        if target_height <= self.committed_height {
            return Ok(0);
        }

        let start_height = self.committed_height + 1;

        // Load blocks
        let blocks = Self::load_blocks_range(db, start_height, target_height)?;
        if blocks.is_empty() {
            return Ok(0);
        }

        // Determine expected first parent hash
        let expected_parent = if self.committed_height == 0 {
            [0u8; 32] // Genesis parent
        } else {
            // Load parent block to get its hash
            let parent_block = Self::load_block(db, self.committed_height)?.ok_or_else(|| {
                ConsensusError::StateError(format!(
                    "Missing parent block at height {}",
                    self.committed_height
                ))
            })?;
            novai_consensus_types::codec::hash_block_v1(&parent_block)
                .map_err(|e| ConsensusError::CodecError(format!("{e:?}")))?
        };

        // Verify chain integrity
        Self::verify_block_chain(&blocks, expected_parent)?;

        // Cache blocks for commit rule
        let count = blocks.len();
        for block in blocks {
            self.cache_block(block)?;
        }

        // Update height to match target
        self.height = target_height;

        tracing::info!(count, start_height, target_height, "CATCH-UP complete");

        Ok(count)
    }

    /// Recover with full catch-up to rebuild block caches.
    ///
    /// This is an enhanced version of `recover` that also loads recent blocks
    /// into the cache for the commit rule to work correctly.
    ///
    /// # Arguments
    /// * `our_address` - Our validator address
    /// * `db` - Database to recover from
    /// * `cache_depth` - How many blocks to cache (typically 3 for 3-chain rule)
    ///
    /// # Errors
    /// Returns error if database operations fail.
    pub fn recover_with_cache<K>(
        our_address: Address,
        db: &K,
        cache_depth: u64,
    ) -> Result<Self, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        // Basic recovery
        let mut state = Self::recover(our_address, db)?;

        // Load recent blocks into cache (for commit rule)
        let cache_start = state.committed_height.saturating_sub(cache_depth);
        if cache_start > 0 || state.committed_height > 0 {
            let start = cache_start.max(1); // Don't try to load height 0
            if let Ok(blocks) = Self::load_blocks_range(db, start, state.committed_height) {
                for block in blocks {
                    if let Err(e) = state.cache_block(block) {
                        tracing::warn!(?e, "RECOVERY: Failed to cache block, skipping");
                    }
                }
                tracing::info!(
                    cached = state.block_cache.len(),
                    start,
                    end = state.committed_height,
                    "RECOVERY: Cached blocks"
                );
            }
        }

        Ok(state)
    }

    /// Recover consensus state from database after restart.
    ///
    /// # Errors
    /// Returns error if database operations fail.
    pub fn recover<K>(our_address: Address, db: &K) -> Result<Self, ConsensusError>
    where
        K: Kv,
        K::Error: std::fmt::Debug,
    {
        let committed_height = Self::load_committed_height(db)?;
        let highest_qc = Self::load_highest_qc(db)?;
        // 535004 Layer 4: restore the lock. If the lock row is absent (a DB
        // written before this fix, or a node that never locked), fall back to
        // highest_qc, since the lock invariantly tracks highest_qc; this keeps a
        // recovered node locked rather than briefly unlocked and able to vote a
        // conflicting block.
        let locked_qc = Self::load_locked_qc(db)?.or_else(|| highest_qc.clone());

        // gate 9: restore the durable vote high-water mark so a restarted node
        // refuses to vote again at any (height, round) it already voted at.
        let voted_view = Self::load_voted_view(db)?;

        // Determine current height from committed height
        let height = committed_height;

        tracing::info!(
            committed_height,
            highest_qc = ?highest_qc.as_ref().map(|q| q.height),
            "RECOVERED consensus state"
        );

        Ok(Self {
            height,
            round: 0,
            highest_qc,
            pending_votes: HashMap::new(),
            our_address,
            last_proposed: None,
            voted_in_round: HashSet::new(),
            committed_height,
            block_cache: HashMap::new(),
            qc_cache: HashMap::new(),
            block_by_hash: HashMap::new(),
            pending_timeouts: HashMap::new(),
            timed_out_in_round: HashSet::new(),
            view_changes_total: 0,
            last_proposed_txs: Vec::new(),
            last_proposed_block_hash: None,
            locked_qc,
            voted_view,
            pending_exec: HashMap::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn make_test_validators(count: usize) -> Vec<(Address, SigningKey, VerifyingKey)> {
        (0..count)
            .map(|i| {
                let mut seed = [0u8; 32];
                seed[0] = i as u8;
                let signing_key = SigningKey::from_bytes(&seed);
                let verifying_key = signing_key.verifying_key();
                let addr = novai_crypto::address_from_pubkey(&verifying_key);
                (addr, signing_key, verifying_key)
            })
            .collect()
    }

    #[test]
    fn test_vote_with_signal_accepted() {
        // Setup: 4 validators
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        // Create a vote WITH an AI signal
        let vote = Vote {
            height: 1,
            round: 0,
            block_hash: [1u8; 32],
            voter: validator_set[1],
            signature: [0u8; 64],
            ai_signal_commitment: Some([0xAA; 32]), // AI signal present
        };

        // Sign the vote
        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&vote);
        let domain_tag = b"NOVAI_VOTE_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validators[1].1, &to_sign);

        let signed_vote = Vote { signature, ..vote };

        // Vote should be accepted
        let result = state.add_vote(signed_vote, &pubkeys);
        assert!(result.is_ok(), "Vote with signal should be accepted");
    }

    #[test]
    fn test_vote_without_signal_accepted() {
        // Setup: 4 validators
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        // Create a vote WITHOUT an AI signal
        let vote = Vote {
            height: 1,
            round: 0,
            block_hash: [1u8; 32],
            voter: validator_set[1],
            signature: [0u8; 64],
            ai_signal_commitment: None, // No AI signal
        };

        // Sign the vote
        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&vote);
        let domain_tag = b"NOVAI_VOTE_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validators[1].1, &to_sign);

        let signed_vote = Vote { signature, ..vote };

        // Vote should be accepted
        let result = state.add_vote(signed_vote, &pubkeys);
        assert!(result.is_ok(), "Vote without signal should be accepted");
    }

    #[test]
    fn test_signal_does_not_affect_qc() {
        // Setup: 4 validators (n=4, f=1, quorum=3)
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);
        let block_hash = [1u8; 32];

        // Add 3 votes: 2 with signals, 1 without
        for i in 0..3 {
            let has_signal = i < 2;
            let vote = Vote {
                height: 1,
                round: 0,
                block_hash,
                voter: validator_set[i],
                signature: [0u8; 64],
                ai_signal_commitment: if has_signal { Some([0xBB; 32]) } else { None },
            };

            let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&vote);
            let domain_tag = b"NOVAI_VOTE_V1";
            let mut to_sign = Vec::new();
            to_sign.extend_from_slice(domain_tag);
            to_sign.extend_from_slice(&unsigned_bytes);
            let signature = novai_crypto::sign_bytes(&validators[i].1, &to_sign);

            let signed_vote = Vote { signature, ..vote };

            state.add_vote(signed_vote, &pubkeys).unwrap();
        }

        // QC should form despite mixed signals
        let qc_result = state.try_form_qc(&block_hash, &validator_set);
        assert!(qc_result.is_ok());
        assert!(
            qc_result.unwrap().is_some(),
            "QC should form with mixed signals"
        );
    }

    #[test]
    fn test_signal_logged_correctly() {
        // This test verifies that the logging code path executes without panic
        // Actual log output verification would require a test harness
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        let vote = Vote {
            height: 1,
            round: 0,
            block_hash: [1u8; 32],
            voter: validator_set[1],
            signature: [0u8; 64],
            ai_signal_commitment: Some([0xCC; 32]),
        };

        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&vote);
        let domain_tag = b"NOVAI_VOTE_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validators[1].1, &to_sign);

        let signed_vote = Vote { signature, ..vote };

        // Should not panic when logging signal
        let result = state.add_vote(signed_vote, &pubkeys);
        assert!(
            result.is_ok(),
            "Vote with signal should be logged and accepted"
        );
    }
    #[test]
    fn test_commit_with_ai_ops() {
        use novai_state::{MemKv, WriteOp};

        // Setup: 4 validators
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        // Create blocks to commit
        let blocks = vec![
            Block {
                height: 1,
                round: 0,
                parent_hash: [0u8; 32],
                state_root: [0xAA; 32],
                txs: vec![],
            },
            Block {
                height: 2,
                round: 0,
                parent_hash: [1u8; 32],
                state_root: [0xBB; 32],
                txs: vec![],
            },
        ];

        // Create QC for height 2
        let qc = QC {
            height: 2,
            round: 0,
            block_hash: [2u8; 32],
            votes: vec![],
        };

        // Create AI operations
        let ai_ops = vec![
            WriteOp::Put(b"ai:entity:1".to_vec(), b"data1".to_vec()),
            WriteOp::Put(b"ai:entity:2".to_vec(), b"data2".to_vec()),
        ];

        // Persist commit with AI ops
        let result = state.persist_commit_atomic(&mut db, &blocks, &qc, 2, Some(&ai_ops));
        assert!(result.is_ok(), "Commit with AI ops should succeed");

        // Verify blocks persisted
        assert!(
            db.get(&block_key(1)).unwrap().is_some(),
            "Block 1 should be persisted"
        );
        assert!(
            db.get(&block_key(2)).unwrap().is_some(),
            "Block 2 should be persisted"
        );

        // Verify AI ops persisted
        assert!(
            db.get(b"ai:entity:1").unwrap().is_some(),
            "AI entity 1 should be persisted"
        );
        assert!(
            db.get(b"ai:entity:2").unwrap().is_some(),
            "AI entity 2 should be persisted"
        );

        // Verify committed height
        let ch = ConsensusState::load_committed_height(&db).unwrap();
        assert_eq!(ch, 2, "Committed height should be 2");
    }
    #[test]
    fn test_ai_ops_fail_rolls_back_everything() {
        use novai_state::{MemKv, WriteOp};

        // Setup
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        // Create blocks to commit
        let blocks = vec![Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: [0xAA; 32],
            txs: vec![],
        }];

        let qc = QC {
            height: 1,
            round: 0,
            block_hash: [1u8; 32],
            votes: vec![],
        };

        // Create AI operations with a duplicate key (will cause batch to fail in strict mode)
        // For MemKv, we simulate failure by trying to write to a read-only location
        // In reality, a real DB backend would enforce transactional semantics
        let ai_ops = vec![
            WriteOp::Put(b"ai:entity:1".to_vec(), b"data1".to_vec()),
            WriteOp::Put(b"ai:entity:1".to_vec(), b"data2".to_vec()), // Duplicate
        ];

        // Persist commit with AI ops - should succeed (MemKv doesn't enforce uniqueness)
        // But this test documents the INTENDED behavior: failures should roll back
        let result = state.persist_commit_atomic(&mut db, &blocks, &qc, 1, Some(&ai_ops));

        // NOTE: MemKv doesn't enforce transactional semantics, so this will succeed
        // In production with RocksDB, a failed AI op would roll back the entire batch
        // This test documents the contract, even if MemKv can't enforce it
        if result.is_ok() {
            // With MemKv, verify last write wins
            let value = db.get(b"ai:entity:1").unwrap().unwrap();
            assert_eq!(value, b"data2", "Last write should win in MemKv");
        }

        // The key invariant is: if we had a real failure (e.g., disk full),
        // then NEITHER blocks NOR AI ops would be persisted
        // This is enforced by the atomic batch mechanism in production DBs
    }
    #[test]
    fn test_no_ai_ops_works() {
        use novai_state::MemKv;

        // Setup
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        // Create blocks to commit
        let blocks = vec![
            Block {
                height: 1,
                round: 0,
                parent_hash: [0u8; 32],
                state_root: [0xAA; 32],
                txs: vec![],
            },
            Block {
                height: 2,
                round: 0,
                parent_hash: [1u8; 32],
                state_root: [0xBB; 32],
                txs: vec![],
            },
        ];

        let qc = QC {
            height: 2,
            round: 0,
            block_hash: [2u8; 32],
            votes: vec![],
        };

        // Persist commit WITHOUT AI ops (None)
        let result = state.persist_commit_atomic(&mut db, &blocks, &qc, 2, None);
        assert!(result.is_ok(), "Commit without AI ops should succeed");

        // Verify blocks persisted
        assert!(
            db.get(&block_key(1)).unwrap().is_some(),
            "Block 1 should be persisted"
        );
        assert!(
            db.get(&block_key(2)).unwrap().is_some(),
            "Block 2 should be persisted"
        );

        // Verify committed height
        let ch = ConsensusState::load_committed_height(&db).unwrap();
        assert_eq!(ch, 2, "Committed height should be 2");

        // Verify QC persisted
        assert!(
            db.get(&qc_key(2)).unwrap().is_some(),
            "QC should be persisted"
        );
    }

    #[test]
    fn test_create_timeout() {
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let state = ConsensusState::new(validator_set[0]);
        let timeout = state.create_timeout(&validators[0].1).unwrap();

        assert_eq!(timeout.height, 1); // height + 1
        assert_eq!(timeout.round, 0);
        assert_eq!(timeout.voter, validator_set[0]);
        assert!(timeout.highest_qc.is_none());
    }

    #[test]
    fn test_add_timeout_success() {
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        // Create timeout from validator 1
        let timeout = Timeout {
            height: 1,
            round: 0,
            voter: validator_set[1],
            highest_qc: None,
            signature: [0u8; 64],
        };

        // Sign it
        let unsigned_bytes =
            novai_consensus_types::codec::encode_timeout_v1_unsigned(&timeout).unwrap();
        let domain_tag = b"NOVAI_TIMEOUT_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validators[1].1, &to_sign);

        let signed_timeout = Timeout {
            signature,
            ..timeout
        };

        // Add timeout
        let result = state.add_timeout(signed_timeout, &pubkeys);
        assert!(result.is_ok(), "Valid timeout should be accepted");

        // Verify it was added
        assert_eq!(state.pending_timeouts.len(), 1);
        let key = (1u64, 0u64);
        let timeouts = state.pending_timeouts.get(&key).unwrap();
        assert_eq!(timeouts.len(), 1);
        assert_eq!(timeouts[0].voter, validator_set[1]);
    }

    #[test]
    fn test_add_timeout_rejects_duplicate() {
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        // Create and sign timeout
        let timeout = Timeout {
            height: 1,
            round: 0,
            voter: validator_set[1],
            highest_qc: None,
            signature: [0u8; 64],
        };

        let unsigned_bytes =
            novai_consensus_types::codec::encode_timeout_v1_unsigned(&timeout).unwrap();
        let domain_tag = b"NOVAI_TIMEOUT_V1";
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(domain_tag);
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validators[1].1, &to_sign);

        let signed_timeout = Timeout {
            signature,
            ..timeout
        };

        // Add timeout once
        state.add_timeout(signed_timeout.clone(), &pubkeys).unwrap();

        // Try to add again - should fail
        let result = state.add_timeout(signed_timeout, &pubkeys);
        assert!(result.is_err(), "Duplicate timeout should be rejected");
    }

    #[test]
    fn test_try_advance_round() {
        let validators = make_test_validators(4); // n=4, f=1, quorum=3
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        // Add 3 timeouts (reaches quorum)
        for i in 0..3 {
            let timeout = Timeout {
                height: 1,
                round: 0,
                voter: validator_set[i],
                highest_qc: None,
                signature: [0u8; 64],
            };

            let unsigned_bytes =
                novai_consensus_types::codec::encode_timeout_v1_unsigned(&timeout).unwrap();
            let domain_tag = b"NOVAI_TIMEOUT_V1";
            let mut to_sign = Vec::new();
            to_sign.extend_from_slice(domain_tag);
            to_sign.extend_from_slice(&unsigned_bytes);
            let signature = novai_crypto::sign_bytes(&validators[i].1, &to_sign);

            let signed_timeout = Timeout {
                signature,
                ..timeout
            };
            state.add_timeout(signed_timeout, &pubkeys).unwrap();
        }

        // Try to advance round
        let advanced = state.try_advance_round(&validator_set);
        assert!(advanced, "Round should advance with 3/4 timeouts");
        assert_eq!(state.round, 1, "Round should be incremented");
        assert!(
            state.voted_in_round.is_empty(),
            "Vote tracking should be cleared"
        );
        assert!(
            state.timed_out_in_round.is_empty(),
            "Timeout tracking should be cleared"
        );
    }

    #[test]
    fn test_try_advance_round_insufficient_timeouts() {
        let validators = make_test_validators(4); // n=4, f=1, quorum=3
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

        let mut state = ConsensusState::new(validator_set[0]);

        // Add only 2 timeouts (below quorum)
        for i in 0..2 {
            let timeout = Timeout {
                height: 1,
                round: 0,
                voter: validator_set[i],
                highest_qc: None,
                signature: [0u8; 64],
            };

            let unsigned_bytes =
                novai_consensus_types::codec::encode_timeout_v1_unsigned(&timeout).unwrap();
            let domain_tag = b"NOVAI_TIMEOUT_V1";
            let mut to_sign = Vec::new();
            to_sign.extend_from_slice(domain_tag);
            to_sign.extend_from_slice(&unsigned_bytes);
            let signature = novai_crypto::sign_bytes(&validators[i].1, &to_sign);

            let signed_timeout = Timeout {
                signature,
                ..timeout
            };
            state.add_timeout(signed_timeout, &pubkeys).unwrap();
        }

        // Try to advance round - should fail
        let advanced = state.try_advance_round(&validator_set);
        assert!(!advanced, "Round should NOT advance with only 2/4 timeouts");
        assert_eq!(state.round, 0, "Round should remain unchanged");
    }

    #[test]
    fn test_timeout_for_round_base_case() {
        assert_eq!(timeout_for_round(0), BASE_TIMEOUT_MS);
        assert_eq!(timeout_for_round(0), 1000);
    }

    #[test]
    fn test_timeout_for_round_exponential_backoff() {
        assert_eq!(timeout_for_round(1), 2000); // 2^1 * 1000
        assert_eq!(timeout_for_round(2), 4000); // 2^2 * 1000
        assert_eq!(timeout_for_round(3), 8000); // 2^3 * 1000
        assert_eq!(timeout_for_round(4), 16000); // 2^4 * 1000
        assert_eq!(timeout_for_round(5), 32000); // 2^5 * 1000
    }

    #[test]
    fn test_timeout_for_round_caps_at_max() {
        // Round 6: 2^6 * 1000 = 64000 > 60000, so capped
        assert_eq!(timeout_for_round(6), MAX_TIMEOUT_MS);
        assert_eq!(timeout_for_round(6), 60000);

        // Higher rounds also capped
        assert_eq!(timeout_for_round(10), MAX_TIMEOUT_MS);
        assert_eq!(timeout_for_round(100), MAX_TIMEOUT_MS);
    }

    #[test]
    fn test_timeout_for_round_no_overflow() {
        // Even with very high round numbers, no overflow
        assert_eq!(timeout_for_round(u64::MAX), MAX_TIMEOUT_MS);
        assert_eq!(timeout_for_round(1000), MAX_TIMEOUT_MS);
    }

    #[test]
    fn test_load_blocks_range_success() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        // Persist some blocks
        for h in 1..=5 {
            let block = Block {
                height: h,
                round: 0,
                parent_hash: [h as u8 - 1; 32],
                state_root: [h as u8; 32],
                txs: vec![],
            };
            state.persist_block(&mut db, &block).unwrap();
        }

        // Load range
        let blocks = ConsensusState::load_blocks_range(&db, 2, 4).unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].height, 2);
        assert_eq!(blocks[1].height, 3);
        assert_eq!(blocks[2].height, 4);
    }

    #[test]
    fn test_load_blocks_range_missing_block() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        // Persist blocks 1, 2, 4 (missing 3)
        for h in [1, 2, 4] {
            let block = Block {
                height: h,
                round: 0,
                parent_hash: [0; 32],
                state_root: [h as u8; 32],
                txs: vec![],
            };
            state.persist_block(&mut db, &block).unwrap();
        }

        // Load range 1-4 should fail (missing block 3)
        let result = ConsensusState::load_blocks_range(&db, 1, 4);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_block_chain_valid() {
        let block1 = Block {
            height: 1,
            round: 0,
            parent_hash: [0; 32],
            state_root: [1; 32],
            txs: vec![],
        };
        let hash1 = novai_consensus_types::codec::hash_block_v1(&block1).unwrap();

        let block2 = Block {
            height: 2,
            round: 0,
            parent_hash: hash1,
            state_root: [2; 32],
            txs: vec![],
        };
        let hash2 = novai_consensus_types::codec::hash_block_v1(&block2).unwrap();

        let block3 = Block {
            height: 3,
            round: 0,
            parent_hash: hash2,
            state_root: [3; 32],
            txs: vec![],
        };

        let blocks = vec![block1, block2, block3];
        let result = ConsensusState::verify_block_chain(&blocks, [0; 32]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_block_chain_broken() {
        let block1 = Block {
            height: 1,
            round: 0,
            parent_hash: [0; 32],
            state_root: [1; 32],
            txs: vec![],
        };

        let block2 = Block {
            height: 2,
            round: 0,
            parent_hash: [0xFF; 32], // Wrong parent!
            state_root: [2; 32],
            txs: vec![],
        };

        let blocks = vec![block1, block2];
        let result = ConsensusState::verify_block_chain(&blocks, [0; 32]);
        assert!(result.is_err());
    }

    #[test]
    fn test_catch_up_to_success() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let mut db = MemKv::new();

        // Create and persist a valid chain
        let mut prev_hash = [0u8; 32];
        for h in 1..=5 {
            let block = Block {
                height: h,
                round: 0,
                parent_hash: prev_hash,
                state_root: [h as u8; 32],
                txs: vec![],
            };
            prev_hash = novai_consensus_types::codec::hash_block_v1(&block).unwrap();

            let state = ConsensusState::new(validator_set[0]);
            state.persist_block(&mut db, &block).unwrap();
        }

        // Create state at committed_height=0
        let mut state = ConsensusState::new(validator_set[0]);
        assert_eq!(state.committed_height, 0);
        assert_eq!(state.block_cache.len(), 0);

        // Catch up to height 5
        let count = state.catch_up_to(&db, 5).unwrap();
        assert_eq!(count, 5);
        assert_eq!(state.height, 5);
        assert_eq!(state.block_cache.len(), 5);
    }

    #[test]
    fn test_catch_up_already_caught_up() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

        let db = MemKv::new();

        let mut state = ConsensusState::new(validator_set[0]);
        state.committed_height = 10;

        // Try to catch up to height 5 (less than committed)
        let count = state.catch_up_to(&db, 5).unwrap();
        assert_eq!(count, 0);
    }

    // ── Size-limit enforcement tests for verify_block ──────────────────

    /// Helper: build a TxV1 with a payload of the given size.
    fn make_tx_with_payload(payload_len: usize) -> novai_types::TxV1 {
        novai_types::TxV1 {
            version: novai_types::TxVersion::V1,
            from: [0x11; 32],
            pubkey: [0x22; 32],
            nonce: 1,
            fee: 10,
            payload: vec![0xAB; payload_len],
            sig: [0xCC; 64],
        }
    }

    #[test]
    fn verify_block_rejects_oversized_tx() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let state = ConsensusState::new(validator_set[0]);
        let db = MemKv::new();

        // A single tx whose encoded size exceeds MAX_TX_SIZE (128 KB).
        // tx_encoded_size = TX_V1_OVERHEAD(149) + payload_len, so payload_len
        // = MAX_TX_SIZE - 149 + 1 puts us 1 byte over the limit.
        let payload_len = novai_types::MAX_TX_SIZE - novai_codec::TX_V1_OVERHEAD + 1;
        let block = Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: [0u8; 32],
            txs: vec![make_tx_with_payload(payload_len)],
        };

        let err = state.verify_block(&block, &db).unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("tx encoded size") && msg.contains("exceeds limit"),
            "expected oversized-tx error, got: {msg}"
        );
    }

    #[test]
    fn verify_block_rejects_too_many_txs() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let state = ConsensusState::new(validator_set[0]);
        let db = MemKv::new();

        // Block with MAX_TXS_PER_BLOCK + 1 tiny transactions.
        let txs: Vec<novai_types::TxV1> = (0..novai_types::MAX_TXS_PER_BLOCK + 1)
            .map(|_| make_tx_with_payload(0))
            .collect();

        let block = Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: [0u8; 32],
            txs,
        };

        let err = state.verify_block(&block, &db).unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("txs") && msg.contains("exceeds limit"),
            "expected too-many-txs error, got: {msg}"
        );
    }

    #[test]
    fn verify_block_rejects_oversized_block_payload() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let state = ConsensusState::new(validator_set[0]);
        let db = MemKv::new();

        // Each tx is just under MAX_TX_SIZE but many of them push total over
        // MAX_BLOCK_SIZE (2 MB). Use payload_len = MAX_TX_SIZE - TX_V1_OVERHEAD
        // (exactly at the limit per-tx). Need ceil(MAX_BLOCK_SIZE / MAX_TX_SIZE) + 1 txs.
        let per_tx_payload = novai_types::MAX_TX_SIZE - novai_codec::TX_V1_OVERHEAD;
        let per_tx_size = novai_types::MAX_TX_SIZE; // TX_V1_OVERHEAD + per_tx_payload
        let num_txs = novai_types::MAX_BLOCK_SIZE / per_tx_size + 1;
        // Ensure we don't exceed MAX_TXS_PER_BLOCK (would trigger that error first).
        assert!(
            num_txs <= novai_types::MAX_TXS_PER_BLOCK,
            "test setup: need {} txs but MAX_TXS_PER_BLOCK is {}",
            num_txs,
            novai_types::MAX_TXS_PER_BLOCK
        );

        let txs: Vec<novai_types::TxV1> = (0..num_txs)
            .map(|_| make_tx_with_payload(per_tx_payload))
            .collect();

        let block = Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: [0u8; 32],
            txs,
        };

        let err = state.verify_block(&block, &db).unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("block payload") && msg.contains("exceeds limit"),
            "expected oversized-block error, got: {msg}"
        );
    }

    #[test]
    fn verify_block_passes_size_checks_for_valid_block() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let state = ConsensusState::new(validator_set[0]);
        let db = MemKv::new();

        // A block with a few small txs — well within all size limits.
        // verify_block will pass size checks then fail on height/state, which
        // proves the size checks accepted the block.
        let block = Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: [0u8; 32],
            txs: vec![make_tx_with_payload(100), make_tx_with_payload(200)],
        };

        let result = state.verify_block(&block, &db);
        // Should NOT be a size-limit error. It will fail on signature or state,
        // but the point is it got past all three size checks.
        match &result {
            Ok(()) => {} // surprisingly passed everything — fine
            Err(e) => {
                let msg = format!("{e:?}");
                assert!(
                    !msg.contains("exceeds limit"),
                    "block within limits should not trigger size error, got: {msg}"
                );
            }
        }
    }

    // ===== Stage 1 (gate-equivocation-535004): dense per-height QC rows =====

    /// Build a parent-chained run of empty blocks starting at `start`,
    /// each paired with the QC that certifies it (same height, matching
    /// block hash).
    fn make_chained_blocks_with_qcs(start: u64, count: u64) -> (Vec<Block>, Vec<QC>) {
        let mut blocks = Vec::new();
        let mut qcs = Vec::new();
        let mut parent = [0u8; 32];
        for height in start..start + count {
            let block = Block {
                height,
                round: 0,
                parent_hash: parent,
                state_root: [0xAA; 32],
                txs: vec![],
            };
            let hash = novai_consensus_types::codec::hash_block_v1(&block).unwrap();
            qcs.push(QC {
                height,
                round: 0,
                block_hash: hash,
                votes: vec![],
            });
            parent = hash;
            blocks.push(block);
        }
        (blocks, qcs)
    }

    #[test]
    fn test_dense_qc_row_for_every_committed_height() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let mut state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        let (blocks, qcs) = make_chained_blocks_with_qcs(1, 3);
        for qc in &qcs {
            state.qc_cache.insert(qc.height, qc.clone());
        }

        // A trigger QC at height 5 commits the batch 1..=3 under the
        // 3-chain rule (commit target = trigger height minus 2).
        let trigger = QC {
            height: 5,
            round: 0,
            block_hash: [0x55; 32],
            votes: vec![],
        };
        state
            .persist_commit_atomic(&mut db, &blocks, &trigger, 3, None)
            .unwrap();

        // Stage 1 invariant: every committed height has a retrievable
        // certifying QC whose block_hash matches the committed block.
        for (block, qc) in blocks.iter().zip(&qcs) {
            let loaded = ConsensusState::load_qc_at_height(&db, block.height)
                .unwrap()
                .expect("committed height must have a QC row");
            assert_eq!(loaded, *qc);
            let block_hash = novai_consensus_types::codec::hash_block_v1(block).unwrap();
            assert_eq!(loaded.block_hash, block_hash);
        }

        // The trigger QC row at its own height is unchanged behavior.
        assert!(db.get(&qc_key(5)).unwrap().is_some());
    }

    #[test]
    fn test_dense_qc_missing_cache_entry_leaves_faithful_gap() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let mut state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        let (blocks, qcs) = make_chained_blocks_with_qcs(1, 3);
        // Height 2's certifying QC was never observed (the sync catch-up
        // shape until Stage 2 carries QCs over the wire).
        state.qc_cache.insert(1, qcs[0].clone());
        state.qc_cache.insert(3, qcs[2].clone());

        let trigger = QC {
            height: 5,
            round: 0,
            block_hash: [0x55; 32],
            votes: vec![],
        };
        state
            .persist_commit_atomic(&mut db, &blocks, &trigger, 3, None)
            .unwrap();

        // The commit itself proceeds; the gap is recorded faithfully as
        // an absent row, never fabricated.
        assert_eq!(ConsensusState::load_committed_height(&db).unwrap(), 3);
        assert!(ConsensusState::load_qc_at_height(&db, 1).unwrap().is_some());
        assert!(ConsensusState::load_qc_at_height(&db, 2).unwrap().is_none());
        assert!(ConsensusState::load_qc_at_height(&db, 3).unwrap().is_some());
    }

    #[test]
    fn test_dense_qc_mismatched_cache_entry_not_written() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let mut state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        let (blocks, qcs) = make_chained_blocks_with_qcs(1, 3);
        state.qc_cache.insert(1, qcs[0].clone());
        state.qc_cache.insert(3, qcs[2].clone());
        // The cached QC at height 2 certifies a DIFFERENT block. It must
        // not be written as height 2's certifying QC row.
        let mut wrong = qcs[1].clone();
        wrong.block_hash = [0xEE; 32];
        state.qc_cache.insert(2, wrong);

        let trigger = QC {
            height: 5,
            round: 0,
            block_hash: [0x55; 32],
            votes: vec![],
        };
        state
            .persist_commit_atomic(&mut db, &blocks, &trigger, 3, None)
            .unwrap();

        assert_eq!(ConsensusState::load_committed_height(&db).unwrap(), 3);
        assert!(ConsensusState::load_qc_at_height(&db, 2).unwrap().is_none());
    }

    #[test]
    fn test_dense_qc_batch_spanning_prune_boundary() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let mut state = ConsensusState::new(validator_set[0]);
        let mut db = MemKv::new();

        // Seed old rows at heights 1..=3 that the prune must delete.
        let (old_blocks, old_qcs) = make_chained_blocks_with_qcs(1, 3);
        for (block, qc) in old_blocks.iter().zip(&old_qcs) {
            db.put(&block_key(block.height), &encode_block_v1(block).unwrap())
                .unwrap();
            db.put(&qc_key(qc.height), &encode_qc_v1(qc).unwrap())
                .unwrap();
        }

        // Commit a batch at heights PRUNE_RETAIN_BLOCKS + 1..=+3. Each
        // committed height h prunes h - PRUNE_RETAIN_BLOCKS, so this batch
        // deletes exactly heights 1..=3 while writing its own dense QC
        // rows, all in one atomic batch.
        let start = PRUNE_RETAIN_BLOCKS + 1;
        let (blocks, qcs) = make_chained_blocks_with_qcs(start, 3);
        for qc in &qcs {
            state.qc_cache.insert(qc.height, qc.clone());
        }
        let trigger = QC {
            height: start + 4,
            round: 0,
            block_hash: [0x55; 32],
            votes: vec![],
        };
        state
            .persist_commit_atomic(&mut db, &blocks, &trigger, start + 2, None)
            .unwrap();

        // Pruned: block and QC rows at heights 1..=3 are gone.
        for height in 1..=3u64 {
            assert!(db.get(&block_key(height)).unwrap().is_none());
            assert!(ConsensusState::load_qc_at_height(&db, height)
                .unwrap()
                .is_none());
        }
        // Dense rows for the committed batch survive and decode correctly.
        for (block, qc) in blocks.iter().zip(&qcs) {
            let loaded = ConsensusState::load_qc_at_height(&db, block.height)
                .unwrap()
                .expect("dense QC row must survive the prune");
            assert_eq!(loaded, *qc);
        }
    }

    #[test]
    fn test_load_qc_at_height_present_absent_corrupt() {
        use novai_state::MemKv;

        let mut db = MemKv::new();
        let qc = QC {
            height: 7,
            round: 1,
            block_hash: [0x77; 32],
            votes: vec![],
        };
        db.put(&qc_key(7), &encode_qc_v1(&qc).unwrap()).unwrap();

        // Present height: the exact QC comes back.
        assert_eq!(ConsensusState::load_qc_at_height(&db, 7).unwrap(), Some(qc));
        // Absent height: None, not an error.
        assert_eq!(ConsensusState::load_qc_at_height(&db, 8).unwrap(), None);
        // Corrupt row: a decode error surfaces as Err.
        db.put(&qc_key(9), b"garbage").unwrap();
        assert!(ConsensusState::load_qc_at_height(&db, 9).is_err());
    }

    // ===== Stage 2 Fix B (gate-equivocation-535004): duplicate-proof QCs =====

    /// Build a domain-separated, validly signed vote.
    fn fixb_signed_vote(
        signer: &SigningKey,
        voter: Address,
        height: u64,
        round: u64,
        block_hash: [u8; 32],
    ) -> Vote {
        let unsigned = Vote {
            height,
            round,
            block_hash,
            voter,
            signature: [0u8; 64],
            ai_signal_commitment: None,
        };
        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned);
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(b"NOVAI_VOTE_V1");
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(signer, &to_sign);
        Vote {
            signature,
            ..unsigned
        }
    }

    #[test]
    fn verify_qc_well_formed_accepts_valid_quorum_qc() {
        let validators = make_test_validators(4);
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let bh = [0x11; 32];
        let votes: Vec<Vote> = (0..3)
            .map(|i| fixb_signed_vote(&validators[i].1, validators[i].0, 5, 0, bh))
            .collect();
        let qc = QC {
            height: 5,
            round: 0,
            block_hash: bh,
            votes,
        };
        assert!(ConsensusState::verify_qc_well_formed(&qc, &pubkeys, 3).is_ok());
    }

    #[test]
    fn verify_qc_well_formed_rejects_duplicate_voter() {
        // The exact 535004 Layer 2 shape: three vote ENTRIES but only two
        // DISTINCT voters, masquerading as quorum 3.
        let validators = make_test_validators(4);
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let bh = [0x11; 32];
        let v_a = fixb_signed_vote(&validators[0].1, validators[0].0, 5, 0, bh);
        let v_b = fixb_signed_vote(&validators[1].1, validators[1].0, 5, 0, bh);
        let qc = QC {
            height: 5,
            round: 0,
            block_hash: bh,
            votes: vec![v_a.clone(), v_a, v_b],
        };
        assert!(ConsensusState::verify_qc_well_formed(&qc, &pubkeys, 3).is_err());
    }

    #[test]
    fn verify_qc_well_formed_rejects_sub_quorum() {
        let validators = make_test_validators(4);
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let bh = [0x11; 32];
        let votes: Vec<Vote> = (0..2)
            .map(|i| fixb_signed_vote(&validators[i].1, validators[i].0, 5, 0, bh))
            .collect();
        let qc = QC {
            height: 5,
            round: 0,
            block_hash: bh,
            votes,
        };
        assert!(ConsensusState::verify_qc_well_formed(&qc, &pubkeys, 3).is_err());
    }

    #[test]
    fn verify_qc_well_formed_rejects_unknown_voter() {
        let validators = make_test_validators(4);
        // Only the first three validators are known to the verifier.
        let pubkeys: Vec<(Address, VerifyingKey)> = validators
            .iter()
            .take(3)
            .map(|(a, _, vk)| (*a, *vk))
            .collect();
        let bh = [0x11; 32];
        let mut votes: Vec<Vote> = (0..2)
            .map(|i| fixb_signed_vote(&validators[i].1, validators[i].0, 5, 0, bh))
            .collect();
        // A third distinct voter, validly signed, but outside the known set.
        votes.push(fixb_signed_vote(
            &validators[3].1,
            validators[3].0,
            5,
            0,
            bh,
        ));
        let qc = QC {
            height: 5,
            round: 0,
            block_hash: bh,
            votes,
        };
        assert!(ConsensusState::verify_qc_well_formed(&qc, &pubkeys, 3).is_err());
    }

    #[test]
    fn verify_qc_well_formed_rejects_invalid_signature() {
        let validators = make_test_validators(4);
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let bh = [0x11; 32];
        let mut votes: Vec<Vote> = (0..3)
            .map(|i| fixb_signed_vote(&validators[i].1, validators[i].0, 5, 0, bh))
            .collect();
        votes[2].signature = [0u8; 64]; // corrupt one signature
        let qc = QC {
            height: 5,
            round: 0,
            block_hash: bh,
            votes,
        };
        assert!(ConsensusState::verify_qc_well_formed(&qc, &pubkeys, 3).is_err());
    }

    #[test]
    fn verify_qc_well_formed_rejects_vote_for_different_block() {
        // A vote validly signed for a DIFFERENT block must not count toward
        // this QC even though its signature checks out for its own block.
        let validators = make_test_validators(4);
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let bh = [0x11; 32];
        let other = [0x22; 32];
        let mut votes: Vec<Vote> = (0..2)
            .map(|i| fixb_signed_vote(&validators[i].1, validators[i].0, 5, 0, bh))
            .collect();
        votes.push(fixb_signed_vote(
            &validators[2].1,
            validators[2].0,
            5,
            0,
            other,
        ));
        let qc = QC {
            height: 5,
            round: 0,
            block_hash: bh,
            votes,
        };
        assert!(ConsensusState::verify_qc_well_formed(&qc, &pubkeys, 3).is_err());
    }

    #[test]
    fn try_form_qc_dedups_duplicate_voter_no_quorum() {
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let mut state = ConsensusState::new(validator_set[0]);
        let bh = [0x11; 32];
        // Three entries, two distinct voters: the pre-fix votes.len() == 3
        // would have formed a 2-distinct-signer QC at quorum 3.
        let v_a = fixb_signed_vote(&validators[0].1, validators[0].0, 1, 0, bh);
        let v_b = fixb_signed_vote(&validators[1].1, validators[1].0, 1, 0, bh);
        state.pending_votes.insert(bh, vec![v_a.clone(), v_a, v_b]);
        assert!(state.try_form_qc(&bh, &validator_set).unwrap().is_none());
    }

    #[test]
    fn try_form_qc_forms_clean_qc_ignoring_duplicate() {
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let mut state = ConsensusState::new(validator_set[0]);
        let bh = [0x11; 32];
        let v_a = fixb_signed_vote(&validators[0].1, validators[0].0, 1, 0, bh);
        let v_b = fixb_signed_vote(&validators[1].1, validators[1].0, 1, 0, bh);
        let v_c = fixb_signed_vote(&validators[2].1, validators[2].0, 1, 0, bh);
        // Four entries, three distinct voters plus a duplicate of A.
        state
            .pending_votes
            .insert(bh, vec![v_a.clone(), v_b, v_c, v_a]);
        let qc = state
            .try_form_qc(&bh, &validator_set)
            .unwrap()
            .expect("a quorum of 3 distinct voters must form a QC");
        assert_eq!(qc.votes.len(), 3);
        let mut voters: Vec<_> = qc.votes.iter().map(|v| v.voter).collect();
        voters.sort();
        voters.dedup();
        assert_eq!(voters.len(), 3, "formed QC must have 3 distinct voters");
        assert!(novai_consensus_types::codec::encode_qc_v1(&qc).is_ok());
    }

    #[test]
    fn add_vote_verified_dedups_duplicate_across_round() {
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let mut state = ConsensusState::new(validator_set[0]);
        let bh = [0x11; 32];
        let vote = fixb_signed_vote(&validators[1].1, validators[1].0, 1, 0, bh);
        state.add_vote_verified(vote.clone(), &pubkeys).unwrap();
        assert_eq!(
            state.pending_votes.get(&bh).map(std::vec::Vec::len),
            Some(1)
        );
        // Simulate a round advance: the within-round guard is cleared.
        state.voted_in_round.clear();
        // The same voter's vote arrives again across the round boundary.
        state.add_vote_verified(vote, &pubkeys).unwrap();
        assert_eq!(
            state.pending_votes.get(&bh).map(std::vec::Vec::len),
            Some(1),
            "cross-round duplicate must not land twice in pending_votes"
        );
    }

    #[test]
    fn commit_path_install_rejects_duplicate_voter_qc() {
        use novai_state::MemKv;
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let mut state = ConsensusState::new(validator_set[0]);
        let db = MemKv::new();
        let bh = [0x11; 32];
        // A duplicate-voter QC that dominates (height 3 > 0) and so reaches
        // the commit-path install, where encode_qc_v1 rejects it.
        let v_a = fixb_signed_vote(&validators[0].1, validators[0].0, 3, 0, bh);
        let qc = QC {
            height: 3,
            round: 0,
            block_hash: bh,
            votes: vec![v_a.clone(), v_a],
        };
        let result = state.cache_qc_and_check_commit(qc, &db);
        assert!(
            result.is_err(),
            "a duplicate-voter QC must not install via the commit path"
        );
        assert!(
            state.highest_qc.is_none(),
            "highest_qc must be unchanged after rejection"
        );
    }

    #[test]
    fn add_timeout_rejects_malformed_qc_via_helper() {
        // Confirms verify_qc_well_formed is wired into the add_timeout
        // install site: a sub-quorum embedded QC is rejected and not adopted.
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let mut state = ConsensusState::new(validator_set[0]);
        let bh = [0x11; 32];
        let sub_quorum_qc = QC {
            height: 3,
            round: 0,
            block_hash: bh,
            votes: vec![
                fixb_signed_vote(&validators[0].1, validators[0].0, 3, 0, bh),
                fixb_signed_vote(&validators[1].1, validators[1].0, 3, 0, bh),
            ],
        };
        let unsigned = Timeout {
            height: 1,
            round: 0,
            voter: validators[1].0,
            highest_qc: Some(sub_quorum_qc),
            signature: [0u8; 64],
        };
        let unsigned_bytes =
            novai_consensus_types::codec::encode_timeout_v1_unsigned(&unsigned).unwrap();
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(b"NOVAI_TIMEOUT_V1");
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(&validators[1].1, &to_sign);
        let timeout = Timeout {
            signature,
            ..unsigned
        };

        let result = state.add_timeout(timeout, &pubkeys);
        assert!(
            result.is_err(),
            "add_timeout must reject a sub-quorum embedded QC via the helper"
        );
        assert!(
            state.highest_qc.is_none(),
            "a sub-quorum QC must not be adopted as highest_qc"
        );
    }

    // ===== Stage 2 Fix C (gate-equivocation-535004): voted_at_height removed =====

    #[test]
    fn view_change_reproposal_not_equivocation() {
        // Regression for the 535004 Layer 3 halt. voted_at_height was keyed
        // by voter and NOT cleared on round advance, so a leader that
        // self-voted at height H round 0 failed its own equivocation guard
        // on every later round's re-proposal (a different block hash at the
        // same height), wedging the leader. With voted_at_height removed, a
        // self-vote for the round-1 re-proposal at the same height must be
        // accepted, not rejected as equivocation.
        let validators = make_test_validators(1);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let mut state = ConsensusState::new(validator_set[0]);

        // Two different blocks at the SAME height; the round differs so the
        // hash differs, the shape of a re-proposal after a view change.
        let block_r0 = Block {
            height: 1,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: [0u8; 32],
            txs: vec![],
        };
        let block_r1 = Block {
            height: 1,
            round: 1,
            parent_hash: [0u8; 32],
            state_root: [0u8; 32],
            txs: vec![],
        };
        assert_ne!(
            novai_consensus_types::codec::hash_block_v1(&block_r0).unwrap(),
            novai_consensus_types::codec::hash_block_v1(&block_r1).unwrap(),
            "the two re-proposals must hash differently for this test to be meaningful"
        );

        // Self-vote for the round-0 block.
        let vote0 = state.create_vote(&block_r0, &validators[0].1).unwrap();
        state.add_vote(vote0, &pubkeys).unwrap();

        // View change: advance the round, which clears voted_in_round (as
        // try_advance_round does). Before Fix C, voted_at_height carried the
        // round-0 hash across this boundary and tripped the guard below.
        state.round = 1;
        state.voted_in_round.clear();

        // Self-vote for the round-1 re-proposal at the same height.
        let vote1 = state.create_vote(&block_r1, &validators[0].1).unwrap();
        let result = state.add_vote(vote1, &pubkeys);

        assert!(
            result.is_ok(),
            "post-view-change re-proposal must not be rejected as equivocation, got {result:?}"
        );
    }

    #[test]
    fn within_round_duplicate_still_rejected_on_both_paths() {
        // Removing voted_at_height must NOT lose within-round duplicate
        // detection: voted_in_round still catches it on add_vote and
        // add_vote_verified alike.
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let bh = [0x11; 32];

        // add_vote path.
        let mut state = ConsensusState::new(validator_set[0]);
        let vote = fixb_signed_vote(&validators[1].1, validators[1].0, 1, 0, bh);
        state.add_vote(vote.clone(), &pubkeys).unwrap();
        assert!(
            state.add_vote(vote, &pubkeys).is_err(),
            "a second vote from the same voter in the same round must be rejected"
        );

        // add_vote_verified path.
        let mut state = ConsensusState::new(validator_set[0]);
        let vote = fixb_signed_vote(&validators[1].1, validators[1].0, 1, 0, bh);
        state.add_vote_verified(vote.clone(), &pubkeys).unwrap();
        assert!(
            state.add_vote_verified(vote, &pubkeys).is_err(),
            "add_vote_verified must reject a same-round duplicate via voted_in_round"
        );
    }

    // ===== 535004 Layer 4 (locked-QC absent): two conflicting commits =====

    /// Executable specification of the locked-QC safety bug (Verdict A).
    ///
    /// n=4, f=1, quorum=3. V3 is Byzantine and equivocates (validly signs votes
    /// for BOTH branches). V1 is honest and stays on branch A; V2 is honest and
    /// stays on branch A'. V0 is the honest quorum-overlap that votes BOTH
    /// branches. The ONLY mechanism V0 uses is the same-height dominating-QC
    /// migration: a round-1 QC at a height replaces a round-0 QC at the SAME
    /// height with no round reset (`cache_qc_and_check_commit` adopts via the
    /// `qc.height == existing.height && qc.round > existing.round` clause at
    /// lib.rs:1232/1290, and the reset at lib.rs:1265 is gated on a STRICT
    /// height increase, so it does not fire). After the migration `verify_block`
    /// (lib.rs:367) blesses the conflicting child because it only checks height
    /// and parent against the migrated `highest_qc` and consults no lock.
    ///
    /// FAITHFULNESS (so this test is not stronger than the real threat):
    /// - V0's votes come from the production decision path: V0 votes a block iff
    ///   `verify_block` accepts it, and the vote is the real `create_vote` output.
    ///   That single gate is what the locked-QC fix will change.
    /// - V1/V2/V3 votes are produced by `fixb_signed_vote` (validly signed under
    ///   the real domain-separated vote encoding). V3 signing two conflicting
    ///   votes is exactly what a real equivocating validator can do.
    /// - Every QC the test assembles is checked with `verify_qc_well_formed`
    ///   (distinct quorum, each vote bound to the QC and validly signed), so each
    ///   QC is one a real node would accept. No QC is fabricated.
    /// - A branch-A' QC forms only if V0 actually contributed a vote; V2+V3 alone
    ///   are sub-quorum. So the conflicting commit is causally gated on V0's real
    ///   `verify_block` verdict, the precise thing the fix flips.
    ///
    /// Against HEAD this FAILS at the final safety assertion: both honest nodes
    /// commit at height 1 and the two committed block hashes differ.
    #[test]
    fn two_conflicting_commits_via_qc_migration_535004() {
        use novai_state::MemKv;

        let validators = make_test_validators(4);
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let quorum = 3usize; // n=4, f=1 => 2f+1
        // Empty DB: KEY_SMT_ROOT absent => verify_block expects state_root [0;32].
        let db = MemKv::new();
        let v0_addr = validators[0].0;
        let v0_key = &validators[0].1;

        // V0 votes via the PRODUCTION path: vote iff verify_block accepts, and
        // the cast vote is the real create_vote output. The fix changes this gate.
        let gated = |v0: &ConsensusState, block: &Block| -> Option<Vote> {
            if v0.verify_block(block, &db).is_ok() {
                Some(
                    v0.create_vote(block, v0_key)
                        .expect("create_vote must succeed for a structurally valid block"),
                )
            } else {
                None
            }
        };

        // A validly signed vote from one of the synthetic validators.
        let synth = |idx: usize, height: u64, round: u64, bh: [u8; 32]| -> Vote {
            fixb_signed_vote(&validators[idx].1, validators[idx].0, height, round, bh)
        };

        // Assemble a QC from real votes and PROVE it is one a real node accepts.
        // Returns None when distinct voters are below quorum (how branch A'
        // starves when V0 declines to contribute).
        let assemble_qc =
            |height: u64, round: u64, block_hash: [u8; 32], votes: Vec<Vote>| -> Option<QC> {
                let mut seen = HashSet::new();
                let mut distinct = Vec::new();
                for v in votes {
                    if seen.insert(v.voter) {
                        distinct.push(v);
                    }
                }
                if distinct.len() < quorum {
                    return None;
                }
                let qc = QC {
                    height,
                    round,
                    block_hash,
                    votes: distinct,
                };
                ConsensusState::verify_qc_well_formed(&qc, &pubkeys, quorum).expect(
                    "assembled QC must be well-formed (distinct quorum, each vote validly \
                     signed) -- if this trips the test fabricated an impossible QC",
                );
                Some(qc)
            };

        // Two conflicting branches diverging at height 1. Branch A at round 0,
        // branch A' at round 1 (A' must out-round A at each height so its QC
        // dominates A's via the same-height-higher-round clause).
        let hash = |b: &Block| novai_consensus_types::codec::hash_block_v1(b).unwrap();
        // F3: an empty MemKv now makes verify_block expect the canonical empty
        // SMT root, not [0u8;32]. The blocks carry that root so V0 still votes via
        // the real verify_block path. The branch conflict is driven by round
        // (b1 round 0 vs b1p round 1), not state_root, so it is unchanged.
        let empty_root = novai_execution::empty_smt_root();
        let mk = |height: u64, round: u64, parent: [u8; 32]| Block {
            height,
            round,
            parent_hash: parent,
            state_root: empty_root,
            txs: vec![],
        };
        let b1 = mk(1, 0, [0u8; 32]);
        let h_b1 = hash(&b1);
        let b2 = mk(2, 0, h_b1);
        let h_b2 = hash(&b2);
        let b3 = mk(3, 0, h_b2);
        let h_b3 = hash(&b3);
        let b1p = mk(1, 1, [0u8; 32]);
        let h_b1p = hash(&b1p);
        let b2p = mk(2, 1, h_b1p);
        let h_b2p = hash(&b2p);
        let b3p = mk(3, 1, h_b2p);
        let h_b3p = hash(&b3p);
        assert_ne!(h_b1, h_b1p, "branches must conflict at height 1");

        let mut v0 = ConsensusState::new(v0_addr);
        // gate wedge-276272: verify_block now resolves the parent post-state, which
        // needs the ancestor bodies cached. Committed is 0 and every branch block is
        // empty, so their post-roots are the convention-invariant empty root; the
        // 535004 lock property under test is unchanged, this only supplies the bodies
        // the resolve walk reads.
        for b in [&b1, &b2, &b3, &b1p, &b2p, &b3p] {
            v0.cache_block(b.clone()).expect("cache branch block for resolve");
        }

        // Height 1: V0 votes BOTH bottom blocks. No QC exists yet, so V0 is not
        // committed to any branch and verify_block accepts both.
        let v0_b1 = gated(&v0, &b1).expect("V0 accepts B_1");
        let v0_b1p = gated(&v0, &b1p).expect("V0 accepts B'_1 (no lock at height 1)");
        let qc_a_h1 = assemble_qc(1, 0, h_b1, vec![v0_b1, synth(1, 1, 0, h_b1), synth(3, 1, 0, h_b1)])
            .expect("branch-A height-1 QC {V0,V1,V3}");
        let qc_ap_h1 =
            assemble_qc(1, 1, h_b1p, vec![v0_b1p, synth(2, 1, 1, h_b1p), synth(3, 1, 1, h_b1p)])
                .expect("branch-A' height-1 QC {V0,V2,V3}");

        // V0 adopts branch A's bottom QC, votes branch A's middle block.
        v0.cache_qc_and_check_commit(qc_a_h1, &db).unwrap();
        let v0_b2 = gated(&v0, &b2).expect("V0 accepts B_2 on branch A");

        // *** THE MIGRATION (mechanic #1) *** feed V0 the SAME-height round-1 QC.
        v0.cache_qc_and_check_commit(qc_ap_h1, &db).unwrap();
        let migrated = v0.highest_qc.as_ref().map(|q| q.block_hash) == Some(h_b1p);
        println!(
            "[535004] V0 highest_qc migrated to conflicting B'_1? {migrated} (round now {}, no \
             strict-height reset fired)",
            v0.highest_qc.as_ref().unwrap().round
        );

        // *** mechanic #2 *** does V0 now vote the CONFLICTING middle block?
        let v0_b2p = gated(&v0, &b2p);
        println!("[535004] V0 voted conflicting middle B'_2: {}", v0_b2p.is_some());

        // Branch A' middle QC forms ONLY with V0's contribution (V2,V3 = sub-quorum).
        let mut ap_h2_votes = vec![synth(2, 2, 1, h_b2p), synth(3, 2, 1, h_b2p)];
        if let Some(vote) = v0_b2p {
            ap_h2_votes.push(vote);
        }
        let qc_ap_h2 = assemble_qc(2, 1, h_b2p, ap_h2_votes);

        // Branch A advances to its top block (always; it is the legit branch).
        let qc_a_h2 = assemble_qc(2, 0, h_b2, vec![v0_b2, synth(1, 2, 0, h_b2), synth(3, 2, 0, h_b2)])
            .expect("branch-A height-2 QC {V0,V1,V3}");
        v0.cache_qc_and_check_commit(qc_a_h2, &db).unwrap();
        let v0_b3 = gated(&v0, &b3).expect("V0 accepts B_3 on branch A");
        let qc_a_h3 = assemble_qc(3, 0, h_b3, vec![v0_b3, synth(1, 3, 0, h_b3), synth(3, 3, 0, h_b3)])
            .expect("branch-A height-3 QC {V0,V1,V3}");

        // Branch A' advances to its top block ONLY if its middle QC formed.
        let qc_ap_h3 = qc_ap_h2.and_then(|mid| {
            v0.cache_qc_and_check_commit(mid, &db).unwrap(); // migration #2 (h2 r1 over h2 r0)
            let mut votes = vec![synth(2, 3, 1, h_b3p), synth(3, 3, 1, h_b3p)];
            if let Some(vote) = gated(&v0, &b3p) {
                votes.push(vote);
            }
            assemble_qc(3, 1, h_b3p, votes)
        });
        println!("[535004] branch A' top QC formed: {}", qc_ap_h3.is_some());

        // Honest V1 commits along branch A (3-chain: QC@h3 commits height 1).
        let mut v1 = ConsensusState::new(validators[1].0);
        v1.cache_block(b1.clone()).unwrap();
        v1.cache_block(b2.clone()).unwrap();
        v1.cache_block(b3.clone()).unwrap();
        let v1_chain = v1
            .cache_qc_and_check_commit(qc_a_h3, &db)
            .expect("V1 branch-A commit walk");
        v1.apply_commits(&v1_chain).expect("V1 apply_commits");
        let v1_h1 = v1_chain.iter().find(|b| b.height == 1).map(hash);

        // Honest V2 commits along branch A' (reachable only if the conflicting
        // 3-chain completed, which requires V0's migrated vote).
        let v2_h1 = qc_ap_h3.and_then(|qc| {
            let mut v2 = ConsensusState::new(validators[2].0);
            v2.cache_block(b1p.clone()).unwrap();
            v2.cache_block(b2p.clone()).unwrap();
            v2.cache_block(b3p.clone()).unwrap();
            let chain = v2
                .cache_qc_and_check_commit(qc, &db)
                .expect("V2 branch-A' commit walk");
            v2.apply_commits(&chain).expect("V2 apply_commits");
            chain.iter().find(|b| b.height == 1).map(hash)
        });

        println!("[535004] V1 committed at height 1: {v1_h1:02x?}");
        println!("[535004] V2 committed at height 1: {v2_h1:02x?}");

        // Harness sanity (holds before AND after the fix): the honest legit
        // branch always commits B_1. A failure here is a harness fault.
        assert_eq!(
            v1_h1,
            Some(h_b1),
            "harness: honest branch A must commit B_1 at height 1"
        );

        // SAFETY: two honest nodes must NEVER commit different blocks at one
        // height. On HEAD V2 committed B'_1, so this assertion trips with two
        // differing hashes. After the locked-QC fix V2 never commits at height 1
        // (branch A' starves) and this passes.
        if let Some(h_v2) = v2_h1 {
            assert_eq!(
                h_v2, h_b1,
                "535004 SAFETY VIOLATION: V1 and V2 (both honest) committed DIFFERENT blocks at \
                 height 1 (V1 B_1={:02x?}, V2 B'_1={:02x?}). Reached because honest V0 migrated \
                 highest_qc between same-height QCs (round 0 -> round 1) with no round reset and \
                 then voted the conflicting middle block, completing BOTH 3-chains.",
                &h_b1[..8],
                &h_v2[..8]
            );
        }
    }

    /// 535004 Layer 4 no-clear invariant (view-change reset): the lock must
    /// SURVIVE the round/pending-state reset that fires on a strict height
    /// advance in cache_qc_and_check_commit (lines ~1328-1342), and still block
    /// a conflicting same-height migration afterward. No prior test drives a
    /// reset between conflicting migration attempts, so this guards the
    /// post-reset end state (lock present and still gating). The dangerous
    /// "stray locked_qc = None in a reset NOT followed by a re-SET" case is
    /// guarded by the post-commit sibling test below.
    #[test]
    fn lock_survives_view_change_reset_and_still_blocks_conflict() {
        use novai_state::MemKv;
        let validators = make_test_validators(4);
        let db = MemKv::new();
        let mut state = ConsensusState::new(validators[0].0);

        let hash = |b: &Block| novai_consensus_types::codec::hash_block_v1(b).unwrap();
        let mk = |height: u64, round: u64, parent: [u8; 32]| Block {
            height,
            round,
            parent_hash: parent,
            state_root: [0u8; 32],
            txs: vec![],
        };
        let b1 = mk(1, 0, [0u8; 32]);
        let h_b1 = hash(&b1);
        let b2 = mk(2, 0, h_b1);
        let h_b2 = hash(&b2);
        // Conflicting block at the SAME height as B_2, higher round.
        let b2p = mk(2, 1, h_b1);
        let h_b2p = hash(&b2p);
        assert_ne!(h_b2, h_b2p, "the conflicting block must hash differently");

        // Empty-votes QCs, the convention of commit_rule_3_chain: the commit
        // path adopts via encode_qc_v1 (distinct-voter check), not signatures.
        let qc_a_h1 = QC { height: 1, round: 0, block_hash: h_b1, votes: vec![] };
        let qc_a_h2 = QC { height: 2, round: 0, block_hash: h_b2, votes: vec![] };
        let qc_ap_h2 = QC { height: 2, round: 1, block_hash: h_b2p, votes: vec![] };

        // Adopt h1, then h2. The h2 adoption is a strict height advance, so the
        // view-change reset fires. Set round nonzero first so the reset shows.
        state.cache_qc_and_check_commit(qc_a_h1, &db).unwrap();
        state.round = 5;
        state.cache_qc_and_check_commit(qc_a_h2, &db).unwrap();
        assert_eq!(state.round, 0, "view-change reset must have fired (round cleared to 0)");

        // The reset advanced the lock to B_2; it did NOT null it.
        assert_eq!(
            state.locked_qc.as_ref().map(|q| q.block_hash),
            Some(h_b2),
            "locked_qc must survive the view-change reset (advanced to B_2, not nulled)"
        );

        // A conflicting same-height higher-round QC is still refused after the
        // reset: highest_qc stays on B_2, the lock did not migrate to B'_2.
        state.cache_qc_and_check_commit(qc_ap_h2, &db).unwrap();
        assert_eq!(
            state.highest_qc.as_ref().map(|q| q.block_hash),
            Some(h_b2),
            "lock must still block the conflicting same-height migration after a reset"
        );
        assert_eq!(
            state.locked_qc.as_ref().map(|q| q.block_hash),
            Some(h_b2),
            "locked_qc unchanged by the refused conflicting QC"
        );
    }

    /// 535004 Layer 4 no-clear invariant (post-commit reset): the lock must
    /// SURVIVE the reset in apply_commits, which clears round-scoped state on
    /// every commit and, unlike the view-change reset, is NOT followed by a
    /// lock SET. A stray `locked_qc = None` here would therefore NOT be re-set,
    /// silently reopening the attack, and would pass the whole suite today. This
    /// is the test that catches that regression directly.
    ///
    /// The round-sync reset in add_timeout (the fast-forward clear) is
    /// deliberately not given its own test: it is structurally identical to
    /// this one (clears round-scoped state, no following lock SET), covered by
    /// the same reasoning and the write-site census (locked_qc is written only
    /// in new, the three gated adoptions, and recover, never in a reset).
    #[test]
    fn lock_survives_commit_reset_and_still_blocks_conflict() {
        use novai_state::MemKv;
        let validators = make_test_validators(4);
        let db = MemKv::new();
        let mut state = ConsensusState::new(validators[0].0);

        let hash = |b: &Block| novai_consensus_types::codec::hash_block_v1(b).unwrap();
        let mk = |height: u64, round: u64, parent: [u8; 32]| Block {
            height,
            round,
            parent_hash: parent,
            state_root: [0u8; 32],
            txs: vec![],
        };
        let b1 = mk(1, 0, [0u8; 32]);
        let h_b1 = hash(&b1);
        let b2 = mk(2, 0, h_b1);
        let h_b2 = hash(&b2);
        let b3 = mk(3, 0, h_b2);
        let h_b3 = hash(&b3);
        // Conflicting block at the SAME height as B_3, higher round.
        let b3p = mk(3, 1, h_b2);
        let h_b3p = hash(&b3p);
        assert_ne!(h_b3, h_b3p, "the conflicting block must hash differently");

        state.cache_block(b1).unwrap();
        state.cache_block(b2).unwrap();
        state.cache_block(b3).unwrap();

        let qc_a_h1 = QC { height: 1, round: 0, block_hash: h_b1, votes: vec![] };
        let qc_a_h2 = QC { height: 2, round: 0, block_hash: h_b2, votes: vec![] };
        let qc_a_h3 = QC { height: 3, round: 0, block_hash: h_b3, votes: vec![] };
        let qc_ap_h3 = QC { height: 3, round: 1, block_hash: h_b3p, votes: vec![] };

        // 3-chain: a QC at height 3 commits the block at height 1.
        state.cache_qc_and_check_commit(qc_a_h1, &db).unwrap();
        state.cache_qc_and_check_commit(qc_a_h2, &db).unwrap();
        let to_commit = state.cache_qc_and_check_commit(qc_a_h3, &db).unwrap();
        assert_eq!(to_commit.len(), 1, "QC at height 3 commits block 1");

        // Drive the post-commit reset. Set round nonzero first so it shows.
        state.round = 5;
        state.apply_commits(&to_commit).unwrap();
        assert_eq!(state.committed_height, 1, "block 1 committed");
        assert_eq!(state.round, 0, "post-commit reset must have fired (round cleared to 0)");

        // The commit reset did NOT null the lock.
        assert_eq!(
            state.locked_qc.as_ref().map(|q| q.block_hash),
            Some(h_b3),
            "locked_qc must survive the post-commit reset (still B_3, not nulled)"
        );

        // A conflicting same-height higher-round QC is still refused after the
        // commit reset: highest_qc stays on B_3, no migration to B'_3.
        state.cache_qc_and_check_commit(qc_ap_h3, &db).unwrap();
        assert_eq!(
            state.highest_qc.as_ref().map(|q| q.block_hash),
            Some(h_b3),
            "lock must still block the conflicting same-height migration after the commit reset"
        );
    }

    // ===== Stage 2 Fix D (gate-equivocation-535004): self-heal on timeout =====

    /// Build a domain-separated, validly signed timeout carrying `qc`.
    fn fixd_signed_timeout(
        signer: &SigningKey,
        voter: Address,
        height: u64,
        round: u64,
        qc: Option<QC>,
    ) -> Timeout {
        let unsigned = Timeout {
            height,
            round,
            voter,
            highest_qc: qc,
            signature: [0u8; 64],
        };
        let unsigned_bytes =
            novai_consensus_types::codec::encode_timeout_v1_unsigned(&unsigned).unwrap();
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(b"NOVAI_TIMEOUT_V1");
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = novai_crypto::sign_bytes(signer, &to_sign);
        Timeout {
            signature,
            ..unsigned
        }
    }

    #[test]
    fn add_timeout_early_adoption_self_heals_wrong_view() {
        // A node at view 0 receives a timeout from a peer far ahead, carrying
        // a dominating valid QC. Before Fix D the height gate rejected the
        // timeout and the node never learned the QC; now it adopts the QC
        // before the gate and self-heals.
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let bh = [0x55; 32];
        let qc5 = QC {
            height: 5,
            round: 0,
            block_hash: bh,
            votes: (0..3)
                .map(|i| fixb_signed_vote(&validators[i].1, validators[i].0, 5, 0, bh))
                .collect(),
        };
        // After adopting QC(5), the node's expected timeout height is 6, so a
        // timeout for height 6 is processed rather than rejected.
        let timeout = fixd_signed_timeout(&validators[1].1, validators[1].0, 6, 0, Some(qc5));

        let mut state = ConsensusState::new(validator_set[0]);
        let result = state.add_timeout(timeout, &pubkeys);

        assert!(
            result.is_ok(),
            "the bringing timeout should be accepted after self-heal, got {result:?}"
        );
        assert_eq!(
            state.highest_qc.as_ref().map(|q| q.height),
            Some(5),
            "the wrong-view node must adopt the dominating QC and self-heal"
        );
    }

    #[test]
    fn add_timeout_height_mismatch_absent_qc_rejected() {
        // A height-mismatched timeout carrying no QC has nothing to adopt and
        // is still rejected at the gate.
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let timeout = fixd_signed_timeout(&validators[1].1, validators[1].0, 6, 0, None);

        let mut state = ConsensusState::new(validator_set[0]);
        let result = state.add_timeout(timeout, &pubkeys);
        assert!(
            result.is_err(),
            "a height-mismatched timeout with no QC must still be rejected"
        );
        assert!(state.highest_qc.is_none());
    }

    #[test]
    fn add_timeout_height_mismatch_invalid_qc_rejected_no_adoption() {
        // A height-mismatched timeout carrying a sub-quorum QC must not adopt
        // the QC (it fails verify_qc_well_formed) and is still rejected.
        let validators = make_test_validators(4);
        let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
        let pubkeys: Vec<(Address, VerifyingKey)> =
            validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
        let bh = [0x55; 32];
        let bad_qc = QC {
            height: 5,
            round: 0,
            block_hash: bh,
            votes: (0..2)
                .map(|i| fixb_signed_vote(&validators[i].1, validators[i].0, 5, 0, bh))
                .collect(),
        };
        let timeout = fixd_signed_timeout(&validators[1].1, validators[1].0, 6, 0, Some(bad_qc));

        let mut state = ConsensusState::new(validator_set[0]);
        let result = state.add_timeout(timeout, &pubkeys);
        assert!(
            result.is_err(),
            "a height-mismatched timeout with an invalid QC must be rejected"
        );
        assert!(
            state.highest_qc.is_none(),
            "an invalid QC must NOT be adopted even via the early-adoption pass"
        );
    }
}
