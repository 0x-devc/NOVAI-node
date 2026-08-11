//! Networked consensus node implementation.

use crate::MutexExt;
use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::{ConsensusError, ConsensusState};
use novai_consensus_types::{Block, SignedProposal, Timeout, Vote, QC};
use novai_crypto::{address_from_pubkey, sign_bytes};
use novai_p2p::noise::{
    handshake_initiator, handshake_responder, is_known_validator, noise_keypair_from_seed,
};
use novai_p2p::{
    connect_to_peer, read_wire_message, start_listener, ConnectionLimiter, NetworkMessage,
    PeerBanList, PeerManager,
};
use novai_state::{Kv, KvBatch, MemKv, RocksKv, WriteOp};
use novai_types::Address;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Maximum number of blocks to request in a single sync chunk.
/// Prevents timeout on large catch-up ranges (e.g., 50k+ blocks).
pub const SYNC_CHUNK_SIZE: u64 = 500;

/// F2 responder byte budget at the DEFAULT send cap: half the cap
/// (fixplan: MAX_WIRE_MSG_BYTES / 2) so a response always encodes with
/// ample headroom. The count clamp alone let a tx-heavy range assemble
/// past the wire cap and die at encode_wire_message, sending nothing to
/// any peer and starving the requester. F3: the responder derives its
/// effective soft budget from the RUNTIME send cap via
/// [`response_byte_budget`]; this constant is that rule evaluated at the
/// default, kept so the deployed Phase A value stays pinned at
/// compile time.
pub const RESPONSE_BYTE_BUDGET: usize = novai_p2p::MAX_WIRE_MSG_BYTES as usize / 2;

/// F3 responder soft budget rule: half the runtime wire send cap.
/// Evaluates to 1,048,576 at the 2 MiB Phase A default (byte-identical
/// to the deployed F2 behavior) and 8,388,608 at the 16 MiB Phase B cap
/// (gate F3 diagnosis 12.3, 12.5). Bulk shaping only: the 3-pair floor
/// in build_block_response is exempt from this budget.
#[must_use]
pub const fn response_byte_budget(wire_send_cap: u32) -> usize {
    wire_send_cap as usize / 2
}

/// Fixed BlockResponse payload header: version + responder + request_start
/// + request_end + block_count + qc_count. Mirrors the decoder's MIN_SIZE
/// (decode_block_response_v2, consensus_types/codec.rs). Public so the
/// gate test can pin this constant plus the per-pair formula against the
/// real encoder output, byte for byte.
pub const RESPONSE_HEADER_BYTES: usize = 1 + 32 + 8 + 8 + 4 + 4;

// The budget plus the response header plus the 2-byte wire envelope must
// sit strictly under the hard cap, or a floor pair at the budget boundary
// could never encode. Compile-time pinned so the budget derivation cannot
// drift from the p2p constant. The same headroom holds at every legal
// runtime cap because the rule is cap / 2 and validate_wire_send_cap
// bounds the cap to [default, receive cap].
const _: () = assert!(
    RESPONSE_BYTE_BUDGET + RESPONSE_HEADER_BYTES + 2 < novai_p2p::MAX_WIRE_MSG_BYTES as usize
);

/// Fixed SignedProposal envelope bytes around the block txs and the
/// justify QC: PROPOSAL_V1 tag + proposer + signature (97 bytes,
/// consensus_types/codec.rs encode_signed_proposal_v1), block header
/// (85 bytes: version + height + round + parent_hash + state_root +
/// tx_count, encode_block_v1), and the 2 wire bytes counted in the
/// length check (p2p encode). The variable remainder is the justify QC,
/// which the proposer guard MEASURES with the real codec (a hand formula
/// for a vote-set-sized value would drift; the F2 lesson).
pub const PROPOSAL_ENVELOPE_FIXED_BYTES: usize = (1 + 32 + 64) + (1 + 8 + 8 + 32 + 32 + 4) + 2;

/// Worst valid encoded (block, QC) pair in a BlockResponse: a block at
/// MAX_BLOCK_SIZE with its 85-byte header, the has_qc flag byte, and a
/// QC at the codec ceiling (53-byte header + MAX_VOTES_PER_QC votes at
/// their 178-byte signed-with-signal encoding). Gate F3 diagnosis 12.1;
/// the 178 mirrors encode_vote_v1_signed (81 unsigned + 64 signature +
/// 1 has_signal flag + 32 commitment).
pub const MAX_PAIR_BYTES: usize = (85 + novai_types::MAX_BLOCK_SIZE)
    + 1
    + (53 + novai_consensus_types::codec::MAX_VOTES_PER_QC * 178);

// The frontier guarantee (fixplan :122, gate F3 diagnosis 12.2/12.3): one
// sync response must be able to carry 3 FULL pairs, because the requester
// restarts at committed+1 and the 3-chain rule needs committed+1..+3, so
// anything less re-serves the same prefix forever. Compile-time pinned:
// if a codec constant grows past this, the receive cap must be re-derived
// before the build is shippable.
const _: () = assert!(
    3 * MAX_PAIR_BYTES + RESPONSE_HEADER_BYTES + 2
        <= novai_p2p::MAX_RECV_WIRE_MSG_BYTES as usize
);

/// Startup validation for the runtime wire send cap
/// (--wire-send-cap-bytes). Below the 2 MiB default, the responder
/// budget and floor lose their deployed guarantees; above the receive
/// cap, this node could emit frames the fleet rejects, which partitions
/// a mixed fleet (gate F3 diagnosis 12.6).
///
/// # Errors
/// Returns a message naming the violated bound.
pub fn validate_wire_send_cap(cap: u32) -> Result<(), String> {
    if cap < novai_p2p::MAX_WIRE_MSG_BYTES {
        return Err(format!(
            "--wire-send-cap-bytes {cap} is below the {} default; the send \
             cap may only be raised, never lowered",
            novai_p2p::MAX_WIRE_MSG_BYTES
        ));
    }
    if cap > novai_p2p::MAX_RECV_WIRE_MSG_BYTES {
        return Err(format!(
            "--wire-send-cap-bytes {cap} exceeds the {} receive cap; a node \
             must never send frames the fleet cannot accept",
            novai_p2p::MAX_RECV_WIRE_MSG_BYTES
        ));
    }
    Ok(())
}

/// F1 sync retry backoff: base delay, doubled per consecutive strike.
pub const SYNC_RETRY_BASE_MS: u64 = 2_000;

/// F1 sync retry backoff ceiling. Also the period of the behind-retention
/// low-rate probe and its ERROR escalation log.
pub const SYNC_RETRY_MAX_MS: u64 = 60_000;

/// Sync strikes at or above this count log at WARN instead of DEBUG.
pub const SYNC_STRIKE_WARN_THRESHOLD: u32 = 3;

/// Backoff delay before the next sync request after `strikes` consecutive
/// failed cycles: min(2s * 2^strikes, 60s). Zero strikes means no gate.
pub fn sync_backoff_ms(strikes: u32) -> u64 {
    if strikes == 0 {
        return 0;
    }
    SYNC_RETRY_BASE_MS
        .checked_shl(strikes)
        .unwrap_or(u64::MAX)
        .min(SYNC_RETRY_MAX_MS)
}

/// Pure retry decision for the sync requester: may a new request be issued
/// `elapsed` after the previous one, given `strikes` consecutive failed
/// cycles? `None` means no request has been issued yet; the first attempt
/// is never gated. Pure so it unit-tests without clocks.
pub fn sync_retry_due(strikes: u32, elapsed: Option<Duration>) -> bool {
    match elapsed {
        None => true,
        Some(e) => e >= Duration::from_millis(sync_backoff_ms(strikes)),
    }
}

/// Gate F5 Stage 1: consecutive UNSERVED probes required before the
/// snapshot-sync machine arms.
///
/// The retention arithmetic alone (the gap exceeds `PRUNE_RETAIN_BLOCKS`) is
/// an inference about other nodes' disks. A probe that comes back unserved is
/// a direct observation that they will not serve the range. Two of them at
/// the 60 second probe period is a two minute arming delay on a node that has
/// already been unrecoverable for hours, so the cost is nil, and it removes
/// the whole class of "armed on one transient answer". Lowering this to 1
/// removes the evidence requirement, which is the rule's entire purpose.
pub const ARM_PROBE_FAILURES: u32 = 2;

/// Gate F5 Stage 1: the behind-retention detection phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapshotSyncPhase {
    /// Block-range sync is viable, or there is no gap at all.
    #[default]
    Idle,
    /// The gap is past the fleet's prune horizon and probes are being issued,
    /// but the evidence threshold is not met yet.
    Arming,
    /// `ARM_PROBE_FAILURES` consecutive probes came back unserved: block-range
    /// sync is structurally impossible for this node, and only an installed
    /// state snapshot can recover it. Stage 1 stops here; the fetch, verify
    /// and install phases are later, separately gated stages.
    Armed,
}

/// Gate F5 Stage 1: the behind-retention detection machine.
///
/// Pure: no clock, no lock, no I/O, so the transition rules unit-test
/// directly, in the same spirit as [`sync_backoff_ms`] and [`sync_retry_due`].
/// It lives inside [`SyncRetryState`] so it shares that lock: both of its
/// drive points (the behind-retention branch of `try_request_missing_blocks`
/// and `record_sync_strike`) already hold it, so the machine adds no lock and
/// no new lock ordering to a node that has paid for lock cycles before.
///
/// The machine decides only WHICH RECOVERY MODE to run. It carries no safety
/// weight: a snapshot is trusted because a quorum signed the header it is
/// verified against, never because this machine decided to go and fetch one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SnapshotSyncMachine {
    phase: SnapshotSyncPhase,
    unserved_probes: u32,
    /// Committed height when the machine last left `Idle`. Any advance past
    /// this is commit progress, which proves block sync is still serving.
    baseline_committed: u64,
}

impl SnapshotSyncMachine {
    #[must_use]
    pub fn phase(&self) -> SnapshotSyncPhase {
        self.phase
    }

    #[must_use]
    pub fn unserved_probes(&self) -> u32 {
        self.unserved_probes
    }

    /// The `novai_sync_mode` gauge value. 0, 1 and 2 are the Stage 1 phases;
    /// 3, 4 and 5 are RESERVED for the fetch, verify and staged phases of the
    /// later stages, so the gauge encoding, the dashboard and the monitor's
    /// alarm never have to be renumbered. The monitor treats anything at or
    /// above 1 as firing, so a later phase cannot silently go quiet.
    #[must_use]
    pub fn gauge(&self) -> u64 {
        match self.phase {
            SnapshotSyncPhase::Idle => 0,
            SnapshotSyncPhase::Arming => 1,
            SnapshotSyncPhase::Armed => 2,
        }
    }

    /// Commit progress disarms, from ANY phase. Returns true if this call
    /// disarmed. A node that is committing is a node block sync can still
    /// serve, and it must never install a snapshot.
    pub fn observe_commit_progress(&mut self, committed: u64) -> bool {
        if self.phase == SnapshotSyncPhase::Idle || committed <= self.baseline_committed {
            return false;
        }
        *self = Self::default();
        true
    }

    /// This cycle observed a gap past the prune horizon. Entering the band is
    /// not itself evidence, so the probe count starts at zero and only an
    /// actually unserved probe advances it.
    pub fn note_behind_retention(&mut self, committed: u64) {
        if self.phase == SnapshotSyncPhase::Idle {
            self.phase = SnapshotSyncPhase::Arming;
            self.unserved_probes = 0;
            self.baseline_committed = committed;
        }
    }

    /// A sync cycle failed: a matching empty response or a timed-out request,
    /// both meaning the requested range was not served. It counts as evidence
    /// ONLY while the machine sits in `Arming`, which it can reach only
    /// through `note_behind_retention`. That is what keeps an ordinary strike
    /// inside the retention window from ever arming.
    pub fn note_unserved_probe(&mut self) {
        if self.phase != SnapshotSyncPhase::Arming {
            return;
        }
        self.unserved_probes = self.unserved_probes.saturating_add(1);
        if self.unserved_probes >= ARM_PROBE_FAILURES {
            self.phase = SnapshotSyncPhase::Armed;
        }
    }

    /// The gap is inside the retention window: block-range sync is viable, so
    /// the machine has no business holding state.
    pub fn note_within_retention(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod snapshot_sync_machine_tests {
    use super::{SnapshotSyncMachine, SnapshotSyncPhase, ARM_PROBE_FAILURES};

    /// Drive the machine to `Armed` from a given committed baseline.
    fn armed_at(committed: u64) -> SnapshotSyncMachine {
        let mut m = SnapshotSyncMachine::default();
        m.note_behind_retention(committed);
        for _ in 0..ARM_PROBE_FAILURES {
            m.note_unserved_probe();
        }
        assert_eq!(m.phase(), SnapshotSyncPhase::Armed);
        m
    }

    #[test]
    fn default_is_idle_with_no_evidence() {
        let m = SnapshotSyncMachine::default();
        assert_eq!(m.phase(), SnapshotSyncPhase::Idle);
        assert_eq!(m.unserved_probes(), 0);
        assert_eq!(m.gauge(), 0);
    }

    #[test]
    fn gauge_encodes_every_phase() {
        let mut m = SnapshotSyncMachine::default();
        assert_eq!(m.gauge(), 0);
        m.note_behind_retention(0);
        assert_eq!(m.gauge(), 1);
        for _ in 0..ARM_PROBE_FAILURES {
            m.note_unserved_probe();
        }
        assert_eq!(m.gauge(), 2);
    }

    #[test]
    fn an_idle_machine_never_reports_a_spurious_disarm() {
        // Without the Idle guard, every call on a node with a positive
        // committed height would report a disarm, because the baseline of an
        // idle machine is zero.
        let mut m = SnapshotSyncMachine::default();
        assert!(!m.observe_commit_progress(1_580_000));
        assert_eq!(m.phase(), SnapshotSyncPhase::Idle);
    }

    #[test]
    fn a_flat_committed_height_is_not_progress() {
        let mut m = armed_at(1_580_000);
        assert!(!m.observe_commit_progress(1_580_000), "equal is not progress");
        assert_eq!(m.phase(), SnapshotSyncPhase::Armed);
    }

    #[test]
    fn commit_progress_clears_phase_and_evidence() {
        let mut m = armed_at(1_580_000);
        assert!(m.observe_commit_progress(1_580_001));
        assert_eq!(m.phase(), SnapshotSyncPhase::Idle);
        assert_eq!(m.unserved_probes(), 0);
    }

    /// Direct coverage for the fail-safe reset that the call site cannot
    /// exercise at this tree (see the note at the `note_within_retention`
    /// call in `try_request_missing_blocks`). Without this test the fail-safe
    /// would be uncovered code, which is how fail-safes rot.
    #[test]
    fn within_retention_clears_phase_and_evidence_from_armed() {
        let mut m = armed_at(1_580_000);
        m.note_within_retention();
        assert_eq!(m.phase(), SnapshotSyncPhase::Idle);
        assert_eq!(m.unserved_probes(), 0);
        assert_eq!(m.gauge(), 0);
    }

    #[test]
    fn re_entering_the_band_restarts_the_evidence_count() {
        let mut m = SnapshotSyncMachine::default();
        m.note_behind_retention(100);
        m.note_unserved_probe();
        assert_eq!(m.unserved_probes(), 1);

        m.observe_commit_progress(101);
        m.note_behind_retention(101);
        assert_eq!(
            m.unserved_probes(),
            0,
            "evidence must be consecutive, never a lifetime tally"
        );
    }

    #[test]
    fn note_behind_retention_is_idempotent_and_never_regresses_evidence() {
        let mut m = SnapshotSyncMachine::default();
        m.note_behind_retention(100);
        m.note_unserved_probe();
        // A second beyond-retention cycle before the threshold must not reset
        // the count it has already banked, or the machine could never arm.
        m.note_behind_retention(100);
        assert_eq!(m.unserved_probes(), 1);
        assert_eq!(m.phase(), SnapshotSyncPhase::Arming);
    }

    #[test]
    fn armed_is_stable_under_further_unserved_probes() {
        let mut m = armed_at(100);
        let before = m.unserved_probes();
        m.note_unserved_probe();
        assert_eq!(m.phase(), SnapshotSyncPhase::Armed);
        assert_eq!(
            m.unserved_probes(),
            before,
            "the count must not grow without bound once armed"
        );
    }

    #[test]
    fn a_strike_on_an_idle_machine_is_ignored() {
        let mut m = SnapshotSyncMachine::default();
        m.note_unserved_probe();
        assert_eq!(m.phase(), SnapshotSyncPhase::Idle);
        assert_eq!(m.unserved_probes(), 0);
    }
}

/// Sized wrapper around a shared `NonceProvider` trait object for gossip tx insertion.
struct GossipNonceProvider(Arc<dyn mempool::NonceProvider + Send + Sync>);

impl mempool::NonceProvider for GossipNonceProvider {
    fn expected_nonce(&self, from: &Address) -> u64 {
        self.0.expected_nonce(from)
    }
}

/// Storage backend for the consensus node.
///
/// Unifies `MemKv` (in-memory, volatile) and `RocksKv` (persistent, disk-backed)
/// behind a single type so `ConsensusNode` is backend-agnostic.
pub enum Storage {
    Memory(MemKv),
    Rocks(RocksKv),
}

impl Kv for Storage {
    type Error = String;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        match self {
            Storage::Memory(kv) => kv.get(key).map_err(|()| "in-memory storage error".into()),
            Storage::Rocks(kv) => kv.get(key).map_err(|e| e.to_string()),
        }
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), String> {
        match self {
            Storage::Memory(kv) => kv
                .put(key, value)
                .map_err(|()| "in-memory storage error".into()),
            Storage::Rocks(kv) => kv.put(key, value).map_err(|e| e.to_string()),
        }
    }

    // gate 9: forward to the inner backend's synced put so the RocksDB path
    // reaches the real WAL fsync. The trait default would call Storage::put,
    // which is NOT synced; MemKv has no WAL, so its default is correct.
    fn put_synced(&mut self, key: &[u8], value: &[u8]) -> Result<(), String> {
        match self {
            Storage::Memory(kv) => kv
                .put_synced(key, value)
                .map_err(|()| "in-memory storage error".into()),
            Storage::Rocks(kv) => kv.put_synced(key, value).map_err(|e| e.to_string()),
        }
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), String> {
        match self {
            Storage::Memory(kv) => kv
                .delete(key)
                .map_err(|()| "in-memory storage error".into()),
            Storage::Rocks(kv) => kv.delete(key).map_err(|e| e.to_string()),
        }
    }

    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        match self {
            Storage::Memory(kv) => kv
                .scan_prefix(prefix)
                .map_err(|()| "in-memory storage error".into()),
            Storage::Rocks(kv) => kv.scan_prefix(prefix).map_err(|e| e.to_string()),
        }
    }
}

