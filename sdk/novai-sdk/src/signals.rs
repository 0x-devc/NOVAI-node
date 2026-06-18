//! Per-signal-type "extras" encoders for AI signal commitments (types 7-22).
//!
//! A signal commitment transaction (tx type 2) carries a fixed 66-byte
//! envelope, `[0x02][signal_hash:32][signal_type:1][issuer_entity_id:32]`,
//! followed by a type-specific "extras" tail. Signal types 0-6 (the original
//! advisory signals) carry no tail; types 7-22 each append a tail whose byte
//! layout is consensus-relevant.
//!
//! Each function here builds one such tail. Pair the returned bytes with the
//! matching [`AiSignalType`](novai_ai_entities::AiSignalType) and pass both to
//! [`crate::tx::signal_commitment_with_extras`].
//!
//! Every layout and length here is pinned to the canonical decoder in the
//! execution crate (`crates/execution/src/lib.rs`,
//! `decode_signal_commitment_payload_v1` and its `*_EXTRA_LEN` constants),
//! which is the ground truth; the SDK only constructs bytes, it does not
//! redefine the wire format. All multi-byte integers are big-endian.

use crate::error::Error;

// ============================================================================
// Wire constants (mirrored from the execution crate; values are consensus-fixed)
// ============================================================================

/// Stub proof submission discriminant (development / smoke tests).
pub const PROOF_TYPE_STUB: u8 = 0;
/// Inline-VK Groth16 proof submission discriminant.
pub const PROOF_TYPE_GROTH16: u8 = 1;
/// Registered-VK Groth16 proof submission discriminant (vk carried by id).
pub const PROOF_TYPE_GROTH16_REGISTERED: u8 = 3;

/// Maximum inline verifying-key size in a v2 proof submission (8 KiB).
pub const PROOF_SUBMISSION_MAX_VK_BYTES: usize = 8 * 1024;
/// Maximum proof-bytes size in a v2 proof submission (1 KiB).
pub const PROOF_SUBMISSION_MAX_PROOF_BYTES: usize = 1024;

/// Minimum oracle-anchor `data_tag` length, in bytes.
pub const ORACLE_ANCHOR_DATA_TAG_MIN_LEN: usize = 1;
/// Maximum oracle-anchor `data_tag` length, in bytes.
pub const ORACLE_ANCHOR_DATA_TAG_MAX_LEN: usize = 32;

/// Minimum split count when a payment splits trailer is present.
pub const MIN_PAYMENT_SPLITS_WHEN_PRESENT: usize = 2;
/// Maximum split count in a payment splits trailer.
pub const MAX_PAYMENT_SPLITS: usize = 8;
/// Basis-points denominator; split shares must sum to exactly this value.
pub const BPS_DENOMINATOR: u32 = 10_000;

// ============================================================================
// Type 7: ReputationUpdate (35-byte tail)
// ============================================================================

/// Build the `ReputationUpdate` extras tail (35 bytes).
///
/// Layout: `target_entity_id:32 | event_type:1 | points_delta_be:2`.
#[must_use]
pub fn reputation_update_extras(
    target_entity_id: &[u8; 32],
    event_type: u8,
    points_delta: i16,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(35);
    out.extend_from_slice(target_entity_id);
    out.push(event_type);
    out.extend_from_slice(&points_delta.to_be_bytes());
    out
}

// ============================================================================
// Type 8: SignalPurchase (41-byte tail)
// ============================================================================

/// Build the `SignalPurchase` extras tail (41 bytes).
///
/// Layout: `seller_entity_id:32 | purchased_signal_type:1 | max_price_be:8`.
#[must_use]
pub fn signal_purchase_extras(
    seller_entity_id: &[u8; 32],
    purchased_signal_type: u8,
    max_price: u64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(41);
    out.extend_from_slice(seller_entity_id);
    out.push(purchased_signal_type);
    out.extend_from_slice(&max_price.to_be_bytes());
    out
}

// ============================================================================
// Type 9: StakeDeposit (16-byte tail)
// ============================================================================

/// Build the `StakeDeposit` extras tail (16 bytes): `amount_be:16`.
#[must_use]
pub fn stake_deposit_extras(amount: u128) -> Vec<u8> {
    amount.to_be_bytes().to_vec()
}

// ============================================================================
// Type 10: StakeWithdraw (16-byte tail)
// ============================================================================

/// Build the `StakeWithdraw` extras tail (16 bytes): `amount_be:16`.
#[must_use]
pub fn stake_withdraw_extras(amount: u128) -> Vec<u8> {
    amount.to_be_bytes().to_vec()
}

// ============================================================================
// Type 11: StakeSlash (51-byte tail)
// ============================================================================

