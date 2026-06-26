use blake3::Hasher;
use ed25519_dalek::Signature;
use ed25519_dalek::Signer;
use rand_core::OsRng;

use novai_codec::{encode_tx_v1_unsigned, CodecError};
use novai_types::{Address, SignatureBytes, TxV1};

// ZK verification hooks (D20.4 stub; real Groth16 in Groth16Verifier)
pub mod zk;
pub use zk::{Groth16Verifier, StubZkVerifier, ZkVerifier};

// Re-export the ed25519-dalek key types so downstream crates (e.g.,
// the execution layer's integration tests for Week 32 channel state
// signing) can construct keys without taking a direct dependency on
// ed25519-dalek. The Signature type stays private because its
// canonical wire form is the 64-byte `SignatureBytes` type alias
// already re-exported via `novai_types`.
pub use ed25519_dalek::{SigningKey, VerifyingKey};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    InvalidPublicKey,
    Codec(CodecError),
}

/// Domain tag for `PaymentChannel` off-chain state update signatures
/// (Week 32). The 167-byte canonical bytes prepended with this tag are
/// what both parties sign for every off-chain channel state update
/// (cooperative or unilateral). Distinct from `NOVAI_TX_V1` to prevent
/// cross-domain signature replay: an entity's `TxV1` signature must
/// never be reinterpretable as a channel state update and vice versa.
pub const DOMAIN_TAG_CHANNEL_STATE_V1: &[u8] = b"NOVAI_CHANNEL_STATE_V1";

/// Build the canonical bytes both parties of a `PaymentChannel` sign
/// for an off-chain state update.
///
/// Layout (167 bytes):
/// `domain_tag (22) | chain_id_be (8) | channel_object_id (32) |
/// party_a (32) | party_b (32) | nonce_be (8) | balance_a_be (16) |
/// balance_b_be (16) | is_final (1)`.
///
/// `chain_id` is bound so an update signed on one NOVAI deployment
/// (e.g., testnet) cannot be replayed on another (e.g., mainnet).
/// `channel_object_id` is bound so an update signed for one channel
/// cannot be replayed on another. `nonce` strictly increases per
/// update; `is_final` distinguishes cooperative-settle states from
/// regular mid-channel snapshots so a mid-channel snapshot cannot be
/// reused to force an instant cooperative settle.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn channel_state_signing_bytes(
    chain_id: u64,
    channel_object_id: &[u8; 32],
    party_a: &[u8; 32],
    party_b: &[u8; 32],
    nonce: u64,
    balance_a: u128,
    balance_b: u128,
    is_final: bool,
) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(DOMAIN_TAG_CHANNEL_STATE_V1.len() + 8 + 32 + 32 + 32 + 8 + 16 + 16 + 1);
    out.extend_from_slice(DOMAIN_TAG_CHANNEL_STATE_V1);
    out.extend_from_slice(&chain_id.to_be_bytes());
    out.extend_from_slice(channel_object_id);
    out.extend_from_slice(party_a);
    out.extend_from_slice(party_b);
    out.extend_from_slice(&nonce.to_be_bytes());
    out.extend_from_slice(&balance_a.to_be_bytes());
    out.extend_from_slice(&balance_b.to_be_bytes());
    out.push(u8::from(is_final));
    out
}

/// Sign a `PaymentChannel` off-chain state update with the given key.
///
/// Returns the raw 64-byte ed25519 signature over
/// `channel_state_signing_bytes(...)`. The returned signature is what
/// gets placed into `sig_a` or `sig_b` in the `ChannelClose` signal
/// payload, depending on which party's key was used.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn sign_channel_state(
    sk: &SigningKey,
    chain_id: u64,
    channel_object_id: &[u8; 32],
    party_a: &[u8; 32],
    party_b: &[u8; 32],
    nonce: u64,
    balance_a: u128,
    balance_b: u128,
    is_final: bool,
) -> SignatureBytes {
    let msg = channel_state_signing_bytes(
        chain_id,
        channel_object_id,
        party_a,
        party_b,
        nonce,
        balance_a,
        balance_b,
        is_final,
    );
    sign_bytes(sk, &msg)
}

