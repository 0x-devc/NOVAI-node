use blake3::Hasher;
use ed25519_dalek::Signer;
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};

use novai_codec::{encode_tx_v1_unsigned, CodecError};
use novai_types::{Address, SignatureBytes, TxV1};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    InvalidPublicKey,
    Codec(CodecError),
}

/// Derive the canonical 32-byte Address from a public key:
/// address = blake3(pubkey_bytes)
pub fn address_from_pubkey(pk: &VerifyingKey) -> Address {
    let mut hasher = Hasher::new();
    hasher.update(pk.as_bytes());
    *hasher.finalize().as_bytes()
}

/// Sign arbitrary bytes (used for signing TxV1 unsigned bytes).
pub fn sign_bytes(sk: &SigningKey, msg: &[u8]) -> SignatureBytes {
    let sig: Signature = sk.sign(msg);
    sig.to_bytes()
}

/// Verify signature over bytes using the provided public key.
pub fn verify_bytes(pk: &VerifyingKey, msg: &[u8], sig: &SignatureBytes) -> bool {
    let sig = Signature::from_bytes(sig);
    pk.verify_strict(msg, &sig).is_ok()
}

/// Parse a VerifyingKey from raw 32-byte public key bytes.
pub fn pubkey_from_bytes(bytes: &[u8; 32]) -> Result<VerifyingKey, CryptoError> {
    VerifyingKey::from_bytes(bytes).map_err(|_| CryptoError::InvalidPublicKey)
}

/// Week 2 rule: sign TxV1 over canonical *unsigned* bytes (everything except `sig`).
pub fn sign_tx_v1(sk: &SigningKey, tx: &mut TxV1) -> Result<(), CryptoError> {
    let unsigned = encode_tx_v1_unsigned(tx).map_err(CryptoError::Codec)?;
    tx.sig = sign_bytes(sk, &unsigned);
    Ok(())
}

/// Week 2 rule: verify TxV1 signature over canonical *unsigned* bytes.
pub fn verify_tx_v1(pk: &VerifyingKey, tx: &TxV1) -> Result<bool, CryptoError> {
    let unsigned = encode_tx_v1_unsigned(tx).map_err(CryptoError::Codec)?;
    Ok(verify_bytes(pk, &unsigned, &tx.sig))
}

#[cfg(test)]
mod tests {
    use super::*;

    use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
    use novai_types::TxVersion;

    #[test]
    fn sign_and_verify_roundtrip() {
        // Deterministic secret key for test (no RNG).
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let vk: VerifyingKey = sk.verifying_key();

        let msg = b"hello world";
        let sig = sk.sign(msg);

        // ed25519 verify should succeed for the same message
        assert!(vk.verify(msg, &sig).is_ok());

        // and fail if the message changes
        assert!(vk.verify(b"hello world!", &sig).is_err());
    }

    #[test]
    fn signature_tamper_fails() {
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let vk: VerifyingKey = sk.verifying_key();

        let msg = b"payload";
        let mut sig_bytes = sk.sign(msg).to_bytes();

        // flip 1 bit
        sig_bytes[0] ^= 0x01;

        let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        assert!(vk.verify(msg, &sig).is_err());
    }

    #[test]
    fn address_is_32_bytes_and_deterministic() {
        let sk1 = SigningKey::from_bytes(&[1u8; 32]);
        let pk1 = sk1.verifying_key();

        let sk2 = SigningKey::from_bytes(&[2u8; 32]);
        let pk2 = sk2.verifying_key();

        let a1 = address_from_pubkey(&pk1);
        let a1_again = address_from_pubkey(&pk1);
        let a2 = address_from_pubkey(&pk2);

        assert_eq!(a1.len(), 32);
        assert_eq!(a1, a1_again);
        assert_ne!(a1, a2);
    }

    #[test]
    fn txv1_signing_rule_is_over_unsigned_bytes() {
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let pk = sk.verifying_key();

        let mut tx = TxV1 {
            version: TxVersion::V1,
            from: *pk.as_bytes(), // for now: Address is pubkey bytes
            nonce: 1,
            fee: 5,
            payload: b"hello".to_vec(),
            sig: [0u8; 64],
        };

        sign_tx_v1(&sk, &mut tx).unwrap();
        assert!(verify_tx_v1(&pk, &tx).unwrap());

        // Mutating any unsigned field should break signature
        tx.fee += 1;
        assert!(!verify_tx_v1(&pk, &tx).unwrap());
    }
}
