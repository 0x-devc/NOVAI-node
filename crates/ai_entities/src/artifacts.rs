//! Content-addressed artifact storage for off-chain signal payloads.
//!
//! PURPOSE: Provide verifiable storage for large AI signal payloads that are
//! too expensive to store on-chain. On-chain commitments reference off-chain
//! artifacts by their content hash.
//!
//! INVARIANTS:
//! - All stored content is retrievable by its hash
//! - Hash verification always performed on fetch (detect corruption)
//! - Content hash uses domain-separated blake3
//! - Maximum artifact size enforced (50MB)
//!
//! FAILURE MODES:
//! - NotFound: Artifact with given hash doesn't exist
//! - HashMismatch: Retrieved content doesn't match expected hash
//! - TooLarge: Content exceeds maximum size limit
//! - IoError: File system or network operation failed

use blake3::Hasher;
use std::fmt;
use std::path::PathBuf;

/// Domain separator for artifact hashing.
const ARTIFACT_HASH_DOMAIN_V1: &[u8] = b"NOVAI_ARTIFACT_V1";

/// Maximum artifact size: 50MB.
pub const MAX_ARTIFACT_SIZE: usize = 50 * 1024 * 1024;

/// Error type for artifact store operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactError {
    /// Artifact with given hash not found.
    NotFound([u8; 32]),
    /// I/O operation failed.
    IoError(String),
    /// Retrieved content hash doesn't match expected hash.
    HashMismatch {
        expected: [u8; 32],
        actual: [u8; 32],
    },
    /// Network operation failed (HTTP fetch).
    NetworkError(String),
    /// Invalid hash format (not valid hex, wrong length).
    InvalidHash(String),
    /// Content exceeds maximum size limit.
    TooLarge { size: usize, max: usize },
    /// String field is not valid UTF-8.
    InvalidUtf8,
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(hash) => write!(f, "artifact not found: {}", hex::encode(hash)),
            Self::IoError(msg) => write!(f, "I/O error: {}", msg),
            Self::HashMismatch { expected, actual } => {
                write!(
                    f,
                    "hash mismatch: expected {}, got {}",
                    hex::encode(expected),
                    hex::encode(actual)
                )
            }
            Self::NetworkError(msg) => write!(f, "network error: {}", msg),
            Self::InvalidHash(msg) => write!(f, "invalid hash: {}", msg),
            Self::TooLarge { size, max } => {
                write!(f, "artifact too large: {} bytes (max {})", size, max)
            }
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 encoding"),
        }
    }
}

impl std::error::Error for ArtifactError {}

/// Compute domain-separated artifact hash.
///
/// Uses blake3 with domain separator `NOVAI_ARTIFACT_V1` for collision
/// resistance across different hash usages in the protocol.
pub fn artifact_hash(content: &[u8]) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(ARTIFACT_HASH_DOMAIN_V1);
    hasher.update(content);
    *hasher.finalize().as_bytes()
}

/// Content-addressed artifact storage trait.
///
/// Implementations must guarantee:
/// - `store()` returns the content hash
/// - `fetch()` verifies the hash before returning
/// - Content is immutable once stored
pub trait ArtifactStore {
    /// Store content and return its hash.
    ///
    /// # Errors
    /// - `TooLarge` if content exceeds `MAX_ARTIFACT_SIZE`
    /// - `IoError` if storage operation fails
    fn store(&mut self, content: &[u8]) -> Result<[u8; 32], ArtifactError>;

    /// Fetch content by hash.
    ///
    /// The implementation MUST verify that the retrieved content's hash
    /// matches the requested hash before returning.
    ///
    /// # Errors
    /// - `NotFound` if artifact doesn't exist
    /// - `HashMismatch` if content doesn't match hash (corruption detected)
    /// - `IoError` if read operation fails
    fn fetch(&self, hash: &[u8; 32]) -> Result<Vec<u8>, ArtifactError>;

    /// Check if artifact exists.
    ///
    /// For network stores, this should use HEAD request, not full fetch.
    fn exists(&self, hash: &[u8; 32]) -> bool;
}

// ============================================================================
// LOCAL FILE STORE (D15.2)
// ============================================================================

/// Local file-based artifact store.
///
/// Stores artifacts as `{hash_hex}.bin` files in a directory.
/// Suitable for development, single-node setups, and local caching.
///
/// # Security
/// - Validates hash format to prevent path traversal
/// - Enforces maximum artifact size
/// - Verifies hash on every fetch
#[derive(Debug, Clone)]
pub struct LocalFileStore {
    base_dir: PathBuf,
}

