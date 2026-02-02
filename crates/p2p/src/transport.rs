//! Transport abstraction over plain TCP and Noise-encrypted streams.
//!
//! `PeerWriter` and `PeerReader` allow `PeerManager` and reader threads to
//! work transparently with both plaintext and encrypted connections.

use std::io::{self, Read, Write};
use std::net::TcpStream;

use crate::noise::{NoiseReader, NoiseWriter};

/// Write half of a peer connection (plain or encrypted).
pub enum PeerWriter {
    Plain(TcpStream),
    Encrypted(NoiseWriter),
}

impl Write for PeerWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Encrypted(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Encrypted(w) => w.flush(),
        }
    }
}

/// Read half of a peer connection (plain or encrypted).
pub enum PeerReader {
    Plain(TcpStream),
    Encrypted(NoiseReader),
}

impl Read for PeerReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Encrypted(r) => r.read(buf),
        }
    }
}

// Compile-time Send verification for cross-thread transfer.
const fn _assert_send<T: Send>() {}
const _: () = {
    let _ = _assert_send::<PeerWriter>;
    let _ = _assert_send::<PeerReader>;
};