/// Verify a `PaymentChannel` off-chain state update signature.
///
/// Returns `false` if the public key bytes are not a valid Ed25519
/// verifying key, or if the signature does not verify under
/// `pubkey` over the canonical signing bytes. The `ChannelClose`
/// handler calls this twice per submission (once for `sig_a` against
/// party A's pubkey, once for `sig_b` against party B's pubkey) and
/// rejects the close on either failure.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn verify_channel_state_signature(
    sig: &SignatureBytes,
    pubkey: &[u8; 32],
    chain_id: u64,
    channel_object_id: &[u8; 32],
    party_a: &[u8; 32],
    party_b: &[u8; 32],
    nonce: u64,
    balance_a: u128,
    balance_b: u128,
    is_final: bool,
) -> bool {
    let Ok(vk) = pubkey_from_bytes(pubkey) else {
        return false;
    };
    let msg = channel_state_signing_bytes(
        chain_id,
        channel_object_id,
        party_a,
        party_b,
        nonce,
        balance_a,
        balance_b,
        is_final,
    );
    verify_bytes(&vk, &msg, sig)
}

pub fn generate_keypair() -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::generate(&mut OsRng);
    let pk = sk.verifying_key();
    (sk, pk)
}

/// Derive the canonical 32-byte Address from raw 32-byte public key bytes:
/// address = blake3(NOVAI_ADDRESS_V1 || pubkey_bytes)
///
/// This is the single source of truth for address derivation. It hashes the raw
/// bytes and does not validate that they form a canonical ed25519 point, so it
/// never fails; callers holding a `VerifyingKey` use `address_from_pubkey`, which
/// forwards the key's canonical encoding here.
pub fn address_from_pubkey_bytes(pubkey: &[u8; 32]) -> Address {
    let mut hasher = Hasher::new();
    hasher.update(b"NOVAI_ADDRESS_V1");
    hasher.update(pubkey);
    *hasher.finalize().as_bytes()
}