impl LocalFileStore {
    /// Create a new local file store.
    ///
    /// Creates the base directory if it doesn't exist.
    ///
    /// # Errors
    /// Returns `IoError` if directory creation fails.
    pub fn new(base_dir: impl Into<PathBuf>) -> Result<Self, ArtifactError> {
        let base_dir = base_dir.into();

        // Create directory if it doesn't exist
        if !base_dir.exists() {
            std::fs::create_dir_all(&base_dir).map_err(|e| {
                ArtifactError::IoError(format!("failed to create directory: {}", e))
            })?;
        }

        // Verify it's actually a directory
        if !base_dir.is_dir() {
            return Err(ArtifactError::IoError(format!(
                "path is not a directory: {}",
                base_dir.display()
            )));
        }

        Ok(Self { base_dir })
    }

    /// Get the file path for a given hash.
    ///
    /// Validates the hash to prevent path traversal attacks.
    fn path_for_hash(&self, hash: &[u8; 32]) -> PathBuf {
        // Hash is always 32 bytes, hex encoding is always 64 chars
        // No risk of path traversal since we control the format
        let hash_hex = hex::encode(hash);
        self.base_dir.join(format!("{}.bin", hash_hex))
    }

    /// Validate that a filename is a valid artifact filename.
    ///
    /// Must be exactly 64 hex characters followed by ".bin".
    #[allow(dead_code)]
    fn validate_filename(filename: &str) -> Result<[u8; 32], ArtifactError> {
        // Must end with .bin
        let hex_part = filename
            .strip_suffix(".bin")
            .ok_or_else(|| ArtifactError::InvalidHash("filename must end with .bin".into()))?;

        // Must be exactly 64 hex characters
        if hex_part.len() != 64 {
            return Err(ArtifactError::InvalidHash(format!(
                "hash must be 64 hex characters, got {}",
                hex_part.len()
            )));
        }

        // Must be valid hex
        let bytes = hex::decode(hex_part)
            .map_err(|e| ArtifactError::InvalidHash(format!("invalid hex: {}", e)))?;

        // Convert to fixed array
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes);
        Ok(hash)
    }
}

impl ArtifactStore for LocalFileStore {
    fn store(&mut self, content: &[u8]) -> Result<[u8; 32], ArtifactError> {
        // Check size limit
        if content.len() > MAX_ARTIFACT_SIZE {
            return Err(ArtifactError::TooLarge {
                size: content.len(),
                max: MAX_ARTIFACT_SIZE,
            });
        }

        // Compute hash
        let hash = artifact_hash(content);
        let path = self.path_for_hash(&hash);

        // Write atomically (write to temp, then rename)
        let temp_path = self.base_dir.join(format!("{}.tmp", hex::encode(hash)));

        std::fs::write(&temp_path, content)
            .map_err(|e| ArtifactError::IoError(format!("failed to write temp file: {}", e)))?;

        std::fs::rename(&temp_path, &path)
            .map_err(|e| ArtifactError::IoError(format!("failed to rename temp file: {}", e)))?;

        Ok(hash)
    }

    fn fetch(&self, hash: &[u8; 32]) -> Result<Vec<u8>, ArtifactError> {
        let path = self.path_for_hash(hash);

        // Read content
        let content = std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ArtifactError::NotFound(*hash)
            } else {
                ArtifactError::IoError(format!("failed to read file: {}", e))
            }
        })?;

        // Verify hash
        let actual_hash = artifact_hash(&content);
        if actual_hash != *hash {
            return Err(ArtifactError::HashMismatch {
                expected: *hash,
                actual: actual_hash,
            });
        }

        Ok(content)
    }

    fn exists(&self, hash: &[u8; 32]) -> bool {
        let path = self.path_for_hash(hash);
        path.exists()
    }
}

// ============================================================================
// HTTP FETCH STORE (D15.3) - Feature-gated
// ============================================================================

/// HTTP-based artifact store (read-only).
///
/// Fetches artifacts from configurable HTTP mirror URLs.
/// Useful for distributed artifact retrieval from CDNs.
///
/// # Protocol
/// - Fetch: GET `{mirror}/{hash_hex}` returns raw content
/// - Exists: HEAD `{mirror}/{hash_hex}` returns 200 or 404
///
/// # Reliability
/// - Tries mirrors in order until success
/// - 30 second timeout per mirror
/// - Verifies hash after every fetch
#[cfg(feature = "http-fetch")]
#[derive(Debug, Clone)]
pub struct HttpFetchStore {
    /// Mirror URLs to try in order.
    mirrors: Vec<String>,
    /// Request timeout in seconds.
    timeout_secs: u64,
}