impl KvBatch for Storage {
    fn apply_batch(&mut self, ops: &[WriteOp]) -> Result<(), String> {
        match self {
            Storage::Memory(kv) => kv
                .apply_batch(ops)
                .map_err(|()| "in-memory storage error".into()),
            Storage::Rocks(kv) => kv.apply_batch(ops).map_err(|e| e.to_string()),
        }
    }
}

impl Storage {
    /// Force a compaction over `[start, end)` on the default column family.
    ///
    /// No-op on the in-memory backend. On RocksDB, used to materialize block
    /// and QC delete tombstones written by `persist_commit_atomic` so the
    /// underlying SST bytes are actually reclaimed. Without periodic
    /// compaction, tombstones accumulate and disk usage grows beyond the
    /// `PRUNE_RETAIN_BLOCKS` retention window.
    pub fn compact_range_default(&self, start: Option<&[u8]>, end: Option<&[u8]>) {
        match self {
            Storage::Memory(_) => {}
            Storage::Rocks(kv) => kv.compact_range_default(start, end),
        }
    }

    /// Synchronously flush the default-CF memtable.
    ///
    /// No-op on the in-memory backend. On RocksDB, delegates to
    /// `RocksKv::flush_default`. See that method for the durability
    /// rationale (Bug 1 latent concern B).
    ///
    /// # Errors
    /// Returns a stringified RocksDB error if the flush fails. Callers
    /// typically log and continue rather than abort the commit loop.
    pub fn flush_default(&self) -> Result<(), String> {
        match self {
            Storage::Memory(_) => Ok(()),
            Storage::Rocks(kv) => kv.flush_default().map_err(|e| e.to_string()),
        }
    }
}

/// Callback invoked after blocks are committed and consensus state is updated.
///
/// Implementations execute transactions against the state DB and update the
/// nonce provider. The DB lock is already held by the caller.
/// Post-persist execution seam, per committed block (gate ACCEL Stage B).
/// `cached` carries the block's vote-time execution when the commit site took
/// it from `pending_exec`; `None` means the callback re-executes once in the
/// overlay. Errors propagate on the same channel as the post-execution
/// divergence halt: the commit batch is already durable, the node freezes
/// fail-closed, and replay self-heals on restart.
pub trait CommitCallback: Send + Sync {
    fn on_commit(
        &self,
        db: &mut Storage,
        block: &Block,
        cached: Option<crate::exec_apply::CachedExec>,
    ) -> Result<(), String>;
}

/// Cache for tracking which QCs have been broadcasted (to avoid duplicates).
type QcBroadcastCache = Arc<Mutex<HashSet<(u64, u64, [u8; 32])>>>;

/// Consensus node with networking.
/// Tracks a pending block sync request.
#[derive(Debug, Clone)]
pub struct PendingSyncRequest {
    pub peer: Address,
    pub start_height: u64,
    pub end_height: u64,
    pub request_time: Instant,
}

/// F1 sync retry state: consecutive failed sync cycles (a matching empty
/// response or a request timeout, both meaning "the range was not served")
/// plus the issuance bookkeeping that gates re-requests. Commit progress
/// resets the strikes; see `try_request_missing_blocks`.
#[derive(Debug, Default)]
pub struct SyncRetryState {
    /// Consecutive failed sync cycles since the last progress or reset.
    pub strikes: u32,
    /// When the most recent sync request was issued.
    pub last_attempt: Option<Instant>,
    /// `committed_height` observed when the last strike was recorded; any
    /// commit past this height resets the strikes.
    pub strike_committed_height: u64,
    /// When the behind-retention condition was last escalated at ERROR.
    pub last_escalation_log: Option<Instant>,
    /// Gate F5 Stage 1 detection machine. It lives here rather than behind its
    /// own mutex because both of its drive points already hold this lock; see
    /// [`SnapshotSyncMachine`].
    pub snapshot_sync: SnapshotSyncMachine,
}

/// Outcome of `try_request_missing_blocks` (F1). `BehindRetention` is the
/// deterministic "cannot block-sync this range, needs snapshot" signal for
/// the snapshot-sync layer (F4); the other variants let call sites and
/// tests observe the retry gate. Callers that only opportunistically nudge
/// sync may ignore the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRequestOutcome {
    /// A sync request was issued and the pending slot armed.
    Requested,
    /// No committable gap: highest QC is not beyond committed + 2.
    NoGap,
    /// A request is already in flight; dedup suppressed this trigger.
    AlreadyPending,
    /// The backoff window from prior strikes has not elapsed yet.
    BackedOff,
    /// The request could not be issued (no peers, or broadcast failure).
    RequestFailed,
    /// The gap exceeds PRUNE_RETAIN_BLOCKS: no honest peer retains the
    /// range, so block-range sync is structurally impossible and the node
    /// needs a snapshot import. `probed` reports whether this call issued
    /// the low-rate probe request.
    BehindRetention { probed: bool },
}

/// Gate F5 Stage 4: what a snapshot send site did.
///
/// `Disabled` is the Phase A steady state and is deliberately NOT an error: a
/// node that cannot send snapshot messages is a correctly configured node
/// during the receive-first half of the deploy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotSendOutcome {
    /// Sending is off (Phase A). Nothing was encoded and nothing left this node.
    Disabled,
    /// The message was handed to the broadcast path.
    Sent,
    /// Broadcast failed (no peers, transport error).
    SendFailed,
    /// This node is itself recovering, so it refuses to be a source (O7).
    RefusedRecovering,
    /// No producer is attached, so there is nothing to serve from.
    NoProducer,
    /// The requesting peer asked again too soon.
    RateLimited,
}

// L-05: Lock contention metrics (e.g., time spent waiting on state/db mutexes)
// are planned for future observability improvements. Currently, the H-11 fix
// (signature verification outside lock) is the primary contention mitigation.
// When adding metrics, instrument lock_or_recover() with Instant::now() delta.
pub struct ConsensusNode {
    pub our_address: Address,
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
    pub state: Arc<Mutex<ConsensusState>>,
    pub db: Arc<Mutex<Storage>>,
    pub peer_manager: Arc<PeerManager>,
    pub validator_set: Vec<Address>,
    pub validator_pubkeys: HashMap<Address, VerifyingKey>,
    /// Cached (address, pubkey) pairs to avoid repeated allocations in hot path
    pub validator_pubkeys_vec: Vec<(Address, VerifyingKey)>,
    pub qc_broadcasted: QcBroadcastCache,
    pub round_start_time: Arc<Mutex<Instant>>,
    /// When we last broadcast a timeout for the current round (None = haven't timed out yet).
    pub last_timeout_time: Arc<Mutex<Option<Instant>>>,
    pub pending_sync_request: Arc<Mutex<Option<PendingSyncRequest>>>,
    /// F1 sync retry gate state (strikes and backoff bookkeeping).
    pub sync_retry: Arc<Mutex<SyncRetryState>>,
    /// Gate F5 Stage 4. Default FALSE: receiving and serving are on once the
    /// binary is deployed, SENDING waits for the flag. See
    /// [`Self::snapshot_send_enabled`].
    snapshot_send_enabled: std::sync::atomic::AtomicBool,
    /// The producer this node serves cached bundles from, when attached.
    snapshot_producer: Option<Arc<crate::snapshot::producer::SnapshotProducer>>,
    /// Per-peer serve spacing, so one peer cannot pull the whole outbound
    /// budget or spam a node into serving in a loop.
    snapshot_serve: Arc<Mutex<crate::snapshot::wire::ServeLimiter>>,
    /// Per-peer strike ladder for peers whose chunks fail the manifest digest.
    snapshot_peers: Arc<Mutex<crate::snapshot::wire::PeerStrikes>>,
    /// Commit-window rule: the last (height, round) view the proposer
    /// intent check warned about, so a parked fleet logs one warning per
    /// view instead of two hundred per second on the 5 ms propose cadence.
    commit_window_warned_view: Arc<Mutex<Option<(u64, u64)>>>,
    /// Configurable base timeout in milliseconds (default: BASE_TIMEOUT_MS = 1000).
    /// Server environments may need higher values (e.g., 3000) to avoid spurious timeouts.
    pub base_timeout_ms: u64,
    /// X25519 static key for Noise encryption (None = plaintext mode).
    encryption_key: Option<[u8; 32]>,
    /// Known validators' X25519 static keys for peer authentication.
    known_noise_keys: Vec<[u8; 32]>,
    /// Connection limiter for incoming TCP connections (C-03, C-04).
    pub connection_limiter: Arc<ConnectionLimiter>,
    /// Ban list for misbehaving peers (C-02).
    pub ban_list: Arc<PeerBanList>,
    /// Callback for post-commit execution (dispatch_tx + nonce updates).
    pub commit_callback: Option<Arc<dyn CommitCallback>>,
    /// Shared mempool for inserting gossipped transactions from peers.
    pub gossip_mempool: Option<Arc<Mutex<mempool::TxMempool>>>,
    /// Nonce provider for validating gossipped transactions.
    gossip_nonce: Option<Arc<dyn mempool::NonceProvider + Send + Sync>>,
}

impl ConsensusNode {
    /// Create a node with in-memory storage (volatile — for tests and backward compat).
    pub fn new(
        signing_key: SigningKey,
        validator_set: Vec<Address>,
        validator_pubkeys: HashMap<Address, VerifyingKey>,
        base_timeout_ms: u64,
    ) -> Self {
        Self::new_with_storage(
            signing_key,
            validator_set,
            validator_pubkeys,
            base_timeout_ms,
            Storage::Memory(MemKv::new()),
            None,
        )
    }

    /// Create a node with the given storage backend.
    ///
    /// If the storage contains committed state from a previous run, the node
    /// recovers automatically via `ConsensusState::recover()`.
    ///
    /// `ed25519_seed` enables Noise encryption when `Some`. The seed is used
    /// to derive an X25519 static key for the Noise XX handshake.
    pub fn new_with_storage(
        signing_key: SigningKey,
        validator_set: Vec<Address>,
        validator_pubkeys: HashMap<Address, VerifyingKey>,
        base_timeout_ms: u64,
        storage: Storage,
        ed25519_seed: Option<[u8; 32]>,
    ) -> Self {
        let verifying_key = signing_key.verifying_key();
        let our_address = address_from_pubkey(&verifying_key);

        // Pre-cache pubkeys as Vec to avoid repeated allocations in hot path
        let validator_pubkeys_vec: Vec<(Address, VerifyingKey)> = validator_pubkeys
            .iter()
            .map(|(addr, pk)| (*addr, *pk))
            .collect();

        // Attempt recovery from persistent state, pre-populating the block
        // cache so the commit chain walk works immediately after restart.
        // Without this, the cache is empty and the first QCs trigger
        // "Missing block at height X (chain broken)" until sync fills the gap.
        let state = match ConsensusState::recover_with_cache(
            our_address,
            &storage,
            novai_consensus::CACHE_RETAIN_DEPTH,
        ) {
            Ok(recovered) => {
                tracing::info!(
                    committed_height = recovered.committed_height,
                    highest_qc = recovered.highest_qc.as_ref().map(|q| q.height).unwrap_or(0),
                    block_cache = recovered.block_cache.len(),
                    "Recovered state with cache"
                );
                recovered
            }
            Err(e) => {
                tracing::info!(?e, "No prior state to recover, starting fresh");
                ConsensusState::new(our_address)
            }
        };

        // Derive encryption key and known validator noise keys
        let encryption_key = ed25519_seed.map(|s| noise_keypair_from_seed(&s));
        let known_noise_keys: Vec<[u8; 32]> = if ed25519_seed.is_some() {
            // We don't have raw seeds for other validators, but we DO have their
            // X25519 public keys derived during handshake. For peer authentication,
            // we build this list lazily. For dev-keys mode, main.rs will pass the
            // precomputed list via set_known_noise_keys().
            Vec::new()
        } else {
            Vec::new()
        };

        Self {
            our_address,
            signing_key,
            verifying_key,
            state: Arc::new(Mutex::new(state)),
            db: Arc::new(Mutex::new(storage)),
            peer_manager: Arc::new(PeerManager::new()),
            validator_set,
            validator_pubkeys,
            validator_pubkeys_vec,
            qc_broadcasted: Arc::new(Mutex::new(HashSet::new())),
            round_start_time: Arc::new(Mutex::new(Instant::now())),
            last_timeout_time: Arc::new(Mutex::new(None)),
            pending_sync_request: Arc::new(Mutex::new(None)),
            sync_retry: Arc::new(Mutex::new(SyncRetryState::default())),
            // Gate F5 Stage 4: sending starts OFF. Phase B turns it on.
            snapshot_send_enabled: std::sync::atomic::AtomicBool::new(false),
            snapshot_producer: None,
            snapshot_serve: Arc::new(Mutex::new(crate::snapshot::wire::ServeLimiter::default())),
            snapshot_peers: Arc::new(Mutex::new(crate::snapshot::wire::PeerStrikes::default())),
            commit_window_warned_view: Arc::new(Mutex::new(None)),
            base_timeout_ms,
            encryption_key,
            known_noise_keys,
            connection_limiter: Arc::new(ConnectionLimiter::new(
                novai_p2p::MAX_PEERS,
                novai_p2p::MAX_CONNECTIONS_PER_IP,
            )),
            ban_list: Arc::new(PeerBanList::new()),
            commit_callback: None,
            gossip_mempool: None,
            gossip_nonce: None,
        }
    }

