//! Noise Protocol Framework transport encryption for validator-to-validator TCP.
//!
//! Uses `Noise_XX_25519_ChaChaPoly_SHA256` (mutual authentication, 3-message handshake).
//!
//! INVARIANTS:
//! - Nonce counters are independent per direction (send vs recv) and never reused.
//! - Handshake completes on the original `TcpStream` BEFORE cloning into reader/writer.
//! - Remote static key is verified against the known validator set after handshake.
//!
//! FAILURE MODES:
//! - Handshake timeout (10s) → connection dropped, logged as warning.
//! - Unknown remote key → connection rejected with log.
//! - Nonce overflow → panic (unreachable in practice: 2^64 messages).

use sha2::{Digest, Sha256};
use snow::params::NoiseParams;
use snow::StatelessTransportState;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use crate::P2PError;

/// Noise protocol pattern.
const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_SHA256";

/// Handshake timeout in seconds.
const HANDSHAKE_TIMEOUT_SECS: u64 = 10;

/// Maximum Noise transport message size (ciphertext including 16-byte Poly1305 tag).
const NOISE_MAX_MSG_LEN: usize = 65535;

/// Maximum plaintext chunk we can encrypt into one Noise transport message.
/// 65535 minus 16 bytes for the Poly1305 authentication tag.
const NOISE_MAX_PLAINTEXT_CHUNK: usize = NOISE_MAX_MSG_LEN - 16;

/// Derive an X25519 static key from an Ed25519 seed via SHA-256.
///
/// This is a one-way derivation: `SHA-256(ed25519_seed)` clamped to a valid
/// X25519 scalar by the Noise library internally.
#[must_use]
pub fn noise_keypair_from_seed(ed25519_seed: &[u8; 32]) -> [u8; 32] {
    let hash = Sha256::digest(ed25519_seed);
    let mut key = [0u8; 32];
    key.copy_from_slice(&hash);
    key
}

/// Encrypted writer half of a Noise-wrapped TCP connection.
///
/// Chunks plaintext into ≤`NOISE_MAX_PLAINTEXT_CHUNK`-byte segments,
/// encrypts each, and frames as `[chunk_len: u16 BE][ciphertext]`.
pub struct NoiseWriter {
    stream: TcpStream,
    transport: Arc<StatelessTransportState>,
    send_nonce: u64,
}

impl NoiseWriter {
    const fn new(stream: TcpStream, transport: Arc<StatelessTransportState>) -> Self {
        Self {
            stream,
            transport,
            send_nonce: 0,
        }
    }

    /// Encrypt and write a single chunk.
    fn write_chunk(&mut self, plaintext: &[u8]) -> io::Result<()> {
        debug_assert!(
            plaintext.len() <= NOISE_MAX_PLAINTEXT_CHUNK,
            "chunk exceeds Noise max plaintext size"
        );

        let mut ciphertext = vec![0u8; plaintext.len() + 16]; // +16 for Poly1305 tag
        let len = self
            .transport
            .write_message(self.send_nonce, plaintext, &mut ciphertext)
            .map_err(|e| io::Error::other(format!("noise encrypt: {e}")))?;

        // Safety net: nonce must never wrap
        self.send_nonce = self.send_nonce.checked_add(1).unwrap_or_else(|| {
            panic!("NoiseWriter send nonce overflow — this should never happen");
        });

        // Frame: [chunk_len: u16 BE][ciphertext]
        #[allow(clippy::cast_possible_truncation)]
        let frame_len = len as u16;
        self.stream.write_all(&frame_len.to_be_bytes())?;
        self.stream.write_all(&ciphertext[..len])?;
        Ok(())
    }
}

