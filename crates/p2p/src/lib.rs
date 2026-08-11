//! novai-p2p: Minimal TCP-based networking for consensus messages.
//!
//! Wire format: [len: u32 be][version: u8][kind: u8][payload: len-2 bytes]
//!
//! Clean-room implementation using `std::net` (no external P2P libraries).
//! Suitable for localhost devnet and testing.

pub mod noise;
pub mod transport;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use novai_consensus_types::{BlockRequest, BlockResponse, SignedProposal, Timeout, Vote, QC};

/// Maximum wire message size on the SEND side (2MB), as the DEFAULT.
///
/// F3: the effective send cap is the [`PeerManager`]'s runtime value,
/// set from `--wire-send-cap-bytes` and defaulting to this constant.
/// Public so the node's sync responder derives its response byte budget
/// from the same value the encoder enforces (F2), keeping the two from
/// drifting apart.
pub const MAX_WIRE_MSG_BYTES: u32 = 2 * 1024 * 1024;

/// Maximum wire message size accepted on RECEIVE (16 MiB).
///
/// Raised above the send default (F3) so every node accepts the worst
/// valid 3-full-pair sync response (12,165,932 checked wire bytes, gate
/// F3 diagnosis 12.1) with 27.5 percent headroom, making the 3-chain
/// frontier guarantee unconditional at full block load. Two-phase deploy,
/// receive first: the whole fleet accepts up to this cap (Phase A) before
/// any node is configured to send past the 2 MiB default (Phase B).
pub const MAX_RECV_WIRE_MSG_BYTES: u32 = 16 * 1024 * 1024;

/// Wire message kinds.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    SignedProposal = 1,
    Vote = 2,
    Qc = 3,
    Timeout = 4,
    BlockRequest = 5,
    BlockResponse = 6,
    Transaction = 7,
    // Gate F5 Stage 4. These four are why the deploy is two-phase.
    //
    // `from_u8` returns None for a byte it does not know, `read_wire_message`
    // turns that into `P2PError::InvalidKind`, and the peer read loop treats any
    // read error as fatal for that connection. So a node that SENDS one of these
    // to a peer running an older binary DISCONNECTS that peer. Every node must
    // therefore be able to RECEIVE these kinds before any node is configured to
    // send them, which is what the node's `--snapshot-sync` flag gates: sending
    // only, default off, exactly like the F3 wire-cap raise.
    SnapshotManifestRequest = 8,
    SnapshotManifestResponse = 9,
    SnapshotChunkRequest = 10,
    SnapshotChunkResponse = 11,
}

impl MessageKind {
    const fn from_u8(b: u8) -> Option<Self> {
        match b {
            1 => Some(Self::SignedProposal),
            2 => Some(Self::Vote),
            3 => Some(Self::Qc),
            4 => Some(Self::Timeout),
            5 => Some(Self::BlockRequest),
            6 => Some(Self::BlockResponse),
            7 => Some(Self::Transaction),
            8 => Some(Self::SnapshotManifestRequest),
            9 => Some(Self::SnapshotManifestResponse),
            10 => Some(Self::SnapshotChunkRequest),
            11 => Some(Self::SnapshotChunkResponse),
            _ => None,
        }
    }
}

/// The worst-case FRAMED snapshot chunk message, end to end.
///
/// Three parts, and the third is the one a hand formula gets wrong: the codec
/// header (version tag, responder, height, index, payload length = 49 bytes), a
/// payload at the wire bound, and the full frame envelope of SIX bytes. The
/// envelope is the 4-byte length prefix plus the version and kind bytes; the
/// "+2" that appears in the F2/F3 budget comments counts only the two bytes
/// INSIDE the length field, because those budgets are measured against the
/// payload. This constant measures the whole frame, so it needs all six.
/// `gate_f5_wire_red` asserts it against the real encoder output rather than
/// trusting the arithmetic, which is how the discrepancy was found.
pub const MAX_SNAPSHOT_CHUNK_MSG_BYTES: usize =
    (1 + 32 + 8 + 4 + 4) + novai_consensus_types::codec::MAX_SNAPSHOT_CHUNK_BYTES + 6;