    /// Set the shared mempool and nonce provider for transaction gossip.
    ///
    /// When set, incoming `Transaction` messages from peers are decoded and
    /// inserted into the mempool so all validators have txs available for proposal.
    pub fn set_gossip_mempool(
        &mut self,
        mempool: Arc<Mutex<mempool::TxMempool>>,
        nonce_provider: Arc<dyn mempool::NonceProvider + Send + Sync>,
    ) {
        self.gossip_mempool = Some(mempool);
        self.gossip_nonce = Some(nonce_provider);
    }

    /// Set the commit callback for post-persist transaction execution.
    ///
    /// Must be called before the node starts handling peer connections.
    pub fn set_commit_callback(&mut self, cb: Arc<dyn CommitCallback>) {
        self.commit_callback = Some(cb);
    }

    /// Execute committed blocks via the commit callback.
    ///
    /// Called after `persist_commit_atomic` + `apply_commits` with the DB
    /// lock still held. Execution writes to different key namespaces than
    /// consensus persistence (no overlap), so this is safe.
    ///
    /// `cached` (gate ACCEL Stage B) carries each block's vote-time execution
    /// taken by the commit site via `take_pending_execs`, positionally zipped
    /// with `blocks`. Before a cached entry is used, the parent binding is
    /// checked here: the current `KEY_SMT_ROOT` must equal the parent's
    /// header `state_root` (the committed tip for the first block of the
    /// batch, the previous batch block otherwise). A failed or unavailable
    /// binding degrades the entry to a re-execution cache miss; cached bytes
    /// are never applied blind.
    fn execute_committed_blocks(
        &self,
        db: &mut Storage,
        blocks: &[Block],
        cached: Vec<Option<novai_consensus::PendingExec>>,
    ) -> Result<(), String> {
        let total_txs: usize = blocks.iter().map(|b| b.txs.len()).sum();
        for block in blocks {
            let hash = novai_consensus_types::codec::hash_block_v1(block).ok();
            tracing::debug!(
                height = block.height,
                round = block.round,
                tx_count = block.txs.len(),
                block_hash = ?hash.as_ref().map(|h| &h[..4]),
                "COMMIT_DIAG: committed block"
            );
        }
        tracing::debug!(
            block_count = blocks.len(),
            total_txs,
            "COMMIT_DIAG: execute_committed_blocks"
        );
        if total_txs > 0 {
            let block_count = blocks.len();
            tracing::info!(block_count, total_txs, "Committed blocks with transactions");
        }
        if let Some(ref cb) = self.commit_callback {
            let mut cached = cached.into_iter();
            for (i, block) in blocks.iter().enumerate() {
                let mut cached_exec = cached
                    .next()
                    .flatten()
                    .and_then(crate::exec_apply::CachedExec::from_pending);

                // Pre-apply parent binding for the cached path (gate ACCEL
                // Stage B): the write set was computed over post-state(parent),
                // so the database must sit at exactly that state before the
                // cached bytes may be applied. Anything else falls back to
                // re-execution, which the existing checks then judge.
                if cached_exec.is_some() {
                    let parent_root = Self::parent_header_root(db, blocks, i);
                    let current_root = match db.get(novai_state::KEY_SMT_ROOT) {
                        Ok(Some(bytes)) => novai_state::decode_smt_root_v1(&bytes).ok(),
                        Ok(None) => Some(novai_execution::empty_smt_root()),
                        Err(_) => None,
                    };
                    let bound = match (parent_root, current_root) {
                        (Some(p), Some(c)) => p == c,
                        _ => false,
                    };
                    if !bound {
                        tracing::warn!(
                            height = block.height,
                            "cached execution parent binding failed or unavailable; \
                             falling back to re-execution"
                        );
                        cached_exec = None;
                    }
                }

                cb.on_commit(db, block, cached_exec)?;
                // Post-execution divergence check (gate wedge-276272): after
                // executing a committed block, the persisted root must equal that
                // block's post-state header. A mismatch is local divergence; halt.
                // This is the lag-0 companion to the pre-commit check and extends
                // coverage to the sync-path commits and boot replay.
                let current_root = match db.get(novai_state::KEY_SMT_ROOT) {
                    Ok(Some(bytes)) => novai_state::decode_smt_root_v1(&bytes)
                        .map_err(|e| format!("Failed to decode SMT root: {e:?}"))?,
                    Ok(None) => novai_execution::empty_smt_root(),
                    Err(e) => return Err(format!("Failed to read SMT root: {e:?}")),
                };
                if current_root != block.state_root {
                    return Err(format!(
                        "CONSENSUS SAFETY HALT: post-execution state root mismatch at height {} \
                         (executed={}, header={}). Local execution diverged from the committed \
                         header; refusing to advance. Reseed from a good snapshot or wipe the data \
                         dir and resync from peers.",
                        block.height,
                        hex::encode(&current_root[..8]),
                        hex::encode(&block.state_root[..8]),
                    ));
                }
            }
        }
        Ok(())
    }

    /// The parent's header `state_root` for `blocks[i]` (gate ACCEL Stage B):
    /// the previous block in the batch, or the committed tip loaded from the
    /// DB for the batch's first block. `None` when no comparable parent
    /// header exists (a height-1 block, whose parent is genesis and never
    /// persisted, or a missing tip), in which case the caller treats the
    /// cached entry as a miss rather than applying it unchecked.
    fn parent_header_root(db: &Storage, blocks: &[Block], i: usize) -> Option<[u8; 32]> {
        if i > 0 {
            return Some(blocks[i - 1].state_root);
        }
        let tip_height = blocks[i].height.checked_sub(1)?;
        if tip_height == 0 {
            return None;
        }
        match ConsensusState::load_block(db, tip_height) {
            Ok(Some(tip)) => Some(tip.state_root),
            _ => None,
        }
    }

    /// Pre-execution state-root guard for the QC-driven catch-up commit path.
    ///
    /// The vote path (`verify_block`) and the block-response sync path (C-01)
    /// both reject a block whose header `state_root` does not match the node's
    /// current SMT root read BEFORE the block is applied. A block finalized via
    /// the QC / 3-chain catch-up path is never voted on locally, so it never hit
    /// that check; this restores the same comparison on that path.
    ///
    /// A block header's `state_root` is the POST-state of that block
    /// (`post-state(N)`, gate wedge-276272), so the committed TIP (the parent of
    /// `to_commit[0]`, at `committed_height`) carries `post-state(committed)`, which
    /// a correct-but-behind node's local root equals. This checks that identity
    /// before applying `to_commit` on top; a mismatch means the local executed
    /// state has diverged from the committed chain, and returning an error halts
    /// the commit. The caller propagates it exactly as it already propagates the
    /// `apply_commits` safety error, so the node stops committing without advancing
    /// state and stays up for operator reseed. Absent root defaults to
    /// `empty_smt_root()` to match execution and genesis. Skipped at genesis
    /// (`committed_height == 0`), which has no committed tip to compare.
    fn verify_pre_commit_state_root(db: &Storage, to_commit: &[Block]) -> Result<(), String> {
        if to_commit.is_empty() {
            return Ok(());
        }
        let first = &to_commit[0];
        // Post-state convention (gate wedge-276272): the committed tip's header
        // carries post-state(committed) == KEY_SMT_ROOT. Compare the PARENT of
        // to_commit[0] (the current committed tip) against the local root, BEFORE
        // applying anything on top of it. Skip at genesis (nothing committed yet).
        let tip_height = first.height - 1;
        if tip_height == 0 {
            return Ok(());
        }
        let tip = match ConsensusState::load_block(db, tip_height) {
            Ok(Some(b)) => b,
            Ok(None) => {
                return Err(format!(
                    "CONSENSUS SAFETY HALT: catch-up commit committed-tip block {tip_height} missing"
                ))
            }
            Err(e) => return Err(format!("Failed to load committed tip {tip_height}: {e:?}")),
        };
        let current_root = match db.get(novai_state::KEY_SMT_ROOT) {
            Ok(Some(bytes)) => novai_state::decode_smt_root_v1(&bytes)
                .map_err(|e| format!("Failed to decode SMT root: {e:?}"))?,
            Ok(None) => novai_execution::empty_smt_root(), // canonical empty root (matches execution/genesis)
            Err(e) => return Err(format!("Failed to read SMT root: {e:?}")),
        };
        if tip.state_root != current_root {
            return Err(format!(
                "CONSENSUS SAFETY HALT: catch-up commit state root mismatch at height {} \
                 (local={}, expected={}). Local executed state has diverged from the committed \
                 chain; refusing to commit (this height is the detection point, the divergence \
                 origin may be earlier). Reseed from a good snapshot or wipe the data dir and \
                 resync from peers.",
                tip_height,
                hex::encode(&current_root[..8]),
                hex::encode(&tip.state_root[..8]),
            ));
        }
        Ok(())
    }

    /// Set the known X25519 noise keys for peer identity verification.
    ///
    /// In dev-keys mode, all validator seeds are known so we can precompute
    /// X25519 keys. In production mode, this is populated from genesis data.
    pub fn set_known_noise_keys(&mut self, keys: Vec<[u8; 32]>) {
        self.known_noise_keys = keys;
    }