/// Build the `StakeSlash` extras tail (51 bytes).
///
/// Layout: `target_entity_id:32 | slash_amount_be:16 | rep_event_type:1 |
/// points_delta_be:2`.
#[must_use]
pub fn stake_slash_extras(
    target_entity_id: &[u8; 32],
    slash_amount: u128,
    rep_event_type: u8,
    points_delta: i16,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(51);
    out.extend_from_slice(target_entity_id);
    out.extend_from_slice(&slash_amount.to_be_bytes());
    out.push(rep_event_type);
    out.extend_from_slice(&points_delta.to_be_bytes());
    out
}

// ============================================================================
// Type 12: CompositionCheck (34-byte tail)
// ============================================================================

/// Build the `CompositionCheck` extras tail (34 bytes).
///
/// Layout: `target_entity_id:32 | failed_dependency_idx:1 | failure_reason:1`.
#[must_use]
pub fn composition_check_extras(
    target_entity_id: &[u8; 32],
    failed_dependency_idx: u8,
    failure_reason: u8,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(34);
    out.extend_from_slice(target_entity_id);
    out.push(failed_dependency_idx);
    out.push(failure_reason);
    out
}

// ============================================================================
// Type 13: ProofSubmission (variable tail: 65-byte prefix, optional v2 body)
// ============================================================================

/// Build the v1 stub `ProofSubmission` extras tail (65 bytes).
///
/// Layout: `proof_type:1 (= 0) | code_hash:32 | computation_hash:32`.
#[must_use]
pub fn proof_submission_stub_extras(
    code_hash: &[u8; 32],
    computation_hash: &[u8; 32],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(65);
    out.push(PROOF_TYPE_STUB);
    out.extend_from_slice(code_hash);
    out.extend_from_slice(computation_hash);
    out
}

/// Build the inline-VK Groth16 `ProofSubmission` extras tail (variable).
///
/// Layout: `proof_type:1 (= 1) | code_hash:32 | computation_hash:32 |
/// vk_len_be:4 | vk_bytes:vk_len | proof_len_be:4 | proof_bytes:proof_len`.
///
/// # Errors
/// Returns [`Error::InvalidArgument`] if `vk_bytes` exceeds
/// [`PROOF_SUBMISSION_MAX_VK_BYTES`] or `proof_bytes` exceeds
/// [`PROOF_SUBMISSION_MAX_PROOF_BYTES`].
pub fn proof_submission_groth16_extras(
    code_hash: &[u8; 32],
    computation_hash: &[u8; 32],
    vk_bytes: &[u8],
    proof_bytes: &[u8],
) -> Result<Vec<u8>, Error> {
    if vk_bytes.len() > PROOF_SUBMISSION_MAX_VK_BYTES {
        return Err(Error::InvalidArgument(format!(
            "vk_bytes exceeds {PROOF_SUBMISSION_MAX_VK_BYTES} bytes: {}",
            vk_bytes.len()
        )));
    }
    if proof_bytes.len() > PROOF_SUBMISSION_MAX_PROOF_BYTES {
        return Err(Error::InvalidArgument(format!(
            "proof_bytes exceeds {PROOF_SUBMISSION_MAX_PROOF_BYTES} bytes: {}",
            proof_bytes.len()
        )));
    }
    let vk_len = u32::try_from(vk_bytes.len())
        .map_err(|_| Error::InvalidArgument("vk_bytes length overflow".into()))?;
    let proof_len = u32::try_from(proof_bytes.len())
        .map_err(|_| Error::InvalidArgument("proof_bytes length overflow".into()))?;

    let mut out = Vec::with_capacity(65 + 4 + vk_bytes.len() + 4 + proof_bytes.len());
    out.push(PROOF_TYPE_GROTH16);
    out.extend_from_slice(code_hash);
    out.extend_from_slice(computation_hash);
    out.extend_from_slice(&vk_len.to_be_bytes());
    out.extend_from_slice(vk_bytes);
    out.extend_from_slice(&proof_len.to_be_bytes());
    out.extend_from_slice(proof_bytes);
    Ok(out)
}

/// Build the registered-VK Groth16 `ProofSubmission` extras tail (variable).
///
/// Layout: `proof_type:1 (= 3) | code_hash:32 | computation_hash:32 |
/// vk_len_be:4 (= 32) | vk_id:32 | proof_len_be:4 | proof_bytes:proof_len`.
///
/// The `vk_id` is the 32-byte memory object id of a previously published
/// `VkRegistration`; the chain enforces `vk_len == 32` for this proof type.
///
/// # Errors
/// Returns [`Error::InvalidArgument`] if `proof_bytes` exceeds
/// [`PROOF_SUBMISSION_MAX_PROOF_BYTES`].
pub fn proof_submission_groth16_registered_extras(
    code_hash: &[u8; 32],
    computation_hash: &[u8; 32],
    vk_id: &[u8; 32],
    proof_bytes: &[u8],
) -> Result<Vec<u8>, Error> {
    if proof_bytes.len() > PROOF_SUBMISSION_MAX_PROOF_BYTES {
        return Err(Error::InvalidArgument(format!(
            "proof_bytes exceeds {PROOF_SUBMISSION_MAX_PROOF_BYTES} bytes: {}",
            proof_bytes.len()
        )));
    }
    let proof_len = u32::try_from(proof_bytes.len())
        .map_err(|_| Error::InvalidArgument("proof_bytes length overflow".into()))?;

    let mut out = Vec::with_capacity(65 + 4 + 32 + 4 + proof_bytes.len());
    out.push(PROOF_TYPE_GROTH16_REGISTERED);
    out.extend_from_slice(code_hash);
    out.extend_from_slice(computation_hash);
    out.extend_from_slice(&32u32.to_be_bytes());
    out.extend_from_slice(vk_id);
    out.extend_from_slice(&proof_len.to_be_bytes());
    out.extend_from_slice(proof_bytes);
    Ok(out)
}