impl Write for NoiseWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        let mut written = 0;
        for chunk in buf.chunks(NOISE_MAX_PLAINTEXT_CHUNK) {
            self.write_chunk(chunk)?;
            written += chunk.len();
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// Encrypted reader half of a Noise-wrapped TCP connection.
///
/// Reads framed chunks `[chunk_len: u16 BE][ciphertext]`, decrypts each,
/// and buffers plaintext for the caller.
pub struct NoiseReader {
    stream: TcpStream,
    transport: Arc<StatelessTransportState>,
    recv_nonce: u64,
    /// Buffered decrypted plaintext from the last chunk.
    buf: Vec<u8>,
    /// Current read position within `buf`.
    buf_pos: usize,
}

impl NoiseReader {
    const fn new(stream: TcpStream, transport: Arc<StatelessTransportState>) -> Self {
        Self {
            stream,
            transport,
            recv_nonce: 0,
            buf: Vec::new(),
            buf_pos: 0,
        }
    }

    /// Read and decrypt one chunk into the internal buffer.
    fn read_chunk(&mut self) -> io::Result<()> {
        let mut len_buf = [0u8; 2];
        self.stream.read_exact(&mut len_buf)?;
        let chunk_len = u16::from_be_bytes(len_buf) as usize;

        if chunk_len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "noise: zero-length chunk",
            ));
        }

        if chunk_len > NOISE_MAX_MSG_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("noise: chunk too large ({chunk_len} > {NOISE_MAX_MSG_LEN})"),
            ));
        }

        let mut ciphertext = vec![0u8; chunk_len];
        self.stream.read_exact(&mut ciphertext)?;

        let mut plaintext = vec![0u8; chunk_len];
        let len = self
            .transport
            .read_message(self.recv_nonce, &ciphertext, &mut plaintext)
            .map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, format!("noise decrypt: {e}"))
            })?;

        self.recv_nonce = self.recv_nonce.checked_add(1).unwrap_or_else(|| {
            panic!("NoiseReader recv nonce overflow — this should never happen");
        });

        self.buf = plaintext;
        self.buf.truncate(len);
        self.buf_pos = 0;
        Ok(())
    }
}

impl Read for NoiseReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.buf_pos >= self.buf.len() {
            self.read_chunk()?;
        }

        let available = &self.buf[self.buf_pos..];
        let to_copy = available.len().min(buf.len());
        buf[..to_copy].copy_from_slice(&available[..to_copy]);
        self.buf_pos += to_copy;
        Ok(to_copy)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handshake helpers
// ─────────────────────────────────────────────────────────────────────────────

fn read_handshake_msg(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf)?;
    let len = u16::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}

fn write_handshake_msg(stream: &mut TcpStream, payload: &[u8]) -> io::Result<()> {
    #[allow(clippy::cast_possible_truncation)]
    let len = payload.len() as u16;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(payload)?;
    stream.flush()?;
    Ok(())
}

fn set_handshake_timeout(stream: &TcpStream) -> io::Result<(Option<Duration>, Option<Duration>)> {
    let prev_read = stream.read_timeout()?;
    let prev_write = stream.write_timeout()?;
    let timeout = Some(Duration::from_secs(HANDSHAKE_TIMEOUT_SECS));
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    Ok((prev_read, prev_write))
}

fn restore_timeout(
    stream: &TcpStream,
    prev: (Option<Duration>, Option<Duration>),
) -> io::Result<()> {
    stream.set_read_timeout(prev.0)?;
    stream.set_write_timeout(prev.1)?;
    Ok(())
}

fn build_noise(local_key: &[u8; 32]) -> Result<snow::Builder<'_>, P2PError> {
    let params: NoiseParams = NOISE_PATTERN
        .parse()
        .map_err(|e| P2PError::Codec(format!("noise params parse: {e}")))?;

    Ok(snow::Builder::new(params).local_private_key(local_key))
}

/// Result of a successful Noise handshake.
pub struct HandshakeResult {
    pub reader: NoiseReader,
    pub writer: NoiseWriter,
    /// The remote peer's X25519 static public key (32 bytes).
    pub remote_static_key: [u8; 32],
}

/// Perform Noise XX handshake as **initiator** (outgoing connection).
///
/// XX pattern (3 messages): → e | ← e, ee, s, es | → s, se
///
/// Handshake runs on the original `stream` with a 10-second timeout.
/// After completion, the stream is cloned into separate reader/writer halves.
///
/// # Errors
/// Returns error on timeout, protocol failure, or stream I/O error.
pub fn handshake_initiator(
    stream: &mut TcpStream,
    local_key: &[u8; 32],
) -> Result<HandshakeResult, P2PError> {
    let prev_timeouts = set_handshake_timeout(stream)?;
    let result = handshake_initiator_inner(stream, local_key);
    let _ = restore_timeout(stream, prev_timeouts);
    result
}