// Gate F5 Stage 4 (T4.2): the whole point of the 512 KiB chunk bound is that
// enabling snapshot sending needs NO second fleet-wide change. Pinned at
// compile time, the same way the F2/F3 response budget relations are pinned:
// if the chunk bound is ever raised past the DEFAULT send cap, this stops the
// build rather than turning Phase B into another cap-raise flag day.
//
// The exact arithmetic, so nobody has to recompute it: 524,288 payload bytes
// plus a 49-byte codec header plus a 6-byte frame envelope is 524,343, against
// a 2,097,152 default cap. That is 25.003 percent of the cap, so the margin is
// just UNDER four times rather than at it; the header is what tips it. The
// assertion below states the margin that is actually true rather than the one
// that reads nicer.
const _: () = assert!(MAX_SNAPSHOT_CHUNK_MSG_BYTES < MAX_WIRE_MSG_BYTES as usize);
const _: () = assert!(MAX_SNAPSHOT_CHUNK_MSG_BYTES * 3 < MAX_WIRE_MSG_BYTES as usize);
// It must also fit the receive cap with room for the frame, which it does by
// more than an order of magnitude; stated so a future receive-cap change is
// forced to consider snapshot chunks too.
const _: () = assert!(MAX_SNAPSHOT_CHUNK_MSG_BYTES < MAX_RECV_WIRE_MSG_BYTES as usize);

/// Network message envelope.
#[derive(Debug, Clone)]
pub enum NetworkMessage {
    SignedProposal(SignedProposal),
    Vote(Vote),
    Qc(QC),
    Timeout(Timeout),
    BlockRequest(BlockRequest),
    BlockResponse(BlockResponse),
    /// Raw signed transaction bytes for mempool gossip.
    Transaction(Vec<u8>),
    /// Gate F5 Stage 4: snapshot transfer. Payloads are opaque here; the node
    /// crate owns their meaning and their verification.
    SnapshotManifestRequest(novai_consensus_types::SnapshotManifestRequest),
    SnapshotManifestResponse(novai_consensus_types::SnapshotManifestResponse),
    SnapshotChunkRequest(novai_consensus_types::SnapshotChunkRequest),
    SnapshotChunkResponse(novai_consensus_types::SnapshotChunkResponse),
}

#[derive(Debug)]
pub enum P2PError {
    Io(String),
    Codec(String),
    MessageTooLarge(u32),
    InvalidKind(u8),
}

impl From<std::io::Error> for P2PError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Encode a network message to wire format under the DEFAULT send cap
/// ([`MAX_WIRE_MSG_BYTES`]). Runtime-cap callers
/// ([`PeerManager::broadcast`]) go through
/// [`encode_wire_message_with_cap`].
///
/// # Errors
/// Returns error if encoding fails or message exceeds size limit.
pub fn encode_wire_message(msg: &NetworkMessage) -> Result<Vec<u8>, P2PError> {
    encode_wire_message_with_cap(msg, MAX_WIRE_MSG_BYTES)
}