// ============================================================================
// Type 14: SubscriptionCreate (49-byte tail)
// ============================================================================

/// Build the `SubscriptionCreate` extras tail (49 bytes).
///
/// Layout: `producer_entity_id:32 | covered_signal_type:1 |
/// rate_per_block_be:8 | duration_blocks_be:8`.
#[must_use]
pub fn subscription_create_extras(
    producer_entity_id: &[u8; 32],
    covered_signal_type: u8,
    rate_per_block: u64,
    duration_blocks: u64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(49);
    out.extend_from_slice(producer_entity_id);
    out.push(covered_signal_type);
    out.extend_from_slice(&rate_per_block.to_be_bytes());
    out.extend_from_slice(&duration_blocks.to_be_bytes());
    out
}

// ============================================================================
// Type 15: SubscriptionCancel (32-byte tail)
// ============================================================================

/// Build the `SubscriptionCancel` extras tail (32 bytes): `subscription_id:32`.
#[must_use]
pub fn subscription_cancel_extras(subscription_id: &[u8; 32]) -> Vec<u8> {
    subscription_id.to_vec()
}

// ============================================================================
// Type 16: PaymentRequest (variable tail: 112-byte base, optional splits)
// ============================================================================

/// One recipient share in a `PaymentRequest` splits trailer (Week 33).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaymentSplit {
    /// 32-byte entity id receiving this share.
    pub recipient_entity_id: [u8; 32],
    /// Share in basis points, in `[1, BPS_DENOMINATOR]`.
    pub basis_points: u16,
}

/// Build the single-recipient `PaymentRequest` extras tail (112 bytes).
///
/// Layout: `payee_entity_id:32 | amount_be:8 | service_descriptor_hash:32 |
/// request_hash:32 | max_block_height_be:8`.
///
/// # Errors
/// Returns [`Error::InvalidArgument`] if `amount` is zero (a zero-amount
/// payment is rejected by the chain handler).
pub fn payment_request_extras(
    payee_entity_id: &[u8; 32],
    amount: u64,
    service_descriptor_hash: &[u8; 32],
    request_hash: &[u8; 32],
    max_block_height: u64,
) -> Result<Vec<u8>, Error> {
    if amount == 0 {
        return Err(Error::InvalidArgument("amount must be non-zero".into()));
    }
    let mut out = Vec::with_capacity(112);
    out.extend_from_slice(payee_entity_id);
    out.extend_from_slice(&amount.to_be_bytes());
    out.extend_from_slice(service_descriptor_hash);
    out.extend_from_slice(request_hash);
    out.extend_from_slice(&max_block_height.to_be_bytes());
    Ok(out)
}

/// Build a multi-recipient `PaymentRequest` extras tail with a Week 33 splits
/// trailer (`112 + 1 + N*34` bytes).
///
/// The trailer is `count:1 | count * (recipient_entity_id:32 |
/// basis_points_be:2)`. Mirrors the client-side validation the chain enforces:
/// `count` in `[MIN_PAYMENT_SPLITS_WHEN_PRESENT, MAX_PAYMENT_SPLITS]`, the
/// first split recipient equals `payee_entity_id`, recipients are unique, each
/// share is in `[1, BPS_DENOMINATOR]`, and all shares sum to exactly
/// [`BPS_DENOMINATOR`].
///
/// # Errors
/// Returns [`Error::InvalidArgument`] if `amount` is zero or any splits
/// invariant is violated.
pub fn payment_request_extras_with_splits(
    payee_entity_id: &[u8; 32],
    amount: u64,
    service_descriptor_hash: &[u8; 32],
    request_hash: &[u8; 32],
    max_block_height: u64,
    splits: &[PaymentSplit],
) -> Result<Vec<u8>, Error> {
    validate_splits(payee_entity_id, splits)?;

    let mut out = payment_request_extras(
        payee_entity_id,
        amount,
        service_descriptor_hash,
        request_hash,
        max_block_height,
    )?;
    out.reserve(1 + splits.len() * 34);
    // `splits.len()` is bounded by MAX_PAYMENT_SPLITS (8) via validate_splits.
    out.push(splits.len() as u8);
    for split in splits {
        out.extend_from_slice(&split.recipient_entity_id);
        out.extend_from_slice(&split.basis_points.to_be_bytes());
    }
    Ok(out)
}