    /// Start listening for incoming connections.
    ///
    /// When encryption is enabled, performs a Noise XX responder handshake on
    /// each accepted connection and verifies the remote peer's identity.
    pub fn start_listener(self: &Arc<Self>, bind_addr: SocketAddr) -> Result<(), String> {
        let node = Arc::clone(self);
        start_listener(bind_addr, move |mut stream| {
            let node_clone = Arc::clone(&node);

            // C-03/C-04: Check connection limits BEFORE spawning thread.
            // This prevents thread exhaustion from SYN floods and eclipse attacks.
            let ip = match stream.peer_addr() {
                Ok(addr) => addr.ip(),
                Err(_) => return,
            };

            // C-02: Reject banned peers before acquiring connection slot.
            if node_clone.ban_list.is_banned(&ip) {
                tracing::debug!(%ip, "Connection rejected: peer is banned");
                return;
            }

            let guard = match ConnectionLimiter::try_acquire(&node_clone.connection_limiter, ip) {
                Some(g) => g,
                None => {
                    tracing::warn!(%ip, "Connection rejected: limit exceeded");
                    return;
                }
            };

            thread::spawn(move || {
                // Guard released when thread exits, freeing the connection slot.
                let _conn_guard = guard;

                // Bound how long broadcast() can block writing to this peer.
                // Shared with the Noise handshake's save/restore_timeout.
                let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));

                if let Some(key) = node_clone.encryption_key {
                    // Encrypted mode: Noise XX responder handshake
                    match handshake_responder(&mut stream, &key) {
                        Ok(result) => {
                            if !node_clone.verify_peer_identity(&result.remote_static_key) {
                                node_clone.ban_list.ban(ip, "unknown peer identity");
                                return;
                            }
                            if !node_clone.peer_manager.add_peer(Box::new(result.writer)) {
                                tracing::warn!("Peer rejected: connection limit reached");
                                return;
                            }
                            node_clone.handle_peer_connection(result.reader, ip);
                        }
                        Err(e) => {
                            tracing::warn!(?e, "Noise handshake failed (responder)");
                            node_clone.ban_list.ban(ip, "handshake failure");
                        }
                    }
                } else {
                    // Plaintext mode
                    let write_stream = match stream.try_clone() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("Failed to clone accepted stream: {e}");
                            return;
                        }
                    };
                    if !node_clone.peer_manager.add_peer(Box::new(write_stream)) {
                        tracing::warn!("Peer rejected: connection limit reached");
                        return;
                    }
                    node_clone.handle_peer_connection(stream, ip);
                }
            });
        })
        .map_err(|e| format!("Failed to start listener: {e:?}"))
    }

    /// Connect to a peer and start reader thread.
    ///
    /// When encryption is enabled, performs a Noise XX initiator handshake
    /// and verifies the remote peer's identity.
    pub fn connect_to_peer(self: &Arc<Self>, addr: SocketAddr) -> Result<(), String> {
        let mut stream =
            connect_to_peer(addr).map_err(|e| format!("Failed to connect to peer: {e:?}"))?;

        if let Some(key) = self.encryption_key {
            // Encrypted mode: Noise XX initiator handshake
            let result = handshake_initiator(&mut stream, &key)
                .map_err(|e| format!("Noise handshake failed (initiator): {e:?}"))?;

            if !self.verify_peer_identity(&result.remote_static_key) {
                return Err("Rejected: remote peer not in validator set".into());
            }

            if !self.peer_manager.add_peer(Box::new(result.writer)) {
                return Err("Peer rejected: connection limit reached".into());
            }

            let node = Arc::clone(self);
            let peer_ip = addr.ip();
            thread::spawn(move || {
                node.handle_peer_connection(result.reader, peer_ip);
            });
        } else {
            // Plaintext mode
            let write_stream = stream
                .try_clone()
                .map_err(|e| format!("Failed to clone stream: {e:?}"))?;
            if !self.peer_manager.add_peer(Box::new(write_stream)) {
                return Err("Peer rejected: connection limit reached".into());
            }

            let node = Arc::clone(self);
            let peer_ip = addr.ip();
            thread::spawn(move || {
                node.handle_peer_connection(stream, peer_ip);
            });
        }

        Ok(())
    }

    /// Verify a remote peer's Noise static key against known validator keys.
    ///
    /// Returns `true` if the peer is authorized, `false` otherwise.
    fn verify_peer_identity(&self, remote_static: &[u8; 32]) -> bool {
        if self.known_noise_keys.is_empty() {
            // H-02: Warn loudly when accepting peers without verification.
            // In production, known_noise_keys should be distributed via genesis.
            tracing::warn!(
                peer_key = %hex::encode(&remote_static[..16]),
                "Peer identity verification DISABLED (known_noise_keys empty). \
                 Any peer can connect — eclipse attack risk. \
                 Configure validator noise pubkeys for production."
            );
            return true;
        }

        if is_known_validator(remote_static, &self.known_noise_keys) {
            true
        } else {
            tracing::warn!(
                noise_key = %hex::encode(remote_static),
                "Rejected unknown peer"
            );
            false
        }
    }

    /// Broadcast a message to all peers.
    pub fn broadcast(&self, msg: NetworkMessage) -> Result<(), String> {
        self.peer_manager
            .broadcast(&msg)
            .map_err(|e| format!("Broadcast failed: {e:?}"))
    }

    /// The runtime wire send cap (F3). ONE stored value, held by the
    /// PeerManager whose encoder enforces it: the proposer guard and the
    /// responder budget read the cap through this method, so guard and
    /// enforcement cannot diverge and a Phase B restart flips both
    /// atomically.
    #[must_use]
    pub fn wire_send_cap(&self) -> u32 {
        self.peer_manager.send_cap()
    }

    /// Set the runtime wire send cap from --wire-send-cap-bytes (startup
    /// only: config is parsed once and SIGHUP is ignored, so a change is
    /// a restart).
    ///
    /// # Errors
    /// Rejects a cap below the 2 MiB default or above the 16 MiB receive
    /// cap (see [`validate_wire_send_cap`]).
    pub fn set_wire_send_cap(&self, cap: u32) -> Result<(), String> {
        validate_wire_send_cap(cap)?;
        self.peer_manager.set_send_cap(cap);
        Ok(())
    }

    /// Prune the QC broadcast dedup cache, removing entries below the retention window.
    ///
    /// Without pruning, this `HashSet` grows by ~100 bytes per block forever,
    /// causing unbounded memory growth (~50MB per 500k blocks per node).
    fn prune_qc_broadcast_cache(&self, committed_height: u64) {
        if committed_height <= novai_consensus::CACHE_RETAIN_DEPTH {
            return;
        }
        let prune_below = committed_height - novai_consensus::CACHE_RETAIN_DEPTH;
        let mut cache = self.qc_broadcasted.lock_or_recover();
        let before = cache.len();
        cache.retain(|&(height, _, _)| height >= prune_below);
        let pruned = before - cache.len();
        if pruned > 0 {
            // Reclaim backing array capacity after pruning. Without
            // shrink_to_fit, the HashSet keeps high-watermark capacity
            // across millions of insert/retain cycles.
            cache.shrink_to_fit();
            tracing::debug!(pruned, remaining = cache.len(), "Pruned QC broadcast cache");
        }
    }

    /// Check if timeout should be triggered and create it.
    ///
    /// Returns Some(Timeout) if timeout duration elapsed and not already timed out.
    pub fn check_timeout(&self) -> Option<Timeout> {
        // FAST PATH: Read round_start_time WITHOUT the state lock to see if
        // the minimum possible timeout (base_timeout_ms for round 0) has elapsed.
        // This avoids acquiring the expensive state lock on ~99% of loop iterations
        // (every 5ms when base_timeout_ms is typically 1000+ms).
        //
        // This is safe because the worst case of a stale read is that we acquire
        // the state lock one extra time — we re-read round_start_time under the
        // state lock below to prevent the TOCTOU race.
        {
            let start_time = *self.round_start_time.lock_or_recover();
            if start_time.elapsed() < std::time::Duration::from_millis(self.base_timeout_ms) {
                return None; // Definitely not timed out yet (even round 0 hasn't elapsed)
            }
        }

        // SLOW PATH: Acquire state lock to get the actual round and recheck.
        // Lock order: state → round_start_time (matches handle_vote, handle_qc,
        // handle_proposal, try_propose_block — all reset round_start_time while
        // holding state lock).
        //
        // The previous ordering (round_start_time → state) caused a TOCTOU race:
        //   1. check_timeout reads old round_start_time (T0)
        //   2. handle_qc acquires state, advances view, resets round_start_time to NOW
        //   3. check_timeout acquires state, sees round=0 but has stale T0
        //   4. T0.elapsed() > timeout → spurious timeout fires at round 0
        // This race caused the chain stall after hours of running.
        let state = self.state.lock_or_recover();
        let start_time = *self.round_start_time.lock_or_recover();

        let timeout_ms =
            novai_consensus::timeout_for_round_with_base(state.round, self.base_timeout_ms);
        let timeout_duration = std::time::Duration::from_millis(timeout_ms);

        if start_time.elapsed() < timeout_duration {
            return None; // Not yet timed out
        }

        // Allow re-broadcast after the full timeout duration to handle lost messages.
        // This replaces the old boolean flag that permanently blocked re-timeout.
        let last_timeout = *self.last_timeout_time.lock_or_recover();
        if let Some(last) = last_timeout {
            let rebroadcast_interval = std::time::Duration::from_millis(timeout_ms);
            if last.elapsed() < rebroadcast_interval {
                return None; // Too soon to re-broadcast
            }
        }

        // Create timeout
        match state.create_timeout(&self.signing_key) {
            Ok(timeout) => {
                *self.last_timeout_time.lock_or_recover() = Some(Instant::now());
                tracing::debug!(
                    round = state.round,
                    elapsed = ?start_time.elapsed(),
                    highest_qc = ?state.highest_qc.as_ref().map(|q| q.height),
                    "TIMEOUT_DIAG: timeout triggered"
                );
                Some(timeout)
            }
            Err(e) => {
                // Fix D (gate-equivocation-535004): record the attempt time on
                // the failure path too. The success branch above sets
                // last_timeout_time, but this branch previously did not, so the
                // rebroadcast throttle never engaged on repeated create_timeout
                // failures and the loop spun at roughly 195/sec, producing 64MB
                // in 30 minutes and overwriting the incident onset logs.
                // Setting it here reuses the existing throttle with no new
                // field: the next attempt waits one timeout interval, which
                // preserves forensic history.
                tracing::error!(?e, "Failed to create timeout");
                *self.last_timeout_time.lock_or_recover() = Some(Instant::now());
                None
            }
        }
    }

    /// Handle incoming timeout message.
    ///
    /// Returns Ok(true) if round was advanced, Ok(false) otherwise.
    pub fn handle_timeout(&self, timeout: Timeout) -> Result<bool, String> {
        tracing::debug!(voter = ?&timeout.voter[..4], "Received timeout");

        let mut state = self.state.lock_or_recover();

        // Record round before add_timeout to detect round sync fast-forward
        let round_before = state.round;

        // Add timeout to state (use cached pubkeys_vec to avoid allocation).
        // Duplicate timeouts (same voter, same round) are expected during
        // re-broadcast and treated as no-ops, not errors — same pattern as
        // handle_vote's duplicate handling.
        match state.add_timeout(timeout, &self.validator_pubkeys_vec) {
            Ok(()) => {}
            Err(novai_consensus::ConsensusError::InvalidVote(ref msg))
                if msg.contains("Duplicate timeout")
                    || msg.contains("height mismatch")
                    || msg.contains("at capacity") =>
            {
                return Ok(false);
            }
            Err(e) => return Err(format!("Add timeout failed: {e:?}")),
        }

        // If add_timeout performed round sync (fast-forwarded our round),
        // reset timeout timers so we get a fresh timeout window at the new round
        if state.round > round_before {
            *self.round_start_time.lock_or_recover() = Instant::now();
            *self.last_timeout_time.lock_or_recover() = None;
        }

        // Try to advance round
        let advanced = state.try_advance_round(&self.validator_set);

        if advanced {
            // Reset round timer
            *self.round_start_time.lock_or_recover() = Instant::now();
            *self.last_timeout_time.lock_or_recover() = None;

            tracing::info!(
                round = state.round,
                height = state.height + 1,
                "ROUND ADVANCED"
            );
        }

        Ok(advanced)
    }

    /// Request blocks from a peer for catch-up.
    ///
    /// Returns `Ok(())` if request was sent, or error if already pending or no peers available.
    pub fn request_blocks_from_peer(
        &self,
        start_height: u64,
        end_height: u64,
    ) -> Result<(), String> {
        // Check if there's already a pending request
        let mut pending = self.pending_sync_request.lock_or_recover();
        if pending.is_some() {
            return Err("Sync request already pending".to_string());
        }

        // Select a peer (simple: just take the first validator that's not us)
        let peer = self
            .validator_set
            .iter()
            .find(|&&addr| addr != self.our_address)
            .copied()
            .ok_or_else(|| "No peers available for sync".to_string())?;

        // Create and send request
        let request = novai_consensus_types::BlockRequest {
            requester: self.our_address,
            start_height,
            end_height,
        };

        tracing::debug!(
            start_height,
            end_height,
            peer = ?&peer[..4],
            "Requesting blocks"
        );

        // Store pending request
        *pending = Some(PendingSyncRequest {
            peer,
            start_height,
            end_height,
            request_time: Instant::now(),
        });

        drop(pending);

        // Broadcast request
        self.broadcast(NetworkMessage::BlockRequest(request))?;

        Ok(())
    }

    /// Build the response for a block request: blocks from DB with
    /// in-memory cache fallback, each paired positionally with its
    /// certifying QC (DB row via load_qc_at_height, qc_cache fallback
    /// for live-tail QCs not yet written as rows, None when neither).
    ///
    /// F2/F3: the served prefix is additionally byte-bounded, measured
    /// per encoded (block, QC) pair: the first three pairs (the frontier
    /// floor) are bounded by the runtime send frame, later pairs by the
    /// soft budget (half the runtime send cap); see the comment in the
    /// assembly loop.
    ///
    /// Extracted from handle_block_request so tests can assert on the
    /// response without capturing a network broadcast.
    pub fn build_block_response(
        &self,
        request: &novai_consensus_types::BlockRequest,
    ) -> novai_consensus_types::BlockResponse {
        // Clamp range to SYNC_CHUNK_SIZE to prevent malicious large requests
        let clamped_end = request
            .end_height
            .min(request.start_height.saturating_add(SYNC_CHUNK_SIZE - 1));

        let state = self.state.lock_or_recover();
        let db = self.db.lock_or_recover();

        // Load individual blocks from DB, falling back to in-memory cache.
        // Each served block gets exactly one qcs entry, so the vectors
        // stay positionally paired. A missing QC is represented
        // faithfully as None, never skipped silently.
        //
        // F2: the prefix is byte-bounded, measured by encoding each
        // candidate (block, QC) PAIR with the same codec the wire path
        // uses (a hand-maintained size formula would drift). The block is
        // held, not pushed, until its QC bytes are in the measurement: QC
        // trailers can dominate the pair.
        //
        // F3 frontier floor (diagnosis 12.3): the first THREE pairs are
        // exempt from the soft budget and bounded only by the send frame
        // (payload + 2 wire bytes within the runtime send cap), because
        // the requester restarts at committed+1 and the 3-chain rule
        // needs committed+1..+3 in ONE response or the identical prefix
        // re-serves forever (fixplan :122). Pair ONE stays unconditional,
        // exactly F2's floor: a first pair beyond even the send frame is
        // served and strands at encode until the Phase B cap flip serves
        // it. Pairs beyond the floor are shaped by the soft budget, half
        // the runtime send cap, which evaluates to the deployed F2 value
        // at the Phase A default.
        let send_cap = self.wire_send_cap() as usize;
        let soft_budget = response_byte_budget(self.wire_send_cap());
        let mut blocks = Vec::new();
        let mut qcs: Vec<Option<QC>> = Vec::new();
        let mut response_bytes: usize = RESPONSE_HEADER_BYTES;
        for height in request.start_height..=clamped_end {
            let block = match ConsensusState::load_block(&*db, height) {
                Ok(Some(block)) => block,
                _ => {
                    // Fallback: check in-memory block cache
                    if let Some(block) = state.block_cache.get(&height) {
                        Block::clone(block)
                    } else {
                        break; // Stop at first missing block
                    }
                }
            };
            let qc = match ConsensusState::load_qc_at_height(&*db, height) {
                Ok(Some(qc)) => Some(qc),
                Ok(None) => state.qc_cache.get(&height).cloned(),
                Err(e) => {
                    tracing::warn!(
                        height,
                        error = ?e,
                        "Block response: QC row unreadable, sending None"
                    );
                    None
                }
            };
            let block_bytes = match novai_consensus_types::codec::encode_block_v1(&block) {
                Ok(bytes) => bytes.len(),
                Err(e) => {
                    // Defensive: a stored block that cannot re-encode would
                    // fail the whole response at the wire; end the prefix
                    // before it instead.
                    tracing::warn!(
                        height,
                        error = ?e,
                        "Block response: block unencodable, ending prefix"
                    );
                    break;
                }
            };
            let (qc, qc_bytes) = match qc {
                Some(qc) => match novai_consensus_types::codec::encode_qc_v1(&qc) {
                    Ok(bytes) => (Some(qc), bytes.len()),
                    Err(e) => {
                        // Defensive: an unencodable QC degrades to the same
                        // faithful None as an unreadable QC row above.
                        tracing::warn!(
                            height,
                            error = ?e,
                            "Block response: QC unencodable, sending None"
                        );
                        (None, 0)
                    }
                },
                None => (None, 0),
            };
            // Encoded pair cost in the response payload: block bytes plus
            // the has_qc flag byte plus QC bytes (encode_block_response_v2
            // layout). Stop BEFORE the pair that would exceed the limit:
            // the send frame for floor pairs two and three, the soft
            // budget after the floor. The first pair is unconditional.
            let pair_bytes = block_bytes + 1 + qc_bytes;
            if !blocks.is_empty() {
                let limit = if blocks.len() < 3 {
                    send_cap - 2
                } else {
                    soft_budget
                };
                if response_bytes + pair_bytes > limit {
                    break;
                }
            }
            response_bytes += pair_bytes;
            blocks.push(block);
            qcs.push(qc);
        }

        drop(db);
        drop(state);

        novai_consensus_types::BlockResponse {
            responder: self.our_address,
            request_start: request.start_height,
            request_end: request.end_height,
            blocks,
            qcs,
        }
    }

    /// Handle incoming block request from a peer.
    ///
    /// Serves blocks from DB first, falling back to in-memory cache for blocks
    /// that have been proposed/voted on but not yet committed.
    pub fn handle_block_request(
        &self,
        request: novai_consensus_types::BlockRequest,
    ) -> Result<(), String> {
        tracing::debug!(
            start_height = request.start_height,
            end_height = request.end_height,
            requester = ?&request.requester[..4],
            "Received block request"
        );

        let response = self.build_block_response(&request);

        tracing::debug!(
            count = response.blocks.len(),
            qc_count = response.qcs.iter().filter(|q| q.is_some()).count(),
            start_height = request.start_height,
            end_height = request.end_height,
            requester = ?&request.requester[..4],
            "Sending blocks"
        );

        // Send response with whatever blocks we have
        self.broadcast(NetworkMessage::BlockResponse(response))?;

        Ok(())
    }

    /// Handle incoming block response from a peer.
    ///
    /// Accepts responses from ANY peer (we broadcast requests to all).
    /// Caches received blocks in memory AND stores to DB, then retries
    /// the commit rule with highest_qc.
    pub fn handle_block_response(
        &self,
        response: novai_consensus_types::BlockResponse,
    ) -> Result<(), String> {
        tracing::debug!(
            count = response.blocks.len(),
            responder = ?&response.responder[..4],
            "Received blocks"
        );

        // Accept block responses regardless of pending_sync_request state.
        // Previously, responses arriving after the 5-second pending timeout
        // were silently discarded, causing rejoining validators to never sync.
        // Non-empty responses are always processed (idempotent: already-committed
        // blocks are filtered out below).
        //
        // F1: settle the pending slot only for a response that answers OUR
        // in-flight request, correlated by the echoed request_start. A
        // matching EMPTY response is a definitive "peer lacks the range"
        // answer, not silence: release the slot now instead of burning the
        // 5s timeout, and record a strike so the retry gate backs off.
        // Responses to another node's broadcast request (or arriving after
        // our timeout already cleared the slot) leave the slot alone. The
        // broadcast fan-in also means only the FIRST matching empty answer
        // per cycle strikes; later ones find the slot already settled.
        let matched_pending = {
            let mut pending = self.pending_sync_request.lock_or_recover();
            match pending.as_ref() {
                Some(p) if p.start_height == response.request_start => {
                    *pending = None;
                    true
                }
                _ => false,
            }
        };

        if response.blocks.is_empty() {
            if matched_pending {
                self.record_sync_strike("matching empty block response");
            } else {
                tracing::debug!(
                    responder = ?&response.responder[..4],
                    "Peer sent empty block response"
                );
            }
            return Ok(());
        }

        // Lock order: state → db
        let mut state = self.state.lock_or_recover();
        let mut db = self.db.lock_or_recover();
        let committed_height = state.committed_height;

        // Filter out blocks we've already committed (stale sync response).
        // This happens when committed_height advances between request and response.
        //
        // Fix A2 (gate-equivocation-535004): pair each fresh block with its
        // positionally-paired certifying QC from the Stage 1 qcs field, and
        // filter already-committed blocks while keeping each block aligned
        // with its QC. response.qcs.get(i) tolerates a malicious peer that
        // sends fewer qcs than blocks: an unpaired block gets None and so is
        // treated as uncertified when the cursor advance certifies below.
        let pairs: Vec<(Block, Option<QC>)> = response
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.height > committed_height)
            .map(|(i, b)| (b.clone(), response.qcs.get(i).cloned().flatten()))
            .collect();
        let blocks: Vec<Block> = pairs.iter().map(|(b, _)| b.clone()).collect();

        if blocks.is_empty() {
            if let (Some(first), Some(last)) = (response.blocks.first(), response.blocks.last()) {
                tracing::debug!(
                    committed_height,
                    response_start = first.height,
                    response_end = last.height,
                    "Stale sync response — all blocks already committed"
                );
            } else {
                tracing::debug!(committed_height, "Empty sync response");
            }
            drop(state);
            drop(db);
            self.try_request_missing_blocks();
            return Ok(());
        }

        // First fresh block must connect to committed chain (height contiguity)
        if blocks[0].height != committed_height + 1 {
            return Err(format!(
                "Block chain gap: committed_height={}, first block height={}",
                committed_height, blocks[0].height
            ));
        }

        // Verify internal chain consistency (each block connects to the previous
        // one via parent_hash). We use the first block's own parent_hash as the
        // anchor — NOT our local block at committed_height — because the local
        // block may have been overwritten by a stale proposal from a different
        // round (handle_proposal stores all received blocks to DB by height).
        // The sync blocks come from a peer's committed chain and are already
        // verified by the BFT network.
        let anchor_parent = blocks[0].parent_hash;
        if let Err(e) = ConsensusState::verify_block_chain(&blocks, anchor_parent) {
            return Err(format!("Block chain verification failed: {e:?}"));
        }

        // C-01: Reject sync response if peer's chain doesn't connect to our
        // committed block. NEVER overwrite committed blocks from peer responses.
        if committed_height > 0 {
            if let Ok(Some(local_block)) = ConsensusState::load_block(&*db, committed_height) {
                let local_hash = novai_consensus_types::block_hash(&local_block);
                if local_hash != anchor_parent {
                    return Err(format!(
                        "Sync rejected: peer's chain doesn't connect to our committed \
                         block at height {} (local={:?}, peer_parent={:?})",
                        committed_height,
                        &local_hash[..8],
                        &anchor_parent[..8],
                    ));
                }
            }
        }

        // C-01 (gate wedge-276272): local self-check. Under the post-state
        // convention the committed tip's header carries post-state(committed) ==
        // KEY_SMT_ROOT, so verify local consistency before applying synced blocks on
        // top. The synced blocks themselves are validated by the anchor (parent
        // linkage) above and by the post-execution check at commit; this replaces
        // the old first-synced-block comparison, which is meaningless once the
        // header carries a different height's post-state. Skip at genesis.
        if committed_height > 0 {
            let current_root = if let Ok(Some(bytes)) = db.get(novai_state::KEY_SMT_ROOT) {
                novai_state::decode_smt_root_v1(&bytes)
                    .map_err(|e| format!("Failed to decode SMT root: {e:?}"))?
            } else {
                novai_execution::empty_smt_root() // canonical empty SMT root, matches execution and genesis
            };
            let tip_root = match ConsensusState::load_block(&*db, committed_height) {
                Ok(Some(b)) => b.state_root,
                Ok(None) => {
                    return Err(format!(
                        "Sync rejected: local committed-tip block {committed_height} missing"
                    ))
                }
                Err(e) => {
                    return Err(format!(
                        "Sync rejected: load committed tip {committed_height}: {e:?}"
                    ))
                }
            };
            if tip_root != current_root {
                return Err(format!(
                    "Sync rejected: local committed-tip header {} does not match local root {} \
                     at height {} (local divergence).",
                    hex::encode(&tip_root[..8]),
                    hex::encode(&current_root[..8]),
                    committed_height,
                ));
            }

            tracing::debug!(
                count = blocks.len(),
                start = blocks[0].height,
                committed_height,
                "Synced blocks passed local committed-tip self-check"
            );
        }

        // Bug 1 latent bug A (docs/gate3-bug1-diagnosis.md Risk 2): the
        // sync-chunk block storage AND the KEY_COMMITTED_HEIGHT cursor
        // advance below were previously two separate non-atomic writes (a
        // per-block db.put loop here plus a standalone db.put on the
        // cursor). A crash in that window left the validator with the
        // executor's state advanced beyond the recorded cursor; recovery
        // would compute a different committed_height than peers and the
        // chain would fork at the next state_root comparison. Both writes
        // are now accumulated into a single Vec<WriteOp> and applied
        // atomically at the end of this method (see the apply_batch call
        // below the conditional cursor block).
        let mut sync_ops: Vec<WriteOp> = Vec::with_capacity(blocks.len() + 1);
        for block in &blocks {
            let key = novai_state::block_key(block.height);
            let value = novai_consensus_types::codec::encode_block_v1(block)
                .map_err(|e| format!("Failed to encode block: {e:?}"))?;
            sync_ops.push(WriteOp::Put(key, value));

            // Cache in memory so commit rule can find them via block_by_hash.
            // In-memory only; not part of the atomic sync batch.
            state
                .cache_block(block.clone())
                .map_err(|e| format!("Cache block failed: {e:?}"))?;
        }

        tracing::info!(
            count = blocks.len(),
            start = blocks.first().unwrap().height,
            end = blocks.last().unwrap().height,
            "Cached synced blocks"
        );

        // Verify each synced block's certifying QC and seed qc_cache so the
        // 3-chain commit rule below can find those QCs and persist_commit_atomic
        // writes a dense qc_key row for each committed height (preserving
        // peer-serving). This pass does NOT advance committed_height. Because
        // committed_height now advances only through the 3-chain rule, a block
        // that earned a QC in one round but lost a same-height view change (no
        // canonical descendant, so no QC two heights above) can never be
        // finalized via sync: the backward walk from a canonical QC never
        // passes through an abandoned block.
        let n = self.validator_set.len();
        let f = (n - 1) / 3;
        let quorum = 2 * f + 1;
        let mut top_qc: Option<QC> = None;
        for (block, qc) in &pairs {
            let Some(qc) = qc else {
                continue;
            };
            let block_hash = novai_consensus_types::block_hash(block);
            if qc.height != block.height || qc.block_hash != block_hash {
                continue;
            }
            if let Err(e) =
                ConsensusState::verify_qc_well_formed(qc, &self.validator_pubkeys_vec, quorum)
            {
                tracing::warn!(
                    height = block.height,
                    error = ?e,
                    "Sync: synced block carries a malformed certifying QC, ignoring it"
                );
                continue;
            }
            // qc_cache is pruned behind committed_height by prune_old_blocks, so
            // seeding it here adds no unbounded growth.
            state.qc_cache.insert(qc.height, qc.clone());
            let is_higher = match &top_qc {
                None => true,
                Some(c) => qc.height > c.height,
            };
            if is_higher {
                top_qc = Some(qc.clone());
            }
        }

        // Try commit rule with current highest_qc (may succeed if we now
        // have enough blocks for the 3-chain rule).
        if let Some(hqc) = state.highest_qc.clone() {
            match state.cache_qc_and_check_commit(hqc.clone(), &*db) {
                Ok(to_commit) if !to_commit.is_empty() => {
                    let new_committed_height = to_commit.last().unwrap().height;
                    // Take any vote-time executions before apply_commits
                    // evicts them (gate ACCEL Stage B; usually all misses on
                    // the sync path, which then re-executes per block).
                    let cached = state.take_pending_execs(&to_commit);
                    state
                        .persist_commit_atomic(
                            &mut *db,
                            &to_commit,
                            &hqc,
                            new_committed_height,
                            None,
                        )
                        .map_err(|e| format!("Sync commit persist failed: {e:?}"))?;
                    state
                        .apply_commits(&to_commit)
                        .map_err(|e| format!("CONSENSUS SAFETY VIOLATION during sync: {e:?}"))?;
                    self.execute_committed_blocks(&mut db, &to_commit, cached)?;
                    tracing::info!(
                        committed_height = new_committed_height,
                        count = to_commit.len(),
                        "Synced and committed"
                    );
                }
                Ok(_) | Err(_) => {
                    // Commit chain incomplete, not enough blocks to reach
                    // highest_qc yet. This is expected during chunked sync.
                }
            }
        }

        // Second commit attempt: drive the SAME 3-chain rule with the highest
        // verified QC carried in this response. Its certified block was just
        // synced, so the backward parent-linkage walk in cache_qc_and_check_commit
        // succeeds during chunked catch-up even when highest_qc is far ahead of
        // this chunk (the highest_qc attempt above errors with the top block
        // missing, which is expected). This mirrors the live handle_qc order:
        // commit rule, persist, apply, execute. cache_qc_and_check_commit is
        // idempotent past the cursor (it returns nothing when commit_target is
        // at or below committed_height), so any height already committed by the
        // attempt above is not committed again, and the rule never finalizes a
        // block lacking a QC two heights above it.
        if let Some(tqc) = top_qc {
            match state.cache_qc_and_check_commit(tqc.clone(), &*db) {
                Ok(to_commit) if !to_commit.is_empty() => {
                    let new_committed_height = to_commit.last().unwrap().height;
                    // Take any vote-time executions before apply_commits
                    // evicts them (gate ACCEL Stage B; usually all misses on
                    // the sync path, which then re-executes per block).
                    let cached = state.take_pending_execs(&to_commit);
                    state
                        .persist_commit_atomic(
                            &mut *db,
                            &to_commit,
                            &tqc,
                            new_committed_height,
                            None,
                        )
                        .map_err(|e| format!("Sync commit persist failed: {e:?}"))?;
                    state
                        .apply_commits(&to_commit)
                        .map_err(|e| format!("CONSENSUS SAFETY VIOLATION during sync: {e:?}"))?;
                    self.execute_committed_blocks(&mut db, &to_commit, cached)?;
                    tracing::info!(
                        committed_height = new_committed_height,
                        count = to_commit.len(),
                        "Sync: committed via 3-chain rule"
                    );
                }
                Ok(_) | Err(_) => {
                    // Incomplete 3-chain: the tip blocks have no descendant QC
                    // yet. Defer; they finalize on a later chunk as the chain
                    // advances. Deferring is the correct safety margin and never
                    // finalizes an unconfirmed block.
                }
            }
        }

        // Atomic write of synced block storage. committed_height, highest_qc,
        // locked_qc, voted_view, and the dense per-height QC rows for committed
        // blocks are written separately and atomically by persist_commit_atomic
        // above. This batch persists the synced blocks themselves (both the
        // committed prefix and the deferred tip) so the node can serve them to
        // peers and the next chunk's backward walk can find them.
        db.apply_batch(&sync_ops)
            .map_err(|e| format!("Failed to persist sync chunk atomically: {e:?}"))?;

        let final_committed = state.committed_height;

        // Drop locks before requesting next chunk
        drop(state);
        drop(db);

        // Prune QC broadcast cache to bound memory growth
        self.prune_qc_broadcast_cache(final_committed);

        // If still behind, request next chunk (chunked sync)
        self.try_request_missing_blocks();

        Ok(())
    }

    /// Check if we're the leader for current view.
    /// Uses view_height = max(committed_height, highest_qc.height) for consistency with propose_block.
    pub fn are_we_leader(&self) -> bool {
        let state = self.state.lock_or_recover();
        let view_height = match &state.highest_qc {
            Some(qc) => std::cmp::max(state.height, qc.height),
            None => state.height,
        };
        match ConsensusState::compute_leader_for_view(view_height, state.round, &self.validator_set)
        {
            Ok(leader) => leader == self.our_address,
            Err(_) => false,
        }
    }

    /// Recover txs from the last abandoned proposal.
    ///
    /// When a round changes (timeout or QC catch-up) before our proposed block
    /// is committed, the drained txs are lost. This method returns them so the
    /// caller can reinsert valid ones into the mempool.
    pub fn recover_abandoned_txs(&self) -> Vec<novai_types::TxV1> {
        let mut state = self.state.lock_or_recover();
        state.take_abandoned_txs()
    }

    /// F3 proposer guard Layer 1: the tx-byte budget for the next
    /// proposal, derived from the runtime send cap minus the MEASURED
    /// envelope (fixed wrapper and header bytes plus the justify QC
    /// encoded by the real codec, never estimated), clamped to
    /// MAX_BLOCK_SIZE. The justify QC is known before tx selection
    /// (genesis QC at height 1, highest_qc after), so the budget
    /// guarantees by construction that the assembled SignedProposal
    /// encodes under the cap. At the 16 MiB Phase B cap MAX_BLOCK_SIZE
    /// binds first and the guard is non-binding by arithmetic.
    ///
    /// # Errors
    /// Returns an error if the justify QC cannot be encoded.
    fn proposal_tx_budget(&self, state: &ConsensusState) -> Result<usize, String> {
        let intended_height = match &state.highest_qc {
            Some(qc) => std::cmp::max(state.height, qc.height) + 1,
            None => state.height + 1,
        };
        let genesis_qc = QC {
            height: 0,
            round: 0,
            block_hash: [0u8; 32],
            votes: vec![],
        };
        // A missing highest_qc above height 1 fails the proposal flow
        // with its own error after assembly; the genesis size keeps the
        // budget computation total in the meantime.
        let justify: &QC = if intended_height == 1 {
            &genesis_qc
        } else {
            state.highest_qc.as_ref().unwrap_or(&genesis_qc)
        };
        let justify_bytes = novai_consensus_types::codec::encode_qc_v1(justify)
            .map_err(|e| format!("Encode justify QC for proposal budget failed: {e:?}"))?
            .len();
        Ok((self.wire_send_cap() as usize)
            .saturating_sub(PROPOSAL_ENVELOPE_FIXED_BYTES + justify_bytes)
            .min(novai_types::MAX_BLOCK_SIZE))
    }

    /// F3 proposer guard Layer 2, the loud backstop: measure an assembled
    /// SignedProposal against the runtime send cap BEFORE broadcast.
    /// Layer 1 makes an over-cap envelope unreachable through tx
    /// selection; if one is ever assembled anyway, this refuses it with
    /// an ERROR naming the invariant instead of the silent
    /// AlreadyProposed round stall. Returns the checked wire length
    /// (payload + 2, the exact value the encoder compares against the
    /// cap).
    ///
    /// # Errors
    /// Returns an error when the envelope cannot encode or exceeds the
    /// runtime send cap.
    pub fn proposal_wire_len(&self, signed_proposal: &SignedProposal) -> Result<usize, String> {
        let payload = novai_consensus_types::codec::encode_signed_proposal_v1(signed_proposal)
            .map_err(|e| format!("Encode proposal for envelope check failed: {e:?}"))?;
        let wire_len = payload.len() + 2;
        let cap = self.wire_send_cap() as usize;
        if wire_len > cap {
            tracing::error!(
                wire_len,
                cap,
                height = signed_proposal.proposal.block.height,
                "F3 proposer guard invariant violated: assembled proposal \
                 exceeds the wire send cap; refusing to broadcast"
            );
            return Err(format!(
                "proposal wire length {wire_len} exceeds the send cap {cap}; not broadcasting"
            ));
        }
        Ok(wire_len)
    }

    /// Propose a block (leader only).
    pub fn propose_block(
        &self,
        mempool: &mut mempool::TxMempool,
        nonce_provider: &impl mempool::NonceProvider,
    ) -> Result<(), String> {
        let mut state = self.state.lock_or_recover();
        let db = self.db.lock_or_recover();

        let tx_budget = self.proposal_tx_budget(&state)?;
        let block = state
            .propose_block_with_budget(
                mempool,
                nonce_provider,
                &*db,
                &self.validator_set,
                tx_budget,
            )
            .map_err(|e| format!("Propose block failed: {e:?}"))?;

        // CRITICAL: Cache our own proposed block so we can form QC when votes arrive
        state
            .cache_block(block.clone())
            .map_err(|e| format!("Cache block failed: {e:?}"))?;

        // justify_qc should certify the parent block (height - 1)
        // For height 1: use GenesisQC (height=0)
        // For height > 1: use highest_qc (which should be for height - 1)
        let justify_qc = if block.height == 1 {
            QC {
                height: 0,
                round: 0,
                block_hash: [0u8; 32],
                votes: vec![],
            }
        } else {
            state.highest_qc.clone().ok_or_else(|| {
                format!("Cannot propose height {} without highest_qc", block.height)
            })?
        };

        let proposal = novai_consensus_types::Proposal {
            block: block.clone(),
            justify_qc,
        };

        let unsigned_bytes = novai_consensus_types::codec::encode_proposal_v1_unsigned(&proposal)
            .map_err(|e| format!("Encode proposal failed: {e:?}"))?;

        let signature = sign_bytes(&self.signing_key, &unsigned_bytes);

        let signed_proposal = SignedProposal {
            proposer: self.our_address,
            proposal,
            signature,
        };

        drop(state);
        drop(db);

        tracing::debug!(
            height = block.height,
            round = block.round,
            "Proposing block"
        );

        // F3 guard Layer 2: refuse an over-cap envelope loudly instead of
        // letting the broadcast encode fail after the proposal state is
        // already marked.
        self.proposal_wire_len(&signed_proposal)?;

        self.broadcast(NetworkMessage::SignedProposal(signed_proposal))
    }

    /// Atomically check leadership and propose a block.
    ///
    /// This method avoids the TOCTOU race between checking leadership and proposing
    /// by performing both operations within a single lock acquisition.
    ///
    /// # Returns
    /// - `Ok(true)` if we successfully proposed a block
    /// - `Ok(false)` if we're not the leader or already proposed (expected, not an error)
    /// - `Err(...)` for actual errors (signing, broadcasting, etc.)
    pub fn try_propose_block(
        &self,
        mempool: &mut mempool::TxMempool,
        nonce_provider: &impl mempool::NonceProvider,
    ) -> Result<bool, String> {
        let mut state = self.state.lock_or_recover();
        let mut db = self.db.lock_or_recover();

        // gate 9: refuse to propose/self-vote at a view this node already durably
        // voted at (a restart replay of a view voted before the crash). Compute
        // the view propose_block would use; if already voted there, skip without
        // draining the mempool. A legitimate first vote at a new view passes. This
        // keeps the durable vote high-water mark global across the leader and
        // follower self-vote sites.
        let intended_height = match &state.highest_qc {
            Some(qc) => std::cmp::max(state.height, qc.height) + 1,
            None => state.height + 1,
        };

        // Commit-window rule (WEDGE-20260718): refuse to propose more than
        // COMMIT_WINDOW heights above committed. While commits stall this
        // parks the proposer (rounds keep churning via timeouts, nothing new
        // is certified), so the frontier and the durable vote marks stay a
        // bounded, restart-recoverable distance above the floor. Checked
        // before the gate 9 skip below so a wedge-shaped restart logs the
        // true story (parked frontier), not a vote-replay symptom. Warned
        // once per view: the propose tick fires every 5 ms but the parked
        // view only changes on a round advance.
        if !state.within_commit_window(intended_height) {
            let view = (intended_height, state.round);
            let mut warned = self.commit_window_warned_view.lock_or_recover();
            if *warned != Some(view) {
                *warned = Some(view);
                tracing::warn!(
                    height = intended_height,
                    round = state.round,
                    committed_height = state.committed_height,
                    window = novai_consensus::COMMIT_WINDOW,
                    "commit window: not proposing; frontier is parked at the bound while commits stall"
                );
            }
            return Ok(false);
        }

        if !state.may_vote(intended_height, state.round) {
            tracing::warn!(
                height = intended_height,
                round = state.round,
                "gate 9: already durably voted at this view; not proposing after restart"
            );
            return Ok(false);
        }

        // F3 guard Layer 1: budget tx selection against the runtime send
        // cap minus the measured envelope, so the assembled proposal
        // always encodes and the round never stalls on an unsendable
        // block.
        let tx_budget = self.proposal_tx_budget(&state)?;

        // Try to propose - NotLeader and AlreadyProposed are expected outcomes, not errors
        let block = match state.propose_block_with_budget(
            mempool,
            nonce_provider,
            &*db,
            &self.validator_set,
            tx_budget,
        ) {
            Ok(block) => block,
            Err(ConsensusError::NotLeader) => return Ok(false),
            Err(ConsensusError::AlreadyProposed) => return Ok(false),
            // Commit-window belt: the intent check above already refused and
            // warned; the engine-level refusal (reachable by any other
            // caller) stays a quiet skip here too.
            Err(ConsensusError::CommitWindowExceeded { .. }) => return Ok(false),
            Err(e) => return Err(format!("Propose block failed: {e:?}")),
        };

        // Cache our own proposed block so we can form QC when votes arrive
        state
            .cache_block(block.clone())
            .map_err(|e| format!("Cache block failed: {e:?}"))?;

        // Leader self-vote: add our own vote so we only need (quorum - 1) external votes.
        // With 4 validators and quorum=3, this means we need 2 of 3 peers instead of all 3.
        let self_vote = state
            .create_vote(&block, &self.signing_key)
            .map_err(|e| format!("Leader self-vote creation failed: {e:?}"))?;
        state
            .add_vote(self_vote, &self.validator_pubkeys_vec)
            .map_err(|e| format!("Leader self-vote add failed: {e:?}"))?;

        // gate 9: synced persist-before-broadcast. add_vote advanced the vote
        // high-water mark via note_self_vote; fsync it now, while the locks are
        // held, so it is durable before the proposal (and the QC this leader will
        // form, which embeds this vote) goes out below.
        state
            .persist_voted_view(&mut *db)
            .map_err(|e| format!("gate 9: persist voted_view failed: {e:?}"))?;

        // Build justify_qc for the proposal
        let justify_qc = if block.height == 1 {
            QC {
                height: 0,
                round: 0,
                block_hash: [0u8; 32],
                votes: vec![],
            }
        } else {
            state.highest_qc.clone().ok_or_else(|| {
                format!("Cannot propose height {} without highest_qc", block.height)
            })?
        };

        let proposal = novai_consensus_types::Proposal {
            block: block.clone(),
            justify_qc,
        };

        let unsigned_bytes = novai_consensus_types::codec::encode_proposal_v1_unsigned(&proposal)
            .map_err(|e| format!("Encode proposal failed: {e:?}"))?;

        let signature = sign_bytes(&self.signing_key, &unsigned_bytes);

        let signed_proposal = SignedProposal {
            proposer: self.our_address,
            proposal,
            signature,
        };

        // Reset timeout timer BEFORE dropping state lock — we just proposed,
        // give ourselves a full fresh timeout window to collect votes.
        // Must happen before dropping state to prevent check_timeout race.
        *self.round_start_time.lock_or_recover() = Instant::now();
        *self.last_timeout_time.lock_or_recover() = None;

        // Release locks before broadcasting
        drop(state);
        drop(db);

        let block_hash = novai_consensus_types::codec::hash_block_v1(&block)
            .map_err(|e| format!("Hash block failed: {e:?}"))?;
        tracing::debug!(
            height = block.height,
            round = block.round,
            tx_count = block.txs.len(),
            block_hash = ?&block_hash[..4],
            "PROPOSE_DIAG: proposed block"
        );

        // F3 guard Layer 2: refuse an over-cap envelope loudly instead of
        // letting the broadcast encode fail after the self-vote and the
        // proposal marks are already durable.
        self.proposal_wire_len(&signed_proposal)?;

        self.broadcast(NetworkMessage::SignedProposal(signed_proposal))?;
        Ok(true)
    }

    /// Handle incoming proposal.
    pub fn handle_proposal(&self, signed_proposal: SignedProposal) -> Result<(), String> {
        tracing::debug!(proposer = ?&signed_proposal.proposer[..4], "Received proposal");

        let block = &signed_proposal.proposal.block;

        // 1. Check proposer is expected leader for this height/round
        // For a block at height H, the leader is determined at view_height H-1
        let view_height = block.height.saturating_sub(1);
        let expected_leader =
            ConsensusState::compute_leader_for_view(view_height, block.round, &self.validator_set)
                .map_err(|e| format!("Failed to compute leader: {e:?}"))?;

        if signed_proposal.proposer != expected_leader {
            return Err(format!(
                "Invalid proposer: expected {:?}, got {:?}",
                &expected_leader[..4],
                &signed_proposal.proposer[..4]
            ));
        }

        // 2. Verify proposal signature
        let proposer_pubkey = self
            .validator_pubkeys
            .get(&signed_proposal.proposer)
            .ok_or_else(|| {
                format!(
                    "Proposer {:?} not in validator set",
                    &signed_proposal.proposer[..4]
                )
            })?;

        let unsigned_bytes =
            novai_consensus_types::codec::encode_proposal_v1_unsigned(&signed_proposal.proposal)
                .map_err(|e| format!("Encode proposal failed: {e:?}"))?;

        if !novai_crypto::verify_bytes(proposer_pubkey, &unsigned_bytes, &signed_proposal.signature)
        {
            return Err(format!(
                "Invalid proposal signature from {:?}",
                &signed_proposal.proposer[..4]
            ));
        }

        // 3. Validate justify_qc
        let justify_qc = &signed_proposal.proposal.justify_qc;
        if block.height == 1 {
            // Height 1 MUST use GenesisQC
            if justify_qc.height != 0 || justify_qc.round != 0 || !justify_qc.votes.is_empty() {
                return Err(format!(
                    "Height 1 proposal must use GenesisQC (height=0, round=0, votes=[]), got height={} round={} votes={}",
                    justify_qc.height, justify_qc.round, justify_qc.votes.len()
                ));
            }
        } else {
            // Height > 1 MUST have valid QC for height - 1
            if justify_qc.height != block.height - 1 {
                return Err(format!(
                    "Height {} proposal must have justify_qc for height {}, got height={}",
                    block.height,
                    block.height - 1,
                    justify_qc.height
                ));
            }

            // I verify the carried justify_qc with the same canonical helper
            // that handle_qc and the sync path use, so the proposal path
            // enforces identical rules: a quorum of distinct voters that are all
            // in the validator set, every vote bound to this QC's height and
            // block hash, and every signature valid. The previous inline check
            // counted raw votes and never bound a vote to the QC it certifies,
            // so a leader could embed genuine votes that validators cast for a
            // different block. I derive quorum from validator_pubkeys_vec, the
            // same set the helper checks membership against, matching handle_qc.
            let n = self.validator_pubkeys_vec.len();
            let quorum = 2 * ((n - 1) / 3) + 1;
            ConsensusState::verify_qc_well_formed(justify_qc, &self.validator_pubkeys_vec, quorum)
                .map_err(|e| format!("Rejecting proposal with malformed justify_qc: {e:?}"))?;
        }

        // 4. Apply justify_qc if it advances our state (QC catch-up).
        //    This fixes the race where proposal for N+1 arrives before the
        //    standalone QC(N) broadcast. The justify_qc was fully validated
        //    above (correct height, quorum votes, and all vote signatures verified).
        //    Idempotent: cache_qc_and_check_commit is a no-op when the QC
        //    does not dominate the current highest_qc.
        // 5. Verify block validity, create vote, and cache block in single lock acquisition
        let mut needs_sync = false;
        let mut committed_height_for_prune: Option<u64> = None;
        let vote = {
            // Lock order: state → db (must match try_propose_block, handle_vote,
            // handle_qc to prevent deadlock between main loop and receive threads).
            let mut state = self.state.lock_or_recover();
            let mut db = self.db.lock_or_recover();

            // Check if justify_qc would advance our view
            let dominated = match &state.highest_qc {
                None => justify_qc.height > 0,
                Some(existing) => {
                    justify_qc.height > existing.height
                        || (justify_qc.height == existing.height
                            && justify_qc.round > existing.round)
                }
            };

            if dominated {
                tracing::debug!(
                    qc_height = justify_qc.height,
                    qc_round = justify_qc.round,
                    our_highest_qc = ?state.highest_qc.as_ref().map(|q| (q.height, q.round)),
                    "QC catch-up from proposal"
                );

                match state.cache_qc_and_check_commit(justify_qc.clone(), &*db) {
                    Ok(to_commit) if !to_commit.is_empty() => {
                        let new_committed_height = to_commit.last().unwrap().height;
                        Self::verify_pre_commit_state_root(&*db, &to_commit)?;
                        // Take the vote-time executions before apply_commits
                        // evicts them (gate ACCEL Stage B).
                        let cached = state.take_pending_execs(&to_commit);
                        state
                            .persist_commit_atomic(
                                &mut *db,
                                &to_commit,
                                justify_qc,
                                new_committed_height,
                                None,
                            )
                            .map_err(|e| format!("QC catch-up atomic persist failed: {e:?}"))?;
                        state.apply_commits(&to_commit).map_err(|e| {
                            format!("CONSENSUS SAFETY VIOLATION during QC catch-up: {e:?}")
                        })?;
                        self.execute_committed_blocks(&mut db, &to_commit, cached)?;
                        committed_height_for_prune = Some(new_committed_height);
                        tracing::debug!(
                            committed_height = new_committed_height,
                            "QC catch-up committed blocks"
                        );
                    }
                    Ok(_) => {
                        state
                            .persist_highest_qc(&mut *db)
                            .map_err(|e| format!("QC catch-up persist highest QC failed: {e:?}"))?;
                    }
                    Err(e) => {
                        // Commit chain incomplete — blocks missing from cache.
                        // highest_qc was already updated by cache_qc_and_check_commit.
                        // Continue to verify and vote; sync will be triggered after
                        // locks are dropped.
                        tracing::warn!(?e, "QC catch-up commit chain incomplete");
                        needs_sync = true;
                        state
                            .persist_highest_qc(&mut *db)
                            .map_err(|e| format!("QC catch-up persist highest QC failed: {e:?}"))?;
                    }
                }
            }

            // Bug 1 fix: Detect late-arriving blocks BEFORE verify_block rejects
            // them. If block.height is behind our expected height but ahead of
            // committed_height, cache + persist and return without voting. This
            // prevents the in-memory cache gap that breaks the commit chain walk.
            let expected_height = match &state.highest_qc {
                Some(hqc) => std::cmp::max(state.height, hqc.height) + 1,
                None => state.height + 1,
            };

            // Skip proposals for already-committed heights. This happens when
            // committed_height advances via QC catch-up or sync before a delayed
            // proposal arrives on a duplicate peer connection.
            if block.height <= state.committed_height {
                tracing::debug!(
                    block_height = block.height,
                    committed_height = state.committed_height,
                    "Proposal for already-committed height — skipping"
                );
                drop(db);
                drop(state);
                return Ok(());
            }

            if block.height < expected_height && block.height > state.committed_height {
                tracing::warn!(
                    block_height = block.height,
                    expected_height,
                    committed_height = state.committed_height,
                    "Late-arriving block — caching without voting"
                );
                state
                    .cache_block(block.clone())
                    .map_err(|e| format!("Cache block failed: {e:?}"))?;

                // Persist to DB so chain walk DB fallback can find this block
                // after in-memory cache eviction. Only write if no block exists
                // at this height yet — avoids overwriting synced/committed blocks
                // with stale proposals from a different round.
                let key = novai_state::block_key(block.height);
                if db.get(&key).ok().flatten().is_none() {
                    let value = novai_consensus_types::codec::encode_block_v1(block)
                        .map_err(|e| format!("Failed to encode block: {e:?}"))?;
                    db.put(&key, &value)
                        .map_err(|e| format!("Failed to store block: {e:?}"))?;
                }

                drop(db);
                drop(state);
                return Ok(());
            }

            let exec = match state.verify_block_execute(block, &*db) {
                Ok(exec) => exec,
                Err(e) => {
                    tracing::debug!(
                        height = block.height,
                        round = block.round,
                        tx_count = block.txs.len(),
                        proposer = ?&signed_proposal.proposer[..4],
                        error = %format!("{:?}", e),
                        "VERIFY_DIAG: block verification FAILED"
                    );
                    return Err(format!("Block verification failed: {e:?}"));
                }
            };

            let recv_block_hash = novai_consensus_types::codec::hash_block_v1(block)
                .map_err(|e| format!("Hash block failed: {e:?}"))?;
            tracing::debug!(
                height = block.height,
                round = block.round,
                tx_count = block.txs.len(),
                block_hash = ?&recv_block_hash[..4],
                "VERIFY_DIAG: block verified OK, voting"
            );

            // 6. Cache block for commit rule (combined to avoid re-acquiring lock)
            state
                .check_no_fork(block)
                .map_err(|e| format!("Fork detection failed: {e:?}"))?;
            state
                .cache_block(block.clone())
                .map_err(|e| format!("Cache block failed: {e:?}"))?;

            // Cache the block's speculative execution so a restarted node can
            // rebuild its pending chain and later proposals resolve this parent
            // (gate wedge-276272). Runs on verified execution regardless of whether
            // gate 9 then refuses the vote below.
            state.note_pending_exec(
                recv_block_hash,
                block.height,
                exec.post_root,
                exec.write_set,
                exec.outcomes,
            );

            // Persist block to DB so the commit chain walk DB fallback can
            // recover it after in-memory cache eviction. Only write if no
            // block exists at this height yet — avoids overwriting synced or
            // committed blocks with proposals from a different round.
            // persist_commit_atomic and handle_block_response always overwrite
            // unconditionally (they store canonical committed blocks).
            let key = novai_state::block_key(block.height);
            if db.get(&key).ok().flatten().is_none() {
                let value = novai_consensus_types::codec::encode_block_v1(block)
                    .map_err(|e| format!("Failed to encode block: {e:?}"))?;
                db.put(&key, &value)
                    .map_err(|e| format!("Failed to store block: {e:?}"))?;
            }

            // 5. Create vote (skip if we already voted in this round — dedup
            // against duplicate proposals arriving via redundant connections)
            if state.voted_in_round.contains(&self.our_address) {
                tracing::debug!(
                    height = block.height,
                    "Already voted in this round, skipping duplicate proposal"
                );
                drop(db);
                drop(state);
                return Ok(());
            }

            // gate 9: durable equivocation guard. Refuse to vote at a view this
            // node already durably voted at (restart replay or a stale proposal);
            // otherwise advance the high-water mark. This survives restart, unlike
            // voted_in_round above. A higher-round re-proposal after a view change
            // is admitted, so it does not reintroduce the height-only halt.
            if let Err(e) = state.note_self_vote(block.height, block.round) {
                tracing::debug!(
                    height = block.height,
                    round = block.round,
                    error = ?e,
                    "gate 9: refusing self-vote at an already-voted view"
                );
                drop(db);
                drop(state);
                return Ok(());
            }

            let vote = state
                .create_vote(block, &self.signing_key)
                .map_err(|e| format!("Vote creation failed: {e:?}"))?;

            // Mark ourselves as voted so duplicate proposals are rejected
            state.voted_in_round.insert(self.our_address);

            // gate 9: synced persist-before-broadcast. This fsync must return
            // before the vote is broadcast. The state and db locks are held until
            // the end of this block; the broadcast is after the lock drop, so the
            // durable write strictly precedes the network send.
            state
                .persist_voted_view(&mut *db)
                .map_err(|e| format!("gate 9: persist voted_view failed: {e:?}"))?;

            // Reset round timer BEFORE dropping state lock to prevent race with
            // check_timeout (same pattern as handle_vote and handle_qc).
            *self.round_start_time.lock_or_recover() = Instant::now();
            *self.last_timeout_time.lock_or_recover() = None;

            vote
        };

        // Trigger block sync if commit chain was incomplete (locks are now dropped)
        if needs_sync {
            self.try_request_missing_blocks();
        }

        // Prune QC broadcast cache after commit (locks already dropped)
        if let Some(ch) = committed_height_for_prune {
            self.prune_qc_broadcast_cache(ch);
        }

        tracing::info!(height = block.height, "Voting for block");

        self.broadcast(NetworkMessage::Vote(vote))
    }

    /// Handle incoming vote.
    pub fn handle_vote(&self, vote: Vote) -> Result<(), String> {
        // H-11: Verify vote signature BEFORE acquiring state lock.
        // Crypto verification (~100µs) no longer blocks other consensus operations.
        let pubkey = self
            .validator_pubkeys
            .get(&vote.voter)
            .ok_or_else(|| format!("Vote from unknown validator {:?}", &vote.voter[..4]))?;
        {
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
                return Err("Invalid vote signature".to_string());
            }
        }

        let mut state = self.state.lock_or_recover();

        // Use add_vote_verified since we already checked the signature above.
        // Duplicate/equivocation votes are expected during normal operation
        // (e.g., redundant network paths) — treat them as no-ops, not errors.
        match state.add_vote_verified(vote.clone(), &self.validator_pubkeys_vec) {
            Ok(()) => {}
            Err(novai_consensus::ConsensusError::InvalidVote(ref msg))
                if msg.contains("Duplicate vote")
                    || msg.contains("height mismatch")
                    || msg.contains("durable vote guard") =>
            {
                return Ok(());
            }
            Err(e) => return Err(format!("Add vote failed: {e:?}")),
        }

        // Log AI signal if present (advisory only)
        if let Some(commitment) = vote.ai_signal_commitment {
            tracing::debug!(?commitment, "Node received vote with AI signal");
        }

        // Check if we're leader for the block's height
        // Leader for height N is determined at state height N-1
        let leader_for_vote = {
            let proposal_state_height = vote.height.saturating_sub(1);
            let leader_idx =
                ((proposal_state_height + vote.round) as usize) % self.validator_set.len();
            self.validator_set[leader_idx] == self.our_address
        };

        // Only leader forms QC - non-leaders just collect votes
        if !leader_for_vote {
            return Ok(());
        }

        if let Some(qc) = state
            .try_form_qc(&vote.block_hash, &self.validator_set)
            .map_err(|e| format!("QC formation failed: {e:?}"))?
        {
            let key = (qc.height, qc.round, qc.block_hash);

            {
                let mut sent = self.qc_broadcasted.lock_or_recover();
                if sent.contains(&key) {
                    return Ok(());
                }
                sent.insert(key);
            }

            // Look up the certified block's tx_count for diagnostics
            let qc_block_txs = state.block_by_hash.get(&qc.block_hash).map(|b| b.txs.len());
            tracing::debug!(
                qc_height = qc.height,
                qc_round = qc.round,
                votes = qc.votes.len(),
                block_hash = ?&qc.block_hash[..4],
                certified_block_txs = ?qc_block_txs,
                "QC_DIAG: QC formed"
            );

            // Process the QC locally before broadcasting.
            // Commit chain errors are non-fatal — highest_qc is updated
            // regardless, and the QC MUST always be broadcast so other
            // nodes can advance.
            // Lock order: state (already held) → db
            let mut vote_committed_height: Option<u64> = None;
            let mut committed_blocks: Vec<novai_consensus_types::Block> = Vec::new();
            let mut committed_cached: Vec<Option<novai_consensus::PendingExec>> = Vec::new();
            let mut db = self.db.lock_or_recover();
            match state.cache_qc_and_check_commit(qc.clone(), &*db) {
                Ok(to_commit) if !to_commit.is_empty() => {
                    let new_committed_height = to_commit.last().unwrap().height;
                    Self::verify_pre_commit_state_root(&*db, &to_commit)?;
                    // Take the vote-time executions before apply_commits
                    // evicts them; the state lock drops before execution at
                    // this site, so this is the last safe moment (gate ACCEL
                    // Stage B).
                    committed_cached = state.take_pending_execs(&to_commit);
                    state
                        .persist_commit_atomic(
                            &mut *db,
                            &to_commit,
                            &qc,
                            new_committed_height,
                            None,
                        )
                        .map_err(|e| format!("Atomic persist failed: {e:?}"))?;
                    state.apply_commits(&to_commit).map_err(|e| {
                        format!("CONSENSUS SAFETY VIOLATION during vote commit: {e:?}")
                    })?;
                    vote_committed_height = Some(new_committed_height);
                    committed_blocks = to_commit;
                    tracing::debug!(
                        committed_height = new_committed_height,
                        "Committed blocks (formed QC locally)"
                    );
                }
                Ok(_) => {
                    state
                        .persist_highest_qc(&mut *db)
                        .map_err(|e| format!("Failed to persist highest QC: {e:?}"))?;
                    tracing::debug!(
                        qc_height = qc.height,
                        "Persisted highest_qc (formed locally)"
                    );
                }
                Err(e) => {
                    // Commit chain incomplete — blocks missing from cache.
                    // highest_qc was already updated. Persist it and ALWAYS
                    // broadcast the QC so other nodes can advance.
                    tracing::warn!(?e, "Commit chain incomplete (will sync)");
                    state
                        .persist_highest_qc(&mut *db)
                        .map_err(|e| format!("Failed to persist highest QC: {e:?}"))?;
                }
            }

            // Reset timeout timer BEFORE dropping state lock to prevent a race
            // with check_timeout. If we drop state first, check_timeout can read
            // the stale round_start_time and the advanced state, firing a spurious
            // timeout immediately after QC formation.
            *self.round_start_time.lock_or_recover() = Instant::now();
            *self.last_timeout_time.lock_or_recover() = None;

            // Release state lock EARLY — execution writes to different DB
            // namespaces than consensus (no overlap), so we only need the
            // db lock. This frees the state lock for other threads during
            // tx execution.
            drop(state);

            if !committed_blocks.is_empty() {
                self.execute_committed_blocks(&mut db, &committed_blocks, committed_cached)?;
            }
            drop(db);

            // Trigger block sync for missing blocks (locks now dropped)
            self.try_request_missing_blocks();

            // Prune QC broadcast cache after commit
            if let Some(ch) = vote_committed_height {
                self.prune_qc_broadcast_cache(ch);
            }

            self.broadcast(NetworkMessage::Qc(qc))?;
        }

        Ok(())
    }

    /// Handle incoming QC.
    pub fn handle_qc(&self, qc: QC) -> Result<(), String> {
        tracing::debug!(height = qc.height, round = qc.round, "Received QC");

        // Stage 3 (gate-handle-qc-unverified-535004): a gossiped QC is an
        // unauthenticated network payload. Verify it fully (quorum of distinct
        // in-set voters, every vote bound to this QC, every signature valid)
        // BEFORE it can reach cache_qc_and_check_commit, whose only install gate
        // is encode_qc_v1 (well-formedness, which accepts a zero-vote QC). Without
        // this, a single QC{height: huge, votes: []} installs as highest_qc and
        // persists to KEY_HIGHEST_QC, wedging the node permanently across restart.
        // Quorum is derived from validator_pubkeys_vec, the same set
        // verify_qc_well_formed checks membership against, so the threshold and the
        // membership test agree. Rejecting here, before the locks below, means a
        // forged QC touches no state and holds no lock.
        let n = self.validator_pubkeys_vec.len();
        let quorum = 2 * ((n - 1) / 3) + 1;
        ConsensusState::verify_qc_well_formed(&qc, &self.validator_pubkeys_vec, quorum)
            .map_err(|e| format!("Rejecting unverified gossiped QC: {e:?}"))?;

        // CRITICAL FIX: Hold state lock across cache_qc_and_check_commit AND apply_commits
        // to prevent race condition where timeouts arriving between the two operations
        // get wiped out by apply_commits clearing pending_timeouts.
        let mut state = self.state.lock_or_recover();

        // Record current round and highest_qc height before processing QC
        let old_round = state.round;
        let old_highest = state.highest_qc.as_ref().map(|q| q.height);

        // Check commit rule and get blocks to commit.
        // Commit chain errors are non-fatal — highest_qc is updated regardless,
        // and commits will happen when missing blocks arrive via sync.
        // Lock order: state (already held) → db
        let mut db = self.db.lock_or_recover();
        let mut committed = false;
        let mut qc_committed_height: Option<u64> = None;
        let mut committed_blocks: Vec<novai_consensus_types::Block> = Vec::new();
        let mut committed_cached: Vec<Option<novai_consensus::PendingExec>> = Vec::new();
        match state.cache_qc_and_check_commit(qc.clone(), &*db) {
            Ok(to_commit) if !to_commit.is_empty() => {
                let new_committed_height = to_commit.last().unwrap().height;
                Self::verify_pre_commit_state_root(&*db, &to_commit)?;
                // Take the vote-time executions before apply_commits evicts
                // them; the state lock drops before execution at this site
                // (gate ACCEL Stage B).
                committed_cached = state.take_pending_execs(&to_commit);
                state
                    .persist_commit_atomic(&mut *db, &to_commit, &qc, new_committed_height, None)
                    .map_err(|e| format!("Atomic persist failed: {e:?}"))?;
                state
                    .apply_commits(&to_commit)
                    .map_err(|e| format!("CONSENSUS SAFETY VIOLATION during QC commit: {e:?}"))?;
                committed = true;
                qc_committed_height = Some(new_committed_height);
                committed_blocks = to_commit;
                tracing::debug!(
                    committed_height = state.committed_height(),
                    highest_qc = state.highest_qc.as_ref().map(|q| q.height).unwrap_or(0),
                    "Persisted state (atomic)"
                );
            }
            Ok(_) => {
                if state.highest_qc.as_ref().map(|q| q.height) == Some(qc.height) {
                    state
                        .persist_highest_qc(&mut *db)
                        .map_err(|e| format!("Failed to persist highest QC: {e:?}"))?;
                    tracing::debug!(
                        qc_height = qc.height,
                        "Persisted highest_qc (no commit triggered)"
                    );
                }
            }
            Err(e) => {
                // Commit chain incomplete — blocks missing from cache.
                // highest_qc was already updated. Persist it and continue.
                tracing::warn!(?e, "Commit chain incomplete (will sync)");
                state
                    .persist_highest_qc(&mut *db)
                    .map_err(|e| format!("Failed to persist highest QC: {e:?}"))?;
            }
        }

        // Check if round was reset (view height advanced) or highest_qc advanced
        let new_highest = state.highest_qc.as_ref().map(|q| q.height);
        let qc_advanced = new_highest > old_highest;
        let round_was_reset = state.round == 0 && old_round != 0;

        // Reset timeout timer BEFORE dropping state lock to prevent a race
        // with check_timeout. If we drop state first, check_timeout can read
        // the stale round_start_time and the advanced state, firing a spurious
        // timeout immediately after receiving a QC.
        if round_was_reset || committed || qc_advanced {
            *self.round_start_time.lock_or_recover() = Instant::now();
            *self.last_timeout_time.lock_or_recover() = None;
        }

        // Release state lock EARLY — execution writes to different DB
        // namespaces than consensus (no overlap), so we only need the
        // db lock. This frees the state lock for other threads during
        // tx execution.
        drop(state);

        if !committed_blocks.is_empty() {
            self.execute_committed_blocks(&mut db, &committed_blocks, committed_cached)?;
        }
        drop(db);

        // Prune QC broadcast cache after commit
        if let Some(ch) = qc_committed_height {
            self.prune_qc_broadcast_cache(ch);
        }

        // Trigger block sync if commit chain was incomplete
        if !committed {
            self.try_request_missing_blocks();
        }

        Ok(())
    }

    /// Handle a peer connection (blocking, spawned per peer).
    ///
    /// Uses `catch_unwind` around message handling to prevent a panic from
    /// poisoning shared mutexes and cascading to all other threads.
    pub fn handle_peer_connection(
        self: Arc<Self>,
        mut reader: impl std::io::Read,
        peer_ip: IpAddr,
    ) {
        tracing::debug!(%peer_ip, "Starting receive loop for peer");

        // C-03: Per-peer message rate limiting.
        // Simple sliding window: count messages per second, disconnect if exceeded.
        let mut msg_count: u64 = 0;
        let mut window_start = Instant::now();

        loop {
            match read_wire_message(&mut reader) {
                Ok(msg) => {
                    // Rate limit: reset window every second, disconnect if exceeded.
                    let elapsed = window_start.elapsed();
                    if elapsed >= std::time::Duration::from_secs(1) {
                        msg_count = 1;
                        window_start = Instant::now();
                    } else {
                        msg_count += 1;
                        if msg_count > novai_p2p::MAX_MESSAGES_PER_SECOND {
                            tracing::warn!(
                                msg_count,
                                limit = novai_p2p::MAX_MESSAGES_PER_SECOND,
                                %peer_ip,
                                "Peer exceeded message rate limit, banning"
                            );
                            self.ban_list.ban(peer_ip, "rate limit exceeded");
                            break;
                        }
                    }

                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        self.handle_network_message(msg)
                    }));
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            tracing::error!(%e, "Message handling failed");
                        }
                        Err(panic_payload) => {
                            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                                (*s).to_string()
                            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "unknown panic".to_string()
                            };
                            tracing::error!(
                                %panic_msg,
                                %peer_ip,
                                "PANIC in message handler — banning peer"
                            );
                            self.ban_list.ban(peer_ip, "caused panic in handler");
                            break;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(?e, "Read failed from peer, disconnecting");
                    break;
                }
            }
        }
    }

    /// Record one failed sync cycle (a matching empty response or a request
    /// timeout: either way the requested range was not served) and stamp the
    /// committed height so commit progress can reset the count.
    /// Observability ladder: early strikes DEBUG, repeated strikes WARN with
    /// the count and the backoff now in force.
    fn record_sync_strike(&self, reason: &'static str) {
        let committed = self.state.lock_or_recover().committed_height;
        let mut retry = self.sync_retry.lock_or_recover();
        retry.strikes = retry.strikes.saturating_add(1);
        retry.strike_committed_height = committed;

        // Gate F5 Stage 1: a strike IS an unserved probe, and both unserved
        // paths (a matching empty response and a request timeout) funnel
        // through here. It advances the snapshot-sync machine only when a
        // beyond-retention cycle already moved it into Arming, so a strike
        // inside the retention window can never arm.
        let phase_before = retry.snapshot_sync.phase();
        retry.snapshot_sync.note_unserved_probe();
        if retry.snapshot_sync.phase() != phase_before {
            tracing::error!(
                committed,
                unserved_probes = retry.snapshot_sync.unserved_probes(),
                reason,
                "SNAPSHOT SYNC ARMED: consecutive probes past the fleet \
                 retention window came back unserved; block-range sync cannot \
                 recover this node and a verified state snapshot is required"
            );
        }

        let backoff_ms = sync_backoff_ms(retry.strikes);
        if retry.strikes >= SYNC_STRIKE_WARN_THRESHOLD {
            tracing::warn!(
                strikes = retry.strikes,
                backoff_ms,
                reason,
                "Sync cycle failed; backing off"
            );
        } else {
            tracing::debug!(
                strikes = retry.strikes,
                backoff_ms,
                reason,
                "Sync cycle failed"
            );
        }
    }

    /// F1: the binary main-loop sweep calls this after clearing a timed-out
    /// pending sync request. A dropped response and a served empty response
    /// both mean the range was not served; both engage the backoff gate.
    pub fn on_sync_request_timeout(&self) {
        self.record_sync_strike("sync request timed out");
    }

    /// Gate F5 Stage 4: may this node put a snapshot message on the wire?
    ///
    /// Default FALSE. Receiving and serving are always on once the binary is
    /// deployed; SENDING is what this gates, by runtime flag, exactly as
    /// `--wire-send-cap-bytes` gates the F3 cap raise. That asymmetry is the
    /// whole two-phase deploy: an un-upgraded peer that receives one of the new
    /// kinds gets `InvalidKind`, which its read loop treats as fatal and
    /// DISCONNECTS on. So no node may send until every node can receive.
    #[must_use]
    pub fn snapshot_send_enabled(&self) -> bool {
        self.snapshot_send_enabled
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Enable snapshot SENDING. Phase B of the deploy, by restart.
    pub fn set_snapshot_send_enabled(&self, enabled: bool) {
        self.snapshot_send_enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Attach the producer whose cached bundle this node serves from.
    pub fn set_snapshot_producer(&mut self, p: Arc<crate::snapshot::producer::SnapshotProducer>) {
        self.snapshot_producer = Some(p);
    }

    /// THE ONLY path by which a Stage 4 message reaches the wire.
    ///
    /// Every send site goes through here, so the Phase A guarantee is one
    /// branch in one place rather than a discipline spread across call sites.
    fn send_snapshot_msg(&self, msg: NetworkMessage) -> SnapshotSendOutcome {
        if !self.snapshot_send_enabled() {
            // Phase A: receive-capable, send-disabled. Nothing is encoded and
            // nothing is broadcast, so no peer can see a byte it cannot decode.
            return SnapshotSendOutcome::Disabled;
        }
        match self.broadcast(msg) {
            Ok(()) => SnapshotSendOutcome::Sent,
            Err(e) => {
                tracing::debug!(%e, "Snapshot message broadcast failed");
                SnapshotSendOutcome::SendFailed
            }
        }
    }

    /// Ask peers for a snapshot manifest.
    pub fn request_snapshot_manifest(&self) -> SnapshotSendOutcome {
        self.send_snapshot_msg(NetworkMessage::SnapshotManifestRequest(
            novai_consensus_types::SnapshotManifestRequest {
                requester: self.our_address,
            },
        ))
    }

    /// Ask peers for one chunk of the snapshot at `height`.
    pub fn request_snapshot_chunk(&self, height: u64, index: u32) -> SnapshotSendOutcome {
        self.send_snapshot_msg(NetworkMessage::SnapshotChunkRequest(
            novai_consensus_types::SnapshotChunkRequest {
                requester: self.our_address,
                height,
                index,
            },
        ))
    }

    /// Serve a manifest request.
    ///
    /// O7: a node that is ITSELF in snapshot sync refuses. It cannot both be
    /// recovering and be a source: its own state is by definition the state it
    /// has not yet repaired, and a bundle built from it would at best waste the
    /// asker's retention budget.
    pub fn handle_snapshot_manifest_request(
        &self,
        req: &novai_consensus_types::SnapshotManifestRequest,
    ) -> SnapshotSendOutcome {
        if self.snapshot_sync().phase() != SnapshotSyncPhase::Idle {
            tracing::debug!("Refusing to serve a snapshot while recovering from one");
            return SnapshotSendOutcome::RefusedRecovering;
        }
        let Some(producer) = &self.snapshot_producer else {
            return SnapshotSendOutcome::NoProducer;
        };
        if !self.allow_serve(req.requester) {
            return SnapshotSendOutcome::RateLimited;
        }

        // A manifest request is also the demand signal: a healthy node produces
        // nothing until somebody asks. The answer to the first ask is usually
        // "none yet", and the asker retries.
        producer.request();
        let manifest = producer.cached().map_or_else(Vec::new, |b| {
            crate::snapshot::bundle::encode_manifest_v1(&b.manifest).unwrap_or_default()
        });
        self.send_snapshot_msg(NetworkMessage::SnapshotManifestResponse(
            novai_consensus_types::SnapshotManifestResponse {
                responder: self.our_address,
                manifest,
            },
        ))
    }

    /// Serve a chunk request. Same recovering and rate-limit rules.
    pub fn handle_snapshot_chunk_request(
        &self,
        req: &novai_consensus_types::SnapshotChunkRequest,
    ) -> SnapshotSendOutcome {
        if self.snapshot_sync().phase() != SnapshotSyncPhase::Idle {
            return SnapshotSendOutcome::RefusedRecovering;
        }
        let Some(producer) = &self.snapshot_producer else {
            return SnapshotSendOutcome::NoProducer;
        };
        if !self.allow_serve(req.requester) {
            return SnapshotSendOutcome::RateLimited;
        }
        // An empty payload is the faithful "I cannot serve that": a height that
        // is not the cached one, or an index past its end. Never a guess.
        let payload = producer
            .cached()
            .filter(|b| b.manifest.height == req.height)
            .and_then(|b| b.chunks.get(req.index as usize).cloned())
            .unwrap_or_default();
        self.send_snapshot_msg(NetworkMessage::SnapshotChunkResponse(
            novai_consensus_types::SnapshotChunkResponse {
                responder: self.our_address,
                height: req.height,
                index: req.index,
                payload,
            },
        ))
    }

    /// Rate-limit gate, keyed on the requesting peer.
    fn allow_serve(&self, peer: Address) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX));
        self.snapshot_serve.lock_or_recover().allow(peer, now)
    }

    /// Record a peer that answered with bytes failing the manifest's digest.
    pub fn strike_snapshot_peer(&self, peer: Address) -> u32 {
        self.snapshot_peers.lock_or_recover().strike(peer)
    }

    /// Is this peer out of the rotation?
    #[must_use]
    pub fn snapshot_peer_shunned(&self, peer: &Address) -> bool {
        self.snapshot_peers.lock_or_recover().is_shunned(peer)
    }

    /// A good answer clears the ladder, so strikes must be consecutive.
    pub fn clear_snapshot_peer(&self, peer: &Address) {
        self.snapshot_peers.lock_or_recover().clear(peer);
    }

    /// Trigger a block sync request if committed_height is behind highest_qc.
    ///
    /// Called after "commit chain incomplete" errors to actually initiate sync
    /// instead of just logging, and from the periodic 2s trigger. Uses existing
    /// dedup via `pending_sync_request`. MUST be called without holding the
    /// state lock.
    ///
    /// F1 retry gate: consecutive failed cycles (matching empty responses or
    /// request timeouts) back off exponentially per `sync_backoff_ms`, any
    /// commit progress resets the gate, and a gap beyond PRUNE_RETAIN_BLOCKS
    /// returns the deterministic `BehindRetention` outcome (block-range sync
    /// is structurally impossible; the node needs a snapshot import) instead
    /// of re-issuing an unservable range forever. The in-window zero-strike
    /// path issues exactly the same request as before the gate existed.
    /// Gate F5 Stage 1: the snapshot-sync detection machine, read by value.
    ///
    /// The metrics collector reads this for the `novai_sync_mode` gauge, and
    /// the gate tests read it to observe transitions. `SnapshotSyncMachine` is
    /// `Copy`, so this never hands out the lock.
    #[must_use]
    pub fn snapshot_sync(&self) -> SnapshotSyncMachine {
        self.sync_retry.lock_or_recover().snapshot_sync
    }

    pub fn try_request_missing_blocks(&self) -> SyncRequestOutcome {
        let (committed, hqc_height) = {
            let state = self.state.lock_or_recover();
            let committed = state.committed_height;
            let hqc_height = state.highest_qc.as_ref().map(|q| q.height).unwrap_or(0);
            (committed, hqc_height)
        };

        let mut retry = self.sync_retry.lock_or_recover();

        // Gate F5 Stage 1: commit progress disarms the snapshot-sync machine
        // from any phase, evaluated BEFORE the no-gap return so a node that
        // catches up completely disarms too. A node that is committing is a
        // node block-range sync can still serve, and it must never install a
        // snapshot. The retry lock moves ahead of the no-gap return to make
        // that reachable; it is the same lock this function already took, in
        // the same state-then-retry order, so no ordering changes.
        if retry.snapshot_sync.observe_commit_progress(committed) {
            tracing::info!(
                committed,
                "Snapshot sync disarmed: commit progress proves block-range \
                 sync is serving this node"
            );
        }

        // Need at least 3-chain gap to have committable blocks
        if hqc_height <= committed + 2 {
            return SyncRequestOutcome::NoGap;
        }

        // Commit progress since the last strike proves the pipeline works
        // again; reset the gate so catch-up runs at full cadence.
        if retry.strikes > 0 && committed > retry.strike_committed_height {
            tracing::debug!(
                strikes = retry.strikes,
                committed,
                "Sync retry strikes reset on commit progress"
            );
            retry.strikes = 0;
        }

        // Behind the fleet retention window no honest peer retains the
        // range (every peer prunes blocks more than PRUNE_RETAIN_BLOCKS
        // behind its tip), so re-requesting is structurally futile: only a
        // snapshot import (F4) can advance committed. Escalate at ERROR and
        // keep a low-rate probe, each at most once per SYNC_RETRY_MAX_MS.
        // A served probe commits progress and resets the gate, which also
        // self-corrects near the retention boundary.
        if hqc_height - committed > novai_consensus::PRUNE_RETAIN_BLOCKS {
            // Gate F5 Stage 1: enter the arming band. Entering is not
            // evidence; only an unserved probe (recorded as a strike) is, so
            // the count starts at zero here. Everything below this line is the
            // F1 escalation and probe behaviour, unchanged.
            let phase_before = retry.snapshot_sync.phase();
            retry.snapshot_sync.note_behind_retention(committed);
            if retry.snapshot_sync.phase() != phase_before {
                tracing::warn!(
                    committed,
                    hqc_height,
                    retention = novai_consensus::PRUNE_RETAIN_BLOCKS,
                    "Snapshot sync arming: the gap is past the fleet retention \
                     window; awaiting probe evidence that peers cannot serve it"
                );
            }

            let period = Duration::from_millis(SYNC_RETRY_MAX_MS);
            if retry
                .last_escalation_log
                .map_or(true, |at| at.elapsed() >= period)
            {
                retry.last_escalation_log = Some(Instant::now());
                tracing::error!(
                    committed,
                    hqc_height,
                    retention = novai_consensus::PRUNE_RETAIN_BLOCKS,
                    "Sync range is behind the fleet retention window; \
                     block-range sync cannot serve it, a snapshot import is \
                     required (see the reseed procedure)"
                );
            }
            let probe_due = retry
                .last_attempt
                .map_or(true, |at| at.elapsed() >= period);
            let mut probed = false;
            if probe_due {
                let end = std::cmp::min(committed + SYNC_CHUNK_SIZE, hqc_height);
                if self.request_blocks_from_peer(committed + 1, end).is_ok() {
                    retry.last_attempt = Some(Instant::now());
                    probed = true;
                }
            }
            return SyncRequestOutcome::BehindRetention { probed };
        }

        // Gate F5 Stage 1: inside the retention window block-range sync is
        // viable by construction, so the machine holds no state.
        //
        // Honest note on this line: at this tree it is provably a no-op, and
        // the Stage 1 mutation run proved it (removing it fails no test). The
        // gap can only shrink back inside the window by committed advancing,
        // because the frontier never regresses, and the commit-progress disarm
        // above runs earlier in the SAME call, so control never reaches here
        // with a non-Idle machine. It is kept as a fail-safe against a future
        // change that makes the frontier non-monotonic or reorders the disarm,
        // and it is covered directly by the unit tests on the machine rather
        // than through this call site, so it is not uncovered code.
        retry.snapshot_sync.note_within_retention();

        // Backoff gate: after failed cycles, refuse to re-issue until
        // min(2s * 2^strikes, 60s) has elapsed since the last request.
        if !sync_retry_due(retry.strikes, retry.last_attempt.map(|at| at.elapsed())) {
            return SyncRequestOutcome::BackedOff;
        }

        // Cap to SYNC_CHUNK_SIZE blocks per request to avoid timeout on large ranges
        let end = std::cmp::min(committed + SYNC_CHUNK_SIZE, hqc_height);

        // request_blocks_from_peer already checks pending_sync_request for dedup
        match self.request_blocks_from_peer(committed + 1, end) {
            Ok(()) => {
                retry.last_attempt = Some(Instant::now());
                SyncRequestOutcome::Requested
            }
            Err(e) if e.contains("already pending") => SyncRequestOutcome::AlreadyPending,
            Err(e) => {
                tracing::warn!(%e, "Block sync request failed");
                SyncRequestOutcome::RequestFailed
            }
        }
    }

    /// Dispatch network message to appropriate handler.
    fn handle_network_message(&self, msg: NetworkMessage) -> Result<(), String> {
        match msg {
            NetworkMessage::SignedProposal(sp) => self.handle_proposal(sp),
            NetworkMessage::Vote(v) => self.handle_vote(v),
            NetworkMessage::Qc(qc) => self.handle_qc(qc),
            NetworkMessage::Timeout(t) => self.handle_timeout(t).map(|_| ()),
            NetworkMessage::BlockRequest(req) => self.handle_block_request(req),
            NetworkMessage::BlockResponse(resp) => self.handle_block_response(resp),
            NetworkMessage::Transaction(bytes) => self.handle_gossip_tx(bytes),
            // Gate F5 Stage 4. Serving is always on once the binary is
            // deployed; whether an ANSWER leaves this node is decided inside,
            // by the send gate. Responses are consumed by the Stage 5 fetch
            // loop; accepting and ignoring them here is what makes Phase A a
            // safe, complete deploy on its own.
            NetworkMessage::SnapshotManifestRequest(req) => {
                self.handle_snapshot_manifest_request(&req);
                Ok(())
            }
            NetworkMessage::SnapshotChunkRequest(req) => {
                self.handle_snapshot_chunk_request(&req);
                Ok(())
            }
            NetworkMessage::SnapshotManifestResponse(_)
            | NetworkMessage::SnapshotChunkResponse(_) => Ok(()),
        }
    }

    /// Handle a gossipped transaction from a peer.
    ///
    /// Decodes, validates via nonce check, and inserts into the local mempool.
    /// Duplicates and nonce-stale txs are silently ignored (expected).
    fn handle_gossip_tx(&self, bytes: Vec<u8>) -> Result<(), String> {
        let (mempool, nonce) = match (&self.gossip_mempool, &self.gossip_nonce) {
            (Some(mp), Some(np)) => (mp, np),
            _ => return Ok(()), // gossip not configured
        };

        let tx = novai_codec::decode_tx_v1_signed(&bytes)
            .map_err(|e| format!("Invalid gossipped tx: {e:?}"))?;

        let nonce_wrapper = GossipNonceProvider(Arc::clone(nonce));
        let mut mp = mempool.lock_or_recover();
        match mp.insert(tx, &nonce_wrapper) {
            Ok(txid) => {
                tracing::debug!(txid = %hex::encode(txid), "Gossip tx accepted");
            }
            Err(e) => {
                // Gate SOAK C2: gossip rejections used to be swallowed
                // entirely, so a fleet refusing everything looked identical
                // to a fleet receiving nothing. Still not logged per tx
                // (far too noisy), but now counted.
                crate::metrics::pool_metrics::record_rejection(&e);
            }
        }
        Ok(())
    }
}