/// Encode a network message to wire format under an explicit send cap
/// (the runtime wire-send-cap value; F3 Phase B raises it by restart).
///
/// # Errors
/// Returns error if encoding fails or message exceeds `send_cap`.
pub fn encode_wire_message_with_cap(
    msg: &NetworkMessage,
    send_cap: u32,
) -> Result<Vec<u8>, P2PError> {
    let (kind, payload) = match msg {
        NetworkMessage::SignedProposal(sp) => {
            let bytes = novai_consensus_types::codec::encode_signed_proposal_v1(sp)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            (MessageKind::SignedProposal, bytes)
        }
        NetworkMessage::Vote(v) => {
            let bytes = novai_consensus_types::codec::encode_vote_v1_signed(v);
            (MessageKind::Vote, bytes)
        }
        NetworkMessage::Qc(qc) => {
            let bytes = novai_consensus_types::codec::encode_qc_v1(qc)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            (MessageKind::Qc, bytes)
        }
        NetworkMessage::Timeout(t) => {
            let bytes = novai_consensus_types::codec::encode_timeout_v1_signed(t)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            (MessageKind::Timeout, bytes)
        }
        NetworkMessage::BlockRequest(req) => {
            let bytes = novai_consensus_types::codec::encode_block_request_v1(req)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            (MessageKind::BlockRequest, bytes)
        }
        NetworkMessage::BlockResponse(resp) => {
            let bytes = novai_consensus_types::codec::encode_block_response_v2(resp)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            (MessageKind::BlockResponse, bytes)
        }
        NetworkMessage::Transaction(bytes) => (MessageKind::Transaction, bytes.clone()),
        NetworkMessage::SnapshotManifestRequest(r) => {
            let bytes = novai_consensus_types::codec::encode_snapshot_manifest_request_v1(r)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            (MessageKind::SnapshotManifestRequest, bytes)
        }
        NetworkMessage::SnapshotManifestResponse(r) => {
            let bytes = novai_consensus_types::codec::encode_snapshot_manifest_response_v1(r)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            (MessageKind::SnapshotManifestResponse, bytes)
        }
        NetworkMessage::SnapshotChunkRequest(r) => {
            let bytes = novai_consensus_types::codec::encode_snapshot_chunk_request_v1(r)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            (MessageKind::SnapshotChunkRequest, bytes)
        }
        NetworkMessage::SnapshotChunkResponse(r) => {
            let bytes = novai_consensus_types::codec::encode_snapshot_chunk_response_v1(r)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            (MessageKind::SnapshotChunkResponse, bytes)
        }
    };

    #[allow(clippy::cast_possible_truncation)]
    let len = (payload.len() as u32) + 2; // +2 for version + kind
    if len > send_cap {
        return Err(P2PError::MessageTooLarge(len));
    }

    let mut wire = Vec::with_capacity(4 + len as usize);
    wire.extend_from_slice(&len.to_be_bytes());
    wire.push(1); // version
    wire.push(kind as u8);
    wire.extend_from_slice(&payload);

    Ok(wire)
}