/// Enforce the chain's `PaymentRequest` splits invariants client-side.
fn validate_splits(payee_entity_id: &[u8; 32], splits: &[PaymentSplit]) -> Result<(), Error> {
    let n = splits.len();
    if !(MIN_PAYMENT_SPLITS_WHEN_PRESENT..=MAX_PAYMENT_SPLITS).contains(&n) {
        return Err(Error::InvalidArgument(format!(
            "splits count must be in [{MIN_PAYMENT_SPLITS_WHEN_PRESENT}, {MAX_PAYMENT_SPLITS}], got {n}"
        )));
    }
    if &splits[0].recipient_entity_id != payee_entity_id {
        return Err(Error::InvalidArgument(
            "splits[0].recipient_entity_id must equal payee_entity_id".into(),
        ));
    }
    let mut total: u32 = 0;
    for (i, split) in splits.iter().enumerate() {
        if split.basis_points < 1 || u32::from(split.basis_points) > BPS_DENOMINATOR {
            return Err(Error::InvalidArgument(format!(
                "basis_points must be in [1, {BPS_DENOMINATOR}], got {}",
                split.basis_points
            )));
        }
        // n <= MAX_PAYMENT_SPLITS (8), so the quadratic duplicate scan is cheap.
        if splits[..i]
            .iter()
            .any(|s| s.recipient_entity_id == split.recipient_entity_id)
        {
            return Err(Error::InvalidArgument(
                "duplicate split recipient_entity_id".into(),
            ));
        }
        total += u32::from(split.basis_points);
    }
    if total != BPS_DENOMINATOR {
        return Err(Error::InvalidArgument(format!(
            "sum of basis_points must equal {BPS_DENOMINATOR}, got {total}"
        )));
    }
    Ok(())
}

// ============================================================================
// Type 17: ServiceAttestation (65-byte tail)
// ============================================================================

/// Build the `ServiceAttestation` extras tail (65 bytes).
///
/// Layout: `payment_signal_hash:32 | payee_entity_id:32 | status:1`
/// (status 0 = Delivered, 1 = Failed).
#[must_use]
pub fn service_attestation_extras(
    payment_signal_hash: &[u8; 32],
    payee_entity_id: &[u8; 32],
    status: u8,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(65);
    out.extend_from_slice(payment_signal_hash);
    out.extend_from_slice(payee_entity_id);
    out.push(status);
    out
}

// ============================================================================
// Type 18: SlaAccept (64-byte tail)
// ============================================================================

/// Build the `SlaAccept` extras tail (64 bytes).
///
/// Layout: `sla_object_id:32 | buyer_entity_id:32`.
#[must_use]
pub fn sla_accept_extras(sla_object_id: &[u8; 32], buyer_entity_id: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(sla_object_id);
    out.extend_from_slice(buyer_entity_id);
    out
}

// ============================================================================
// Type 19: ChannelAccept (64-byte tail)
// ============================================================================

/// Build the `ChannelAccept` extras tail (64 bytes).
///
/// Layout: `channel_object_id:32 | party_a_entity_id:32`.
#[must_use]
pub fn channel_accept_extras(channel_object_id: &[u8; 32], party_a_entity_id: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(channel_object_id);
    out.extend_from_slice(party_a_entity_id);
    out
}

// ============================================================================
// Type 20: ChannelClose (233-byte tail)
// ============================================================================

/// Build the `ChannelClose` extras tail (233 bytes).
///
/// Layout: `channel_object_id:32 | party_a_entity_id:32 | nonce_be:8 |
/// balance_a_be:16 | balance_b_be:16 | is_final:1 | sig_a:64 | sig_b:64`.
/// `is_final` encodes as 1 (cooperative settle) or 0 (unilateral close).
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn channel_close_extras(
    channel_object_id: &[u8; 32],
    party_a_entity_id: &[u8; 32],
    nonce: u64,
    balance_a: u128,
    balance_b: u128,
    is_final: bool,
    sig_a: &[u8; 64],
    sig_b: &[u8; 64],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(233);
    out.extend_from_slice(channel_object_id);
    out.extend_from_slice(party_a_entity_id);
    out.extend_from_slice(&nonce.to_be_bytes());
    out.extend_from_slice(&balance_a.to_be_bytes());
    out.extend_from_slice(&balance_b.to_be_bytes());
    out.push(u8::from(is_final));
    out.extend_from_slice(sig_a);
    out.extend_from_slice(sig_b);
    out
}

// ============================================================================
// Type 21: ChannelFinalize (64-byte tail)
// ============================================================================

/// Build the `ChannelFinalize` extras tail (64 bytes).
///
/// Layout: `channel_object_id:32 | party_a_entity_id:32`.
#[must_use]
pub fn channel_finalize_extras(
    channel_object_id: &[u8; 32],
    party_a_entity_id: &[u8; 32],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(channel_object_id);
    out.extend_from_slice(party_a_entity_id);
    out
}

