//! Unified error type for the NOVAI SDK.

use std::fmt;

/// SDK error type.
#[derive(Debug)]
pub enum Error {
    /// Key file I/O error.
    KeyFile(String),
    /// Cryptographic operation failed.
    Crypto(String),
    /// Transaction encoding failed.
    Codec(String),
    /// RPC request failed.
    Rpc(String),
    /// Invalid argument.
    InvalidArgument(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeyFile(msg) => write!(f, "key file error: {msg}"),
            Self::Crypto(msg) => write!(f, "crypto error: {msg}"),
            Self::Codec(msg) => write!(f, "codec error: {msg}"),
            Self::Rpc(msg) => write!(f, "RPC error: {msg}"),
            Self::InvalidArgument(msg) => write!(f, "invalid argument: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<novai_crypto::CryptoError> for Error {
    fn from(e: novai_crypto::CryptoError) -> Self {
        Self::Crypto(format!("{e:?}"))
    }
}

impl From<novai_codec::CodecError> for Error {
    fn from(e: novai_codec::CodecError) -> Self {
        Self::Codec(format!("{e:?}"))
    }
}