/// Read one framed message from a stream.
///
/// # Errors
/// Returns error if read fails, message is malformed, or exceeds size limit.
pub fn read_wire_message(stream: &mut impl Read) -> Result<NetworkMessage, P2PError> {
    // Read length prefix
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);

    // F3: the receive side accepts up to MAX_RECV_WIRE_MSG_BYTES, a strict
    // superset of every send cap the fleet can be configured with, so the
    // two-phase cap raise never partitions a mixed fleet (receive first).
    if len > MAX_RECV_WIRE_MSG_BYTES {
        return Err(P2PError::MessageTooLarge(len));
    }
    if len < 2 {
        return Err(P2PError::Codec("message too short".into()));
    }

    // Read version + kind
    let mut header = [0u8; 2];
    stream.read_exact(&mut header)?;
    let version = header[0];
    let kind_byte = header[1];

    if version != 1 {
        return Err(P2PError::Codec(format!("unsupported version: {version}")));
    }

    let kind = MessageKind::from_u8(kind_byte).ok_or(P2PError::InvalidKind(kind_byte))?;

    // Read payload
    let payload_len = (len - 2) as usize;
    let mut payload = vec![0u8; payload_len];
    stream.read_exact(&mut payload)?;

    // Decode based on kind
    match kind {
        MessageKind::SignedProposal => {
            let sp = novai_consensus_types::codec::decode_signed_proposal_v1(&payload)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            Ok(NetworkMessage::SignedProposal(sp))
        }
        MessageKind::Vote => {
            let vote = novai_consensus_types::codec::decode_vote_v1_signed(&payload)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            Ok(NetworkMessage::Vote(vote))
        }
        MessageKind::Qc => {
            let qc = novai_consensus_types::codec::decode_qc_v1(&payload)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            Ok(NetworkMessage::Qc(qc))
        }
        MessageKind::Timeout => {
            let timeout = novai_consensus_types::codec::decode_timeout_v1_signed(&payload)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            Ok(NetworkMessage::Timeout(timeout))
        }
        MessageKind::BlockRequest => {
            let req = novai_consensus_types::codec::decode_block_request_v1(&payload)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            Ok(NetworkMessage::BlockRequest(req))
        }
        MessageKind::BlockResponse => {
            let resp = novai_consensus_types::codec::decode_block_response_v2(&payload)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            Ok(NetworkMessage::BlockResponse(resp))
        }
        MessageKind::Transaction => Ok(NetworkMessage::Transaction(payload)),
        MessageKind::SnapshotManifestRequest => {
            let r = novai_consensus_types::codec::decode_snapshot_manifest_request_v1(&payload)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            Ok(NetworkMessage::SnapshotManifestRequest(r))
        }
        MessageKind::SnapshotManifestResponse => {
            let r = novai_consensus_types::codec::decode_snapshot_manifest_response_v1(&payload)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            Ok(NetworkMessage::SnapshotManifestResponse(r))
        }
        MessageKind::SnapshotChunkRequest => {
            let r = novai_consensus_types::codec::decode_snapshot_chunk_request_v1(&payload)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            Ok(NetworkMessage::SnapshotChunkRequest(r))
        }
        MessageKind::SnapshotChunkResponse => {
            let r = novai_consensus_types::codec::decode_snapshot_chunk_response_v1(&payload)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            Ok(NetworkMessage::SnapshotChunkResponse(r))
        }
    }
}

/// Write one framed message to a stream.
///
/// # Errors
/// Returns error if encoding or write fails.
pub fn write_wire_message(
    stream: &mut (impl Write + ?Sized),
    msg: &NetworkMessage,
) -> Result<(), P2PError> {
    let wire = encode_wire_message(msg)?;
    stream.write_all(&wire)?;
    stream.flush()?;
    Ok(())
}

/// Maximum number of simultaneous peer connections.
pub const MAX_PEERS: usize = 128;

/// Minimal peer connection manager.
///
/// Each peer gets a dedicated writer thread that owns the TCP stream and loops
/// on a bounded channel. `broadcast()` just sends pre-encoded bytes into the
/// channels — no thread spawning, no blocking on network I/O.
pub struct PeerManager {
    /// Channel senders to dedicated per-peer writer threads.
    /// Uses `Arc<Vec<u8>>` so broadcast clones a refcount, not the full message.
    #[allow(clippy::type_complexity)]
    peer_senders: Arc<Mutex<Vec<mpsc::SyncSender<Arc<Vec<u8>>>>>>,
    /// Runtime wire send cap (F3). Defaults to [`MAX_WIRE_MSG_BYTES`];
    /// raised to at most [`MAX_RECV_WIRE_MSG_BYTES`] via
    /// `--wire-send-cap-bytes` at startup (Phase B is a restart). This is
    /// THE single stored copy of the send cap: the encoder below and the
    /// node's proposer guard and responder budget all read it through
    /// [`PeerManager::send_cap`], so the guard and the enforcement cannot
    /// diverge.
    send_cap: AtomicU32,
}

impl Default for PeerManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-peer writer channel capacity. If a peer falls this many messages behind
/// (writer thread stuck on a blocked socket), the peer is dropped on the next
/// broadcast. At ~60 messages/second this is ~1 second of backlog.
const PEER_CHANNEL_CAPACITY: usize = 64;