// ============================================================================
// Type 22: OracleAnchor (variable tail: 81-byte prefix + tag)
// ============================================================================

/// Build the `OracleAnchor` extras tail (82..=113 bytes).
///
/// Layout: `data_hash:32 | external_timestamp_be:8 | source_hash:32 |
/// expiry_height_be:8 | data_tag_len:1 | data_tag:[1..=32]`.
///
/// Pass an all-zero `source_hash` to mean "no source". `expiry_height` of 0
/// means "no declared expiry" (advisory, not enforced by the chain).
///
/// # Errors
/// Returns [`Error::InvalidArgument`] if `external_timestamp` is zero, if
/// `data_hash` is all-zero, or if `data_tag` length is outside
/// `[ORACLE_ANCHOR_DATA_TAG_MIN_LEN, ORACLE_ANCHOR_DATA_TAG_MAX_LEN]`.
pub fn oracle_anchor_extras(
    data_hash: &[u8; 32],
    external_timestamp: u64,
    source_hash: &[u8; 32],
    expiry_height: u64,
    data_tag: &[u8],
) -> Result<Vec<u8>, Error> {
    if external_timestamp == 0 {
        return Err(Error::InvalidArgument(
            "external_timestamp must be non-zero".into(),
        ));
    }
    if data_hash == &[0u8; 32] {
        return Err(Error::InvalidArgument("data_hash must be non-zero".into()));
    }
    let tag_len = data_tag.len();
    if !(ORACLE_ANCHOR_DATA_TAG_MIN_LEN..=ORACLE_ANCHOR_DATA_TAG_MAX_LEN).contains(&tag_len) {
        return Err(Error::InvalidArgument(format!(
            "data_tag length must be in [{ORACLE_ANCHOR_DATA_TAG_MIN_LEN}, {ORACLE_ANCHOR_DATA_TAG_MAX_LEN}], got {tag_len}"
        )));
    }

    let mut out = Vec::with_capacity(81 + tag_len);
    out.extend_from_slice(data_hash);
    out.extend_from_slice(&external_timestamp.to_be_bytes());
    out.extend_from_slice(source_hash);
    out.extend_from_slice(&expiry_height.to_be_bytes());
    // tag_len <= ORACLE_ANCHOR_DATA_TAG_MAX_LEN (32) always fits in u8.
    out.push(tag_len as u8);
    out.extend_from_slice(data_tag);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden vectors below mirror, scenario for scenario, the Python SDK tests
    // in sdk/novai-python-sdk/tests/ (test_signal_extras.py, test_signal_oracle.py,
    // test_signal_payment.py). Lengths additionally pin to the execution crate
    // *_EXTRA_LEN constants (crates/execution/src/lib.rs).

    // ---- Type 7: ReputationUpdate ----
    #[test]
    fn reputation_update_golden() {
        let e = reputation_update_extras(&[0x11; 32], 5, -3);
        assert_eq!(e.len(), 35, "pins to REPUTATION_UPDATE_EXTRA_LEN");
        assert_eq!(&e[0..32], &[0x11u8; 32][..]);
        assert_eq!(e[32], 5);
        assert_eq!(&e[33..35], &(-3i16).to_be_bytes()[..]); // 0xFFFD

        let e2 = reputation_update_extras(&[0; 32], 0, 100);
        assert_eq!(&e2[33..35], &100i16.to_be_bytes()[..]); // 0x0064
    }

    // ---- Type 8: SignalPurchase ----
    #[test]
    fn signal_purchase_golden() {
        let e = signal_purchase_extras(&[0x22; 32], 0, 999_999);
        assert_eq!(e.len(), 41, "pins to SIGNAL_PURCHASE_EXTRA_LEN");
        assert_eq!(&e[0..32], &[0x22u8; 32][..]);
        assert_eq!(e[32], 0);
        assert_eq!(&e[33..41], &999_999u64.to_be_bytes()[..]);
    }

    // ---- Types 9 / 10: StakeDeposit / StakeWithdraw ----
    #[test]
    fn stake_golden() {
        let d = stake_deposit_extras(1_000_000_000_000_000_000u128);
        assert_eq!(d.len(), 16, "pins to STAKE_DEPOSIT_EXTRA_LEN");
        assert_eq!(&d[..], &1_000_000_000_000_000_000u128.to_be_bytes()[..]);

        let w = stake_withdraw_extras(42);
        assert_eq!(w.len(), 16, "pins to STAKE_WITHDRAW_EXTRA_LEN");
        assert_eq!(&w[..], &42u128.to_be_bytes()[..]);
    }

    // ---- Type 11: StakeSlash ----
    #[test]
    fn stake_slash_golden() {
        let e = stake_slash_extras(&[0x33; 32], 500_000, 7, -5);
        assert_eq!(e.len(), 51, "pins to STAKE_SLASH_EXTRA_LEN");
        assert_eq!(&e[0..32], &[0x33u8; 32][..]);
        assert_eq!(&e[32..48], &500_000u128.to_be_bytes()[..]);
        assert_eq!(e[48], 7);
        assert_eq!(&e[49..51], &(-5i16).to_be_bytes()[..]); // 0xFFFB
    }

    // ---- Type 12: CompositionCheck ----
    #[test]
    fn composition_check_golden() {
        let e = composition_check_extras(&[0x44; 32], 3, 2);
        assert_eq!(e.len(), 34, "pins to COMPOSITION_CHECK_EXTRA_LEN");
        assert_eq!(&e[0..32], &[0x44u8; 32][..]);
        assert_eq!(e[32], 3);
        assert_eq!(e[33], 2);
    }

    // ---- Type 13: ProofSubmission (stub / groth16 / registered) ----
    #[test]
    fn proof_stub_golden() {
        let e = proof_submission_stub_extras(&[0xAA; 32], &[0xBB; 32]);
        assert_eq!(e.len(), 65, "pins to PROOF_SUBMISSION_EXTRA_LEN");
        assert_eq!(e[0], PROOF_TYPE_STUB);
        assert_eq!(&e[1..33], &[0xAAu8; 32][..]);
        assert_eq!(&e[33..65], &[0xBBu8; 32][..]);
    }

    #[test]
    fn proof_groth16_golden() {
        let vk = vec![b'V'; 100];
        let proof = vec![b'P'; 80];
        let e = proof_submission_groth16_extras(&[0; 32], &[0; 32], &vk, &proof).unwrap();
        assert_eq!(e.len(), 65 + 4 + 100 + 4 + 80);
        assert_eq!(e[0], PROOF_TYPE_GROTH16);
        assert_eq!(&e[65..69], &100u32.to_be_bytes()[..]);
        assert_eq!(&e[69..169], &vk[..]);
        assert_eq!(&e[169..173], &80u32.to_be_bytes()[..]);
        assert_eq!(&e[173..253], &proof[..]);
    }

    #[test]
    fn proof_groth16_registered_golden() {
        let proof = vec![b'P'; 50];
        let e =
            proof_submission_groth16_registered_extras(&[0; 32], &[0; 32], &[0xCC; 32], &proof)
                .unwrap();
        assert_eq!(e.len(), 65 + 4 + 32 + 4 + 50);
        assert_eq!(e[0], PROOF_TYPE_GROTH16_REGISTERED);
        assert_eq!(&e[65..69], &32u32.to_be_bytes()[..]); // vk_len fixed at 32
        assert_eq!(&e[69..101], &[0xCCu8; 32][..]); // vk_id
        assert_eq!(&e[101..105], &50u32.to_be_bytes()[..]);
        assert_eq!(&e[105..155], &proof[..]);
    }

    #[test]
    fn proof_oversize_rejected() {
        let big_vk = vec![0u8; PROOF_SUBMISSION_MAX_VK_BYTES + 1];
        assert!(proof_submission_groth16_extras(&[0; 32], &[0; 32], &big_vk, &[]).is_err());
        let big_proof = vec![0u8; PROOF_SUBMISSION_MAX_PROOF_BYTES + 1];
        assert!(proof_submission_groth16_extras(&[0; 32], &[0; 32], &[], &big_proof).is_err());
        assert!(proof_submission_groth16_registered_extras(
            &[0; 32],
            &[0; 32],
            &[0; 32],
            &big_proof
        )
        .is_err());
    }

    // ---- Type 14: SubscriptionCreate ----
    #[test]
    fn subscription_create_golden() {
        let e = subscription_create_extras(&[0x55; 32], 2, 10, 1000);
        assert_eq!(e.len(), 49, "pins to SUBSCRIPTION_CREATE_EXTRA_LEN");
        assert_eq!(&e[0..32], &[0x55u8; 32][..]);
        assert_eq!(e[32], 2);
        assert_eq!(&e[33..41], &10u64.to_be_bytes()[..]);
        assert_eq!(&e[41..49], &1000u64.to_be_bytes()[..]);
    }

    // ---- Type 15: SubscriptionCancel ----
    #[test]
    fn subscription_cancel_golden() {
        let e = subscription_cancel_extras(&[0x66; 32]);
        assert_eq!(e.len(), 32, "pins to SUBSCRIPTION_CANCEL_EXTRA_LEN");
        assert_eq!(&e[..], &[0x66u8; 32][..]);
    }

    // ---- Type 16: PaymentRequest (base + splits) ----
    #[test]
    fn payment_request_base_golden() {
        let e = payment_request_extras(&[0x11; 32], 42, &[0x22; 32], &[0x33; 32], 1_000_000)
            .unwrap();
        assert_eq!(e.len(), 112, "pins to PAYMENT_REQUEST_EXTRA_LEN");
        assert_eq!(&e[0..32], &[0x11u8; 32][..]);
        assert_eq!(&e[32..40], &42u64.to_be_bytes()[..]);
        assert_eq!(&e[40..72], &[0x22u8; 32][..]);
        assert_eq!(&e[72..104], &[0x33u8; 32][..]);
        assert_eq!(&e[104..112], &1_000_000u64.to_be_bytes()[..]);
    }

    #[test]
    fn payment_request_amount_zero_rejected() {
        assert!(payment_request_extras(&[0x11; 32], 0, &[0x22; 32], &[0x33; 32], 0).is_err());
    }

    #[test]
    fn payment_request_with_splits_golden() {
        let splits = [
            PaymentSplit {
                recipient_entity_id: [0x11; 32],
                basis_points: 7000,
            },
            PaymentSplit {
                recipient_entity_id: [0xAA; 32],
                basis_points: 3000,
            },
        ];
        let e = payment_request_extras_with_splits(
            &[0x11; 32],
            5000,
            &[0x22; 32],
            &[0x33; 32],
            1_000_000,
            &splits,
        )
        .unwrap();
        assert_eq!(e.len(), 112 + 1 + 2 * 34); // 181
        assert_eq!(e[112], 2); // split count
        assert_eq!(&e[113..145], &[0x11u8; 32][..]);
        assert_eq!(&e[145..147], &7000u16.to_be_bytes()[..]);
        assert_eq!(&e[147..179], &[0xAAu8; 32][..]);
        assert_eq!(&e[179..181], &3000u16.to_be_bytes()[..]);
    }

    #[test]
    fn payment_splits_invariants_rejected() {
        let payee = [0x11u8; 32];
        let ok_a = PaymentSplit {
            recipient_entity_id: payee,
            basis_points: 7000,
        };
        let ok_b = PaymentSplit {
            recipient_entity_id: [0xAA; 32],
            basis_points: 3000,
        };
        let mk = |splits: &[PaymentSplit]| {
            payment_request_extras_with_splits(
                &payee,
                5000,
                &[0x22; 32],
                &[0x33; 32],
                1,
                splits,
            )
        };
        // Too few (count 1) and too many (count 9).
        assert!(mk(&[ok_a]).is_err());
        let nine: Vec<PaymentSplit> = (0..9)
            .map(|i| PaymentSplit {
                recipient_entity_id: [i as u8; 32],
                basis_points: 1,
            })
            .collect();
        assert!(mk(&nine).is_err());
        // First recipient not equal to payee.
        assert!(mk(&[ok_b, ok_a]).is_err());
        // Sum != 10000.
        assert!(mk(&[
            PaymentSplit {
                recipient_entity_id: payee,
                basis_points: 5000
            },
            ok_b
        ])
        .is_err());
        // Duplicate recipient.
        assert!(mk(&[
            PaymentSplit {
                recipient_entity_id: payee,
                basis_points: 5000
            },
            PaymentSplit {
                recipient_entity_id: payee,
                basis_points: 5000
            }
        ])
        .is_err());
        // Valid baseline still succeeds.
        assert!(mk(&[ok_a, ok_b]).is_ok());
    }

    // ---- Type 17: ServiceAttestation ----
    #[test]
    fn service_attestation_golden() {
        let e = service_attestation_extras(&[0x77; 32], &[0x88; 32], 1);
        assert_eq!(e.len(), 65, "pins to SERVICE_ATTESTATION_EXTRA_LEN");
        assert_eq!(&e[0..32], &[0x77u8; 32][..]);
        assert_eq!(&e[32..64], &[0x88u8; 32][..]);
        assert_eq!(e[64], 1);
    }

    // ---- Type 18: SlaAccept ----
    #[test]
    fn sla_accept_golden() {
        let e = sla_accept_extras(&[0x99; 32], &[0xAA; 32]);
        assert_eq!(e.len(), 64, "pins to SLA_ACCEPT_EXTRA_LEN");
        assert_eq!(&e[0..32], &[0x99u8; 32][..]);
        assert_eq!(&e[32..64], &[0xAAu8; 32][..]);
    }

    // ---- Type 19: ChannelAccept ----
    #[test]
    fn channel_accept_golden() {
        let e = channel_accept_extras(&[0xBB; 32], &[0xCC; 32]);
        assert_eq!(e.len(), 64, "pins to CHANNEL_ACCEPT_EXTRA_LEN");
        assert_eq!(&e[0..32], &[0xBBu8; 32][..]);
        assert_eq!(&e[32..64], &[0xCCu8; 32][..]);
    }

    // ---- Type 20: ChannelClose ----
    #[test]
    fn channel_close_golden() {
        let e = channel_close_extras(
            &[0x10; 32],
            &[0x20; 32],
            7,
            1000,
            500,
            true,
            &[0x30; 64],
            &[0x40; 64],
        );
        assert_eq!(e.len(), 233, "pins to CHANNEL_CLOSE_EXTRA_LEN");
        assert_eq!(&e[0..32], &[0x10u8; 32][..]);
        assert_eq!(&e[32..64], &[0x20u8; 32][..]);
        assert_eq!(&e[64..72], &7u64.to_be_bytes()[..]);
        assert_eq!(&e[72..88], &1000u128.to_be_bytes()[..]);
        assert_eq!(&e[88..104], &500u128.to_be_bytes()[..]);
        assert_eq!(e[104], 1); // is_final = true
        assert_eq!(&e[105..169], &[0x30u8; 64][..]);
        assert_eq!(&e[169..233], &[0x40u8; 64][..]);

        let e_open = channel_close_extras(
            &[0x10; 32],
            &[0x20; 32],
            7,
            1000,
            500,
            false,
            &[0x30; 64],
            &[0x40; 64],
        );
        assert_eq!(e_open[104], 0); // is_final = false
    }

    // ---- Type 21: ChannelFinalize ----
    #[test]
    fn channel_finalize_golden() {
        let e = channel_finalize_extras(&[0xDD; 32], &[0xEE; 32]);
        assert_eq!(e.len(), 64, "pins to CHANNEL_FINALIZE_EXTRA_LEN");
        assert_eq!(&e[0..32], &[0xDDu8; 32][..]);
        assert_eq!(&e[32..64], &[0xEEu8; 32][..]);
    }

    // ---- Type 22: OracleAnchor ----
    #[test]
    fn oracle_anchor_golden() {
        let tag = b"price/ETH-USD";
        let e = oracle_anchor_extras(&[0xAB; 32], 0x0102_0304_0506_0708, &[0xCD; 32], 5000, tag)
            .unwrap();
        assert_eq!(e.len(), 81 + tag.len()); // 94
        assert_eq!(&e[0..32], &[0xABu8; 32][..]);
        assert_eq!(&e[32..40], &0x0102_0304_0506_0708u64.to_be_bytes()[..]);
        assert_eq!(&e[40..72], &[0xCDu8; 32][..]);
        assert_eq!(&e[72..80], &5000u64.to_be_bytes()[..]);
        assert_eq!(e[80], tag.len() as u8);
        assert_eq!(&e[81..81 + tag.len()], &tag[..]);
    }

    #[test]
    fn oracle_anchor_min_and_max_tag() {
        let min = oracle_anchor_extras(&[0xAB; 32], 1, &[0; 32], 0, b"x").unwrap();
        assert_eq!(min.len(), 82, "fixed 81 + 1-byte tag");
        let max = oracle_anchor_extras(&[0xAB; 32], 1, &[0; 32], 0, &[b'x'; 32]).unwrap();
        assert_eq!(max.len(), 113, "fixed 81 + 32-byte tag");
    }

    #[test]
    fn oracle_anchor_invalid_rejected() {
        // Empty tag and oversize tag.
        assert!(oracle_anchor_extras(&[0xAB; 32], 1, &[0; 32], 0, b"").is_err());
        assert!(oracle_anchor_extras(&[0xAB; 32], 1, &[0; 32], 0, &[0u8; 33]).is_err());
        // Zero timestamp and zero data_hash.
        assert!(oracle_anchor_extras(&[0xAB; 32], 0, &[0; 32], 0, b"x").is_err());
        assert!(oracle_anchor_extras(&[0; 32], 1, &[0; 32], 0, b"x").is_err());
    }

    // ---- Round-trip for the variable tails (re-parse back to inputs) ----
    #[test]
    fn oracle_anchor_round_trip() {
        let data_hash = [0xAB; 32];
        let ts = 0x0102_0304_0506_0708u64;
        let source = [0xCD; 32];
        let expiry = 5000u64;
        let tag = b"price/ETH-USD";
        let e = oracle_anchor_extras(&data_hash, ts, &source, expiry, tag).unwrap();

        assert_eq!(&e[0..32], &data_hash[..]);
        assert_eq!(u64::from_be_bytes(e[32..40].try_into().unwrap()), ts);
        assert_eq!(&e[40..72], &source[..]);
        assert_eq!(u64::from_be_bytes(e[72..80].try_into().unwrap()), expiry);
        let tag_len = usize::from(e[80]);
        assert_eq!(tag_len, tag.len());
        assert_eq!(&e[81..81 + tag_len], &tag[..]);
    }

    #[test]
    fn payment_splits_round_trip() {
        let payee = [0x11u8; 32];
        let splits = [
            PaymentSplit {
                recipient_entity_id: payee,
                basis_points: 6000,
            },
            PaymentSplit {
                recipient_entity_id: [0xAA; 32],
                basis_points: 4000,
            },
        ];
        let e = payment_request_extras_with_splits(
            &payee,
            5000,
            &[0x22; 32],
            &[0x33; 32],
            9,
            &splits,
        )
        .unwrap();

        let count = usize::from(e[112]);
        assert_eq!(count, splits.len());
        for (i, split) in splits.iter().enumerate() {
            let off = 113 + i * 34;
            assert_eq!(&e[off..off + 32], &split.recipient_entity_id[..]);
            assert_eq!(
                u16::from_be_bytes(e[off + 32..off + 34].try_into().unwrap()),
                split.basis_points
            );
        }
    }
}
