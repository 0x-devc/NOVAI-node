//! novai-p2p: Minimal TCP-based networking for consensus messages.
//!
//! Wire format: [len: u32 be][version: u8][kind: u8][payload: len-2 bytes]
//!
//! Clean-room implementation using `std::net` (no external P2P libraries).
//! Suitable for localhost devnet and testing.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use novai_consensus_types::{SignedProposal, Vote, QC};

/// Maximum wire message size (2MB).
const MAX_WIRE_MSG_BYTES: u32 = 2 * 1024 * 1024;

/// Wire message kinds.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    SignedProposal = 1,
    Vote = 2,
    Qc = 3,
}

impl MessageKind {
    const fn from_u8(b: u8) -> Option<Self> {
        match b {
            1 => Some(Self::SignedProposal),
            2 => Some(Self::Vote),
            3 => Some(Self::Qc),
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

/// Read one framed message from a TCP stream.
///
/// # Errors
/// Returns error if read fails, message is malformed, or exceeds size limit.
pub fn read_wire_message(stream: &mut TcpStream) -> Result<NetworkMessage, P2PError> {
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
    }
}

/// Write one framed message to a TCP stream.
///
/// # Errors
/// Returns error if encoding or write fails.
pub fn write_wire_message(stream: &mut TcpStream, msg: &NetworkMessage) -> Result<(), P2PError> {
    let wire = encode_wire_message(msg)?;
    stream.write_all(&wire)?;
    stream.flush()?;
    Ok(())
}

/// Minimal peer connection manager.
pub struct PeerManager {
    peers: Arc<Mutex<Vec<TcpStream>>>,
}

impl Default for PeerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            peers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Add a connected peer.
    ///
    /// # Panics
    /// Panics if the mutex is poisoned.
    pub fn add_peer(&self, stream: TcpStream) {
        let mut peers = self.peers.lock().unwrap();
        peers.push(stream);
    }

    /// Broadcast a message to all connected peers.
    ///
    /// # Errors
    /// Returns error if encoding fails (individual peer write failures are handled internally).
    ///
    /// # Panics
    /// Panics if the mutex is poisoned.
    pub fn broadcast(&self, msg: &NetworkMessage) -> Result<(), P2PError> {
        self.peers.lock().unwrap().retain_mut(|stream| {
            if matches!(write_wire_message(stream, msg), Ok(())) {
                true
            } else {
                eprintln!("Peer disconnected, removing");
                false
            }
        });

        Ok(())
    }

    /// Get count of connected peers.
    ///
    /// # Panics
    /// Panics if the mutex is poisoned.
    #[must_use]
    pub fn peer_count(&self) -> usize {
        self.peers.lock().unwrap().len()
    }
}

/// Start a TCP listener for incoming peer connections.
/// Callback is invoked for each new connection.
///
/// # Errors
/// Returns error if binding fails.
///
/// # Panics
/// Panics if stream cloning fails.
pub fn start_listener<F>(
    bind_addr: SocketAddr,
    peer_manager: Arc<PeerManager>,
    on_peer_connected: F,
) -> Result<(), P2PError>
where
    F: Fn(TcpStream) + Send + 'static,
{
    let listener = TcpListener::bind(bind_addr)?;

    println!("P2P listener started on {bind_addr}");

    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    println!("New peer connected from {:?}", stream.peer_addr());

                    // Clone stream for peer manager
                    let stream_clone = stream.try_clone().expect("clone stream");
                    peer_manager.add_peer(stream_clone);

                    // Give stream to callback for reading
                    on_peer_connected(stream);
                }
                Err(e) => {
                    eprintln!("Failed to accept connection: {e}");
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
    println!("Connected to peer at {addr}");
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
        };

        // This should work
        let msg = NetworkMessage::Vote(vote);
        assert!(encode_wire_message(&msg).is_ok());
    }
}