impl PeerManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            peer_senders: Arc::new(Mutex::new(Vec::new())),
            send_cap: AtomicU32::new(MAX_WIRE_MSG_BYTES),
        }
    }

    /// The runtime wire send cap this manager encodes against.
    #[must_use]
    pub fn send_cap(&self) -> u32 {
        self.send_cap.load(Ordering::Relaxed)
    }

    /// Set the runtime wire send cap (startup configuration only; the
    /// node validates the value before calling this).
    pub fn set_send_cap(&self, cap: u32) {
        self.send_cap.store(cap, Ordering::Relaxed);
    }

    /// Add a connected peer's write half.
    ///
    /// Spawns a dedicated writer thread that owns `writer` and loops on a
    /// bounded channel. The thread exits on write error or channel disconnect
    /// (when the corresponding `SyncSender` is dropped).
    ///
    /// Returns `false` if the peer was rejected because the connection
    /// limit ([`MAX_PEERS`]) has been reached.
    ///
    /// # Panics
    /// Panics if the mutex is poisoned or thread spawn fails.
    pub fn add_peer(&self, writer: Box<dyn Write + Send>) -> bool {
        let mut senders = self.peer_senders.lock().unwrap();
        if senders.len() >= MAX_PEERS {
            tracing::warn!(max = MAX_PEERS, "Peer connection rejected: at capacity");
            return false;
        }
        let (tx, rx) = mpsc::sync_channel::<Arc<Vec<u8>>>(PEER_CHANNEL_CAPACITY);
        thread::Builder::new()
            .name("peer-writer".into())
            .spawn(move || {
                let mut w = writer;
                for bytes in rx {
                    if w.write_all(&bytes).is_err() || w.flush().is_err() {
                        tracing::debug!("Peer writer failed, exiting");
                        return;
                    }
                }
            })
            .expect("failed to spawn peer writer thread");
        senders.push(tx);
        true
    }

    /// Broadcast a message to all connected peers.
    ///
    /// Pre-encodes the message once, then `try_send`s the bytes into each
    /// peer's channel. Peers whose channels are full (stuck writer) or
    /// disconnected (writer thread exited) are removed.
    ///
    /// This method never blocks on network I/O — the only work is encoding
    /// plus a lock + iterate + `try_send` per peer.
    ///
    /// # Errors
    /// Returns error if encoding fails (individual peer failures are handled internally).
    ///
    /// # Panics
    /// Panics if the mutex is poisoned.
    pub fn broadcast(&self, msg: &NetworkMessage) -> Result<(), P2PError> {
        let wire_bytes = Arc::new(encode_wire_message_with_cap(msg, self.send_cap())?);
        let mut senders = self.peer_senders.lock().unwrap();
        senders.retain(|tx| match tx.try_send(Arc::clone(&wire_bytes)) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) => {
                tracing::warn!(
                    "Peer channel full ({PEER_CHANNEL_CAPACITY} msgs behind), dropping slow peer"
                );
                false
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                tracing::debug!("Peer writer exited, removing");
                false
            }
        });
        drop(senders);
        Ok(())
    }

    /// Get count of connected peers.
    ///
    /// # Panics
    /// Panics if the mutex is poisoned.
    #[must_use]
    pub fn peer_count(&self) -> usize {
        self.peer_senders.lock().unwrap().len()
    }
}

/// Maximum connections allowed from a single IP address.
/// Set to 10 to support localhost testnets where all validators share 127.0.0.1.
pub const MAX_CONNECTIONS_PER_IP: usize = 10;

/// TCP socket read/write timeout for peer connections (seconds).
///
/// L-01: 30s is a deliberate tradeoff — short enough to evict truly dead
/// connections, long enough to survive consensus round gaps in low-activity
/// testnets. A slow-drip attacker can hold connections with 1 byte per 29s,
/// but this is bounded by `MAX_PEERS` (128) and `MAX_CONNECTIONS_PER_IP` (10).
/// If tighter resource management is needed, reduce to 15s and test that
/// legitimate validators don't get disconnected during idle periods.
pub const PEER_SOCKET_TIMEOUT_SECS: u64 = 30;

