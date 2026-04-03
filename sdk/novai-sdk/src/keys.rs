//! Key generation, loading, saving, and address derivation.

use crate::error::Error;
use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_crypto::{address_from_pubkey, generate_keypair};
use novai_types::Address;

/// Generate a new random Ed25519 keypair.
#[must_use]
pub fn generate() -> (SigningKey, VerifyingKey) {
    generate_keypair()
}

/// Derive the canonical 32-byte address from a public key.
///
/// `address = blake3("NOVAI_ADDRESS_V1" || pubkey)`
#[must_use]
pub fn address(pk: &VerifyingKey) -> Address {
    address_from_pubkey(pk)
}

/// Load a keypair from a 32-byte seed file.
///
/// # Errors
///
/// Returns error if the file cannot be read or is not exactly 32 bytes.
pub fn load(path: &str) -> Result<(SigningKey, VerifyingKey), Error> {
    let bytes = std::fs::read(path).map_err(|e| Error::KeyFile(format!("read '{path}': {e}")))?;
    from_seed(&bytes)
}

/// Create a keypair from a 32-byte seed.
///
/// # Errors
///
/// Returns error if the seed is not exactly 32 bytes.
pub fn from_seed(seed: &[u8]) -> Result<(SigningKey, VerifyingKey), Error> {
    if seed.len() != 32 {
        return Err(Error::KeyFile(format!(
            "seed must be 32 bytes, got {}",
            seed.len()
        )));
    }
    let mut buf = [0u8; 32];
    buf.copy_from_slice(seed);
    let sk = SigningKey::from_bytes(&buf);
    let pk = sk.verifying_key();
    Ok((sk, pk))
}

/// Save a signing key's 32-byte seed to a file with restrictive permissions.
///
/// # Errors
///
/// Returns error if the file cannot be written.
pub fn save(path: &str, sk: &SigningKey) -> Result<(), Error> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::KeyFile(format!("create dir: {e}")))?;
        }
    }

    std::fs::write(path, sk.to_bytes())
        .map_err(|e| Error::KeyFile(format!("write '{path}': {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)
            .map_err(|e| Error::KeyFile(format!("set permissions: {e}")))?;
    }

    Ok(())
}