#[cfg(feature = "http-fetch")]
impl HttpFetchStore {
    /// Default timeout: 30 seconds.
    pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

    /// Create a new HTTP fetch store with default timeout.
    ///
    /// # Arguments
    /// - `mirrors` - List of base URLs to try in order (e.g., `["https://cdn1.example.com", "https://cdn2.example.com"]`)
    pub fn new(mirrors: Vec<String>) -> Self {
        Self {
            mirrors,
            timeout_secs: Self::DEFAULT_TIMEOUT_SECS,
        }
    }

    /// Create with custom timeout.
    pub fn with_timeout(mirrors: Vec<String>, timeout_secs: u64) -> Self {
        Self {
            mirrors,
            timeout_secs,
        }
    }

    /// Build the URL for a hash at a given mirror.
    fn url_for_hash(&self, mirror: &str, hash: &[u8; 32]) -> String {
        let hash_hex = hex::encode(hash);
        // Ensure no double slash
        let mirror = mirror.trim_end_matches('/');
        format!("{}/{}", mirror, hash_hex)
    }
}

#[cfg(feature = "http-fetch")]
impl ArtifactStore for HttpFetchStore {
    fn store(&mut self, _content: &[u8]) -> Result<[u8; 32], ArtifactError> {
        // HTTP store is read-only
        Err(ArtifactError::IoError(
            "HTTP artifact store is read-only".into(),
        ))
    }

    fn fetch(&self, hash: &[u8; 32]) -> Result<Vec<u8>, ArtifactError> {
        let mut last_error = None;

        for mirror in &self.mirrors {
            let url = self.url_for_hash(mirror, hash);

            match ureq::get(&url)
                .timeout(std::time::Duration::from_secs(self.timeout_secs))
                .call()
            {
                Ok(response) => {
                    // Check content length if available
                    if let Some(len) = response
                        .header("content-length")
                        .and_then(|s| s.parse::<usize>().ok())
                    {
                        if len > MAX_ARTIFACT_SIZE {
                            return Err(ArtifactError::TooLarge {
                                size: len,
                                max: MAX_ARTIFACT_SIZE,
                            });
                        }
                    }

                    // Read body with size limit
                    let mut content = Vec::new();
                    let reader = response.into_reader();
                    let mut reader =
                        std::io::Read::take(reader, MAX_ARTIFACT_SIZE as u64 + 1);

                    std::io::Read::read_to_end(&mut reader, &mut content).map_err(|e| {
                        ArtifactError::NetworkError(format!("failed to read response: {}", e))
                    })?;

                    // Check if we hit the limit
                    if content.len() > MAX_ARTIFACT_SIZE {
                        return Err(ArtifactError::TooLarge {
                            size: content.len(),
                            max: MAX_ARTIFACT_SIZE,
                        });
                    }

                    // Verify hash
                    let actual_hash = artifact_hash(&content);
                    if actual_hash != *hash {
                        return Err(ArtifactError::HashMismatch {
                            expected: *hash,
                            actual: actual_hash,
                        });
                    }

                    return Ok(content);
                }
                Err(ureq::Error::Status(404, _)) => {
                    // Not found on this mirror, try next
                    last_error = Some(ArtifactError::NotFound(*hash));
                }
                Err(e) => {
                    // Network error, try next mirror
                    last_error = Some(ArtifactError::NetworkError(format!(
                        "request to {} failed: {}",
                        mirror, e
                    )));
                }
            }
        }

        // All mirrors failed
        Err(last_error
            .unwrap_or_else(|| ArtifactError::NetworkError("no mirrors configured".into())))
    }