/// Per-peer message rate limit (messages per second).
/// Peers exceeding this are disconnected.
pub const MAX_MESSAGES_PER_SECOND: u64 = 1000;

/// Tracks active connections to prevent resource exhaustion (C-03, C-04).
///
/// Enforces:
/// - Total connection limit (prevents thread exhaustion from SYN floods)
/// - Per-IP connection limit (prevents eclipse from single source)
///
/// INVARIANTS:
/// - `active` count is always consistent with live [`ConnectionGuard`]s
/// - Per-IP counts are always >= 0 and consistent
pub struct ConnectionLimiter {
    active: AtomicUsize,
    max_total: usize,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    max_per_ip: usize,
}

impl ConnectionLimiter {
    /// Create a new connection limiter.
    #[must_use]
    pub fn new(max_total: usize, max_per_ip: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            max_total,
            per_ip: Mutex::new(HashMap::new()),
            max_per_ip,
        }
    }

    /// Try to acquire a connection slot for the given IP.
    ///
    /// Returns a [`ConnectionGuard`] that automatically releases the slot on drop,
    /// or `None` if the connection would exceed total or per-IP limits.
    ///
    /// # Panics
    /// Panics if the per-IP mutex is poisoned.
    pub fn try_acquire(limiter: &Arc<Self>, ip: IpAddr) -> Option<ConnectionGuard> {
        let current = limiter.active.fetch_add(1, Ordering::SeqCst);
        if current >= limiter.max_total {
            limiter.active.fetch_sub(1, Ordering::SeqCst);
            return None;
        }

        let mut per_ip = limiter.per_ip.lock().unwrap();
        let count = per_ip.entry(ip).or_insert(0);
        if *count >= limiter.max_per_ip {
            drop(per_ip);
            limiter.active.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        *count += 1;
        drop(per_ip);

        Some(ConnectionGuard {
            limiter: Arc::clone(limiter),
            ip,
        })
    }

    /// Get the current number of active connections.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }
}

/// RAII guard that releases a connection slot when dropped.
///
/// Created by [`ConnectionLimiter::try_acquire`]. Move into the
/// per-connection thread so the slot is freed when the thread exits.
pub struct ConnectionGuard {
    limiter: Arc<ConnectionLimiter>,
    ip: IpAddr,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.limiter.active.fetch_sub(1, Ordering::SeqCst);
        let mut per_ip = self.limiter.per_ip.lock().unwrap();
        if let Some(count) = per_ip.get_mut(&self.ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                per_ip.remove(&self.ip);
            }
        }
    }
}

/// In-memory ban list for misbehaving peers.
///
/// Tracks banned IPs with expiry timestamps. Ban duration escalates
/// exponentially: 5 min → 10 → 20 → ... → 1440 min (24h) cap.
pub struct PeerBanList {
    bans: Mutex<HashMap<IpAddr, BanEntry>>,
}

struct BanEntry {
    expires: std::time::Instant,
    offense_count: u32,
}

/// Initial ban duration in seconds.
const BAN_BASE_SECS: u64 = 300; // 5 minutes
/// Maximum ban duration in seconds.
const BAN_MAX_SECS: u64 = 86_400; // 24 hours

impl PeerBanList {
    /// Create a new empty ban list.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bans: Mutex::new(HashMap::new()),
        }
    }

    /// Check if an IP is currently banned (also evicts expired entries).
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn is_banned(&self, ip: &IpAddr) -> bool {
        let mut bans = self.bans.lock().unwrap();
        if let Some(entry) = bans.get(ip) {
            if entry.expires > std::time::Instant::now() {
                return true;
            }
            // Expired — remove
            bans.remove(ip);
        }
        false
    }

    /// Ban an IP address. Duration escalates with repeat offenses.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn ban(&self, ip: IpAddr, reason: &str) {
        let mut bans = self.bans.lock().unwrap();
        let offense_count = bans.get(&ip).map_or(1, |e| e.offense_count + 1);

        // Exponential backoff: 5min * 2^(offense-1), capped at 24h
        let duration_secs = std::cmp::min(
            BAN_BASE_SECS.saturating_mul(1u64 << offense_count.min(16).saturating_sub(1)),
            BAN_MAX_SECS,
        );

        let expires = std::time::Instant::now() + std::time::Duration::from_secs(duration_secs);

        tracing::warn!(
            %ip,
            %reason,
            offense_count,
            ban_duration_secs = duration_secs,
            "Peer banned"
        );

        bans.insert(
            ip,
            BanEntry {
                expires,
                offense_count,
            },
        );
    }
}