fn handshake_initiator_inner(
    stream: &mut TcpStream,
    local_key: &[u8; 32],
) -> Result<HandshakeResult, P2PError> {
    let builder = build_noise(local_key)?;
    let mut hs = builder
        .build_initiator()
        .map_err(|e| P2PError::Codec(format!("noise build initiator: {e}")))?;

    let mut buf = vec![0u8; NOISE_MAX_MSG_LEN];

    // Message 1: → e
    let len = hs
        .write_message(&[], &mut buf)
        .map_err(|e| P2PError::Codec(format!("noise hs msg1 write: {e}")))?;
    write_handshake_msg(stream, &buf[..len])?;

    // Message 2: ← e, ee, s, es
    let msg2 = read_handshake_msg(stream)?;
    hs.read_message(&msg2, &mut buf)
        .map_err(|e| P2PError::Codec(format!("noise hs msg2 read: {e}")))?;

    // Message 3: → s, se
    let len = hs
        .write_message(&[], &mut buf)
        .map_err(|e| P2PError::Codec(format!("noise hs msg3 write: {e}")))?;
    write_handshake_msg(stream, &buf[..len])?;

    finish_handshake(stream, hs)
}

/// Perform Noise XX handshake as **responder** (incoming connection).
///
/// XX pattern (3 messages): → e | ← e, ee, s, es | → s, se
///
/// # Errors
/// Returns error on timeout, protocol failure, or stream I/O error.
pub fn handshake_responder(
    stream: &mut TcpStream,
    local_key: &[u8; 32],
) -> Result<HandshakeResult, P2PError> {
    let prev_timeouts = set_handshake_timeout(stream)?;
    let result = handshake_responder_inner(stream, local_key);
    let _ = restore_timeout(stream, prev_timeouts);
    result
}

fn handshake_responder_inner(
    stream: &mut TcpStream,
    local_key: &[u8; 32],
) -> Result<HandshakeResult, P2PError> {
    let builder = build_noise(local_key)?;
    let mut hs = builder
        .build_responder()
        .map_err(|e| P2PError::Codec(format!("noise build responder: {e}")))?;

    let mut buf = vec![0u8; NOISE_MAX_MSG_LEN];

    // Message 1: → e
    let msg1 = read_handshake_msg(stream)?;
    hs.read_message(&msg1, &mut buf)
        .map_err(|e| P2PError::Codec(format!("noise hs msg1 read: {e}")))?;

    // Message 2: ← e, ee, s, es
    let len = hs
        .write_message(&[], &mut buf)
        .map_err(|e| P2PError::Codec(format!("noise hs msg2 write: {e}")))?;
    write_handshake_msg(stream, &buf[..len])?;

    // Message 3: → s, se
    let msg3 = read_handshake_msg(stream)?;
    hs.read_message(&msg3, &mut buf)
        .map_err(|e| P2PError::Codec(format!("noise hs msg3 read: {e}")))?;

    finish_handshake(stream, hs)
}

/// Common post-handshake: extract remote key, convert to transport, clone streams.
fn finish_handshake(
    stream: &TcpStream,
    hs: snow::HandshakeState,
) -> Result<HandshakeResult, P2PError> {
    let remote_static = extract_remote_static(&hs)?;

    let transport = hs
        .into_stateless_transport_mode()
        .map_err(|e| P2PError::Codec(format!("noise into transport: {e}")))?;
    let transport = Arc::new(transport);

    let read_stream = stream
        .try_clone()
        .map_err(|e| P2PError::Io(format!("clone stream for noise reader: {e}")))?;
    let write_stream = stream
        .try_clone()
        .map_err(|e| P2PError::Io(format!("clone stream for noise writer: {e}")))?;

    Ok(HandshakeResult {
        reader: NoiseReader::new(read_stream, Arc::clone(&transport)),
        writer: NoiseWriter::new(write_stream, transport),
        remote_static_key: remote_static,
    })
}

fn extract_remote_static(hs: &snow::HandshakeState) -> Result<[u8; 32], P2PError> {
    let remote = hs
        .get_remote_static()
        .ok_or_else(|| P2PError::Codec("noise: no remote static key after handshake".into()))?;

    if remote.len() != 32 {
        return Err(P2PError::Codec(format!(
            "noise: remote static key wrong length: {}",
            remote.len()
        )));
    }

    let mut key = [0u8; 32];
    key.copy_from_slice(remote);
    Ok(key)
}