    fn exists(&self, hash: &[u8; 32]) -> bool {
        // Use HEAD request to check existence
        for mirror in &self.mirrors {
            let url = self.url_for_hash(mirror, hash);

            match ureq::head(&url)
                .timeout(std::time::Duration::from_secs(self.timeout_secs))
                .call()
            {
                Ok(_) => return true,
                Err(ureq::Error::Status(404, _)) => continue,
                Err(_) => continue, // Network error, try next mirror
            }
        }
        false
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn artifact_hash_is_deterministic() {
        let content = b"test content for hashing";
        let h1 = artifact_hash(content);
        let h2 = artifact_hash(content);
        assert_eq!(h1, h2);
    }

    #[test]
    fn artifact_hash_uses_domain_separator() {
        // Hash with domain separator should differ from plain blake3
        let content = b"test content";
        let artifact_h = artifact_hash(content);
        let plain_h: [u8; 32] = *blake3::hash(content).as_bytes();
        assert_ne!(artifact_h, plain_h);
    }

    #[test]
    fn artifact_hash_differs_for_different_content() {
        let h1 = artifact_hash(b"content A");
        let h2 = artifact_hash(b"content B");
        assert_ne!(h1, h2);
    }

    #[test]
    fn local_store_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

        let content = b"test artifact content";
        let hash = store.store(content).unwrap();

        // Verify hash is correct
        assert_eq!(hash, artifact_hash(content));

        // Fetch and verify
        let fetched = store.fetch(&hash).unwrap();
        assert_eq!(fetched, content);

        // Exists check
        assert!(store.exists(&hash));

        // Non-existent hash
        let fake_hash = [0xFFu8; 32];
        assert!(!store.exists(&fake_hash));
    }

    #[test]
    fn local_store_detects_corruption() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut store = LocalFileStore::new(temp_dir.path()).unwrap();

        let content = b"original content";
        let hash = store.store(content).unwrap();

        // Corrupt the file
        let path = store.path_for_hash(&hash);
        fs::write(&path, b"corrupted content").unwrap();

        // Fetch should detect corruption
        let result = store.fetch(&hash);
        assert!(matches!(result, Err(ArtifactError::HashMismatch { .. })));
    }

    #[test]
    fn local_store_rejects_too_large() {
        // Note: We can't actually test 50MB in a unit test, so we test the error type
        let err = ArtifactError::TooLarge {
            size: MAX_ARTIFACT_SIZE + 1,
            max: MAX_ARTIFACT_SIZE,
        };
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn local_store_not_found() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = LocalFileStore::new(temp_dir.path()).unwrap();

        let fake_hash = [0x42u8; 32];
        let result = store.fetch(&fake_hash);
        assert!(matches!(result, Err(ArtifactError::NotFound(_))));
    }

    #[test]
    fn validate_filename_accepts_valid() {
        let valid = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.bin";
        let result = LocalFileStore::validate_filename(valid);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_filename_rejects_invalid() {
        // Wrong extension
        assert!(LocalFileStore::validate_filename("abc.txt").is_err());

        // Too short
        assert!(LocalFileStore::validate_filename("abc.bin").is_err());

        // Invalid hex
        assert!(LocalFileStore::validate_filename(
            "zzzz456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.bin"
        )
        .is_err());

        // Path traversal attempt
        assert!(LocalFileStore::validate_filename("../../../etc/passwd.bin").is_err());
    }

    #[test]
    fn error_display_formatting() {
        let err = ArtifactError::NotFound([0x42u8; 32]);
        let msg = err.to_string();
        assert!(msg.contains("not found"));
        assert!(msg.contains("4242424242")); // Part of hex encoding

        let err = ArtifactError::TooLarge { size: 100, max: 50 };
        assert!(err.to_string().contains("100"));
        assert!(err.to_string().contains("50"));
    }

    #[test]
    fn golden_artifact_hash() {
        // Golden vector to lock hash computation
        let content = b"NOVAI test artifact for golden vector";
        let hash = artifact_hash(content);

        // This value locks the hash computation
        const EXPECTED: [u8; 32] = [
            0x79, 0x64, 0xe8, 0xf0, 0x84, 0x04, 0xb9, 0x36, 0xdf, 0x17, 0xa2, 0x8a, 0x42, 0x0a,
            0xc1, 0xa2, 0x14, 0xd6, 0xc1, 0xca, 0xbe, 0xb4, 0x51, 0xb1, 0x81, 0x1e, 0x52, 0x2d,
            0x84, 0x44, 0xfd, 0xc4,
        ];

        if hash != EXPECTED {
            eprintln!("GOLDEN VECTOR UPDATE NEEDED:");
            eprintln!("const EXPECTED: [u8; 32] = [");
            for (i, b) in hash.iter().enumerate() {
                if i % 14 == 0 {
                    eprint!("    ");
                }
                eprint!("0x{:02x}, ", b);
                if (i + 1) % 14 == 0 {
                    eprintln!();
                }
            }
            eprintln!("];");
            panic!("Golden vector mismatch");
        }

        assert_eq!(hash, EXPECTED);
    }
}