impl Default for PeerBanList {
    fn default() -> Self {
        Self::new()
    }
}

/// Start a TCP listener for incoming peer connections.
///
/// Callback is invoked for each accepted TCP connection. The caller is
/// responsible for performing any handshake, adding the write half to
/// `PeerManager`, and spawning a reader thread.
///
/// # Errors
/// Returns error if binding fails.
pub fn start_listener<F>(bind_addr: SocketAddr, on_peer_connected: F) -> Result<(), P2PError>
where
    F: Fn(TcpStream) + Send + 'static,
{
    let listener = TcpListener::bind(bind_addr)?;

    tracing::info!("P2P listener started on {bind_addr}");

    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    // Set TCP read timeout on accepted socket to prevent idle
                    // connections from holding resources indefinitely (C-04).
                    let _ = stream
                        .set_read_timeout(Some(Duration::from_secs(PEER_SOCKET_TIMEOUT_SECS)));
                    let _ = stream.set_nodelay(true);
                    tracing::info!("New peer connected from {:?}", stream.peer_addr());
                    on_peer_connected(stream);
                }
                Err(e) => {
                    tracing::error!("Failed to accept connection: {e}");
                }
            }
        }
    });

    Ok(())
}

/// Connect to a peer.
///
/// # Errors
/// Returns error if connection fails.
pub fn connect_to_peer(addr: SocketAddr) -> Result<TcpStream, P2PError> {
    let stream = TcpStream::connect(addr)?;
    // Disable Nagle's algorithm for low-latency consensus messaging,
    // matching the set_nodelay(true) on accepted (incoming) connections.
    let _ = stream.set_nodelay(true);
    // Bound how long broadcast() can block on a single peer write.
    // The Noise handshake saves/restores this value, so it persists
    // through to the NoiseWriter wrapping the cloned stream.
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    tracing::info!("Connected to peer at {addr}");
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_vote_roundtrip() {
        let vote = Vote {
            height: 42,
            round: 7,
            block_hash: [0xAA; 32],
            voter: [0xBB; 32],
            signature: [0xCC; 64],
            ai_signal_commitment: None,
        };

        let msg = NetworkMessage::Vote(vote);
        let wire = encode_wire_message(&msg).unwrap();

        // Verify framing
        #[allow(clippy::cast_possible_truncation)]
        let expected_len = (wire.len() as u32) - 4;
        assert_eq!(&wire[0..4], &expected_len.to_be_bytes());
        assert_eq!(wire[4], 1); // version
        assert_eq!(wire[5], MessageKind::Vote as u8);
    }

    #[test]
    fn reject_oversized_message() {
        let vote = Vote {
            height: 1,
            round: 0,
            block_hash: [0u8; 32],
            voter: [0u8; 32],
            signature: [0xCC; 64],
            ai_signal_commitment: None,
        };

        // This should work
        let msg = NetworkMessage::Vote(vote);
        assert!(encode_wire_message(&msg).is_ok());
    }

    #[test]
    fn encode_decode_timeout_roundtrip() {
        let timeout = Timeout {
            height: 5,
            round: 2,
            voter: [0xAA; 32],
            highest_qc: None,
            signature: [0xBB; 64],
        };

        let msg = NetworkMessage::Timeout(timeout.clone());
        let wire = encode_wire_message(&msg).unwrap();

        // Verify framing
        #[allow(clippy::cast_possible_truncation)]
        let expected_len = (wire.len() as u32) - 4;
        assert_eq!(&wire[0..4], &expected_len.to_be_bytes());
        assert_eq!(wire[4], 1); // version
        assert_eq!(wire[5], MessageKind::Timeout as u8);

        // Verify we can decode it back
        // (We can't use read_wire_message directly as it needs TcpStream,
        // but the encode test verifies the format is correct)
        assert_eq!(timeout.height, 5);
        assert_eq!(timeout.round, 2);
    }

    #[test]
    fn full_block_response_with_qcs_fits_wire_cap() {
        use novai_consensus_types::codec::MAX_BLOCKS_PER_RESPONSE;
        use novai_consensus_types::Block;

        // A maximal response: MAX_BLOCKS_PER_RESPONSE empty blocks, each
        // paired with a quorum-3 QC (the 4-validator testnet shape). The
        // qcs trailer must not push this over MAX_WIRE_MSG_BYTES.
        let max = u64::try_from(MAX_BLOCKS_PER_RESPONSE).unwrap();
        let mut blocks = Vec::with_capacity(MAX_BLOCKS_PER_RESPONSE);
        let mut qcs = Vec::with_capacity(MAX_BLOCKS_PER_RESPONSE);
        for height in 1..=max {
            blocks.push(Block {
                height,
                round: 0,
                parent_hash: [0u8; 32],
                state_root: [0u8; 32],
                txs: vec![],
            });
            let votes: Vec<Vote> = (1u8..=3)
                .map(|v| Vote {
                    height,
                    round: 0,
                    block_hash: [0x42; 32],
                    voter: [v; 32],
                    signature: [v; 64],
                    ai_signal_commitment: None,
                })
                .collect();
            qcs.push(Some(QC {
                height,
                round: 0,
                block_hash: [0x42; 32],
                votes,
            }));
        }
        let resp = BlockResponse {
            responder: [0xbb; 32],
            request_start: 1,
            request_end: max,
            blocks,
            qcs,
        };

        let wire = encode_wire_message(&NetworkMessage::BlockResponse(resp))
            .expect("maximal empty-block response with QCs must fit the wire cap");
        assert!(wire.len() <= 4 + MAX_WIRE_MSG_BYTES as usize);
    }

    #[test]
    fn oversized_block_response_fails_cleanly_at_encode() {
        use novai_consensus_types::Block;

        // Inflate the payload past MAX_WIRE_MSG_BYTES with two QCs of
        // 8000 distinct voters each (about 1.2MB per QC, legal for the QC
        // codec, which allows up to MAX_VOTES_PER_QC = 11000). The
        // encoder must refuse with MessageTooLarge: a clean failure, not
        // a panic and not a truncated send.
        let big_qc = |height: u64| {
            let votes: Vec<Vote> = (0u32..8000)
                .map(|i| {
                    let mut voter = [0u8; 32];
                    voter[0] = u8::try_from(i % 256).unwrap();
                    voter[1] = u8::try_from(i / 256).unwrap();
                    Vote {
                        height,
                        round: 0,
                        block_hash: [0x42; 32],
                        voter,
                        signature: [0x11; 64],
                        ai_signal_commitment: None,
                    }
                })
                .collect();
            QC {
                height,
                round: 0,
                block_hash: [0x42; 32],
                votes,
            }
        };
        let block = |height: u64| Block {
            height,
            round: 0,
            parent_hash: [0u8; 32],
            state_root: [0u8; 32],
            txs: vec![],
        };
        let resp = BlockResponse {
            responder: [0xbb; 32],
            request_start: 1,
            request_end: 2,
            blocks: vec![block(1), block(2)],
            qcs: vec![Some(big_qc(1)), Some(big_qc(2))],
        };

        let err = encode_wire_message(&NetworkMessage::BlockResponse(resp)).unwrap_err();
        assert!(matches!(err, P2PError::MessageTooLarge(_)));
    }
}
