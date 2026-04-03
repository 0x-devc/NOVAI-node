//! Key management commands: keygen and key-info.

use novai_crypto::{address_from_pubkey, generate_keypair};

/// Generate a new Ed25519 keypair and save to file.
pub fn run_keygen(output: &str) -> Result<(), String> {
    let (sk, pk) = generate_keypair();
    let addr = address_from_pubkey(&pk);

    // Write 32-byte seed with restrictive permissions
    let seed = sk.to_bytes();

    // Create parent directories if needed
    if let Some(parent) = std::path::Path::new(output).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {e}"))?;
        }
    }

    std::fs::write(output, seed).map_err(|e| format!("Failed to write key file: {e}"))?;

    // Set file permissions to 0600 on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(output, perms)
            .map_err(|e| format!("Failed to set file permissions: {e}"))?;
    }

    println!("Address: {}", hex::encode(addr));
    println!("Pubkey:  {}", hex::encode(pk.to_bytes()));
    println!("Key saved to: {output}");

    Ok(())
}

/// Show address and public key from an existing key file.
pub fn run_key_info(key_file: &str) -> Result<(), String> {
    let (_, pk) = load_keypair(key_file)?;
    let addr = address_from_pubkey(&pk);

    println!("Address: {}", hex::encode(addr));
    println!("Pubkey:  {}", hex::encode(pk.to_bytes()));

    Ok(())
}

/// Load a signing key from a 32-byte seed file.
pub fn load_keypair(
    path: &str,
) -> Result<(ed25519_dalek::SigningKey, ed25519_dalek::VerifyingKey), String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("Failed to read key file '{path}': {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "Key file must be exactly 32 bytes, got {}",
            bytes.len()
        ));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pk = sk.verifying_key();
    Ok((sk, pk))
}

/// Parse a hex string into a 32-byte array.
pub fn parse_hex32(hex_str: &str, field_name: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(hex_str).map_err(|e| format!("Invalid {field_name} hex: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "{field_name} must be 32 bytes (64 hex chars), got {}",
            bytes.len()
        ));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}