/// Derive the canonical 32-byte Address from a public key:
/// address = blake3(NOVAI_ADDRESS_V1 || pubkey_bytes)
pub fn address_from_pubkey(pk: &VerifyingKey) -> Address {
    address_from_pubkey_bytes(pk.as_bytes())
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

/// Week 2 rule: sign TxV1 over domain-tagged canonical *unsigned* bytes.
pub fn sign_tx_v1(sk: &SigningKey, tx: &mut TxV1) -> Result<(), CryptoError> {
    let unsigned = encode_tx_v1_unsigned(tx).map_err(CryptoError::Codec)?;
    let mut to_sign = Vec::with_capacity(b"NOVAI_TX_V1".len() + unsigned.len());
    to_sign.extend_from_slice(b"NOVAI_TX_V1");
    to_sign.extend_from_slice(&unsigned);
    tx.sig = sign_bytes(sk, &to_sign);
    Ok(())
}

/// Week 2 rule: verify TxV1 signature over domain-tagged canonical *unsigned* bytes.
pub fn verify_tx_v1(pk: &VerifyingKey, tx: &TxV1) -> Result<bool, CryptoError> {
    let unsigned = encode_tx_v1_unsigned(tx).map_err(CryptoError::Codec)?;
    let mut to_verify = Vec::with_capacity(b"NOVAI_TX_V1".len() + unsigned.len());
    to_verify.extend_from_slice(b"NOVAI_TX_V1");
    to_verify.extend_from_slice(&unsigned);
    Ok(verify_bytes(pk, &to_verify, &tx.sig))
}

/// Test helper: build a TxV1 with valid keypair, signature, and address.
///
/// **WARNING**: Only for tests! Generates new keypair each time.
/// DO NOT use in production code.
pub fn build_test_tx_v1(nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
    let (sk, pk) = generate_keypair();
    let from = address_from_pubkey(&pk);

    let mut tx = TxV1 {
        version: novai_types::TxVersion::V1,
        from,
        pubkey: pk.to_bytes(),
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    };

    sign_tx_v1(&sk, &mut tx).expect("test tx signing failed");
    tx
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
            from: address_from_pubkey(&pk),
            pubkey: pk.to_bytes(),
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

    // ========================================================================
    // Week 32 Phase 1: PaymentChannel off-chain state signature helpers
    // ========================================================================

    #[test]
    fn channel_state_signing_bytes_is_167_bytes_and_carries_domain_tag() {
        let bytes =
            channel_state_signing_bytes(1, &[0u8; 32], &[1u8; 32], &[2u8; 32], 0, 0, 0, false);
        assert_eq!(bytes.len(), 167);
        assert!(bytes.starts_with(DOMAIN_TAG_CHANNEL_STATE_V1));
    }

    #[test]
    fn channel_state_sign_verify_roundtrip() {
        let sk_a = SigningKey::from_bytes(&[4u8; 32]);
        let pk_a = sk_a.verifying_key().to_bytes();
        let sk_b = SigningKey::from_bytes(&[5u8; 32]);
        let pk_b = sk_b.verifying_key().to_bytes();

        let channel_id = [0xAAu8; 32];
        let party_a = [0xBBu8; 32];
        let party_b = [0xCCu8; 32];
        let chain_id: u64 = 7;
        let nonce: u64 = 42;
        let balance_a: u128 = 1_000;
        let balance_b: u128 = 500;
        let is_final = false;

        let sig_a = sign_channel_state(
            &sk_a,
            chain_id,
            &channel_id,
            &party_a,
            &party_b,
            nonce,
            balance_a,
            balance_b,
            is_final,
        );
        let sig_b = sign_channel_state(
            &sk_b,
            chain_id,
            &channel_id,
            &party_a,
            &party_b,
            nonce,
            balance_a,
            balance_b,
            is_final,
        );

        assert!(verify_channel_state_signature(
            &sig_a,
            &pk_a,
            chain_id,
            &channel_id,
            &party_a,
            &party_b,
            nonce,
            balance_a,
            balance_b,
            is_final,
        ));
        assert!(verify_channel_state_signature(
            &sig_b,
            &pk_b,
            chain_id,
            &channel_id,
            &party_a,
            &party_b,
            nonce,
            balance_a,
            balance_b,
            is_final,
        ));
        // Cross-key: A's signature does not verify under B's pubkey.
        assert!(!verify_channel_state_signature(
            &sig_a,
            &pk_b,
            chain_id,
            &channel_id,
            &party_a,
            &party_b,
            nonce,
            balance_a,
            balance_b,
            is_final,
        ));
    }

    #[test]
    fn channel_state_signature_binds_every_field() {
        let sk = SigningKey::from_bytes(&[6u8; 32]);
        let pk = sk.verifying_key().to_bytes();

        let base = (
            1u64,
            [0x11u8; 32],
            [0x22u8; 32],
            [0x33u8; 32],
            10u64,
            100u128,
            50u128,
            false,
        );
        let sig = sign_channel_state(
            &sk, base.0, &base.1, &base.2, &base.3, base.4, base.5, base.6, base.7,
        );

        // Flipping any single field breaks verification.
        let mutated = [
            (
                base.0 + 1,
                base.1,
                base.2,
                base.3,
                base.4,
                base.5,
                base.6,
                base.7,
            ),
            (
                base.0,
                [0xFFu8; 32],
                base.2,
                base.3,
                base.4,
                base.5,
                base.6,
                base.7,
            ),
            (
                base.0,
                base.1,
                [0xFFu8; 32],
                base.3,
                base.4,
                base.5,
                base.6,
                base.7,
            ),
            (
                base.0,
                base.1,
                base.2,
                [0xFFu8; 32],
                base.4,
                base.5,
                base.6,
                base.7,
            ),
            (
                base.0,
                base.1,
                base.2,
                base.3,
                base.4 + 1,
                base.5,
                base.6,
                base.7,
            ),
            (
                base.0,
                base.1,
                base.2,
                base.3,
                base.4,
                base.5 + 1,
                base.6,
                base.7,
            ),
            (
                base.0,
                base.1,
                base.2,
                base.3,
                base.4,
                base.5,
                base.6 + 1,
                base.7,
            ),
            (
                base.0, base.1, base.2, base.3, base.4, base.5, base.6, !base.7,
            ),
        ];
        for m in &mutated {
            assert!(
                !verify_channel_state_signature(
                    &sig, &pk, m.0, &m.1, &m.2, &m.3, m.4, m.5, m.6, m.7,
                ),
                "verification must reject mutated field"
            );
        }
    }

    #[test]
    fn channel_state_verify_rejects_garbage_pubkey() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let sig = sign_channel_state(&sk, 0, &[0u8; 32], &[0u8; 32], &[0u8; 32], 0, 0, 0, false);
        // An all-zero pubkey is not a valid Ed25519 point; verification
        // must return false rather than panicking.
        let bogus = [0u8; 32];
        assert!(!verify_channel_state_signature(
            &sig, &bogus, 0, &[0u8; 32], &[0u8; 32], &[0u8; 32], 0, 0, 0, false,
        ));
    }
}
