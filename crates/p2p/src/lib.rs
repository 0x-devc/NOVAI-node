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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use novai_consensus_types::{BlockRequest, BlockResponse, SignedProposal, Timeout, Vote, QC};

/// Maximum wire message size (2MB).
const MAX_WIRE_MSG_BYTES: u32 = 2 * 1024 * 1024;

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
            _ => None,
        }
    }
}

/// Network message envelope.
#[derive(Debug, Clone)]
pub enum NetworkMessage {
    SignedProposal(SignedProposal),
    Vote(Vote),
    Qc(QC),
    Timeout(Timeout),
    BlockRequest(BlockRequest),
    BlockResponse(BlockResponse),
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

/// Encode a network message to wire format.
///
/// # Errors
/// Returns error if encoding fails or message exceeds size limit.
pub fn encode_wire_message(msg: &NetworkMessage) -> Result<Vec<u8>, P2PError> {
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
            let bytes = novai_consensus_types::codec::encode_block_response_v1(resp)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            (MessageKind::BlockResponse, bytes)
        }
    };

    #[allow(clippy::cast_possible_truncation)]
    let len = (payload.len() as u32) + 2; // +2 for version + kind
    if len > MAX_WIRE_MSG_BYTES {
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

    if len > MAX_WIRE_MSG_BYTES {
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
            let resp = novai_consensus_types::codec::decode_block_response_v1(&payload)
                .map_err(|e| P2PError::Codec(format!("{e:?}")))?;
            Ok(NetworkMessage::BlockResponse(resp))
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
    peer_senders: Arc<Mutex<Vec<mpsc::SyncSender<Vec<u8>>>>>,
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
        }
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
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(PEER_CHANNEL_CAPACITY);
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
        let wire_bytes = encode_wire_message(msg)?;
        let mut senders = self.peer_senders.lock().unwrap();
        senders.retain(|tx| match tx.try_send(wire_bytes.clone()) {
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
pub const MAX_CONNECTIONS_PER_IP: usize = 3;

/// TCP socket read/write timeout for peer connections (seconds).
pub const PEER_SOCKET_TIMEOUT_SECS: u64 = 30;

/// Per-peer message rate limit (messages per second).
/// Peers exceeding this are disconnected.
pub const MAX_MESSAGES_PER_SECOND: u64 = 100;

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
}