/// Check if a remote X25519 static key belongs to any known validator.
///
/// `known_noise_keys` should be the set of X25519 keys derived from each validator's
/// Ed25519 seed via `noise_keypair_from_seed`.
#[must_use]
pub fn is_known_validator(remote_static: &[u8; 32], known_noise_keys: &[[u8; 32]]) -> bool {
    known_noise_keys.iter().any(|k| k == remote_static)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn noise_keypair_deterministic() {
        let seed = [42u8; 32];
        let k1 = noise_keypair_from_seed(&seed);
        let k2 = noise_keypair_from_seed(&seed);
        assert_eq!(k1, k2);
        assert_ne!(k1, [0u8; 32]);
    }

    #[test]
    fn noise_keypair_different_seeds() {
        let k1 = noise_keypair_from_seed(&[0u8; 32]);
        let k2 = noise_keypair_from_seed(&[1u8; 32]);
        assert_ne!(k1, k2);
    }

    #[test]
    fn handshake_roundtrip() {
        let initiator_key = noise_keypair_from_seed(&[0u8; 32]);
        let responder_key = noise_keypair_from_seed(&[1u8; 32]);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let rk = responder_key;
        let responder_handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake_responder(&mut stream, &rk).unwrap()
        });

        let mut stream = TcpStream::connect(addr).unwrap();
        let init_result = handshake_initiator(&mut stream, &initiator_key).unwrap();
        let resp_result = responder_handle.join().unwrap();

        // Each side got a 32-byte remote static key
        assert_ne!(init_result.remote_static_key, [0u8; 32]);
        assert_ne!(resp_result.remote_static_key, [0u8; 32]);
        // Initiator sees responder's key, responder sees initiator's key
        assert_ne!(init_result.remote_static_key, resp_result.remote_static_key);
    }

    #[test]
    fn encrypted_data_roundtrip() {
        let initiator_key = noise_keypair_from_seed(&[10u8; 32]);
        let responder_key = noise_keypair_from_seed(&[20u8; 32]);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let rk = responder_key;
        let responder_handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let result = handshake_responder(&mut stream, &rk).unwrap();
            let mut reader = result.reader;
            let mut buf = vec![0u8; 1024];
            let n = reader.read(&mut buf).unwrap();
            buf.truncate(n);
            buf
        });

        let mut stream = TcpStream::connect(addr).unwrap();
        let result = handshake_initiator(&mut stream, &initiator_key).unwrap();
        let mut writer = result.writer;

        let test_data = b"hello encrypted world";
        writer.write_all(test_data).unwrap();
        writer.flush().unwrap();

        // Drop all stream clones to trigger TCP close (EOF for responder)
        drop(writer);
        drop(result.reader);
        drop(stream);

        let received = responder_handle.join().unwrap();
        assert_eq!(&received, test_data);
    }

    #[test]
    fn encrypted_large_message_chunking() {
        let initiator_key = noise_keypair_from_seed(&[30u8; 32]);
        let responder_key = noise_keypair_from_seed(&[40u8; 32]);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        // 200KB — must be chunked across multiple Noise messages
        #[allow(clippy::cast_possible_truncation)]
        let large_data: Vec<u8> = (0u32..200_000).map(|i| (i % 256) as u8).collect();
        let expected = large_data.clone();

        let rk = responder_key;
        let responder_handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let result = handshake_responder(&mut stream, &rk).unwrap();
            let mut reader = result.reader;
            let mut received = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => received.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(e) => panic!("read error: {e}"),
                }
            }
            received
        });

        let mut stream = TcpStream::connect(addr).unwrap();
        let result = handshake_initiator(&mut stream, &initiator_key).unwrap();
        let mut writer = result.writer;

        writer.write_all(&large_data).unwrap();
        writer.flush().unwrap();

        // Drop all stream clones to trigger TCP close (EOF for responder)
        drop(writer);
        drop(result.reader);
        drop(stream);

        let received = responder_handle.join().unwrap();
        assert_eq!(received.len(), expected.len());
        assert_eq!(received, expected);
    }

    #[test]
    fn is_known_validator_matches() {
        let key_a = noise_keypair_from_seed(&[0u8; 32]);
        let key_b = noise_keypair_from_seed(&[1u8; 32]);

        let known = vec![key_a, key_b];
        assert!(is_known_validator(&key_a, &known));
        assert!(is_known_validator(&key_b, &known));
        assert!(!is_known_validator(&[99u8; 32], &known));
    }
}
