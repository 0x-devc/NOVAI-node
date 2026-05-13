//! novai-execution
//!
//! Week 3 scope: deterministic state transition for account model.
//!
//! Invariants (week 3):
//! - No floats.
//! - No iteration over HashMap/HashSet in consensus-critical ordering.
//! - All arithmetic is checked (overflow/underflow -> error).
//! - Transfer payload is canonical and versioned.
//!
//! Failure modes:
//! - Bad payload encoding -> reject deterministically.
//! - Nonce mismatch -> reject deterministically.
//! - Insufficient funds -> reject deterministically.
//! - Any overflow -> reject deterministically.

use novai_state::{
    account_key, approval_gate_key, decode_account_v1, decode_fee_pool_v1, decode_smt_root_v1,
    encode_account_v1, encode_fee_pool_v1, encode_smt_root_v1, governance_proposal_key,
    smt_key_for_state_key, smt_node_key, AccountStateV1, FeePoolV1, Kv, KvBatch, StateDecodeError,
    WriteOp, KEY_AI_KILL_SWITCH, KEY_FEE_POOL, KEY_SMT_ROOT,
};

use novai_ai_entities::tiers::ActionType;
use novai_governance::{
    decode_proposal_v1, encode_proposal_v1, Proposal, ProposalState, ProposalType,
};

use novai_smt::hash::{empty_hash_at_height, Hash32};
use novai_smt::node::Node;
use novai_smt::smt::{Smt, SmtError, SmtStore};
use novai_types::{Address, Nonce, TxV1};

pub const EXECUTION_VERSION: u8 = 1;

/// Derive the canonical 32-byte address from raw public key bytes.
///
/// `address = blake3("NOVAI_ADDRESS_V1" || pubkey_bytes)`
///
/// This matches the derivation in `novai-crypto::address_from_pubkey` but operates
/// on raw bytes, avoiding the need for ed25519 `VerifyingKey` validation at registration
/// time. Actual key validity is verified when the entity signs a transaction.
fn derive_address_from_pubkey_bytes(pubkey: &[u8; 32]) -> Address {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"NOVAI_ADDRESS_V1");
    hasher.update(pubkey);
    *hasher.finalize().as_bytes()
}

/// Transfer payload version.
pub const TRANSFER_PAYLOAD_V1: u8 = 1;

/// Canonical Transfer payload:
/// `[version:1][to:32][amount_be:8]`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPayloadV1 {
    pub to: Address,
    pub amount: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError<E> {
    Db(E),
    Decode(StateDecodeError),
    BadPayloadLength {
        expected: usize,
        got: usize,
    },
    BadPayloadVersion {
        expected: u8,
        got: u8,
    },
    NonceMismatch {
        expected: Nonce,
        got: Nonce,
    },
    InsufficientFunds {
        balance: u128,
        needed: u128,
    },
    Overflow,
    /// Codec/decode failure for non-state data (entity, proposal, gate, signal, memory object).
    CodecDecode(String),
    NonceOverflow,
    // Week 14 - Signal commitment errors (D14.2)
    /// Issuer entity ID not found in state.
    IssuerNotFound,
    /// Issuer entity does not have `emit_proposals` capability.
    IssuerMissingCapability,
    /// Issuer entity ID in payload does not match tx.from.
    IssuerMismatch,
    // Week 21 - Memory object errors (D21.4)
    /// Memory object data exceeds maximum size.
    MemoryObjectTooLarge {
        size: usize,
        max: usize,
    },
    /// Entity has too many memory objects.
    MemoryObjectCountExceeded {
        count: u32,
        max: u32,
    },
    /// Memory object not found.
    MemoryObjectNotFound,
    /// Invalid memory object type byte.
    InvalidMemoryObjectType {
        byte: u8,
    },
    /// Entity does not own the memory object.
    MemoryObjectOwnerMismatch,
    // Week 22 - NNPX privacy errors (D22.4)
    /// AI entity attempted to access NNPX private data.
    NnpxAccessDenied,
    /// Nullifier has already been spent (double-spend attempt).
    NullifierAlreadySpent,
    /// Invalid private payload commitment.
    InvalidPrivateCommitment,
    // Week 23 - Derived view errors (D23.5)
    /// AI entity missing `read_nnpx_derived` capability.
    DerivedViewAccessDenied,
    /// Derived view not found.
    DerivedViewNotFound,
    /// Invalid derived view schema.
    InvalidDerivedViewSchema {
        schema_id: u32,
    },
    // Week 24 - Module activation/rollback errors (D24.1)
    /// AI entity not found in state.
    EntityNotFound,
    /// AI entity is not active (rolled back or deactivated via governance).
    EntityNotActive,
    // Week 24 - Governance execution errors (D24.3)
    /// Governance proposal not found.
    ProposalNotFound,
    /// Approval gate not found.
    GateNotFound,
    /// Proposal is not executable (wrong state or timelock not elapsed).
    ProposalNotExecutable,
    /// Proposal has expired.
    ProposalExpired,
    /// Invalid proposal type for the requested operation.
    InvalidProposalType,
    /// Proposer is not authorized to submit this proposal.
    UnauthorizedProposer,
    // Week 25 - Adversarial hardening (A25.2)
    /// Proposal with this ID already exists and is not in terminal state.
    ProposalAlreadyExists,
    // Week 25 - Adversarial hardening (A25.4)
    /// Proposal contains a Tier 0 action which is NEVER allowed.
    Tier0ActionForbidden,
    /// Attempted to register an AI entity that already exists.
    EntityAlreadyExists,
    /// Attempted to register with Autonomous mode (reserved, not yet supported).
    AutonomousModeReserved,
    /// AI emergency kill switch is active — all AI entity operations blocked.
    AiKillSwitchActive,
    /// Payload version byte not recognized by the dispatcher.
    UnknownPayloadVersion {
        version: u8,
    },
    /// Transaction fee is below the minimum required for this payload type.
    FeeBelowMinimum {
        minimum: u64,
        provided: u64,
    },
    /// Reputation update targeted the issuer's own entity (self-update prohibited).
    SelfReputationUpdate,
    /// Reputation update target entity does not exist in state.
    TargetEntityNotFound,
    /// Reputation `event_type` discriminant is outside the valid range.
    InvalidReputationEventType {
        byte: u8,
    },
    /// Signal purchase referenced a seller that does not exist in state.
    SellerEntityNotFound,
    /// Signal purchase referenced a seller whose entity is deactivated.
    SellerEntityNotActive,
    /// Signal purchase target seller has no `SignalCatalog` memory object.
    SignalCatalogNotFound,
    /// Signal purchase asked for a `signal_type` not listed in the catalog.
    SignalOfferingNotFound {
        signal_type: u8,
    },
    /// Catalog offering exists but is currently inactive.
    SignalOfferingInactive,
    /// Catalog price exceeds the buyer-supplied price ceiling.
    PriceExceedsMaxPrice {
        offered: u64,
        max: u64,
    },
    /// Buyer's entity balance is insufficient to cover price + service fee.
    InsufficientEntityBalance {
        required: u128,
        available: u128,
    },
    /// Buyer and seller are the same entity (self-purchase prohibited).
    SellerIsBuyer,
    /// `StakeWithdraw` rejected because `stake_locked_until > current_height`.
    StakeStillLocked {
        unlocks_at: u64,
        current: u64,
    },
    /// `StakeWithdraw` requested more than the issuer's `stake_balance`.
    InsufficientStakeBalance {
        required: u128,
        available: u128,
    },
    /// `StakeSlash` issuer attempted to slash itself (prohibited).
    SelfSlash,
    /// `CompositionCheck` payload carried a `failure_reason` byte outside
    /// the valid range `[0, COMPOSITION_FAILURE_REASON_MAX]`.
    InvalidCompositionFailureReason {
        byte: u8,
    },
    /// `CompositionCheck` target has no `CompositionGraph` memory object.
    CompositionGraphNotFound,
    /// `CompositionCheck` referenced a `failed_dependency_idx` outside the
    /// target graph's dependency vec.
    InvalidDependencyIndex {
        index: u8,
        max: u8,
    },
    /// `CompositionCheck` claimed a dependency failure that does not match
    /// the source entity's current chain state (e.g., oracle reported
    /// inactive but source is active).
    DependencyFailureNotVerified,
    /// `CompositionGraph` create or update declared a dependency whose
    /// `source_entity_id` equals the owning entity's id (self-dependency
    /// is prohibited).
    SelfDependency,
    /// `CompositionCheck` issuer attempted to check itself (prohibited).
    SelfCompositionCheck,
    /// `ProofSubmission` payload carries a `proof_type` byte above
    /// `PROOF_TYPE_MAX`.
    UnsupportedProofType {
        proof_type: u8,
    },
    /// `ProofSubmission` v2 payload declared a `vk_bytes` length above
    /// `PROOF_SUBMISSION_MAX_VK_BYTES`.
    VerifyingKeyTooLarge {
        actual: usize,
        max: usize,
    },
    /// `ProofSubmission` v2 payload declared a `proof_bytes` length above
    /// `PROOF_SUBMISSION_MAX_PROOF_BYTES`.
    ProofBytesTooLarge {
        actual: usize,
        max: usize,
    },
    /// `ZkVerifier::verify_proof` returned `false` for a `ProofSubmission`
    /// signal. For `PROOF_TYPE_STUB` this is unreachable (stub always
    /// returns `true`); for real verifiers (`PROOF_TYPE_GROTH16`+) this
    /// fires on invalid proofs, mismatched VKs, malformed bytes, etc.
    ProofVerificationFailed,
    /// (Reserved.) Same `proof_hash` already recorded for this entity.
    /// v1 does not enforce dedup — every accepted proof produces its own
    /// `VerificationRecord`. Kept defined so a future dedup index can
    /// raise this without another `ExecError` ABI change.
    #[allow(dead_code)]
    ProofAlreadySubmitted,
    /// `SubscriptionCreate`: producer entity referenced by the tail does
    /// not exist in state.
    SubscriptionProducerNotFound,
    /// `SubscriptionCreate`: producer entity exists but `is_active` is
    /// false; subscriptions to inactive producers are rejected.
    SubscriptionProducerNotActive,
    /// `SubscriptionCreate`: subscriber and producer would be the same
    /// entity; self-subscription is not allowed.
    SubscriptionSelfReferential,
    /// `SubscriptionCreate`: `rate_per_block * duration_blocks` overflowed
    /// `u128`. Rejected before any balance mutation.
    SubscriptionRateOverflow,
    /// `SubscriptionCreate`: requested `duration_blocks` is below the
    /// `MIN_SUBSCRIPTION_DURATION` floor.
    SubscriptionDurationTooShort {
        /// Minimum required duration in blocks.
        required: u64,
        /// Duration the subscriber asked for.
        given: u64,
    },
    /// `SubscriptionCreate`: subscriber already holds the maximum allowed
    /// `Subscription` memory objects (`MAX_SUBSCRIPTIONS_PER_ENTITY`).
    /// Cancelled records still count toward the cap until the subscriber
    /// deletes them via `DELETE_MEMORY_OBJECT`.
    SubscriptionLimitExceeded {
        /// Subscriber's current `Subscription` memory object count.
        current: u32,
        /// Cap (`MAX_SUBSCRIPTIONS_PER_ENTITY`).
        max: u32,
    },
    /// `SubscriptionCreate`: subscriber's `economic_balance` does not
    /// cover the full `total_locked` amount.
    SubscriptionInsufficientBalance {
        /// Required `total_locked`.
        required: u128,
        /// Subscriber's current `economic_balance`.
        available: u128,
    },
    /// `SubscriptionCancel`: no `Subscription` memory object with the
    /// requested `subscription_id` exists under the issuer.
    SubscriptionNotFound,
    /// `SubscriptionCancel`: the referenced memory object exists but is
    /// not a `Subscription` (`object_type` mismatch).
    SubscriptionWrongObjectType,
    /// `SubscriptionCancel`: payload bytes failed to decode as
    /// `SubscriptionData`. Indicates state corruption.
    SubscriptionMemoryDecodeFailed,
    /// `SubscriptionCancel`: the issuer is not the recorded subscriber on
    /// the referenced subscription record. Only the subscriber can cancel.
    SubscriptionNotOwner,
    /// `SubscriptionCancel`: the referenced subscription has already been
    /// cancelled or expired (`is_active == false`).
    SubscriptionNotActive,
    /// `CreateMemoryObject` (Feature 8): payload bytes for a
    /// `DelegationGrant` failed to decode as `DelegationGrantData`, or
    /// the version byte does not match `DELEGATION_GRANT_VERSION`.
    InvalidDelegationGrant,
    /// `CreateMemoryObject` (Feature 8): a `DelegationGrant` names the
    /// delegator itself as the delegate. Self-delegation is rejected.
    InvalidDelegationSelf,
    /// `CreateMemoryObject` (Feature 8): the grant's
    /// `granted_capabilities` is not a subset of the delegator's static
    /// capabilities. An entity cannot delegate authority it does not hold.
    DelegationCapabilityNotHeld,
    /// `CreateMemoryObject` (Feature 8): the delegator already holds
    /// `MAX_DELEGATION_GRANTS` open `DelegationGrant` memory objects.
    /// Existing grants must be deleted before issuing more.
    DelegationCountExceeded {
        /// Delegator's current `DelegationGrant` count.
        current: u32,
        /// Cap (`MAX_DELEGATION_GRANTS`).
        max: u32,
    },
    /// `UpdateMemoryObject` (Feature 8): updating a `DelegationGrant`
    /// memory object is forbidden. Grants are immutable once issued; to
    /// change scope or duration, delete the grant and create a new one.
    DelegationGrantNotUpdatable,
    // Week 28 - Native x402 payment rail errors.
    /// `PaymentRequest`: payer and payee are the same entity. The
    /// handler refuses to settle a payment to oneself.
    PaymentSelfReferential,
    /// `PaymentRequest`: `amount` field is zero. Zero-value payments
    /// are rejected to keep replay-protection records meaningful and to
    /// guarantee every accepted payment has economic substance.
    PaymentAmountZero,
    /// `PaymentRequest`: `current_height > max_block_height`. The
    /// payment's validity window has elapsed and the signal is rejected.
    PaymentExpired {
        /// Block height at which the request was processed.
        current_height: u64,
        /// `max_block_height` field carried in the payload.
        max_block_height: u64,
    },
    /// `PaymentRequest`: payee entity not found in state.
    PaymentPayeeNotFound,
    /// `PaymentRequest`: payee entity exists but `is_active == false`.
    PaymentPayeeNotActive,
    /// `PaymentRequest`: a `PaymentRecord` already exists for this
    /// payment's `signal_hash`. The signal-level seen-set is the
    /// per-payment replay guard. Resubmitting an identical signal (or
    /// wrapping it in a fresh transaction with a different nonce) is
    /// rejected here.
    PaymentAlreadySettled {
        /// `signal_hash` of the duplicate `PaymentRequest`.
        signal_hash: [u8; 32],
    },
    /// `PaymentRequest`: payer's `economic_balance` does not cover
    /// `amount + fee`.
    PaymentInsufficientBalance {
        /// `amount + fee` debited from the payer.
        required: u128,
        /// Payer's current `economic_balance`.
        available: u128,
    },
    /// `PaymentRequest`: stored `PaymentRecord` bytes failed to decode.
    /// Indicates state corruption; never returned for a freshly-built
    /// signal.
    PaymentRecordDecodeFailed,
    /// `ServiceAttestation`: `status` byte is above
    /// `PAYMENT_ATTESTATION_STATUS_MAX`.
    ServiceAttestationInvalidStatus {
        /// `status` field carried in the payload.
        status: u8,
    },
    /// `ServiceAttestation`: no `PaymentRecord` exists under
    /// `payment_signal_hash`. Either the payment was never made or the
    /// hash was mis-typed.
    ServiceAttestationPaymentNotFound,
    /// `ServiceAttestation`: the issuer is not the payer recorded on
    /// the referenced `PaymentRecord`. Only the payer may attest.
    ServiceAttestationNotPayer,
    /// `ServiceAttestation`: `payee_entity_id` in the payload does not
    /// match the payee recorded on the `PaymentRecord`. Sanity check
    /// that prevents tampered payloads from misdirecting reputation
    /// effects.
    ServiceAttestationPayeeMismatch,
    /// `ServiceAttestation`: the referenced `PaymentRecord` has already
    /// been attested. Attestation is a once-per-payment event.
    ServiceAttestationAlreadyAttested,
}

impl<E> From<StateDecodeError> for ExecError<E> {
    fn from(e: StateDecodeError) -> Self {
        Self::Decode(e)
    }
}

/// Deterministically decode a transfer payload from `tx.payload`.
///
/// # Errors
/// Returns error if payload length or version is invalid.
pub fn decode_transfer_payload_v1(payload: &[u8]) -> Result<TransferPayloadV1, ExecError<()>> {
    const LEN: usize = 1 + 32 + 8;
    if payload.len() != LEN {
        return Err(ExecError::BadPayloadLength {
            expected: LEN,
            got: payload.len(),
        });
    }
    let ver = payload[0];
    if ver != TRANSFER_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: TRANSFER_PAYLOAD_V1,
            got: ver,
        });
    }
    let mut to = [0u8; 32];
    to.copy_from_slice(&payload[1..33]);
    let mut amt = [0u8; 8];
    amt.copy_from_slice(&payload[33..41]);
    Ok(TransferPayloadV1 {
        to,
        amount: u64::from_be_bytes(amt),
    })
}

/// Deterministically encode a transfer payload.
#[must_use]
pub fn encode_transfer_payload_v1(p: &TransferPayloadV1) -> [u8; 1 + 32 + 8] {
    let mut out = [0u8; 1 + 32 + 8];
    out[0] = TRANSFER_PAYLOAD_V1;
    out[1..33].copy_from_slice(&p.to);
    out[33..41].copy_from_slice(&p.amount.to_be_bytes());
    out
}

// ============================================================================
// SIGNAL COMMITMENT PAYLOAD (Week 14 - D14.1)
// ============================================================================

/// Signal commitment payload version.
pub const SIGNAL_COMMITMENT_PAYLOAD_V1: u8 = 2;

/// Base size of a signal commitment payload (signal types 0..=6).
pub const SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN: usize = 66;

/// Inline-extra size for a `ReputationUpdate` signal payload.
/// `target_entity_id:32 | event_type:1 | points_delta_be:2`
pub const REPUTATION_UPDATE_EXTRA_LEN: usize = 35;

/// Total size of a `ReputationUpdate` signal payload (base + extra).
pub const SIGNAL_COMMITMENT_PAYLOAD_V1_REP_LEN: usize =
    SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN + REPUTATION_UPDATE_EXTRA_LEN;

/// Inline-extra size for a `SignalPurchase` signal payload.
/// `seller_entity_id:32 | purchased_signal_type:1 | max_price_be:8`
pub const SIGNAL_PURCHASE_EXTRA_LEN: usize = 41;

/// Total size of a `SignalPurchase` signal payload (base + extra).
pub const SIGNAL_COMMITMENT_PAYLOAD_V1_PURCHASE_LEN: usize =
    SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN + SIGNAL_PURCHASE_EXTRA_LEN;

/// Inline-extra size for a `StakeDeposit` signal payload.
/// `amount_be:16`
pub const STAKE_DEPOSIT_EXTRA_LEN: usize = 16;

/// Total size of a `StakeDeposit` signal payload (base + extra).
pub const SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_DEPOSIT_LEN: usize =
    SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN + STAKE_DEPOSIT_EXTRA_LEN;

/// Inline-extra size for a `StakeWithdraw` signal payload.
/// `amount_be:16`
pub const STAKE_WITHDRAW_EXTRA_LEN: usize = 16;

/// Total size of a `StakeWithdraw` signal payload (base + extra).
pub const SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_WITHDRAW_LEN: usize =
    SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN + STAKE_WITHDRAW_EXTRA_LEN;

/// Inline-extra size for a `StakeSlash` signal payload.
/// `target_entity_id:32 | slash_amount_be:16 | rep_event_type:1 | points_delta_be:2`
pub const STAKE_SLASH_EXTRA_LEN: usize = 51;

/// Total size of a `StakeSlash` signal payload (base + extra).
pub const SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_SLASH_LEN: usize =
    SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN + STAKE_SLASH_EXTRA_LEN;

/// Inline-extra size for a `CompositionCheck` signal payload.
/// `target_entity_id:32 | failed_dependency_idx:1 | failure_reason:1`
pub const COMPOSITION_CHECK_EXTRA_LEN: usize = 34;

/// Total size of a `CompositionCheck` signal payload (base + extra).
pub const SIGNAL_COMMITMENT_PAYLOAD_V1_COMPOSITION_CHECK_LEN: usize =
    SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN + COMPOSITION_CHECK_EXTRA_LEN;

/// Inline-extra size for a v1 `ProofSubmission` signal payload (stub layout).
/// `proof_type:1 | code_hash:32 | computation_hash:32`
///
/// The v1 layout is used when `proof_type == PROOF_TYPE_STUB`. For
/// `proof_type >= PROOF_TYPE_GROTH16` the encoder switches to the v2
/// layout which appends length-prefixed `vk_bytes` and `proof_bytes`
/// after this fixed tail (see `PROOF_SUBMISSION_MAX_VK_BYTES` and
/// `PROOF_SUBMISSION_MAX_PROOF_BYTES`).
pub const PROOF_SUBMISSION_EXTRA_LEN: usize = 1 + 32 + 32;

/// Total size of a v1 `ProofSubmission` signal payload (stub layout).
pub const SIGNAL_COMMITMENT_PAYLOAD_V1_PROOF_LEN: usize =
    SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN + PROOF_SUBMISSION_EXTRA_LEN;

/// Maximum length of the `vk_bytes` field in a v2 `ProofSubmission` payload.
///
/// Set to 8 KiB. The canonical BN254 Groth16 VK for 4 public inputs is
/// roughly 200-300 bytes compressed; the cap is a generous denial-of-service
/// guard.
pub const PROOF_SUBMISSION_MAX_VK_BYTES: usize = 8 * 1024;

/// Maximum length of the `proof_bytes` field in a v2 `ProofSubmission`
/// payload (1 KiB). BN254 Groth16 proofs are roughly 128 bytes compressed.
pub const PROOF_SUBMISSION_MAX_PROOF_BYTES: usize = 1024;

/// Minimum total size of a v2 `ProofSubmission` payload (empty vk + empty
/// proof): base + extra + `vk_len`:4 + `proof_len`:4.
pub const SIGNAL_COMMITMENT_PAYLOAD_V2_PROOF_MIN_LEN: usize =
    SIGNAL_COMMITMENT_PAYLOAD_V1_PROOF_LEN + 4 + 4;

/// Inline-extra size for a `SubscriptionCreate` signal payload.
/// `producer_entity_id:32 | covered_signal_type:1 | rate_per_block_be:8 |
/// duration_blocks_be:8`.
pub const SUBSCRIPTION_CREATE_EXTRA_LEN: usize = 32 + 1 + 8 + 8;

/// Total size of a `SubscriptionCreate` signal payload (base + extra).
pub const SIGNAL_COMMITMENT_PAYLOAD_V1_SUBSCRIPTION_CREATE_LEN: usize =
    SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN + SUBSCRIPTION_CREATE_EXTRA_LEN;

/// Inline-extra size for a `SubscriptionCancel` signal payload.
/// `subscription_id:32` (memory object id of the `Subscription` record).
pub const SUBSCRIPTION_CANCEL_EXTRA_LEN: usize = 32;

/// Total size of a `SubscriptionCancel` signal payload (base + extra).
pub const SIGNAL_COMMITMENT_PAYLOAD_V1_SUBSCRIPTION_CANCEL_LEN: usize =
    SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN + SUBSCRIPTION_CANCEL_EXTRA_LEN;

/// Inline-extra size for a `PaymentRequest` signal payload.
/// `payee_entity_id:32 | amount_be:8 | service_descriptor_hash:32 |
/// request_hash:32 | max_block_height_be:8`.
pub const PAYMENT_REQUEST_EXTRA_LEN: usize = 32 + 8 + 32 + 32 + 8;

/// Total size of a `PaymentRequest` signal payload (base + extra).
pub const SIGNAL_COMMITMENT_PAYLOAD_V1_PAYMENT_REQUEST_LEN: usize =
    SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN + PAYMENT_REQUEST_EXTRA_LEN;

/// Inline-extra size for a `ServiceAttestation` signal payload.
/// `payment_signal_hash:32 | payee_entity_id:32 | status:1`.
pub const SERVICE_ATTESTATION_EXTRA_LEN: usize = 32 + 32 + 1;

/// Total size of a `ServiceAttestation` signal payload (base + extra).
pub const SIGNAL_COMMITMENT_PAYLOAD_V1_SERVICE_ATTESTATION_LEN: usize =
    SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN + SERVICE_ATTESTATION_EXTRA_LEN;

/// `ServiceAttestation` status discriminant: service was delivered.
pub const PAYMENT_ATTESTATION_STATUS_DELIVERED: u8 = 0;
/// `ServiceAttestation` status discriminant: service was NOT delivered.
pub const PAYMENT_ATTESTATION_STATUS_FAILED: u8 = 1;
/// Maximum valid `ServiceAttestation` status discriminant.
pub const PAYMENT_ATTESTATION_STATUS_MAX: u8 = PAYMENT_ATTESTATION_STATUS_FAILED;
/// Sentinel stored in `PaymentRecord.attested_status` before any
/// attestation has been recorded against the payment.
pub const PAYMENT_ATTESTATION_STATUS_NONE: u8 = 0xFF;

/// Fee basis points applied on every `PaymentRequest`.
///
/// Kept as a distinct constant (rather than reusing `MARKETPLACE_FEE_BPS`)
/// so the payment-rail fee can be governance-tuned without affecting
/// signal-purchase or subscription pricing.
pub const PAYMENT_FEE_BPS: u128 = 200;

/// `PaymentRecord` wire-format version byte.
pub const PAYMENT_RECORD_V1: u8 = 1;

/// Encoded size of a `PaymentRecord` (the value stored under
/// `payment_by_hash_key`).
///
/// Layout: `version:1 | payer:32 | payee:32 | amount_be:8 |
/// service_descriptor_hash:32 | request_hash:32 | payment_height_be:8 |
/// max_block_height_be:8 | attested_status:1 | attested_height_be:8`.
pub const PAYMENT_RECORD_LEN: usize = 1 + 32 + 32 + 8 + 32 + 32 + 8 + 8 + 1 + 8;

/// KV-key prefix for the canonical payment record store indexed by the
/// payment's signal hash. `b"ai/payments/by_hash/" || signal_hash[32]`.
pub const KEY_PREFIX_AI_PAYMENTS_BY_HASH: &[u8] = b"ai/payments/by_hash/";
/// KV-key prefix for the per-payer scan index.
/// `b"ai/payments/by_payer/" || payer[32] || height_be[8] || signal_hash[32]`.
pub const KEY_PREFIX_AI_PAYMENTS_BY_PAYER: &[u8] = b"ai/payments/by_payer/";
/// KV-key prefix for the per-payee scan index.
/// `b"ai/payments/by_payee/" || payee[32] || height_be[8] || signal_hash[32]`.
pub const KEY_PREFIX_AI_PAYMENTS_BY_PAYEE: &[u8] = b"ai/payments/by_payee/";

/// Block-count duration that newly deposited stake is locked for.
/// `stake_locked_until = current_height + STAKE_LOCK_PERIOD` on every deposit.
pub const STAKE_LOCK_PERIOD: u64 = 1000;

/// Minimum stake required for an entity to publish a `SignalCatalog` memory
/// object. Set to `0` (gate disabled). Reserved for governance activation.
pub const MIN_STAKE_FOR_CATALOG: u128 = 0;

// Reputation event_type discriminants (carried inline in the signal payload).
/// Job/transaction successfully completed by the target entity.
pub const REP_EVENT_JOB_COMPLETED: u8 = 0;
/// Dispute resolved in favour of the deliverer.
pub const REP_EVENT_DISPUTE_WON_DELIVERER: u8 = 1;
/// Dispute resolved in favour of the customer.
pub const REP_EVENT_DISPUTE_WON_CUSTOMER: u8 = 2;
/// Fraud detected; oracle is asserting a strong negative judgement.
pub const REP_EVENT_FRAUD_DETECTED: u8 = 3;
/// Auto-release applied because the customer did not respond in time.
pub const REP_EVENT_AUTO_RELEASE_PENALTY: u8 = 4;
/// Reputation decay applied for inactivity.
pub const REP_EVENT_DECAY: u8 = 5;
/// Stake slash applied; oracle is slashing the target's `stake_balance`.
pub const REP_EVENT_STAKE_SLASH: u8 = 6;
/// Composition dependency failure detected; oracle is auto-pausing the target
/// owner of a `CompositionGraph` whose required dependency has failed.
pub const REP_EVENT_COMPOSITION_FAILURE: u8 = 7;
/// ZK proof verified successfully by the `ProofSubmission` signal handler.
/// Applied to the issuing entity with delta +3.
pub const REP_EVENT_PROOF_VERIFIED: u8 = 8;
/// ZK proof verification failed (reserved).
///
/// Currently unreachable: the v1 handler rejects the transaction with
/// `ProofVerificationFailed` instead of recording an on-chain failure
/// event. Defined here for forward compatibility with a future
/// "record + slash" failure path.
pub const REP_EVENT_PROOF_FAILED: u8 = 9;
/// Payer-issued attestation of successful service delivery.
///
/// Applied to the payee of a prior `PaymentRequest` with delta
/// `REP_DELTA_PAYMENT_DELIVERED`. Handler-emitted by the
/// `ServiceAttestation` signal; never user-supplied with a custom delta.
pub const REP_EVENT_PAYMENT_DELIVERED: u8 = 10;
/// Payer-issued attestation of failed service delivery.
///
/// Applied to the payee of a prior `PaymentRequest` with delta
/// `REP_DELTA_PAYMENT_FAILED`. Handler-emitted by the
/// `ServiceAttestation` signal; never user-supplied with a custom delta.
pub const REP_EVENT_PAYMENT_FAILED: u8 = 11;
/// Maximum valid reputation `event_type` discriminant.
pub const REP_EVENT_MAX: u8 = REP_EVENT_PAYMENT_FAILED;

/// Reputation delta on a `Delivered` attestation.
///
/// Calibrated below `REP_DELTA_PROOF_VERIFIED` (+3) because attestation
/// is self-reported by the payer without cryptographic proof.
pub const REP_DELTA_PAYMENT_DELIVERED: i32 = 1;
/// Reputation delta on a `Failed` attestation.
///
/// Magnitude exceeds `REP_DELTA_COMPOSITION_FAILURE` (-1) because a
/// failed payment is a stronger negative signal than a composition
/// mismatch.
pub const REP_DELTA_PAYMENT_FAILED: i32 = -3;

// CompositionCheck failure_reason discriminants (carried inline in the
// signal payload). The handler verifies the reported reason against the
// source entity's current chain state and rejects mismatches with
// `DependencyFailureNotVerified`.
/// Source entity exists but `is_active == false`.
pub const COMPOSITION_FAILURE_SOURCE_INACTIVE: u8 = 0;
/// Source entity exists, is active, but its `reputation_score` is below
/// the dependency's declared `min_reputation`.
pub const COMPOSITION_FAILURE_REPUTATION_BELOW_MIN: u8 = 1;
/// Source entity exists, is active, but its `stake_balance` is below
/// the dependency's declared `min_stake`.
pub const COMPOSITION_FAILURE_STAKE_BELOW_MIN: u8 = 2;
/// Source entity does not exist in state.
pub const COMPOSITION_FAILURE_SOURCE_NOT_FOUND: u8 = 3;
/// Maximum valid `failure_reason` discriminant.
pub const COMPOSITION_FAILURE_REASON_MAX: u8 = COMPOSITION_FAILURE_SOURCE_NOT_FOUND;

// ProofSubmission proof_type discriminants (carried inline in the
// `ProofSubmission` signal payload). The handler rejects any value above
// `PROOF_TYPE_MAX` with `UnsupportedProofType`. The decoder/encoder pick
// the wire layout from this byte: `PROOF_TYPE_STUB` uses the 131-byte v1
// layout; any other accepted value uses the variable-length v2 layout
// carrying length-prefixed `vk_bytes` and `proof_bytes`.
/// Stub verifier (always accepts; NOT for production proofs).
pub const PROOF_TYPE_STUB: u8 = 0;
/// BN254 Groth16 verifier, served by `novai_crypto::Groth16Verifier`.
pub const PROOF_TYPE_GROTH16: u8 = 1;
/// Reserved for a future PLONK verifier integration.
pub const PROOF_TYPE_PLONK: u8 = 2;
/// Maximum `proof_type` discriminant accepted by the handler.
///
/// The decoder rejects any higher value with `UnsupportedProofType` before
/// the handler runs. Bumping this constant is the gate for activating each
/// new verifier; activating PLONK would require both raising this and
/// wiring the dispatch in `apply_signal_commitment_tx_inner`.
pub const PROOF_TYPE_MAX: u8 = PROOF_TYPE_GROTH16;

/// Inline reputation-update tail carried in `ReputationUpdate` signal payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReputationUpdateExtraV1 {
    /// Entity whose reputation should be mutated.
    pub target_entity_id: [u8; 32],
    /// Reputation `event_type` discriminant (see `REP_EVENT_*` constants).
    pub event_type: u8,
    /// Signed delta to apply (i16 big-endian on the wire). Final score is
    /// clamped to [0, 100] in the execution handler.
    pub points_delta: i16,
}

/// Inline purchase tail carried in `SignalPurchase` signal payloads.
///
/// Wire layout (41 bytes): `seller_entity_id:32 | purchased_signal_type:1 | max_price_be:8`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalPurchaseExtraV1 {
    /// AI entity selling the signal (catalog owner).
    pub seller_entity_id: [u8; 32],
    /// `AiSignalType` byte the buyer is purchasing.
    pub purchased_signal_type: u8,
    /// Buyer-side price ceiling. Execution rejects the purchase if the
    /// catalog's current price exceeds this value.
    pub max_price: u64,
}

/// Inline stake-deposit tail carried in `StakeDeposit` signal payloads.
///
/// Wire layout (16 bytes): `amount_be:16`. The issuer entity is debited
/// from `economic_balance` and credited to `stake_balance`, and
/// `stake_locked_until` is set to `current_height + STAKE_LOCK_PERIOD`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StakeDepositExtraV1 {
    /// Amount to move from `economic_balance` to `stake_balance`.
    pub amount: u128,
}

/// Inline stake-withdraw tail carried in `StakeWithdraw` signal payloads.
///
/// Wire layout (16 bytes): `amount_be:16`. The issuer entity is debited
/// from `stake_balance` and credited to `economic_balance`, but only when
/// `stake_locked_until <= current_height`. Partial withdrawals leave the
/// remaining `stake_balance` unlocked (no re-lock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StakeWithdrawExtraV1 {
    /// Amount to move from `stake_balance` back to `economic_balance`.
    pub amount: u128,
}

/// Inline slash tail carried in `StakeSlash` signal payloads.
///
/// Wire layout (51 bytes): `target_entity_id:32 | slash_amount_be:16 |
/// rep_event_type:1 | points_delta_be:2`. Every slash MUST carry a
/// reputation update; the handler applies the rep delta and the stake
/// deduction in a single atomic batch. Slashing is saturating: requesting
/// more than the target's `stake_balance` deducts only what is available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StakeSlashExtraV1 {
    /// Entity whose stake is being slashed.
    pub target_entity_id: [u8; 32],
    /// Amount to deduct from `target.stake_balance` and credit to
    /// `KEY_SLASH_TREASURY`. Saturating against `stake_balance`.
    pub slash_amount: u128,
    /// Reputation event discriminant accompanying the slash. Validated
    /// against `REP_EVENT_MAX`.
    pub rep_event_type: u8,
    /// Reputation points delta clamped into [0, 100] on the target.
    pub points_delta: i16,
}

/// Inline composition-check tail carried in `CompositionCheck` signal payloads.
///
/// Wire layout (34 bytes): `target_entity_id:32 | failed_dependency_idx:1 |
/// failure_reason:1`. The handler reads the target's latest
/// `CompositionGraph` memory object, looks up the dependency at
/// `failed_dependency_idx`, then independently verifies the claimed
/// `failure_reason` against the source entity's current state. Verified
/// failures with `is_required = true` set `target.is_active = false` and
/// always emit a `REP_EVENT_COMPOSITION_FAILURE` event with delta -1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionCheckExtraV1 {
    /// Entity whose composition graph is being checked.
    pub target_entity_id: [u8; 32],
    /// Index into the target's `CompositionGraphData.dependencies` vec.
    pub failed_dependency_idx: u8,
    /// Failure mode the oracle is reporting. Must be one of the
    /// `COMPOSITION_FAILURE_*` constants (validated at decode time
    /// against `COMPOSITION_FAILURE_REASON_MAX`).
    pub failure_reason: u8,
}

/// Inline proof-submission tail carried in `ProofSubmission` signal payloads.
///
/// The wire layout depends on `proof_type`:
///
/// - **v1 (stub, 65 bytes)** — used when `proof_type == PROOF_TYPE_STUB`.
///   `proof_type:1 | code_hash:32 | computation_hash:32`. `vk_bytes` and
///   `proof_bytes` MUST be empty.
/// - **v2 (real verifier, variable)** — used when
///   `proof_type >= PROOF_TYPE_GROTH16`.
///   `proof_type:1 | code_hash:32 | computation_hash:32 | vk_len_be:4 |
///   vk_bytes | proof_len_be:4 | proof_bytes`. `vk_bytes.len()` MUST be
///   `<= PROOF_SUBMISSION_MAX_VK_BYTES`; `proof_bytes.len()` MUST be
///   `<= PROOF_SUBMISSION_MAX_PROOF_BYTES`.
///
/// The decoder validates `proof_type <= PROOF_TYPE_MAX`. `code_hash` is
/// the AI module/weight hash the proof attests to; `computation_hash`
/// identifies the specific computation context. The verifier consumes both
/// as 64 bytes of public inputs (`code_hash || computation_hash`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofSubmissionExtraV1 {
    /// Discriminant identifying the proof system (see `PROOF_TYPE_*`).
    pub proof_type: u8,
    /// Hash of the AI module code/weights the proof attests to.
    pub code_hash: [u8; 32],
    /// Hash of the computation context (inputs/outputs) the proof asserts.
    pub computation_hash: [u8; 32],
    /// Verifying-key bytes for real verifiers (v2 layout). Empty for
    /// `PROOF_TYPE_STUB`. For `PROOF_TYPE_GROTH16` this is the
    /// ark-serialize compressed `VerifyingKey<Bn254>`.
    pub vk_bytes: Vec<u8>,
    /// Proof bytes for real verifiers (v2 layout). Empty for
    /// `PROOF_TYPE_STUB`. For `PROOF_TYPE_GROTH16` this is the
    /// ark-serialize compressed `Proof<Bn254>`.
    pub proof_bytes: Vec<u8>,
}

/// Inline subscription-create tail carried in `SubscriptionCreate` signal
/// payloads (Feature 9).
///
/// Wire layout (49 bytes): `producer_entity_id:32 | covered_signal_type:1 |
/// rate_per_block_be:8 | duration_blocks_be:8`. The handler debits the
/// issuer's `economic_balance` by `rate_per_block * duration_blocks`
/// (checked u128 multiplication) and creates a `Subscription` memory
/// object owned by the issuer with `start_height = current_height` and
/// `end_height = current_height + duration_blocks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionCreateExtraV1 {
    /// AI entity that will receive accrued payment under this subscription.
    pub producer_entity_id: [u8; 32],
    /// `AiSignalType` byte the subscription pays the producer for
    /// (informational; not enforced at handler time).
    pub covered_signal_type: u8,
    /// Per-block payment rate, in base units of `economic_balance`.
    pub rate_per_block: u64,
    /// Number of blocks the subscription will run from `current_height`.
    /// Must satisfy `>= MIN_SUBSCRIPTION_DURATION` (validated in the handler).
    pub duration_blocks: u64,
}

/// Inline subscription-cancel tail carried in `SubscriptionCancel` signal
/// payloads (Feature 9).
///
/// Wire layout (32 bytes): `subscription_id:32`. The handler loads the
/// referenced `Subscription` memory object, verifies the issuer is the
/// recorded subscriber, settles accrued payment to the producer (with the
/// 2% marketplace fee), pays the producer the 5% cancel fee on the
/// remaining locked funds (no marketplace cut on the cancel fee), refunds
/// the rest to the subscriber, and rewrites the memory object with
/// `is_active = false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionCancelExtraV1 {
    /// Memory object id of the `Subscription` record being cancelled.
    pub subscription_id: [u8; 32],
}

/// Inline payment-request tail carried in `PaymentRequest` signal
/// payloads (Week 28, native x402 rail).
///
/// Wire layout (112 bytes): `payee_entity_id:32 | amount_be:8 |
/// service_descriptor_hash:32 | request_hash:32 | max_block_height_be:8`.
/// The handler debits the issuer's `economic_balance` by `amount + fee`
/// (where `fee = amount * PAYMENT_FEE_BPS / BPS_DENOMINATOR`), credits
/// the payee's `economic_balance` by `amount`, and routes the fee to
/// `KEY_MARKETPLACE_TREASURY`. Replay protection is enforced by writing a
/// `PaymentRecord` under `payment_by_hash_key(signal_hash)` and refusing
/// any subsequent `PaymentRequest` whose `signal_hash` already has a
/// record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentRequestExtraV1 {
    /// AI entity that receives the payment.
    pub payee_entity_id: [u8; 32],
    /// Payment amount, in base units of `economic_balance`. Must be
    /// non-zero (the handler rejects zero-amount payments with
    /// `PaymentAmountZero`).
    pub amount: u64,
    /// Opaque service identifier (caller-computed). Never decoded
    /// on-chain. Useful for off-chain analytics (e.g., "all payments
    /// to this API").
    pub service_descriptor_hash: [u8; 32],
    /// Opaque per-request identifier (caller-computed). Combined with
    /// the rest of the envelope, this is what makes `signal_hash`
    /// unique per request and is therefore what the seen-set in
    /// `payment_by_hash_key` is keyed on.
    pub request_hash: [u8; 32],
    /// Absolute block height past which this payment is no longer
    /// valid. The handler rejects with `PaymentExpired` if
    /// `current_height > max_block_height`.
    pub max_block_height: u64,
}

/// Inline service-attestation tail carried in `ServiceAttestation` signal
/// payloads (Week 28).
///
/// Wire layout (65 bytes): `payment_signal_hash:32 | payee_entity_id:32 |
/// status:1`. The issuer of this signal MUST be the payer recorded on
/// the referenced `PaymentRecord`; otherwise the handler returns
/// `ServiceAttestationNotPayer`. On success the handler applies
/// `REP_EVENT_PAYMENT_DELIVERED` (delta `+REP_DELTA_PAYMENT_DELIVERED`)
/// or `REP_EVENT_PAYMENT_FAILED` (delta `REP_DELTA_PAYMENT_FAILED`) to
/// the payee depending on `status`, and rewrites the `PaymentRecord`
/// with the attested status and height. A record can be attested at
/// most once; resubmission fails with
/// `ServiceAttestationAlreadyAttested`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAttestationExtraV1 {
    /// `signal_hash` of the `PaymentRequest` being attested.
    pub payment_signal_hash: [u8; 32],
    /// Payee recorded on the referenced `PaymentRecord`. Cross-checked
    /// against the stored record to catch tampered payloads.
    pub payee_entity_id: [u8; 32],
    /// Attestation outcome. Must be one of the
    /// `PAYMENT_ATTESTATION_STATUS_*` constants (validated at decode
    /// time against `PAYMENT_ATTESTATION_STATUS_MAX`).
    pub status: u8,
}

/// Canonical Signal Commitment payload (D14.1):
/// - Base (signal types 0..=6): 66 bytes
///   `[version:1][signal_hash:32][signal_type:1][issuer_entity_id:32]`
/// - `ReputationUpdate` (signal type 7): 101 bytes (base + 35-byte tail)
///   `... [target_entity_id:32][event_type:1][points_delta_be:2]`
/// - `SignalPurchase` (signal type 8): 107 bytes (base + 41-byte tail)
///   `... [seller_entity_id:32][purchased_signal_type:1][max_price_be:8]`
/// - `StakeDeposit` (signal type 9): 82 bytes (base + 16-byte tail)
///   `... [amount_be:16]`
/// - `StakeWithdraw` (signal type 10): 82 bytes (base + 16-byte tail)
///   `... [amount_be:16]`
/// - `StakeSlash` (signal type 11): 117 bytes (base + 51-byte tail)
///   `... [target_id:32][slash_amount_be:16][rep_event_type:1][points_delta_be:2]`
/// - `CompositionCheck` (signal type 12): 100 bytes (base + 34-byte tail)
///   `... [target_id:32][failed_dependency_idx:1][failure_reason:1]`
/// - `ProofSubmission` (signal type 13): 131 bytes (base + 65-byte tail)
///   `... [proof_type:1][code_hash:32][computation_hash:32]`
/// - `SubscriptionCreate` (signal type 14): 115 bytes (base + 49-byte tail)
///   `... [producer_id:32][covered_signal_type:1][rate_per_block_be:8][duration_blocks_be:8]`
/// - `SubscriptionCancel` (signal type 15): 98 bytes (base + 32-byte tail)
///   `... [subscription_id:32]`
/// - `PaymentRequest` (signal type 16): 178 bytes (base + 112-byte tail)
///   `... [payee_id:32][amount_be:8][service_descriptor_hash:32][request_hash:32][max_block_height_be:8]`
/// - `ServiceAttestation` (signal type 17): 131 bytes (base + 65-byte tail)
///   `... [payment_signal_hash:32][payee_id:32][status:1]`
///
/// At most one tail is populated; the active tail is determined by `signal_type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalCommitmentPayloadV1 {
    /// Commitment hash of the full signal.
    pub signal_hash: [u8; 32],
    /// Signal type (0..=17).
    pub signal_type: novai_ai_entities::AiSignalType,
    /// AI entity ID that issued this signal.
    pub issuer_entity_id: [u8; 32],
    /// Inline reputation tail. MUST be `Some` iff `signal_type == ReputationUpdate`.
    pub reputation: Option<ReputationUpdateExtraV1>,
    /// Inline purchase tail. MUST be `Some` iff `signal_type == SignalPurchase`.
    pub purchase: Option<SignalPurchaseExtraV1>,
    /// Inline stake-deposit tail. MUST be `Some` iff `signal_type == StakeDeposit`.
    pub stake_deposit: Option<StakeDepositExtraV1>,
    /// Inline stake-withdraw tail. MUST be `Some` iff `signal_type == StakeWithdraw`.
    pub stake_withdraw: Option<StakeWithdrawExtraV1>,
    /// Inline slash tail. MUST be `Some` iff `signal_type == StakeSlash`.
    pub stake_slash: Option<StakeSlashExtraV1>,
    /// Inline composition-check tail. MUST be `Some` iff
    /// `signal_type == CompositionCheck`.
    pub composition_check: Option<CompositionCheckExtraV1>,
    /// Inline proof-submission tail. MUST be `Some` iff
    /// `signal_type == ProofSubmission`.
    pub proof_submission: Option<ProofSubmissionExtraV1>,
    /// Inline subscription-create tail. MUST be `Some` iff
    /// `signal_type == SubscriptionCreate`.
    pub subscription_create: Option<SubscriptionCreateExtraV1>,
    /// Inline subscription-cancel tail. MUST be `Some` iff
    /// `signal_type == SubscriptionCancel`.
    pub subscription_cancel: Option<SubscriptionCancelExtraV1>,
    /// Inline payment-request tail. MUST be `Some` iff
    /// `signal_type == PaymentRequest`.
    pub payment_request: Option<PaymentRequestExtraV1>,
    /// Inline service-attestation tail. MUST be `Some` iff
    /// `signal_type == ServiceAttestation`.
    pub service_attestation: Option<ServiceAttestationExtraV1>,
}

/// Deterministically encode a signal commitment payload.
///
/// Returns 66 bytes for base signals, 101 bytes for `ReputationUpdate`,
/// 107 bytes for `SignalPurchase`, 82 bytes for `StakeDeposit` /
/// `StakeWithdraw`, and 117 bytes for `StakeSlash`. Panics in debug if a
/// tail is set inconsistently with `signal_type`; in release builds the
/// inconsistency is silently fixed by zero-padding the active tail.
///
/// # Panics
///
/// Panics if a `ProofSubmission` v2 tail carries `vk_bytes` or `proof_bytes`
/// whose lengths do not fit in a `u32`. Callers must keep these under
/// `PROOF_SUBMISSION_MAX_VK_BYTES` and `PROOF_SUBMISSION_MAX_PROOF_BYTES`
/// respectively; both caps are well below `u32::MAX`.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn encode_signal_commitment_payload_v1(p: &SignalCommitmentPayloadV1) -> Vec<u8> {
    let is_reputation = p.signal_type == novai_ai_entities::AiSignalType::ReputationUpdate;
    let is_purchase = p.signal_type == novai_ai_entities::AiSignalType::SignalPurchase;
    let is_stake_deposit = p.signal_type == novai_ai_entities::AiSignalType::StakeDeposit;
    let is_stake_withdraw = p.signal_type == novai_ai_entities::AiSignalType::StakeWithdraw;
    let is_stake_slash = p.signal_type == novai_ai_entities::AiSignalType::StakeSlash;
    let is_composition_check = p.signal_type == novai_ai_entities::AiSignalType::CompositionCheck;
    let is_proof_submission = p.signal_type == novai_ai_entities::AiSignalType::ProofSubmission;
    let is_subscription_create =
        p.signal_type == novai_ai_entities::AiSignalType::SubscriptionCreate;
    let is_subscription_cancel =
        p.signal_type == novai_ai_entities::AiSignalType::SubscriptionCancel;
    let is_payment_request = p.signal_type == novai_ai_entities::AiSignalType::PaymentRequest;
    let is_service_attestation =
        p.signal_type == novai_ai_entities::AiSignalType::ServiceAttestation;
    debug_assert_eq!(
        is_reputation,
        p.reputation.is_some(),
        "reputation tail presence must match signal_type"
    );
    debug_assert_eq!(
        is_purchase,
        p.purchase.is_some(),
        "purchase tail presence must match signal_type"
    );
    debug_assert_eq!(
        is_stake_deposit,
        p.stake_deposit.is_some(),
        "stake_deposit tail presence must match signal_type"
    );
    debug_assert_eq!(
        is_stake_withdraw,
        p.stake_withdraw.is_some(),
        "stake_withdraw tail presence must match signal_type"
    );
    debug_assert_eq!(
        is_stake_slash,
        p.stake_slash.is_some(),
        "stake_slash tail presence must match signal_type"
    );
    debug_assert_eq!(
        is_composition_check,
        p.composition_check.is_some(),
        "composition_check tail presence must match signal_type"
    );
    debug_assert_eq!(
        is_proof_submission,
        p.proof_submission.is_some(),
        "proof_submission tail presence must match signal_type"
    );
    debug_assert_eq!(
        is_subscription_create,
        p.subscription_create.is_some(),
        "subscription_create tail presence must match signal_type"
    );
    debug_assert_eq!(
        is_subscription_cancel,
        p.subscription_cancel.is_some(),
        "subscription_cancel tail presence must match signal_type"
    );
    debug_assert_eq!(
        is_payment_request,
        p.payment_request.is_some(),
        "payment_request tail presence must match signal_type"
    );
    debug_assert_eq!(
        is_service_attestation,
        p.service_attestation.is_some(),
        "service_attestation tail presence must match signal_type"
    );
    debug_assert!(
        u8::from(is_reputation)
            + u8::from(is_purchase)
            + u8::from(is_stake_deposit)
            + u8::from(is_stake_withdraw)
            + u8::from(is_stake_slash)
            + u8::from(is_composition_check)
            + u8::from(is_proof_submission)
            + u8::from(is_subscription_create)
            + u8::from(is_subscription_cancel)
            + u8::from(is_payment_request)
            + u8::from(is_service_attestation)
            <= 1,
        "tails are mutually exclusive"
    );

    let total = if is_reputation {
        SIGNAL_COMMITMENT_PAYLOAD_V1_REP_LEN
    } else if is_purchase {
        SIGNAL_COMMITMENT_PAYLOAD_V1_PURCHASE_LEN
    } else if is_stake_deposit {
        SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_DEPOSIT_LEN
    } else if is_stake_withdraw {
        SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_WITHDRAW_LEN
    } else if is_stake_slash {
        SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_SLASH_LEN
    } else if is_composition_check {
        SIGNAL_COMMITMENT_PAYLOAD_V1_COMPOSITION_CHECK_LEN
    } else if is_proof_submission {
        // v1 (131 bytes) for PROOF_TYPE_STUB; v2 (variable) otherwise.
        match p.proof_submission.as_ref() {
            Some(extra) if extra.proof_type != PROOF_TYPE_STUB => {
                SIGNAL_COMMITMENT_PAYLOAD_V1_PROOF_LEN
                    + 4
                    + extra.vk_bytes.len()
                    + 4
                    + extra.proof_bytes.len()
            }
            _ => SIGNAL_COMMITMENT_PAYLOAD_V1_PROOF_LEN,
        }
    } else if is_subscription_create {
        SIGNAL_COMMITMENT_PAYLOAD_V1_SUBSCRIPTION_CREATE_LEN
    } else if is_subscription_cancel {
        SIGNAL_COMMITMENT_PAYLOAD_V1_SUBSCRIPTION_CANCEL_LEN
    } else if is_payment_request {
        SIGNAL_COMMITMENT_PAYLOAD_V1_PAYMENT_REQUEST_LEN
    } else if is_service_attestation {
        SIGNAL_COMMITMENT_PAYLOAD_V1_SERVICE_ATTESTATION_LEN
    } else {
        SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN
    };
    let mut out = Vec::with_capacity(total);
    out.push(SIGNAL_COMMITMENT_PAYLOAD_V1);
    out.extend_from_slice(&p.signal_hash);
    out.push(p.signal_type.to_byte());
    out.extend_from_slice(&p.issuer_entity_id);

    if is_reputation {
        if let Some(extra) = &p.reputation {
            out.extend_from_slice(&extra.target_entity_id);
            out.push(extra.event_type);
            out.extend_from_slice(&extra.points_delta.to_be_bytes());
        } else {
            // Zero-tail in the inconsistent-release-build path.
            out.extend_from_slice(&[0u8; REPUTATION_UPDATE_EXTRA_LEN]);
        }
    } else if is_purchase {
        if let Some(extra) = &p.purchase {
            out.extend_from_slice(&extra.seller_entity_id);
            out.push(extra.purchased_signal_type);
            out.extend_from_slice(&extra.max_price.to_be_bytes());
        } else {
            // Zero-tail in the inconsistent-release-build path.
            out.extend_from_slice(&[0u8; SIGNAL_PURCHASE_EXTRA_LEN]);
        }
    } else if is_stake_deposit {
        if let Some(extra) = &p.stake_deposit {
            out.extend_from_slice(&extra.amount.to_be_bytes());
        } else {
            // Zero-tail in the inconsistent-release-build path.
            out.extend_from_slice(&[0u8; STAKE_DEPOSIT_EXTRA_LEN]);
        }
    } else if is_stake_withdraw {
        if let Some(extra) = &p.stake_withdraw {
            out.extend_from_slice(&extra.amount.to_be_bytes());
        } else {
            // Zero-tail in the inconsistent-release-build path.
            out.extend_from_slice(&[0u8; STAKE_WITHDRAW_EXTRA_LEN]);
        }
    } else if is_stake_slash {
        if let Some(extra) = &p.stake_slash {
            out.extend_from_slice(&extra.target_entity_id);
            out.extend_from_slice(&extra.slash_amount.to_be_bytes());
            out.push(extra.rep_event_type);
            out.extend_from_slice(&extra.points_delta.to_be_bytes());
        } else {
            // Zero-tail in the inconsistent-release-build path.
            out.extend_from_slice(&[0u8; STAKE_SLASH_EXTRA_LEN]);
        }
    } else if is_composition_check {
        if let Some(extra) = &p.composition_check {
            out.extend_from_slice(&extra.target_entity_id);
            out.push(extra.failed_dependency_idx);
            out.push(extra.failure_reason);
        } else {
            // Zero-tail in the inconsistent-release-build path.
            out.extend_from_slice(&[0u8; COMPOSITION_CHECK_EXTRA_LEN]);
        }
    } else if is_proof_submission {
        if let Some(extra) = &p.proof_submission {
            out.push(extra.proof_type);
            out.extend_from_slice(&extra.code_hash);
            out.extend_from_slice(&extra.computation_hash);
            if extra.proof_type != PROOF_TYPE_STUB {
                // v2 tail: vk_len_be:4 | vk_bytes | proof_len_be:4 | proof_bytes
                let vk_len = u32::try_from(extra.vk_bytes.len())
                    .expect("vk_bytes len fits in u32 (bounded by PROOF_SUBMISSION_MAX_VK_BYTES)");
                out.extend_from_slice(&vk_len.to_be_bytes());
                out.extend_from_slice(&extra.vk_bytes);
                let proof_len = u32::try_from(extra.proof_bytes.len()).expect(
                    "proof_bytes len fits in u32 (bounded by PROOF_SUBMISSION_MAX_PROOF_BYTES)",
                );
                out.extend_from_slice(&proof_len.to_be_bytes());
                out.extend_from_slice(&extra.proof_bytes);
            }
        } else {
            // Zero-tail in the inconsistent-release-build path.
            out.extend_from_slice(&[0u8; PROOF_SUBMISSION_EXTRA_LEN]);
        }
    } else if is_subscription_create {
        if let Some(extra) = &p.subscription_create {
            out.extend_from_slice(&extra.producer_entity_id);
            out.push(extra.covered_signal_type);
            out.extend_from_slice(&extra.rate_per_block.to_be_bytes());
            out.extend_from_slice(&extra.duration_blocks.to_be_bytes());
        } else {
            // Zero-tail in the inconsistent-release-build path.
            out.extend_from_slice(&[0u8; SUBSCRIPTION_CREATE_EXTRA_LEN]);
        }
    } else if is_subscription_cancel {
        if let Some(extra) = &p.subscription_cancel {
            out.extend_from_slice(&extra.subscription_id);
        } else {
            // Zero-tail in the inconsistent-release-build path.
            out.extend_from_slice(&[0u8; SUBSCRIPTION_CANCEL_EXTRA_LEN]);
        }
    } else if is_payment_request {
        if let Some(extra) = &p.payment_request {
            out.extend_from_slice(&extra.payee_entity_id);
            out.extend_from_slice(&extra.amount.to_be_bytes());
            out.extend_from_slice(&extra.service_descriptor_hash);
            out.extend_from_slice(&extra.request_hash);
            out.extend_from_slice(&extra.max_block_height.to_be_bytes());
        } else {
            // Zero-tail in the inconsistent-release-build path.
            out.extend_from_slice(&[0u8; PAYMENT_REQUEST_EXTRA_LEN]);
        }
    } else if is_service_attestation {
        if let Some(extra) = &p.service_attestation {
            out.extend_from_slice(&extra.payment_signal_hash);
            out.extend_from_slice(&extra.payee_entity_id);
            out.push(extra.status);
        } else {
            // Zero-tail in the inconsistent-release-build path.
            out.extend_from_slice(&[0u8; SERVICE_ATTESTATION_EXTRA_LEN]);
        }
    }

    debug_assert_eq!(out.len(), total);
    out
}

/// Deterministically decode a signal commitment payload from `tx.payload`.
///
/// Accepts the exact per-signal-type byte length:
/// - 66 bytes for base signals (types 0..=6)
/// - 101 bytes for `ReputationUpdate`
/// - 107 bytes for `SignalPurchase`
/// - 82 bytes for `StakeDeposit` / `StakeWithdraw`
/// - 117 bytes for `StakeSlash`
/// - 100 bytes for `CompositionCheck`
/// - 131 bytes for `ProofSubmission` (stub) or variable for v2 layouts
/// - 115 bytes for `SubscriptionCreate`, 98 bytes for `SubscriptionCancel`
/// - 178 bytes for `PaymentRequest`
/// - 131 bytes for `ServiceAttestation`
///
/// Length-vs-signal-type mismatch is rejected.
///
/// # Errors
/// Returns error if payload length, version, or signal type is invalid.
#[allow(clippy::too_many_lines)]
pub fn decode_signal_commitment_payload_v1(
    payload: &[u8],
) -> Result<SignalCommitmentPayloadV1, ExecError<()>> {
    // Minimum: must be at least the base header so we can read signal_type.
    // Each per-signal-type branch below enforces its own exact length. The
    // ProofSubmission branch additionally accepts the variable-length v2
    // layout for proof_type >= PROOF_TYPE_GROTH16.
    if payload.len() < SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN {
        return Err(ExecError::BadPayloadLength {
            expected: SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN,
            got: payload.len(),
        });
    }
    let ver = payload[0];
    if ver != SIGNAL_COMMITMENT_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: SIGNAL_COMMITMENT_PAYLOAD_V1,
            got: ver,
        });
    }

    let mut signal_hash = [0u8; 32];
    signal_hash.copy_from_slice(&payload[1..33]);

    let signal_type = novai_ai_entities::AiSignalType::from_byte(payload[33]).ok_or(
        ExecError::BadPayloadVersion {
            expected: 17, // max valid signal type
            got: payload[33],
        },
    )?;

    let mut issuer_entity_id = [0u8; 32];
    issuer_entity_id.copy_from_slice(&payload[34..66]);

    let is_reputation = signal_type == novai_ai_entities::AiSignalType::ReputationUpdate;
    let is_purchase = signal_type == novai_ai_entities::AiSignalType::SignalPurchase;
    let is_stake_deposit = signal_type == novai_ai_entities::AiSignalType::StakeDeposit;
    let is_stake_withdraw = signal_type == novai_ai_entities::AiSignalType::StakeWithdraw;
    let is_stake_slash = signal_type == novai_ai_entities::AiSignalType::StakeSlash;
    let is_composition_check = signal_type == novai_ai_entities::AiSignalType::CompositionCheck;
    let is_proof_submission = signal_type == novai_ai_entities::AiSignalType::ProofSubmission;
    let is_subscription_create = signal_type == novai_ai_entities::AiSignalType::SubscriptionCreate;
    let is_subscription_cancel = signal_type == novai_ai_entities::AiSignalType::SubscriptionCancel;
    let is_payment_request = signal_type == novai_ai_entities::AiSignalType::PaymentRequest;
    let is_service_attestation = signal_type == novai_ai_entities::AiSignalType::ServiceAttestation;
    let (
        reputation,
        purchase,
        stake_deposit,
        stake_withdraw,
        stake_slash,
        composition_check,
        proof_submission,
        subscription_create,
        subscription_cancel,
        payment_request,
        service_attestation,
    ) = if is_reputation {
        if payload.len() != SIGNAL_COMMITMENT_PAYLOAD_V1_REP_LEN {
            return Err(ExecError::BadPayloadLength {
                expected: SIGNAL_COMMITMENT_PAYLOAD_V1_REP_LEN,
                got: payload.len(),
            });
        }
        let mut target_entity_id = [0u8; 32];
        target_entity_id.copy_from_slice(&payload[66..98]);
        let event_type = payload[98];
        let points_delta = i16::from_be_bytes([payload[99], payload[100]]);
        (
            Some(ReputationUpdateExtraV1 {
                target_entity_id,
                event_type,
                points_delta,
            }),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
    } else if is_purchase {
        if payload.len() != SIGNAL_COMMITMENT_PAYLOAD_V1_PURCHASE_LEN {
            return Err(ExecError::BadPayloadLength {
                expected: SIGNAL_COMMITMENT_PAYLOAD_V1_PURCHASE_LEN,
                got: payload.len(),
            });
        }
        let mut seller_entity_id = [0u8; 32];
        seller_entity_id.copy_from_slice(&payload[66..98]);
        let purchased_signal_type = payload[98];
        let max_price = u64::from_be_bytes([
            payload[99],
            payload[100],
            payload[101],
            payload[102],
            payload[103],
            payload[104],
            payload[105],
            payload[106],
        ]);
        (
            None,
            Some(SignalPurchaseExtraV1 {
                seller_entity_id,
                purchased_signal_type,
                max_price,
            }),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
    } else if is_stake_deposit {
        if payload.len() != SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_DEPOSIT_LEN {
            return Err(ExecError::BadPayloadLength {
                expected: SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_DEPOSIT_LEN,
                got: payload.len(),
            });
        }
        let mut amount_bytes = [0u8; 16];
        amount_bytes.copy_from_slice(&payload[66..82]);
        let amount = u128::from_be_bytes(amount_bytes);
        (
            None,
            None,
            Some(StakeDepositExtraV1 { amount }),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
    } else if is_stake_withdraw {
        if payload.len() != SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_WITHDRAW_LEN {
            return Err(ExecError::BadPayloadLength {
                expected: SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_WITHDRAW_LEN,
                got: payload.len(),
            });
        }
        let mut amount_bytes = [0u8; 16];
        amount_bytes.copy_from_slice(&payload[66..82]);
        let amount = u128::from_be_bytes(amount_bytes);
        (
            None,
            None,
            None,
            Some(StakeWithdrawExtraV1 { amount }),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
    } else if is_stake_slash {
        if payload.len() != SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_SLASH_LEN {
            return Err(ExecError::BadPayloadLength {
                expected: SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_SLASH_LEN,
                got: payload.len(),
            });
        }
        let mut target_entity_id = [0u8; 32];
        target_entity_id.copy_from_slice(&payload[66..98]);
        let mut slash_amount_bytes = [0u8; 16];
        slash_amount_bytes.copy_from_slice(&payload[98..114]);
        let slash_amount = u128::from_be_bytes(slash_amount_bytes);
        let rep_event_type = payload[114];
        let points_delta = i16::from_be_bytes([payload[115], payload[116]]);
        (
            None,
            None,
            None,
            None,
            Some(StakeSlashExtraV1 {
                target_entity_id,
                slash_amount,
                rep_event_type,
                points_delta,
            }),
            None,
            None,
            None,
            None,
            None,
            None,
        )
    } else if is_composition_check {
        if payload.len() != SIGNAL_COMMITMENT_PAYLOAD_V1_COMPOSITION_CHECK_LEN {
            return Err(ExecError::BadPayloadLength {
                expected: SIGNAL_COMMITMENT_PAYLOAD_V1_COMPOSITION_CHECK_LEN,
                got: payload.len(),
            });
        }
        let mut target_entity_id = [0u8; 32];
        target_entity_id.copy_from_slice(&payload[66..98]);
        let failed_dependency_idx = payload[98];
        let failure_reason = payload[99];
        if failure_reason > COMPOSITION_FAILURE_REASON_MAX {
            return Err(ExecError::InvalidCompositionFailureReason {
                byte: failure_reason,
            });
        }
        (
            None,
            None,
            None,
            None,
            None,
            Some(CompositionCheckExtraV1 {
                target_entity_id,
                failed_dependency_idx,
                failure_reason,
            }),
            None,
            None,
            None,
            None,
            None,
        )
    } else if is_proof_submission {
        // The fixed prefix (131 bytes) must always fit so we can read
        // proof_type at offset 66 and decide between v1 and v2 layouts.
        if payload.len() < SIGNAL_COMMITMENT_PAYLOAD_V1_PROOF_LEN {
            return Err(ExecError::BadPayloadLength {
                expected: SIGNAL_COMMITMENT_PAYLOAD_V1_PROOF_LEN,
                got: payload.len(),
            });
        }
        let proof_type = payload[66];
        if proof_type > PROOF_TYPE_MAX {
            return Err(ExecError::UnsupportedProofType { proof_type });
        }
        let mut code_hash = [0u8; 32];
        code_hash.copy_from_slice(&payload[67..99]);
        let mut computation_hash = [0u8; 32];
        computation_hash.copy_from_slice(&payload[99..131]);

        let (vk_bytes, proof_bytes) = if proof_type == PROOF_TYPE_STUB {
            // v1 layout: exactly 131 bytes, no vk/proof tail.
            if payload.len() != SIGNAL_COMMITMENT_PAYLOAD_V1_PROOF_LEN {
                return Err(ExecError::BadPayloadLength {
                    expected: SIGNAL_COMMITMENT_PAYLOAD_V1_PROOF_LEN,
                    got: payload.len(),
                });
            }
            (Vec::new(), Vec::new())
        } else {
            // v2 layout: vk_len_be:4 | vk_bytes | proof_len_be:4 | proof_bytes
            if payload.len() < SIGNAL_COMMITMENT_PAYLOAD_V2_PROOF_MIN_LEN {
                return Err(ExecError::BadPayloadLength {
                    expected: SIGNAL_COMMITMENT_PAYLOAD_V2_PROOF_MIN_LEN,
                    got: payload.len(),
                });
            }
            let vk_len =
                u32::from_be_bytes([payload[131], payload[132], payload[133], payload[134]])
                    as usize;
            if vk_len > PROOF_SUBMISSION_MAX_VK_BYTES {
                return Err(ExecError::VerifyingKeyTooLarge {
                    actual: vk_len,
                    max: PROOF_SUBMISSION_MAX_VK_BYTES,
                });
            }
            let proof_len_off = 135 + vk_len;
            if payload.len() < proof_len_off + 4 {
                return Err(ExecError::BadPayloadLength {
                    expected: proof_len_off + 4,
                    got: payload.len(),
                });
            }
            let mut vk = vec![0u8; vk_len];
            vk.copy_from_slice(&payload[135..proof_len_off]);
            let proof_len = u32::from_be_bytes([
                payload[proof_len_off],
                payload[proof_len_off + 1],
                payload[proof_len_off + 2],
                payload[proof_len_off + 3],
            ]) as usize;
            if proof_len > PROOF_SUBMISSION_MAX_PROOF_BYTES {
                return Err(ExecError::ProofBytesTooLarge {
                    actual: proof_len,
                    max: PROOF_SUBMISSION_MAX_PROOF_BYTES,
                });
            }
            let proof_data_off = proof_len_off + 4;
            let expected_total = proof_data_off + proof_len;
            if payload.len() != expected_total {
                return Err(ExecError::BadPayloadLength {
                    expected: expected_total,
                    got: payload.len(),
                });
            }
            let mut proof = vec![0u8; proof_len];
            proof.copy_from_slice(&payload[proof_data_off..expected_total]);
            (vk, proof)
        };

        (
            None,
            None,
            None,
            None,
            None,
            None,
            Some(ProofSubmissionExtraV1 {
                proof_type,
                code_hash,
                computation_hash,
                vk_bytes,
                proof_bytes,
            }),
            None,
            None,
            None,
            None,
        )
    } else if is_subscription_create {
        if payload.len() != SIGNAL_COMMITMENT_PAYLOAD_V1_SUBSCRIPTION_CREATE_LEN {
            return Err(ExecError::BadPayloadLength {
                expected: SIGNAL_COMMITMENT_PAYLOAD_V1_SUBSCRIPTION_CREATE_LEN,
                got: payload.len(),
            });
        }
        let mut producer_entity_id = [0u8; 32];
        producer_entity_id.copy_from_slice(&payload[66..98]);
        let covered_signal_type = payload[98];
        let rate_per_block = u64::from_be_bytes([
            payload[99],
            payload[100],
            payload[101],
            payload[102],
            payload[103],
            payload[104],
            payload[105],
            payload[106],
        ]);
        let duration_blocks = u64::from_be_bytes([
            payload[107],
            payload[108],
            payload[109],
            payload[110],
            payload[111],
            payload[112],
            payload[113],
            payload[114],
        ]);
        (
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(SubscriptionCreateExtraV1 {
                producer_entity_id,
                covered_signal_type,
                rate_per_block,
                duration_blocks,
            }),
            None,
            None,
            None,
        )
    } else if is_subscription_cancel {
        if payload.len() != SIGNAL_COMMITMENT_PAYLOAD_V1_SUBSCRIPTION_CANCEL_LEN {
            return Err(ExecError::BadPayloadLength {
                expected: SIGNAL_COMMITMENT_PAYLOAD_V1_SUBSCRIPTION_CANCEL_LEN,
                got: payload.len(),
            });
        }
        let mut subscription_id = [0u8; 32];
        subscription_id.copy_from_slice(&payload[66..98]);
        (
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(SubscriptionCancelExtraV1 { subscription_id }),
            None,
            None,
        )
    } else if is_payment_request {
        if payload.len() != SIGNAL_COMMITMENT_PAYLOAD_V1_PAYMENT_REQUEST_LEN {
            return Err(ExecError::BadPayloadLength {
                expected: SIGNAL_COMMITMENT_PAYLOAD_V1_PAYMENT_REQUEST_LEN,
                got: payload.len(),
            });
        }
        let mut payee_entity_id = [0u8; 32];
        payee_entity_id.copy_from_slice(&payload[66..98]);
        let amount = u64::from_be_bytes([
            payload[98],
            payload[99],
            payload[100],
            payload[101],
            payload[102],
            payload[103],
            payload[104],
            payload[105],
        ]);
        let mut service_descriptor_hash = [0u8; 32];
        service_descriptor_hash.copy_from_slice(&payload[106..138]);
        let mut request_hash = [0u8; 32];
        request_hash.copy_from_slice(&payload[138..170]);
        let max_block_height = u64::from_be_bytes([
            payload[170],
            payload[171],
            payload[172],
            payload[173],
            payload[174],
            payload[175],
            payload[176],
            payload[177],
        ]);
        (
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(PaymentRequestExtraV1 {
                payee_entity_id,
                amount,
                service_descriptor_hash,
                request_hash,
                max_block_height,
            }),
            None,
        )
    } else if is_service_attestation {
        if payload.len() != SIGNAL_COMMITMENT_PAYLOAD_V1_SERVICE_ATTESTATION_LEN {
            return Err(ExecError::BadPayloadLength {
                expected: SIGNAL_COMMITMENT_PAYLOAD_V1_SERVICE_ATTESTATION_LEN,
                got: payload.len(),
            });
        }
        let mut payment_signal_hash = [0u8; 32];
        payment_signal_hash.copy_from_slice(&payload[66..98]);
        let mut payee_entity_id = [0u8; 32];
        payee_entity_id.copy_from_slice(&payload[98..130]);
        let status = payload[130];
        if status > PAYMENT_ATTESTATION_STATUS_MAX {
            return Err(ExecError::ServiceAttestationInvalidStatus { status });
        }
        (
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(ServiceAttestationExtraV1 {
                payment_signal_hash,
                payee_entity_id,
                status,
            }),
        )
    } else {
        if payload.len() != SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN {
            return Err(ExecError::BadPayloadLength {
                expected: SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN,
                got: payload.len(),
            });
        }
        (
            None, None, None, None, None, None, None, None, None, None, None,
        )
    };

    Ok(SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type,
        issuer_entity_id,
        reputation,
        purchase,
        stake_deposit,
        stake_withdraw,
        stake_slash,
        composition_check,
        proof_submission,
        subscription_create,
        subscription_cancel,
        payment_request,
        service_attestation,
    })
}

// ============================================================================
// PAYMENT RECORDS (Week 28 - native x402 rail)
// ============================================================================

/// Canonical payment record stored under `payment_by_hash_key`.
///
/// The same record doubles as the per-payment seen-set entry (its
/// existence rejects replays of the same `signal_hash`) and as the audit
/// row consulted by `novai_getPaymentsByEntity`. The `attested_*` fields
/// are updated in-place when a matching `ServiceAttestation` is
/// processed; until then `attested_status` is
/// `PAYMENT_ATTESTATION_STATUS_NONE` and `attested_height` is `0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentRecord {
    /// AI entity that issued the `PaymentRequest`.
    pub payer: [u8; 32],
    /// AI entity that received the payment.
    pub payee: [u8; 32],
    /// Payment amount, in base units of `economic_balance`. Net of the
    /// `PAYMENT_FEE_BPS` fee, the payee receives exactly this many units.
    pub amount: u64,
    /// Caller-supplied service identifier carried verbatim from the
    /// `PaymentRequest` tail.
    pub service_descriptor_hash: [u8; 32],
    /// Caller-supplied per-request commitment carried verbatim from the
    /// `PaymentRequest` tail.
    pub request_hash: [u8; 32],
    /// Block height at which the payment was settled.
    pub payment_height: u64,
    /// Absolute block height past which the payment would have been
    /// rejected as expired. Preserved so attestation logic can be
    /// expiry-aware in future versions.
    pub max_block_height: u64,
    /// `PAYMENT_ATTESTATION_STATUS_NONE` until an attestation lands;
    /// otherwise one of the `PAYMENT_ATTESTATION_STATUS_*` discriminants.
    pub attested_status: u8,
    /// Block height at which the attestation was recorded. `0` while
    /// `attested_status == PAYMENT_ATTESTATION_STATUS_NONE`.
    pub attested_height: u64,
}

/// Deterministically encode a `PaymentRecord` (`PAYMENT_RECORD_LEN` bytes).
#[must_use]
pub fn encode_payment_record_v1(p: &PaymentRecord) -> [u8; PAYMENT_RECORD_LEN] {
    let mut out = [0u8; PAYMENT_RECORD_LEN];
    out[0] = PAYMENT_RECORD_V1;
    out[1..33].copy_from_slice(&p.payer);
    out[33..65].copy_from_slice(&p.payee);
    out[65..73].copy_from_slice(&p.amount.to_be_bytes());
    out[73..105].copy_from_slice(&p.service_descriptor_hash);
    out[105..137].copy_from_slice(&p.request_hash);
    out[137..145].copy_from_slice(&p.payment_height.to_be_bytes());
    out[145..153].copy_from_slice(&p.max_block_height.to_be_bytes());
    out[153] = p.attested_status;
    out[154..162].copy_from_slice(&p.attested_height.to_be_bytes());
    out
}

/// Deterministically decode a `PaymentRecord` from the bytes stored at
/// `payment_by_hash_key`.
///
/// # Errors
/// Returns `ExecError::BadPayloadLength` if the slice length does not
/// equal `PAYMENT_RECORD_LEN`, or `ExecError::BadPayloadVersion` if the
/// leading version byte does not equal `PAYMENT_RECORD_V1`.
#[allow(clippy::similar_names)]
pub fn decode_payment_record_v1(bytes: &[u8]) -> Result<PaymentRecord, ExecError<()>> {
    if bytes.len() != PAYMENT_RECORD_LEN {
        return Err(ExecError::BadPayloadLength {
            expected: PAYMENT_RECORD_LEN,
            got: bytes.len(),
        });
    }
    if bytes[0] != PAYMENT_RECORD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: PAYMENT_RECORD_V1,
            got: bytes[0],
        });
    }
    let mut payer = [0u8; 32];
    payer.copy_from_slice(&bytes[1..33]);
    let mut payee = [0u8; 32];
    payee.copy_from_slice(&bytes[33..65]);
    let amount = u64::from_be_bytes([
        bytes[65], bytes[66], bytes[67], bytes[68], bytes[69], bytes[70], bytes[71], bytes[72],
    ]);
    let mut service_descriptor_hash = [0u8; 32];
    service_descriptor_hash.copy_from_slice(&bytes[73..105]);
    let mut request_hash = [0u8; 32];
    request_hash.copy_from_slice(&bytes[105..137]);
    let payment_height = u64::from_be_bytes([
        bytes[137], bytes[138], bytes[139], bytes[140], bytes[141], bytes[142], bytes[143],
        bytes[144],
    ]);
    let max_block_height = u64::from_be_bytes([
        bytes[145], bytes[146], bytes[147], bytes[148], bytes[149], bytes[150], bytes[151],
        bytes[152],
    ]);
    let attested_status = bytes[153];
    let attested_height = u64::from_be_bytes([
        bytes[154], bytes[155], bytes[156], bytes[157], bytes[158], bytes[159], bytes[160],
        bytes[161],
    ]);
    Ok(PaymentRecord {
        payer,
        payee,
        amount,
        service_descriptor_hash,
        request_hash,
        payment_height,
        max_block_height,
        attested_status,
        attested_height,
    })
}

/// Build the canonical KV key for the by-hash payment record:
/// `b"ai/payments/by_hash/" || signal_hash[32]`.
#[must_use]
pub fn payment_by_hash_key(signal_hash: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(KEY_PREFIX_AI_PAYMENTS_BY_HASH.len() + 32);
    out.extend_from_slice(KEY_PREFIX_AI_PAYMENTS_BY_HASH);
    out.extend_from_slice(signal_hash);
    out
}

/// Build the canonical KV key for the per-payer scan index entry:
/// `b"ai/payments/by_payer/" || payer[32] || height_be[8] || signal_hash[32]`.
///
/// `height` is encoded big-endian so prefix scans return entries in
/// height order.
#[must_use]
pub fn payment_by_payer_key(payer: &[u8; 32], height: u64, signal_hash: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(KEY_PREFIX_AI_PAYMENTS_BY_PAYER.len() + 32 + 8 + 32);
    out.extend_from_slice(KEY_PREFIX_AI_PAYMENTS_BY_PAYER);
    out.extend_from_slice(payer);
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(signal_hash);
    out
}

/// Build the canonical KV key for the per-payee scan index entry:
/// `b"ai/payments/by_payee/" || payee[32] || height_be[8] || signal_hash[32]`.
///
/// `height` is encoded big-endian so prefix scans return entries in
/// height order.
#[must_use]
pub fn payment_by_payee_key(payee: &[u8; 32], height: u64, signal_hash: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(KEY_PREFIX_AI_PAYMENTS_BY_PAYEE.len() + 32 + 8 + 32);
    out.extend_from_slice(KEY_PREFIX_AI_PAYMENTS_BY_PAYEE);
    out.extend_from_slice(payee);
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(signal_hash);
    out
}

/// Role discriminator for `get_payments_by_entity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentRole {
    /// Match payments where the entity is the payer (outgoing).
    Payer,
    /// Match payments where the entity is the payee (incoming).
    Payee,
}

/// Query payment records by entity in height range
/// `[start_height, end_height]` (inclusive on both ends).
///
/// Scans the appropriate `by_payer` / `by_payee` index and resolves
/// each marker back to the canonical `PaymentRecord` stored under
/// `by_hash`. Results are returned in big-endian-height-ascending
/// order (the natural lex order of the scan index).
///
/// # Errors
/// Returns `ExecError::Db` if the KV scan fails,
/// `ExecError::PaymentRecordDecodeFailed` if a referenced `by_hash`
/// value is malformed, or `ExecError::CodecDecode` if an index key
/// is shorter than expected (would indicate state corruption).
pub fn get_payments_by_entity<K: Kv>(
    db: &K,
    entity_id: &[u8; 32],
    role: PaymentRole,
    start_height: u64,
    end_height: u64,
) -> Result<Vec<PaymentRecord>, ExecError<K::Error>> {
    let role_prefix: &[u8] = match role {
        PaymentRole::Payer => KEY_PREFIX_AI_PAYMENTS_BY_PAYER,
        PaymentRole::Payee => KEY_PREFIX_AI_PAYMENTS_BY_PAYEE,
    };
    let mut prefix = Vec::with_capacity(role_prefix.len() + 32);
    prefix.extend_from_slice(role_prefix);
    prefix.extend_from_slice(entity_id);

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;
    let mut results = Vec::with_capacity(entries.len());

    for (key, _value) in entries {
        // Key layout (after the role_prefix || entity_id[32] portion):
        // `height_be[8] || signal_hash[32]` — 40 bytes total.
        if key.len() < prefix.len() + 8 + 32 {
            return Err(ExecError::CodecDecode(format!(
                "payment index key too short: {} bytes",
                key.len()
            )));
        }
        let tail = &key[prefix.len()..];
        let mut height_bytes = [0u8; 8];
        height_bytes.copy_from_slice(&tail[..8]);
        let height = u64::from_be_bytes(height_bytes);
        if height < start_height || height > end_height {
            continue;
        }
        let mut signal_hash = [0u8; 32];
        signal_hash.copy_from_slice(&tail[8..40]);

        let record_bytes = db
            .get(&payment_by_hash_key(&signal_hash))
            .map_err(ExecError::Db)?
            .ok_or_else(|| {
                ExecError::CodecDecode(
                    "payment index entry references missing by_hash record".into(),
                )
            })?;
        let record = decode_payment_record_v1(&record_bytes)
            .map_err(|_| ExecError::PaymentRecordDecodeFailed)?;
        results.push(record);
    }

    Ok(results)
}

// ============================================================================
// MEMORY OBJECT PAYLOADS (Week 21 - D21.4)
// ============================================================================

/// Create memory object payload version.
pub const CREATE_MEMORY_OBJECT_PAYLOAD_V1: u8 = 3;

/// Update memory object payload version.
pub const UPDATE_MEMORY_OBJECT_PAYLOAD_V1: u8 = 4;

/// Delete memory object payload version.
pub const DELETE_MEMORY_OBJECT_PAYLOAD_V1: u8 = 5;

// ============================================================================
// GOVERNANCE PAYLOADS (Week 24 - D24.3)
// ============================================================================

/// Submit governance proposal payload version.
pub const SUBMIT_PROPOSAL_PAYLOAD_V1: u8 = 6;

/// Execute governance proposal payload version.
pub const EXECUTE_PROPOSAL_PAYLOAD_V1: u8 = 7;

// ============================================================================
// REGISTER / CREDIT AI ENTITY PAYLOADS
// ============================================================================

/// Register AI entity payload version.
pub const REGISTER_AI_ENTITY_PAYLOAD_V1: u8 = 8;

/// Credit AI entity payload version.
pub const CREDIT_AI_ENTITY_PAYLOAD_V1: u8 = 9;

/// Register AI Entity payload:
/// `[version:1][code_hash:32][autonomy_mode:1][capabilities:1][initial_balance_be:16]`
///
/// Total size: 51 bytes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterAiEntityPayloadV1 {
    /// Hash of the AI module code/weights.
    pub code_hash: [u8; 32],
    /// Autonomy mode for the entity.
    pub autonomy_mode: novai_ai_entities::AutonomyMode,
    /// Capability flags for the entity.
    pub capabilities: novai_ai_entities::Capabilities,
    /// Initial balance to fund the entity with (transferred from creator).
    pub initial_balance: u128,
}

/// Deterministically encode a register AI entity payload.
#[must_use]
pub fn encode_register_ai_entity_payload_v1(p: &RegisterAiEntityPayloadV1) -> [u8; 51] {
    let mut out = [0u8; 51];
    out[0] = REGISTER_AI_ENTITY_PAYLOAD_V1;
    out[1..33].copy_from_slice(&p.code_hash);
    out[33] = p.autonomy_mode.to_byte();
    out[34] = p.capabilities.to_byte();
    out[35..51].copy_from_slice(&p.initial_balance.to_be_bytes());
    out
}

/// Deterministically decode a register AI entity payload.
///
/// # Errors
/// Returns error if payload length or version is invalid.
pub fn decode_register_ai_entity_payload_v1(
    payload: &[u8],
) -> Result<RegisterAiEntityPayloadV1, ExecError<()>> {
    const LEN: usize = 51;
    if payload.len() != LEN {
        return Err(ExecError::BadPayloadLength {
            expected: LEN,
            got: payload.len(),
        });
    }
    let ver = payload[0];
    if ver != REGISTER_AI_ENTITY_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: REGISTER_AI_ENTITY_PAYLOAD_V1,
            got: ver,
        });
    }

    let mut code_hash = [0u8; 32];
    code_hash.copy_from_slice(&payload[1..33]);

    let autonomy_mode = novai_ai_entities::AutonomyMode::from_byte(payload[33]).ok_or(
        ExecError::BadPayloadVersion {
            expected: 2, // max valid autonomy mode
            got: payload[33],
        },
    )?;

    let capabilities = novai_ai_entities::Capabilities::from_byte(payload[34]);

    let mut balance_bytes = [0u8; 16];
    balance_bytes.copy_from_slice(&payload[35..51]);
    let initial_balance = u128::from_be_bytes(balance_bytes);

    Ok(RegisterAiEntityPayloadV1 {
        code_hash,
        autonomy_mode,
        capabilities,
        initial_balance,
    })
}

/// Credit AI Entity payload:
/// `[version:1][entity_id:32][amount_be:16]`
///
/// Total size: 49 bytes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreditAiEntityPayloadV1 {
    /// ID of the AI entity to credit.
    pub entity_id: [u8; 32],
    /// Amount to credit to the entity's balance.
    pub amount: u128,
}

/// Deterministically encode a credit AI entity payload.
#[must_use]
pub fn encode_credit_ai_entity_payload_v1(p: &CreditAiEntityPayloadV1) -> [u8; 49] {
    let mut out = [0u8; 49];
    out[0] = CREDIT_AI_ENTITY_PAYLOAD_V1;
    out[1..33].copy_from_slice(&p.entity_id);
    out[33..49].copy_from_slice(&p.amount.to_be_bytes());
    out
}

/// Deterministically decode a credit AI entity payload.
///
/// # Errors
/// Returns error if payload length or version is invalid.
pub fn decode_credit_ai_entity_payload_v1(
    payload: &[u8],
) -> Result<CreditAiEntityPayloadV1, ExecError<()>> {
    const LEN: usize = 49;
    if payload.len() != LEN {
        return Err(ExecError::BadPayloadLength {
            expected: LEN,
            got: payload.len(),
        });
    }
    let ver = payload[0];
    if ver != CREDIT_AI_ENTITY_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: CREDIT_AI_ENTITY_PAYLOAD_V1,
            got: ver,
        });
    }

    let mut entity_id = [0u8; 32];
    entity_id.copy_from_slice(&payload[1..33]);

    let mut amount_bytes = [0u8; 16];
    amount_bytes.copy_from_slice(&payload[33..49]);
    let amount = u128::from_be_bytes(amount_bytes);

    Ok(CreditAiEntityPayloadV1 { entity_id, amount })
}

// ============================================================================
// REGISTER AI ENTITY WITH KEY PAYLOAD (Type 10)
// ============================================================================

/// Register AI entity with ed25519 pubkey payload version.
pub const REGISTER_AI_ENTITY_WITH_KEY_PAYLOAD_V1: u8 = 10;

/// Register AI Entity with Key payload:
/// `[version:1][code_hash:32][pubkey:32][autonomy_mode:1][capabilities:1][initial_balance_be:16]`
///
/// Total size: 83 bytes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterAiEntityWithKeyPayloadV1 {
    /// Hash of the AI module code/weights.
    pub code_hash: [u8; 32],
    /// Ed25519 public key for the entity.
    pub pubkey: [u8; 32],
    /// Autonomy mode for the entity.
    pub autonomy_mode: novai_ai_entities::AutonomyMode,
    /// Capability flags for the entity.
    pub capabilities: novai_ai_entities::Capabilities,
    /// Initial balance to fund the entity with (transferred from creator).
    pub initial_balance: u128,
}

/// Deterministically encode a register AI entity with key payload.
#[must_use]
pub fn encode_register_ai_entity_with_key_payload_v1(
    p: &RegisterAiEntityWithKeyPayloadV1,
) -> [u8; 83] {
    let mut out = [0u8; 83];
    out[0] = REGISTER_AI_ENTITY_WITH_KEY_PAYLOAD_V1;
    out[1..33].copy_from_slice(&p.code_hash);
    out[33..65].copy_from_slice(&p.pubkey);
    out[65] = p.autonomy_mode.to_byte();
    out[66] = p.capabilities.to_byte();
    out[67..83].copy_from_slice(&p.initial_balance.to_be_bytes());
    out
}

/// Deterministically decode a register AI entity with key payload.
///
/// # Errors
/// Returns error if payload length is not 83 bytes, version byte is wrong,
/// or autonomy mode byte is invalid.
pub fn decode_register_ai_entity_with_key_payload_v1(
    payload: &[u8],
) -> Result<RegisterAiEntityWithKeyPayloadV1, ExecError<()>> {
    const LEN: usize = 83;
    if payload.len() != LEN {
        return Err(ExecError::BadPayloadLength {
            expected: LEN,
            got: payload.len(),
        });
    }
    let ver = payload[0];
    if ver != REGISTER_AI_ENTITY_WITH_KEY_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: REGISTER_AI_ENTITY_WITH_KEY_PAYLOAD_V1,
            got: ver,
        });
    }

    let mut code_hash = [0u8; 32];
    code_hash.copy_from_slice(&payload[1..33]);

    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&payload[33..65]);

    let autonomy_mode = novai_ai_entities::AutonomyMode::from_byte(payload[65]).ok_or(
        ExecError::BadPayloadVersion {
            expected: 2,
            got: payload[65],
        },
    )?;

    let capabilities = novai_ai_entities::Capabilities::from_byte(payload[66]);

    let mut balance_bytes = [0u8; 16];
    balance_bytes.copy_from_slice(&payload[67..83]);
    let initial_balance = u128::from_be_bytes(balance_bytes);

    Ok(RegisterAiEntityWithKeyPayloadV1 {
        code_hash,
        pubkey,
        autonomy_mode,
        capabilities,
        initial_balance,
    })
}

/// Submit Proposal payload (D24.3):
/// `[version:1][proposal_type:1][gate_id:32][data_len_be:4][proposal_data:var]`
///
/// Minimum size: 38 bytes (empty data)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitProposalPayloadV1 {
    /// Type of proposal.
    pub proposal_type: ProposalType,
    /// Gate ID that must approve this proposal.
    pub gate_id: [u8; 32],
    /// Encoded proposal data (interpretation depends on `proposal_type`).
    /// For `ModuleActivation`/`ModuleRollback`: `entity_id` (32 bytes)
    pub proposal_data: Vec<u8>,
}

/// Execute Proposal payload (D24.3):
/// `[version:1][proposal_id:32]`
///
/// Fixed size: 33 bytes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteProposalPayloadV1 {
    /// ID of the proposal to execute.
    pub proposal_id: [u8; 32],
}

/// Deterministically encode a submit proposal payload.
#[must_use]
pub fn encode_submit_proposal_payload_v1(p: &SubmitProposalPayloadV1) -> Vec<u8> {
    let mut out = Vec::with_capacity(38 + p.proposal_data.len());
    out.push(SUBMIT_PROPOSAL_PAYLOAD_V1);
    out.push(p.proposal_type.to_byte());
    out.extend_from_slice(&p.gate_id);
    #[allow(clippy::cast_possible_truncation)]
    let data_len = p.proposal_data.len() as u32;
    out.extend_from_slice(&data_len.to_be_bytes());
    out.extend_from_slice(&p.proposal_data);
    out
}

/// Deterministically decode a submit proposal payload.
///
/// # Errors
/// Returns error if payload is malformed.
pub fn decode_submit_proposal_payload_v1(
    payload: &[u8],
) -> Result<SubmitProposalPayloadV1, ExecError<()>> {
    const MIN_LEN: usize = 38; // version + type + gate_id + data_len
    if payload.len() < MIN_LEN {
        return Err(ExecError::BadPayloadLength {
            expected: MIN_LEN,
            got: payload.len(),
        });
    }

    let ver = payload[0];
    if ver != SUBMIT_PROPOSAL_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: SUBMIT_PROPOSAL_PAYLOAD_V1,
            got: ver,
        });
    }

    let proposal_type =
        ProposalType::from_byte(payload[1]).ok_or(ExecError::InvalidProposalType)?;

    let mut gate_id = [0u8; 32];
    gate_id.copy_from_slice(&payload[2..34]);

    let data_len =
        u32::from_be_bytes([payload[34], payload[35], payload[36], payload[37]]) as usize;

    if payload.len() != MIN_LEN + data_len {
        return Err(ExecError::BadPayloadLength {
            expected: MIN_LEN + data_len,
            got: payload.len(),
        });
    }

    let proposal_data = payload[38..].to_vec();

    Ok(SubmitProposalPayloadV1 {
        proposal_type,
        gate_id,
        proposal_data,
    })
}

/// Deterministically encode an execute proposal payload.
#[must_use]
pub fn encode_execute_proposal_payload_v1(p: &ExecuteProposalPayloadV1) -> [u8; 33] {
    let mut out = [0u8; 33];
    out[0] = EXECUTE_PROPOSAL_PAYLOAD_V1;
    out[1..33].copy_from_slice(&p.proposal_id);
    out
}

/// Deterministically decode an execute proposal payload.
///
/// # Errors
/// Returns error if payload is malformed.
pub fn decode_execute_proposal_payload_v1(
    payload: &[u8],
) -> Result<ExecuteProposalPayloadV1, ExecError<()>> {
    const LEN: usize = 33;
    if payload.len() != LEN {
        return Err(ExecError::BadPayloadLength {
            expected: LEN,
            got: payload.len(),
        });
    }

    let ver = payload[0];
    if ver != EXECUTE_PROPOSAL_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: EXECUTE_PROPOSAL_PAYLOAD_V1,
            got: ver,
        });
    }

    let mut proposal_id = [0u8; 32];
    proposal_id.copy_from_slice(&payload[1..33]);

    Ok(ExecuteProposalPayloadV1 { proposal_id })
}

/// Create Memory Object payload (D21.4):
/// `[version:1][object_type:1][data_len_be:4][data:var]`
///
/// Minimum size: 6 bytes (empty data)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateMemoryObjectPayloadV1 {
    /// Type of memory object to create.
    pub object_type: novai_ai_entities::MemoryObjectType,
    /// Initial data for the object.
    pub data: Vec<u8>,
}

/// Deterministically encode a create memory object payload.
#[must_use]
pub fn encode_create_memory_object_payload_v1(p: &CreateMemoryObjectPayloadV1) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + p.data.len());
    out.push(CREATE_MEMORY_OBJECT_PAYLOAD_V1);
    out.push(p.object_type.to_byte());
    #[allow(clippy::cast_possible_truncation)]
    let data_len = p.data.len() as u32;
    out.extend_from_slice(&data_len.to_be_bytes());
    out.extend_from_slice(&p.data);
    out
}

/// Deterministically decode a create memory object payload.
///
/// # Errors
/// Returns error if payload is malformed.
pub fn decode_create_memory_object_payload_v1(
    payload: &[u8],
) -> Result<CreateMemoryObjectPayloadV1, ExecError<()>> {
    const MIN_LEN: usize = 6; // version + type + data_len
    if payload.len() < MIN_LEN {
        return Err(ExecError::BadPayloadLength {
            expected: MIN_LEN,
            got: payload.len(),
        });
    }

    let ver = payload[0];
    if ver != CREATE_MEMORY_OBJECT_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: CREATE_MEMORY_OBJECT_PAYLOAD_V1,
            got: ver,
        });
    }

    let object_type = novai_ai_entities::MemoryObjectType::from_byte(payload[1])
        .ok_or(ExecError::InvalidMemoryObjectType { byte: payload[1] })?;

    let mut data_len_bytes = [0u8; 4];
    data_len_bytes.copy_from_slice(&payload[2..6]);
    let data_len = u32::from_be_bytes(data_len_bytes) as usize;

    if payload.len() != MIN_LEN + data_len {
        return Err(ExecError::BadPayloadLength {
            expected: MIN_LEN + data_len,
            got: payload.len(),
        });
    }

    let data = payload[6..].to_vec();

    Ok(CreateMemoryObjectPayloadV1 { object_type, data })
}

/// Update Memory Object payload (D21.4):
/// `[version:1][object_id:32][data_len_be:4][new_data:var]`
///
/// Minimum size: 37 bytes (empty data)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateMemoryObjectPayloadV1 {
    /// ID of the memory object to update.
    pub object_id: [u8; 32],
    /// New data for the object.
    pub new_data: Vec<u8>,
}

/// Deterministically encode an update memory object payload.
#[must_use]
pub fn encode_update_memory_object_payload_v1(p: &UpdateMemoryObjectPayloadV1) -> Vec<u8> {
    let mut out = Vec::with_capacity(37 + p.new_data.len());
    out.push(UPDATE_MEMORY_OBJECT_PAYLOAD_V1);
    out.extend_from_slice(&p.object_id);
    #[allow(clippy::cast_possible_truncation)]
    let data_len = p.new_data.len() as u32;
    out.extend_from_slice(&data_len.to_be_bytes());
    out.extend_from_slice(&p.new_data);
    out
}

/// Deterministically decode an update memory object payload.
///
/// # Errors
/// Returns error if payload is malformed.
pub fn decode_update_memory_object_payload_v1(
    payload: &[u8],
) -> Result<UpdateMemoryObjectPayloadV1, ExecError<()>> {
    const MIN_LEN: usize = 37; // version + object_id + data_len
    if payload.len() < MIN_LEN {
        return Err(ExecError::BadPayloadLength {
            expected: MIN_LEN,
            got: payload.len(),
        });
    }

    let ver = payload[0];
    if ver != UPDATE_MEMORY_OBJECT_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: UPDATE_MEMORY_OBJECT_PAYLOAD_V1,
            got: ver,
        });
    }

    let mut object_id = [0u8; 32];
    object_id.copy_from_slice(&payload[1..33]);

    let mut data_len_bytes = [0u8; 4];
    data_len_bytes.copy_from_slice(&payload[33..37]);
    let data_len = u32::from_be_bytes(data_len_bytes) as usize;

    if payload.len() != MIN_LEN + data_len {
        return Err(ExecError::BadPayloadLength {
            expected: MIN_LEN + data_len,
            got: payload.len(),
        });
    }

    let new_data = payload[37..].to_vec();

    Ok(UpdateMemoryObjectPayloadV1 {
        object_id,
        new_data,
    })
}

/// Delete Memory Object payload (D21.4):
/// `[version:1][object_id:32]`
///
/// Total size: 33 bytes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteMemoryObjectPayloadV1 {
    /// ID of the memory object to delete.
    pub object_id: [u8; 32],
}

/// Deterministically encode a delete memory object payload.
#[must_use]
pub fn encode_delete_memory_object_payload_v1(p: &DeleteMemoryObjectPayloadV1) -> [u8; 33] {
    let mut out = [0u8; 33];
    out[0] = DELETE_MEMORY_OBJECT_PAYLOAD_V1;
    out[1..33].copy_from_slice(&p.object_id);
    out
}

/// Deterministically decode a delete memory object payload.
///
/// # Errors
/// Returns error if payload is malformed.
pub fn decode_delete_memory_object_payload_v1(
    payload: &[u8],
) -> Result<DeleteMemoryObjectPayloadV1, ExecError<()>> {
    const LEN: usize = 33;
    if payload.len() != LEN {
        return Err(ExecError::BadPayloadLength {
            expected: LEN,
            got: payload.len(),
        });
    }

    let ver = payload[0];
    if ver != DELETE_MEMORY_OBJECT_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: DELETE_MEMORY_OBJECT_PAYLOAD_V1,
            got: ver,
        });
    }

    let mut object_id = [0u8; 32];
    object_id.copy_from_slice(&payload[1..33]);

    Ok(DeleteMemoryObjectPayloadV1 { object_id })
}

fn u64_to_u128_checked(x: u64) -> u128 {
    u128::from(x)
}

/// Read account state, returning default (zero balance, zero nonce) if absent.
///
/// # Errors
///
/// Returns `ExecError::Db` on storage read failure or `ExecError::CodecDecode`
/// if stored bytes are malformed.
pub fn read_account_or_default<K: Kv>(
    db: &K,
    addr: &Address,
) -> Result<AccountStateV1, ExecError<K::Error>> {
    let k = account_key(addr);
    match db.get(&k).map_err(ExecError::Db)? {
        None => Ok(AccountStateV1 {
            balance: 0,
            nonce: 0,
        }),
        Some(bytes) => Ok(decode_account_v1(&bytes)?),
    }
}

fn read_fee_pool_or_default<K: Kv>(db: &K) -> Result<FeePoolV1, ExecError<K::Error>> {
    match db.get(KEY_FEE_POOL).map_err(ExecError::Db)? {
        None => Ok(FeePoolV1 { balance: 0 }),
        Some(bytes) => Ok(decode_fee_pool_v1(&bytes)?),
    }
}

fn read_smt_root_or_default<K: Kv>(db: &K) -> Result<Hash32, ExecError<K::Error>> {
    match db.get(KEY_SMT_ROOT).map_err(ExecError::Db)? {
        None => Ok(empty_hash_at_height(256)),
        Some(bytes) => Ok(decode_smt_root_v1(&bytes)?),
    }
}

// ============================================================================
// AI KILL SWITCH HELPERS
// ============================================================================

/// Read the AI emergency kill switch state.
///
/// Returns `true` if the kill switch is active (all AI operations blocked).
/// Key absent = normal operation (false).
///
/// # Errors
///
/// Returns `ExecError::Db` on storage read failure.
pub fn read_ai_kill_switch<K: Kv>(db: &K) -> Result<bool, ExecError<K::Error>> {
    db.get(KEY_AI_KILL_SWITCH)
        .map_err(ExecError::Db)
        .map(|opt| opt.is_some_and(|bytes| bytes.first().copied().unwrap_or(0) == 1))
}

/// Create a `WriteOp` to set the AI emergency kill switch.
#[must_use]
pub fn write_ai_kill_switch_op(active: bool) -> WriteOp {
    WriteOp::Put(KEY_AI_KILL_SWITCH.to_vec(), vec![u8::from(active)])
}

/// Store adapter: reads existing nodes from Kv, buffers writes as `WriteOp::Put`.
/// Deterministic: uses Vec + linear search (no `HashMap`).
struct SmtOverlayStore<'a, K: Kv> {
    db: &'a K,
    pending: Vec<(Vec<u8>, Vec<u8>)>, // (db_key, value_bytes)
}

impl<'a, K: Kv> SmtOverlayStore<'a, K> {
    const fn new(db: &'a K) -> Self {
        Self {
            db,
            pending: Vec::new(),
        }
    }

    fn into_write_ops(mut self) -> Vec<WriteOp> {
        // Sort by key for deterministic ordering (consensus-critical).
        self.pending.sort_by(|a, b| a.0.cmp(&b.0));

        self.pending
            .into_iter()
            .map(|(k, v)| WriteOp::Put(k, v))
            .collect()
    }

    fn pending_get(&self, key: &[u8]) -> Option<&[u8]> {
        // last-write-wins
        for (k, v) in self.pending.iter().rev() {
            if k.as_slice() == key {
                return Some(v.as_slice());
            }
        }
        None
    }
}

impl<K: Kv> SmtStore for SmtOverlayStore<'_, K> {
    type Error = K::Error;

    fn get_node(&self, node_hash: &Hash32) -> Result<Option<[u8; Node::ENCODED_LEN]>, Self::Error> {
        let key = smt_node_key(node_hash);

        // First check buffered writes.
        if let Some(v) = self.pending_get(&key) {
            if v.len() != Node::ENCODED_LEN {
                return Ok(None);
            }
            let mut out = [0u8; Node::ENCODED_LEN];
            out.copy_from_slice(v);
            return Ok(Some(out));
        }

        match self.db.get(&key)? {
            None => Ok(None),
            Some(v) => {
                if v.len() != Node::ENCODED_LEN {
                    return Ok(None);
                }
                let mut out = [0u8; Node::ENCODED_LEN];
                out.copy_from_slice(&v);
                Ok(Some(out))
            }
        }
    }

    fn put_node(
        &mut self,
        node_hash: &Hash32,
        node_bytes: &[u8; Node::ENCODED_LEN],
    ) -> Result<(), Self::Error> {
        let key = smt_node_key(node_hash);
        self.pending.push((key, node_bytes.to_vec()));
        Ok(())
    }
}

/// Compute SMT updates for a set of state operations and append them to the
/// provided output Vec, returning the new SMT root.
///
/// This is the single source of truth for translating `account` /
/// `fee_pool` / etc. state writes into authenticated SMT updates.
/// `apply_tx_v1_transfer_inner` uses it to bundle SMT writes with state
/// writes in a single atomic batch. Genesis code paths use it to get a
/// consistent `state_root` for the genesis block.
///
/// The caller is responsible for actually writing `out_ops` to the DB.
///
/// # Errors
/// Returns error on DB read failure or SMT internal errors.
pub fn append_smt_ops_for_state_ops<K: Kv>(
    db: &K,
    state_ops: &[WriteOp],
    out_ops: &mut Vec<WriteOp>,
) -> Result<Hash32, ExecError<K::Error>> {
    let cur_root = read_smt_root_or_default(db)?;

    // Build SMT updates in an overlay store so we can batch node writes with state writes.
    let store = SmtOverlayStore::new(db);
    let mut smt = Smt::with_root(store, cur_root);

    for op in state_ops {
        match op {
            WriteOp::Put(k, v) => {
                let sk: Hash32 = smt_key_for_state_key(k);
                smt.update(sk, v).map_err(|e| match e {
                    SmtError::Store(err) => ExecError::Db(err),
                    _ => ExecError::Overflow,
                })?;
            }
            WriteOp::Delete(k) => {
                let sk: Hash32 = smt_key_for_state_key(k);
                smt.delete(sk).map_err(|e| match e {
                    SmtError::Store(err) => ExecError::Db(err),
                    _ => ExecError::Overflow,
                })?;
            }
        }
    }

    let new_root = smt.root();
    let store = smt.into_store();

    // Add SMT node writes.
    out_ops.extend(store.into_write_ops());

    // Add root record write.
    out_ops.push(WriteOp::Put(
        KEY_SMT_ROOT.to_vec(),
        encode_smt_root_v1(&new_root).to_vec(),
    ));

    Ok(new_root)
}

/// Apply a single `TxV1` as a `TransferPayloadV1` against the account state machine.
///
/// Rules (Week 3):
/// - Nonce exact match.
/// - Sender balance >= amount + fee.
/// - Checked arithmetic only.
/// - Debit sender by (amount + fee), credit receiver by amount.
/// - Increment sender nonce by 1.
/// - Add fee to `fee_pool`.
///
/// ATOMIC: All state changes are applied in a single batch (all-or-nothing).
///
/// # Errors
/// Returns error if nonce mismatch, insufficient funds, payload decode fails, or DB error.
#[allow(clippy::too_many_lines)]
pub fn apply_tx_v1_transfer<K: KvBatch>(db: &mut K, tx: &TxV1) -> Result<(), ExecError<K::Error>> {
    let ai_sender = lookup_ai_entity_by_address(db, &tx.from)?;
    apply_tx_v1_transfer_inner(db, tx, ai_sender)
}

/// Inner transfer implementation that accepts a pre-resolved AI entity.
///
/// Called by `dispatch_tx` (which already looked up the entity via
/// `check_ai_entity_sender`) and by the public `apply_tx_v1_transfer`
/// wrapper (which does its own lookup for standalone callers).
#[allow(clippy::too_many_lines)]
fn apply_tx_v1_transfer_inner<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    ai_sender: Option<AiEntity>,
) -> Result<(), ExecError<K::Error>> {
    // Decode payload (deterministic).
    let payload = decode_transfer_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        _ => ExecError::Overflow,
    })?;

    let amount_u128 = u64_to_u128_checked(payload.amount);
    let fee_u128 = u64_to_u128_checked(tx.fee);
    let needed = amount_u128
        .checked_add(fee_u128)
        .ok_or(ExecError::Overflow)?;

    // M-06: Prevent state spam with dust accounts. If the recipient has no
    // account in the DB (truly new, not just drained), require the transfer
    // amount to meet MIN_ACCOUNT_BALANCE.
    let to_key = account_key(&payload.to);
    let recipient_exists = db.get(&to_key).map_err(ExecError::Db)?.is_some();
    if !recipient_exists && amount_u128 < MIN_ACCOUNT_BALANCE {
        return Err(ExecError::InsufficientFunds {
            balance: amount_u128,
            needed: MIN_ACCOUNT_BALANCE,
        });
    }

    let mut to_acct = read_account_or_default(db, &payload.to)?;
    let mut fee_pool = read_fee_pool_or_default(db)?;

    to_acct.balance = to_acct
        .balance
        .checked_add(amount_u128)
        .ok_or(ExecError::Overflow)?;
    fee_pool.balance = fee_pool
        .balance
        .checked_add(fee_u128)
        .ok_or(ExecError::Overflow)?;

    if let Some(mut entity) = ai_sender {
        // AI entity sender path: use entity balance/nonce
        if !entity.is_active {
            return Err(ExecError::EntityNotActive);
        }
        if tx.nonce != entity.nonce {
            return Err(ExecError::NonceMismatch {
                expected: entity.nonce,
                got: tx.nonce,
            });
        }
        if entity.economic_balance < needed {
            return Err(ExecError::InsufficientFunds {
                balance: entity.economic_balance,
                needed,
            });
        }

        entity.economic_balance = entity
            .economic_balance
            .checked_sub(needed)
            .ok_or(ExecError::Overflow)?;
        entity.nonce = entity
            .nonce
            .checked_add(1)
            .ok_or(ExecError::NonceOverflow)?;

        let ops = vec![
            write_ai_entity_op(&entity),
            WriteOp::Put(
                account_key(&payload.to),
                encode_account_v1(&to_acct).to_vec(),
            ),
            WriteOp::Put(
                KEY_FEE_POOL.to_vec(),
                encode_fee_pool_v1(&fee_pool).to_vec(),
            ),
        ];

        let mut all_ops = ops;
        let state_ops_snapshot = all_ops.clone();
        let _new_root = append_smt_ops_for_state_ops(db, &state_ops_snapshot, &mut all_ops)?;
        db.apply_batch(&all_ops).map_err(ExecError::Db)?;
    } else {
        // Normal account sender path (original logic)
        let mut from_acct = read_account_or_default(db, &tx.from)?;
        if tx.nonce != from_acct.nonce {
            return Err(ExecError::NonceMismatch {
                expected: from_acct.nonce,
                got: tx.nonce,
            });
        }
        if from_acct.balance < needed {
            return Err(ExecError::InsufficientFunds {
                balance: from_acct.balance,
                needed,
            });
        }

        from_acct.balance = from_acct
            .balance
            .checked_sub(needed)
            .ok_or(ExecError::Overflow)?;
        from_acct.nonce = from_acct
            .nonce
            .checked_add(1)
            .ok_or(ExecError::NonceOverflow)?;

        let ops = vec![
            WriteOp::Put(
                account_key(&tx.from),
                encode_account_v1(&from_acct).to_vec(),
            ),
            WriteOp::Put(
                account_key(&payload.to),
                encode_account_v1(&to_acct).to_vec(),
            ),
            WriteOp::Put(
                KEY_FEE_POOL.to_vec(),
                encode_fee_pool_v1(&fee_pool).to_vec(),
            ),
        ];

        let mut all_ops = ops;
        let state_ops_snapshot = all_ops.clone();
        let _new_root = append_smt_ops_for_state_ops(db, &state_ops_snapshot, &mut all_ops)?;
        db.apply_batch(&all_ops).map_err(ExecError::Db)?;
    }

    Ok(())
}

// ============================================================================
// AI STORAGE OPERATIONS (Retrofit Week 3)
// ============================================================================

use novai_ai_entities::{
    encode_memory_object_v1, AiEntity, AiSignalType, CompositionGraphData, MemoryObject,
    MemoryObjectType, SignalCatalogData, SignalCommitment, SubscriptionData,
    VerificationRecordData, MAX_REPUTATION_SCORE, MAX_SUBSCRIPTIONS_PER_ENTITY,
};
use novai_codec::{decode_ai_entity, encode_ai_entity_v5, encode_signal_commitment_v1};
use novai_crypto::{Groth16Verifier, StubZkVerifier, ZkVerifier};
use novai_state::{
    ai_entity_key, ai_memory_by_type_key, ai_memory_key, ai_memory_object_key,
    ai_signal_by_issuer_key, ai_signal_by_type_key, ai_signal_key,
};

/// Read an AI entity from storage.
///
/// # Errors
///
/// Returns error if DB read fails or stored bytes are malformed.
pub fn read_ai_entity<K: Kv>(
    db: &K,
    entity_id: &[u8; 32],
) -> Result<Option<AiEntity>, ExecError<K::Error>> {
    let key = ai_entity_key(entity_id);
    match db.get(&key).map_err(ExecError::Db)? {
        None => Ok(None),
        Some(bytes) => {
            // decode_ai_entity handles V1, V2, and V3 formats
            let entity =
                decode_ai_entity(&bytes).map_err(|e| ExecError::CodecDecode(format!("{e:?}")))?;
            Ok(Some(entity))
        }
    }
}

/// Write an AI entity to storage (returns `WriteOp` for batching).
///
/// Canonical writer is V5 (270 bytes). Old V1/V2/V3/V4 reads are still accepted
/// by `decode_ai_entity` and are promoted to V5 on next write.
#[must_use]
pub fn write_ai_entity_op(entity: &AiEntity) -> WriteOp {
    let key = ai_entity_key(&entity.id);
    let value = encode_ai_entity_v5(entity);
    WriteOp::Put(key, value)
}

/// Read AI memory slot value.
///
/// # Errors
///
/// Returns error if DB read fails.
pub fn read_ai_memory<K: Kv>(
    db: &K,
    entity_id: &[u8; 32],
    slot: &[u8],
) -> Result<Option<Vec<u8>>, ExecError<K::Error>> {
    let key = ai_memory_key(entity_id, slot);
    db.get(&key).map_err(ExecError::Db)
}

/// Create `WriteOp` to write AI memory slot.
#[must_use]
pub fn write_ai_memory_op(entity_id: &[u8; 32], slot: &[u8], value: Vec<u8>) -> WriteOp {
    let key = ai_memory_key(entity_id, slot);
    WriteOp::Put(key, value)
}

/// Create `WriteOp` to delete AI memory slot.
#[must_use]
pub fn delete_ai_memory_op(entity_id: &[u8; 32], slot: &[u8]) -> WriteOp {
    let key = ai_memory_key(entity_id, slot);
    WriteOp::Delete(key)
}

// ============================================================================
// MODULE ACTIVATION/ROLLBACK (Week 24 - D24.1)
// ============================================================================

/// Apply a `ModuleActivation` governance proposal.
///
/// Sets the AI entity's `is_active` flag to `true`.
/// Idempotent: returns success if already active.
///
/// # Errors
///
/// Returns `EntityNotFound` if the entity does not exist.
pub fn apply_module_activation<K: KvBatch>(
    db: &mut K,
    entity_id: &[u8; 32],
) -> Result<(), ExecError<K::Error>> {
    let mut entity = read_ai_entity(db, entity_id)?.ok_or(ExecError::EntityNotFound)?;

    // Idempotent: no error if already active
    if !entity.is_active {
        entity.is_active = true;
        let op = write_ai_entity_op(&entity);
        db.apply_batch(&[op]).map_err(ExecError::Db)?;
    }

    Ok(())
}

/// Apply a `ModuleRollback` governance proposal.
///
/// Sets the AI entity's `is_active` flag to `false`.
/// Idempotent: returns success if already inactive.
///
/// # Errors
///
/// Returns `EntityNotFound` if the entity does not exist.
pub fn apply_module_rollback<K: KvBatch>(
    db: &mut K,
    entity_id: &[u8; 32],
) -> Result<(), ExecError<K::Error>> {
    let mut entity = read_ai_entity(db, entity_id)?.ok_or(ExecError::EntityNotFound)?;

    // Idempotent: no error if already inactive
    if entity.is_active {
        entity.is_active = false;
        let op = write_ai_entity_op(&entity);
        db.apply_batch(&[op]).map_err(ExecError::Db)?;
    }

    Ok(())
}

// ============================================================================
// GOVERNANCE PROPOSAL EXECUTION (Week 24 - D24.3)
// ============================================================================

/// Read a governance proposal from storage.
///
/// # Errors
///
/// Returns error if DB read fails or stored bytes are malformed.
pub fn read_proposal<K: Kv>(
    db: &K,
    proposal_id: &[u8; 32],
) -> Result<Option<Proposal>, ExecError<K::Error>> {
    let key = governance_proposal_key(proposal_id);
    match db.get(&key).map_err(ExecError::Db)? {
        None => Ok(None),
        Some(bytes) => {
            let proposal =
                decode_proposal_v1(&bytes).map_err(|e| ExecError::CodecDecode(format!("{e:?}")))?;
            Ok(Some(proposal))
        }
    }
}

/// Write a governance proposal to storage (returns `WriteOp` for batching).
#[must_use]
pub fn write_proposal_op(proposal: &Proposal) -> WriteOp {
    let key = governance_proposal_key(&proposal.id);
    let value = encode_proposal_v1(proposal);
    WriteOp::Put(key, value)
}

/// Read an approval gate from storage.
///
/// # Errors
///
/// Returns error if DB read fails or stored bytes are malformed.
pub fn read_approval_gate<K: Kv>(
    db: &K,
    gate_id: &[u8; 32],
) -> Result<Option<novai_ai_entities::ApprovalGate>, ExecError<K::Error>> {
    let key = approval_gate_key(gate_id);
    match db.get(&key).map_err(ExecError::Db)? {
        None => Ok(None),
        Some(bytes) => {
            let gate = novai_codec::decode_approval_gate_v1(&bytes)
                .map_err(|e| ExecError::CodecDecode(format!("{e:?}")))?;
            Ok(Some(gate))
        }
    }
}

/// Apply a `SubmitProposal` transaction.
///
/// Creates a new governance proposal in Submitted state.
///
/// # Validation
/// 1. Gate must exist
/// 2. Proposal type must be valid
/// 3. For `ModuleActivation`/`Rollback`: `entity_id` must be 32 bytes
///
/// # State Changes
/// - Store proposal at `governance/proposals/{proposal_id}`
///
/// # Errors
///
/// Returns error if validation fails or DB operations fail.
pub fn apply_governance_submit_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<[u8; 32], ExecError<K::Error>> {
    // Decode payload
    let payload = decode_submit_proposal_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        _ => ExecError::Overflow,
    })?;

    // Verify gate exists
    let gate = read_approval_gate(db, &payload.gate_id)?.ok_or(ExecError::GateNotFound)?;

    // Validate proposal data based on type
    match payload.proposal_type {
        ProposalType::ModuleActivation | ProposalType::ModuleRollback => {
            // Proposal data must be entity_id (32 bytes)
            if payload.proposal_data.len() != 32 {
                return Err(ExecError::BadPayloadLength {
                    expected: 32,
                    got: payload.proposal_data.len(),
                });
            }
        }
        ProposalType::ParamChange | ProposalType::PolicyChange | ProposalType::EmergencyFreeze => {
            // Other types have variable-length data, no specific validation here
        }
    }

    // Week 25 Hardening (A25.4): Block Tier 0 actions at submission
    // ParamChange and PolicyChange accept arbitrary data that could encode Tier 0 actions
    if matches!(
        payload.proposal_type,
        ProposalType::ParamChange | ProposalType::PolicyChange
    ) {
        if let Some(&first_byte) = payload.proposal_data.first() {
            // Tier 0 actions (ModifyConsensusRule=0, ModifyStateTransition=1) are NEVER allowed
            if first_byte == ActionType::ModifyConsensusRule.to_byte()
                || first_byte == ActionType::ModifyStateTransition.to_byte()
            {
                return Err(ExecError::Tier0ActionForbidden);
            }
        }
    }

    // If proposer is an AI entity, enforce capability and kill switch checks.
    // Normal human accounts (entity not found) are always allowed to submit proposals.
    if let Some(entity) = read_ai_entity(db, &tx.from)? {
        // Kill switch blocks governance proposals from AI entities
        if read_ai_kill_switch(db)? {
            return Err(ExecError::AiKillSwitchActive);
        }
        // AI entity must be active
        if !entity.is_active {
            return Err(ExecError::EntityNotActive);
        }
        // All governance proposals from AI entities require emit_proposals
        if !entity.capabilities.emit_proposals {
            return Err(ExecError::IssuerMissingCapability);
        }
        // Execution-requesting proposal types require request_execution
        if matches!(
            payload.proposal_type,
            ProposalType::ParamChange
                | ProposalType::PolicyChange
                | ProposalType::ModuleActivation
                | ProposalType::ModuleRollback
                | ProposalType::EmergencyFreeze
        ) && !entity.capabilities.request_execution
        {
            return Err(ExecError::IssuerMissingCapability);
        }
    }

    // Create proposal (computes ID from content)
    let mut proposal = Proposal::new(
        payload.proposal_type,
        payload.proposal_data,
        tx.from,
        payload.gate_id,
        current_height,
        gate.expiry_blocks,
    );

    let proposal_id = proposal.id;

    // Week 25 Hardening (A25.2): Prevent overwriting non-terminal proposals
    // Only allow resubmission if proposal doesn't exist OR is in terminal state
    if let Some(existing) = read_proposal(db, &proposal_id)? {
        match existing.state {
            ProposalState::Executed | ProposalState::Rejected => {
                // Terminal states - allow resubmission (creates fresh proposal)
            }
            _ => {
                // Non-terminal (Submitted, Approved, Executable, or Expired)
                // Reject to prevent timing reset attacks
                return Err(ExecError::ProposalAlreadyExists);
            }
        }
    }

    // For TimelockOnly gates (threshold == 0): auto-approve immediately
    // The timelock countdown starts from submission
    if gate.threshold == 0 {
        proposal
            .approve(current_height, gate.timelock_blocks)
            .map_err(|_| ExecError::ProposalNotExecutable)?;
    }

    // Store proposal
    let op = write_proposal_op(&proposal);
    db.apply_batch(&[op]).map_err(ExecError::Db)?;

    Ok(proposal_id)
}

/// Apply a `ExecuteProposal` transaction.
///
/// Executes an approved proposal after timelock has elapsed.
///
/// # Validation
/// 1. Proposal must exist
/// 2. Proposal must be in Approved or Executable state
/// 3. Timelock must have elapsed (`current_height` >= `executable_at`)
/// 4. Proposal must not be expired
///
/// # State Changes
/// - For `ModuleActivation`: Set `entity.is_active` = true
/// - For `ModuleRollback`: Set `entity.is_active` = false
/// - Update proposal state to Executed
///
/// # Errors
///
/// Returns error if validation fails or DB operations fail.
pub fn apply_governance_execute_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<(), ExecError<K::Error>> {
    // Decode payload
    let payload = decode_execute_proposal_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        _ => ExecError::Overflow,
    })?;

    // Read proposal
    let mut proposal =
        read_proposal(db, &payload.proposal_id)?.ok_or(ExecError::ProposalNotFound)?;

    // Check expiry
    if proposal.is_expired(current_height) {
        return Err(ExecError::ProposalExpired);
    }

    // Check if proposal can be executed now
    // For TimelockOnly gates, proposal was auto-approved at submission
    // and will be executable once current_height >= executable_at
    if !proposal.can_execute_at(current_height) {
        return Err(ExecError::ProposalNotExecutable);
    }

    // Execute the proposal effect based on type
    match proposal.proposal_type {
        ProposalType::ModuleActivation => {
            // proposal_data contains entity_id
            if proposal.proposal_data.len() != 32 {
                return Err(ExecError::InvalidProposalType);
            }
            let mut entity_id = [0u8; 32];
            entity_id.copy_from_slice(&proposal.proposal_data);

            apply_module_activation(db, &entity_id)?;
        }
        ProposalType::ModuleRollback => {
            // proposal_data contains entity_id
            if proposal.proposal_data.len() != 32 {
                return Err(ExecError::InvalidProposalType);
            }
            let mut entity_id = [0u8; 32];
            entity_id.copy_from_slice(&proposal.proposal_data);

            apply_module_rollback(db, &entity_id)?;
        }
        ProposalType::EmergencyFreeze => {
            if proposal.proposal_data.len() == 1 {
                // Global AI kill switch: 1-byte payload toggles the kill switch
                // 0x01 = activate (block all AI operations)
                // 0x00 = deactivate (restore normal operation)
                let active = proposal.proposal_data[0] == 1;
                let op = write_ai_kill_switch_op(active);
                db.apply_batch(&[op]).map_err(ExecError::Db)?;
            } else if proposal.proposal_data.len() == 32 {
                // Per-entity emergency freeze
                let mut entity_id = [0u8; 32];
                entity_id.copy_from_slice(&proposal.proposal_data);
                apply_module_rollback(db, &entity_id)?;
            } else {
                return Err(ExecError::InvalidProposalType);
            }
        }
        ProposalType::ParamChange | ProposalType::PolicyChange => {
            // Week 25 Hardening (A25.4): Defense-in-depth check for Tier 0 actions
            if let Some(&first_byte) = proposal.proposal_data.first() {
                if first_byte == ActionType::ModifyConsensusRule.to_byte()
                    || first_byte == ActionType::ModifyStateTransition.to_byte()
                {
                    return Err(ExecError::Tier0ActionForbidden);
                }
            }

            // M-05: Block any governance action that would grant read_nnpx_derived.
            // This capability is a privacy boundary violation and must NEVER be
            // grantable via governance — only protocol upgrades can change this.
            if proposal
                .proposal_data
                .windows(b"read_nnpx_derived".len())
                .any(|w| w == b"read_nnpx_derived")
            {
                return Err(ExecError::Tier0ActionForbidden);
            }

            // These types would modify protocol parameters
            // For now, we just mark them as executed (implementation depends on specific params)
            // Full implementation would parse proposal_data and apply the parameter changes
        }
    }

    // Mark proposal as executed
    proposal
        .execute(current_height)
        .map_err(|_| ExecError::ProposalNotExecutable)?;

    // Store updated proposal
    let op = write_proposal_op(&proposal);
    db.apply_batch(&[op]).map_err(ExecError::Db)?;

    Ok(())
}

// ============================================================================
// SIGNAL COMMITMENT EXECUTION (Week 14 - D14.2, D14.3, D14.6)
// ============================================================================

/// Apply a `PublishSignalCommitment` transaction.
///
/// # Validation (D14.2)
/// 1. Issuer entity ID must match `tx.from`
/// 2. Issuer must be a registered AI entity
/// 3. Issuer must have `emit_proposals` capability
/// 4. Signal type must be valid (0-6) - checked during payload decode
/// 5. Issuer must have sufficient balance for fee
/// 6. Nonce must be correct
///
/// # State Changes (D14.3, D14.4, D14.6)
/// - Store commitment at `ai_signal_key(height, issuer)`
/// - Create secondary index by type
/// - Create secondary index by issuer
/// - Deduct fee from AI entity balance
/// - Increment AI entity nonce
/// - Update `last_active_at`
///
/// # Errors
/// Returns error if validation fails or DB error occurs.
pub fn apply_signal_commitment_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<(), ExecError<K::Error>> {
    let entity = lookup_ai_entity_by_address(db, &tx.from)?.ok_or(ExecError::IssuerNotFound)?;
    apply_signal_commitment_tx_inner(db, tx, entity, current_height)
}

/// Inner signal-commitment implementation taking a pre-resolved AI entity.
///
/// Called by `dispatch_tx` (which already resolved the entity via
/// `check_ai_entity_sender`) and by the public `apply_signal_commitment_tx`
/// wrapper (which does its own lookup for standalone callers).
///
/// Storage keys are derived from the canonical `entity.id` rather than `tx.from`,
/// because `tx.from` is `address_from_pubkey(entity.pubkey)` while signals are
/// indexed by the entity's primary id (`compute_id(code_hash, creator)`).
#[allow(clippy::too_many_lines)]
fn apply_signal_commitment_tx_inner<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    mut entity: AiEntity,
    current_height: u64,
) -> Result<(), ExecError<K::Error>> {
    // Kill switch: block all AI entity operations when active
    if read_ai_kill_switch(db)? {
        return Err(ExecError::AiKillSwitchActive);
    }

    // Decode payload (validates signal_type, failure_reason, proof_type)
    let payload = decode_signal_commitment_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        ExecError::InvalidCompositionFailureReason { byte } => {
            ExecError::InvalidCompositionFailureReason { byte }
        }
        ExecError::UnsupportedProofType { proof_type } => {
            ExecError::UnsupportedProofType { proof_type }
        }
        ExecError::ServiceAttestationInvalidStatus { status } => {
            ExecError::ServiceAttestationInvalidStatus { status }
        }
        _ => ExecError::Overflow,
    })?;

    // The payload's issuer_entity_id must match the canonical entity.id.
    // The entity was resolved from tx.from via the address→id reverse index;
    // this check pins the user-supplied issuer to the same identity.
    if payload.issuer_entity_id != entity.id {
        return Err(ExecError::IssuerMismatch);
    }

    // W5-06: Reject operations from deactivated entities
    if !entity.is_active {
        return Err(ExecError::EntityNotActive);
    }

    // D14.2: Validate emit_proposals capability (static or delegated)
    requires_capability(db, &entity, current_height, |c| c.emit_proposals)?;

    // D14.2: Validate nonce
    if tx.nonce != entity.nonce {
        return Err(ExecError::NonceMismatch {
            expected: entity.nonce,
            got: tx.nonce,
        });
    }

    // D14.2: Validate sufficient balance for fee
    let fee_u128 = u128::from(tx.fee);
    if entity.economic_balance < fee_u128 {
        return Err(ExecError::InsufficientFunds {
            balance: entity.economic_balance,
            needed: fee_u128,
        });
    }

    // D14.6: Deduct fee from AI entity balance
    entity.economic_balance = entity
        .economic_balance
        .checked_sub(fee_u128)
        .ok_or(ExecError::Overflow)?;

    // D14.6: Increment AI entity nonce
    entity.nonce = entity
        .nonce
        .checked_add(1)
        .ok_or(ExecError::NonceOverflow)?;

    // D14.6: Update last_active_at
    entity.last_active_at = current_height;

    // D14.3: Build SignalCommitment for storage (issuer = canonical entity.id)
    let commitment = SignalCommitment {
        commitment_hash: payload.signal_hash,
        signal_type: payload.signal_type,
        height: current_height,
        issuer: entity.id,
    };
    let commitment_bytes = encode_signal_commitment_v1(&commitment);

    // Build atomic batch of all state changes; all keys use canonical entity.id.
    let mut ops = Vec::new();

    let primary_key = ai_signal_key(current_height, &entity.id);
    ops.push(WriteOp::Put(primary_key, commitment_bytes.clone()));

    let type_key = ai_signal_by_type_key(payload.signal_type.to_byte(), current_height, &entity.id);
    ops.push(WriteOp::Put(type_key, commitment_bytes.clone()));

    let issuer_key = ai_signal_by_issuer_key(&entity.id, current_height);
    ops.push(WriteOp::Put(issuer_key, commitment_bytes));

    // Reputation update branch: only oracle entities, target lookup + clamp + write.
    // Runs BEFORE the issuer write so a single atomic batch covers both records.
    if payload.signal_type == AiSignalType::ReputationUpdate {
        requires_capability(db, &entity, current_height, |c| c.submit_reputation_updates)?;
        let extra = payload
            .reputation
            .as_ref()
            .ok_or_else(|| ExecError::CodecDecode("ReputationUpdate missing extra".into()))?;
        if extra.event_type > REP_EVENT_MAX {
            return Err(ExecError::InvalidReputationEventType {
                byte: extra.event_type,
            });
        }
        if extra.target_entity_id == entity.id {
            return Err(ExecError::SelfReputationUpdate);
        }

        let mut target =
            read_ai_entity(db, &extra.target_entity_id)?.ok_or(ExecError::TargetEntityNotFound)?;

        // Clamped i32 arithmetic so u16 underflow is impossible.
        let new_score: u16 = (i32::from(target.reputation_score) + i32::from(extra.points_delta))
            .clamp(0, i32::from(MAX_REPUTATION_SCORE))
            .try_into()
            .map_err(|_| ExecError::Overflow)?;
        target.reputation_score = new_score;

        if extra.event_type == REP_EVENT_JOB_COMPLETED {
            target.total_transactions = target.total_transactions.saturating_add(1);
        }
        target.reputation_events_count = target.reputation_events_count.saturating_add(1);

        ops.push(write_ai_entity_op(&target));
    }

    // Signal purchase branch: pay seller for a priced signal listed in their
    // SignalCatalog memory object. Service fee accrues to KEY_MARKETPLACE_TREASURY.
    // Runs BEFORE the buyer (entity) write so the atomic batch carries every
    // balance change together.
    if payload.signal_type == AiSignalType::SignalPurchase {
        let extra = payload
            .purchase
            .as_ref()
            .ok_or_else(|| ExecError::CodecDecode("SignalPurchase missing extra".into()))?;

        if extra.seller_entity_id == entity.id {
            return Err(ExecError::SellerIsBuyer);
        }

        let mut seller =
            read_ai_entity(db, &extra.seller_entity_id)?.ok_or(ExecError::SellerEntityNotFound)?;
        if !seller.is_active {
            return Err(ExecError::SellerEntityNotActive);
        }

        let catalogs = get_memory_objects_by_entity_and_type(
            db,
            &seller.id,
            MemoryObjectType::SignalCatalog.to_byte(),
        )?;
        if catalogs.is_empty() {
            return Err(ExecError::SignalCatalogNotFound);
        }
        // "Latest wins" when multiple catalogs exist: ai/memory_by_type/{type}/{entity}/{object_id}
        // is sorted by trailing object_id, which is the deterministic blake3 over (owner, type,
        // created_at, data); newer publications produce a different id, and the last entry in
        // the prefix scan is the canonical one for any given (entity, type) slot.
        let catalog_obj = catalogs.last().expect("non-empty checked above");
        let catalog = SignalCatalogData::decode(&catalog_obj.data)
            .ok_or_else(|| ExecError::CodecDecode("malformed SignalCatalog payload".into()))?;

        let offering = catalog.find_offering(extra.purchased_signal_type).ok_or(
            ExecError::SignalOfferingNotFound {
                signal_type: extra.purchased_signal_type,
            },
        )?;
        if !offering.is_active {
            return Err(ExecError::SignalOfferingInactive);
        }
        if offering.price_per_signal > extra.max_price {
            return Err(ExecError::PriceExceedsMaxPrice {
                offered: offering.price_per_signal,
                max: extra.max_price,
            });
        }

        let price = u128::from(offering.price_per_signal);
        let service_fee = price
            .checked_mul(MARKETPLACE_FEE_BPS)
            .ok_or(ExecError::Overflow)?
            / BPS_DENOMINATOR;
        let total = price.checked_add(service_fee).ok_or(ExecError::Overflow)?;

        if entity.economic_balance < total {
            return Err(ExecError::InsufficientEntityBalance {
                required: total,
                available: entity.economic_balance,
            });
        }
        entity.economic_balance = entity
            .economic_balance
            .checked_sub(total)
            .ok_or(ExecError::Overflow)?;
        seller.economic_balance = seller
            .economic_balance
            .checked_add(price)
            .ok_or(ExecError::Overflow)?;

        // Treasury credit only when there's a non-zero fee, so free signals
        // (price = 0) do not touch the treasury record at all.
        if service_fee > 0 {
            let new_treasury = read_treasury_balance(db, KEY_MARKETPLACE_TREASURY)?
                .checked_add(service_fee)
                .ok_or(ExecError::Overflow)?;
            ops.push(WriteOp::Put(
                KEY_MARKETPLACE_TREASURY.to_vec(),
                encode_fee_pool_v1(&FeePoolV1 {
                    balance: new_treasury,
                })
                .to_vec(),
            ));
        }

        entity.total_transactions = entity.total_transactions.saturating_add(1);
        seller.total_transactions = seller.total_transactions.saturating_add(1);

        ops.push(write_ai_entity_op(&seller));
    }

    // Stake deposit branch: move funds from issuer's economic_balance to its
    // stake_balance and refresh the lock height. Atomic with the entity write.
    if payload.signal_type == AiSignalType::StakeDeposit {
        let extra = payload
            .stake_deposit
            .as_ref()
            .ok_or_else(|| ExecError::CodecDecode("StakeDeposit missing extra".into()))?;

        if entity.economic_balance < extra.amount {
            return Err(ExecError::InsufficientEntityBalance {
                required: extra.amount,
                available: entity.economic_balance,
            });
        }
        entity.economic_balance = entity
            .economic_balance
            .checked_sub(extra.amount)
            .ok_or(ExecError::Overflow)?;
        entity.stake_balance = entity
            .stake_balance
            .checked_add(extra.amount)
            .ok_or(ExecError::Overflow)?;
        entity.stake_locked_until = current_height
            .checked_add(STAKE_LOCK_PERIOD)
            .ok_or(ExecError::Overflow)?;
    }

    // Stake withdraw branch: move funds from issuer's stake_balance back to
    // economic_balance. Rejected unless lock has expired. Partial withdrawals
    // leave remaining stake_balance unlocked (no re-lock on the leftover).
    if payload.signal_type == AiSignalType::StakeWithdraw {
        let extra = payload
            .stake_withdraw
            .as_ref()
            .ok_or_else(|| ExecError::CodecDecode("StakeWithdraw missing extra".into()))?;

        if entity.stake_locked_until > current_height {
            return Err(ExecError::StakeStillLocked {
                unlocks_at: entity.stake_locked_until,
                current: current_height,
            });
        }
        if entity.stake_balance < extra.amount {
            return Err(ExecError::InsufficientStakeBalance {
                required: extra.amount,
                available: entity.stake_balance,
            });
        }
        entity.stake_balance = entity
            .stake_balance
            .checked_sub(extra.amount)
            .ok_or(ExecError::Overflow)?;
        entity.economic_balance = entity
            .economic_balance
            .checked_add(extra.amount)
            .ok_or(ExecError::Overflow)?;
    }

    // Slash branch: oracle deducts target.stake_balance, credits the slashed
    // amount to KEY_SLASH_TREASURY, and applies a reputation update in the
    // same atomic batch. Slash is saturating - requesting more than is staked
    // takes everything available and credits that lower amount to the treasury.
    if payload.signal_type == AiSignalType::StakeSlash {
        requires_capability(db, &entity, current_height, |c| c.submit_reputation_updates)?;
        let extra = payload
            .stake_slash
            .as_ref()
            .ok_or_else(|| ExecError::CodecDecode("StakeSlash missing extra".into()))?;

        if extra.rep_event_type > REP_EVENT_MAX {
            return Err(ExecError::InvalidReputationEventType {
                byte: extra.rep_event_type,
            });
        }
        if extra.target_entity_id == entity.id {
            return Err(ExecError::SelfSlash);
        }

        let mut target =
            read_ai_entity(db, &extra.target_entity_id)?.ok_or(ExecError::TargetEntityNotFound)?;

        // Saturating slash: take min(stake_balance, slash_amount) and only
        // credit that actual amount to the treasury.
        let actual_slashed = target.stake_balance.min(extra.slash_amount);
        target.stake_balance = target
            .stake_balance
            .checked_sub(actual_slashed)
            .ok_or(ExecError::Overflow)?;

        if actual_slashed > 0 {
            let new_treasury = read_treasury_balance(db, KEY_SLASH_TREASURY)?
                .checked_add(actual_slashed)
                .ok_or(ExecError::Overflow)?;
            ops.push(WriteOp::Put(
                KEY_SLASH_TREASURY.to_vec(),
                encode_fee_pool_v1(&FeePoolV1 {
                    balance: new_treasury,
                })
                .to_vec(),
            ));
        }

        // Apply reputation update on the same target. Clamp i32 arithmetic
        // matches the ReputationUpdate handler's clamp semantics.
        let new_score: u16 = (i32::from(target.reputation_score) + i32::from(extra.points_delta))
            .clamp(0, i32::from(MAX_REPUTATION_SCORE))
            .try_into()
            .map_err(|_| ExecError::Overflow)?;
        target.reputation_score = new_score;
        target.reputation_events_count = target.reputation_events_count.saturating_add(1);

        ops.push(write_ai_entity_op(&target));
    }

    if payload.signal_type == AiSignalType::CompositionCheck {
        requires_capability(db, &entity, current_height, |c| c.submit_reputation_updates)?;
        let extra = payload
            .composition_check
            .as_ref()
            .ok_or_else(|| ExecError::CodecDecode("CompositionCheck missing extra".into()))?;

        // Self-check: an oracle cannot check itself (mirrors SelfSlash and
        // SelfReputationUpdate gates).
        if extra.target_entity_id == entity.id {
            return Err(ExecError::SelfCompositionCheck);
        }

        let mut target =
            read_ai_entity(db, &extra.target_entity_id)?.ok_or(ExecError::TargetEntityNotFound)?;

        // Read the target's latest CompositionGraph. Same "last() wins"
        // pattern as SignalCatalog purchase: get_memory_objects_by_entity_and_type
        // returns entries sorted by trailing object_id; the lexicographically
        // last one is treated as canonical.
        let graphs = get_memory_objects_by_entity_and_type(
            db,
            &target.id,
            MemoryObjectType::CompositionGraph.to_byte(),
        )?;
        if graphs.is_empty() {
            return Err(ExecError::CompositionGraphNotFound);
        }
        let graph_obj = graphs.last().expect("non-empty checked above");
        let graph = CompositionGraphData::decode(&graph_obj.data)
            .ok_or_else(|| ExecError::CodecDecode("malformed CompositionGraph payload".into()))?;

        let dep_count = graph.dependencies.len();
        #[allow(clippy::cast_possible_truncation)]
        let max_idx_byte = dep_count as u8;
        let idx = extra.failed_dependency_idx as usize;
        let dep = graph
            .dependencies
            .get(idx)
            .ok_or(ExecError::InvalidDependencyIndex {
                index: extra.failed_dependency_idx,
                max: max_idx_byte,
            })?;

        // Verify the claimed failure_reason against the source entity's
        // current state. failure_reason was already validated against
        // COMPOSITION_FAILURE_REASON_MAX at decode time, so the match is
        // exhaustive over [0, REASON_MAX].
        let source_opt = read_ai_entity(db, &dep.source_entity_id)?;
        let verified = match extra.failure_reason {
            COMPOSITION_FAILURE_SOURCE_NOT_FOUND => source_opt.is_none(),
            COMPOSITION_FAILURE_SOURCE_INACTIVE => {
                source_opt.as_ref().is_some_and(|s| !s.is_active)
            }
            COMPOSITION_FAILURE_REPUTATION_BELOW_MIN => source_opt
                .as_ref()
                .is_some_and(|s| s.reputation_score < dep.min_reputation),
            COMPOSITION_FAILURE_STAKE_BELOW_MIN => source_opt
                .as_ref()
                .is_some_and(|s| s.stake_balance < u128::from(dep.min_stake)),
            _ => false,
        };
        if !verified {
            return Err(ExecError::DependencyFailureNotVerified);
        }

        // Auto-pause if the dependency is required. Idempotent: re-pausing
        // an already-inactive target is a no-op for is_active but still
        // emits the reputation event, matching StakeSlash semantics on
        // already-inactive targets.
        if dep.is_required {
            target.is_active = false;
        }

        // Always emit a REP_EVENT_COMPOSITION_FAILURE event with delta -1.
        let new_score: u16 = (i32::from(target.reputation_score) - 1)
            .clamp(0, i32::from(MAX_REPUTATION_SCORE))
            .try_into()
            .map_err(|_| ExecError::Overflow)?;
        target.reputation_score = new_score;
        target.reputation_events_count = target.reputation_events_count.saturating_add(1);

        ops.push(write_ai_entity_op(&target));
    }

    // ProofSubmission branch: verify a ZK proof attesting to off-chain
    // computation integrity. On success, persist a VerificationRecord
    // memory object owned by the issuer and apply +3 reputation to the
    // issuer. proof_type was already validated against PROOF_TYPE_MAX at
    // decode time, so the verifier call is guaranteed to be on a
    // supported system.
    //
    // v1 NOTE: the proof bytes themselves are NOT carried in the
    // SignalCommitment tail (which is fixed-size 65 bytes for this
    // signal). The stub verifier accepts an empty proof slice and always
    // returns true. When a real verifier is plumbed in, proof bytes will
    // be resolved off-chain via the artifact referenced by signal_hash;
    // proof_hash below is blake3 of the bytes that were verified, so it
    // remains a stable per-proof identifier across that transition.
    if payload.signal_type == AiSignalType::ProofSubmission {
        let extra = payload
            .proof_submission
            .as_ref()
            .ok_or_else(|| ExecError::CodecDecode("ProofSubmission missing extra".into()))?;

        // Build public inputs by concatenating code_hash and computation_hash.
        // The verifier also receives code_hash separately for circuit-key
        // routing; keeping it in public_inputs lets the proof bind to the
        // same value the trait caller passes.
        let mut public_inputs = [0u8; 64];
        public_inputs[..32].copy_from_slice(&extra.code_hash);
        public_inputs[32..].copy_from_slice(&extra.computation_hash);

        // Dispatch by proof_type. For PROOF_TYPE_STUB the v1 wire layout
        // carries no proof bytes, so vk/proof slices are empty and the stub
        // accepts unconditionally. For PROOF_TYPE_GROTH16 the v2 wire
        // layout carries real vk_bytes and proof_bytes which Groth16Verifier
        // deserialises and pairing-checks against `public_inputs`.
        let proof_bytes: &[u8] = &extra.proof_bytes;
        let vk_bytes: &[u8] = &extra.vk_bytes;
        let ok = match extra.proof_type {
            PROOF_TYPE_STUB => StubZkVerifier::verify_proof(
                proof_bytes,
                vk_bytes,
                &public_inputs,
                extra.proof_type,
                &extra.code_hash,
            ),
            PROOF_TYPE_GROTH16 => Groth16Verifier::verify_proof(
                proof_bytes,
                vk_bytes,
                &public_inputs,
                extra.proof_type,
                &extra.code_hash,
            ),
            // Decoder rejects proof_type > PROOF_TYPE_MAX, so any value
            // arriving here is one of the dispatched discriminants above.
            _ => unreachable!("proof_type validated against PROOF_TYPE_MAX at decode time"),
        };
        if !ok {
            return Err(ExecError::ProofVerificationFailed);
        }

        // Persist VerificationRecord memory object owned by the issuer.
        let proof_hash = *blake3::hash(proof_bytes).as_bytes();
        let record = VerificationRecordData {
            proof_type: extra.proof_type,
            code_hash: extra.code_hash,
            computation_hash: extra.computation_hash,
            proof_hash,
            height: current_height,
        };
        let mem_obj = MemoryObject::new(
            entity.id,
            MemoryObjectType::VerificationRecord,
            current_height,
            record.encode().to_vec(),
        );
        let mem_obj_id = mem_obj.object_id;
        let mem_encoded = encode_memory_object_v1(&mem_obj);
        ops.push(WriteOp::Put(
            ai_memory_object_key(&entity.id, &mem_obj_id),
            mem_encoded,
        ));
        ops.push(WriteOp::Put(
            ai_memory_by_type_key(
                MemoryObjectType::VerificationRecord.to_byte(),
                &entity.id,
                &mem_obj_id,
            ),
            Vec::new(),
        ));

        // Apply REP_EVENT_PROOF_VERIFIED with delta +3 to the issuer.
        // Clamped i32 arithmetic mirrors the ReputationUpdate handler.
        let new_score: u16 = (i32::from(entity.reputation_score) + 3)
            .clamp(0, i32::from(MAX_REPUTATION_SCORE))
            .try_into()
            .map_err(|_| ExecError::Overflow)?;
        entity.reputation_score = new_score;
        entity.reputation_events_count = entity.reputation_events_count.saturating_add(1);
    }

    // SubscriptionCreate branch (Feature 9): the issuer (subscriber) locks
    // `rate_per_block * duration_blocks` of `economic_balance` and creates
    // a `Subscription` memory object owned by itself. The locked amount
    // sits inside the memory object record (NOT in stake_balance) and is
    // released by a subsequent `SubscriptionCancel` signal.
    if payload.signal_type == AiSignalType::SubscriptionCreate {
        let extra = payload
            .subscription_create
            .as_ref()
            .ok_or_else(|| ExecError::CodecDecode("SubscriptionCreate missing extra".into()))?;

        if extra.producer_entity_id == entity.id {
            return Err(ExecError::SubscriptionSelfReferential);
        }
        if extra.duration_blocks < MIN_SUBSCRIPTION_DURATION {
            return Err(ExecError::SubscriptionDurationTooShort {
                required: MIN_SUBSCRIPTION_DURATION,
                given: extra.duration_blocks,
            });
        }

        let producer = read_ai_entity(db, &extra.producer_entity_id)?
            .ok_or(ExecError::SubscriptionProducerNotFound)?;
        if !producer.is_active {
            return Err(ExecError::SubscriptionProducerNotActive);
        }

        // Cap enforcement: count Subscription memory objects under this
        // subscriber. Cancelled records still occupy a slot until the
        // subscriber deletes them via DELETE_MEMORY_OBJECT.
        let existing = get_memory_objects_by_entity_and_type(
            db,
            &entity.id,
            MemoryObjectType::Subscription.to_byte(),
        )?;
        #[allow(clippy::cast_possible_truncation)]
        let existing_count = existing.len() as u32;
        if existing_count >= MAX_SUBSCRIPTIONS_PER_ENTITY {
            return Err(ExecError::SubscriptionLimitExceeded {
                current: existing_count,
                max: MAX_SUBSCRIPTIONS_PER_ENTITY,
            });
        }

        // Subscriptions also count against the global per-entity memory cap.
        let mem_count = read_memory_count(db, &entity.id)?;
        if mem_count >= MAX_MEMORY_OBJECTS_PER_ENTITY {
            return Err(ExecError::MemoryObjectCountExceeded {
                count: mem_count,
                max: MAX_MEMORY_OBJECTS_PER_ENTITY,
            });
        }

        let total_locked = u128::from(extra.rate_per_block)
            .checked_mul(u128::from(extra.duration_blocks))
            .ok_or(ExecError::SubscriptionRateOverflow)?;
        if entity.economic_balance < total_locked {
            return Err(ExecError::SubscriptionInsufficientBalance {
                required: total_locked,
                available: entity.economic_balance,
            });
        }
        entity.economic_balance = entity
            .economic_balance
            .checked_sub(total_locked)
            .ok_or(ExecError::Overflow)?;

        let end_height = current_height
            .checked_add(extra.duration_blocks)
            .ok_or(ExecError::Overflow)?;
        let sub_data = SubscriptionData {
            subscriber_entity_id: entity.id,
            producer_entity_id: extra.producer_entity_id,
            covered_signal_type: extra.covered_signal_type,
            rate_per_block: extra.rate_per_block,
            start_height: current_height,
            end_height,
            last_settled_height: current_height,
            total_locked,
            is_active: true,
        };
        let encoded_data = sub_data.encode().to_vec();
        let mem_obj = MemoryObject::new(
            entity.id,
            MemoryObjectType::Subscription,
            current_height,
            encoded_data,
        );
        let mem_obj_id = mem_obj.object_id;
        let mem_encoded = encode_memory_object_v1(&mem_obj);
        ops.push(WriteOp::Put(
            ai_memory_object_key(&entity.id, &mem_obj_id),
            mem_encoded,
        ));
        ops.push(WriteOp::Put(
            ai_memory_by_type_key(
                MemoryObjectType::Subscription.to_byte(),
                &entity.id,
                &mem_obj_id,
            ),
            Vec::new(),
        ));
        ops.push(WriteOp::Put(
            ai_memory_count_key(&entity.id),
            encode_memory_count(mem_count + 1).to_vec(),
        ));
    }

    // SubscriptionCancel branch (Feature 9): the original subscriber
    // terminates an active subscription early. Settles accrued payment
    // (with the standard 2% marketplace fee), pays the producer the 5%
    // cancel fee on the unaccrued remainder, refunds the rest to the
    // subscriber, and rewrites the memory object with is_active = false.
    if payload.signal_type == AiSignalType::SubscriptionCancel {
        let extra = payload
            .subscription_cancel
            .as_ref()
            .ok_or_else(|| ExecError::CodecDecode("SubscriptionCancel missing extra".into()))?;

        // Subscription records are stored under their owner (the subscriber);
        // the issuer of the cancel signal must be that owner, which we
        // enforce both by addressing the primary record under entity.id and
        // by re-checking the embedded subscriber_entity_id below.
        let sub_obj = read_memory_object(db, &entity.id, &extra.subscription_id)?
            .ok_or(ExecError::SubscriptionNotFound)?;
        if sub_obj.object_type != MemoryObjectType::Subscription {
            return Err(ExecError::SubscriptionWrongObjectType);
        }
        let mut sub_data = SubscriptionData::decode(&sub_obj.data)
            .ok_or(ExecError::SubscriptionMemoryDecodeFailed)?;
        if sub_data.subscriber_entity_id != entity.id {
            return Err(ExecError::SubscriptionNotOwner);
        }
        if !sub_data.is_active {
            return Err(ExecError::SubscriptionNotActive);
        }

        // Settlement: accrued blocks are capped at end_height. The gross
        // amount cannot overflow u128 in v1 (both factors are u64) but is
        // checked anyway so a future widening of either field stays safe.
        let cap = if current_height < sub_data.end_height {
            current_height
        } else {
            sub_data.end_height
        };
        let settled_blocks = cap.saturating_sub(sub_data.last_settled_height);
        let accrued_gross = u128::from(settled_blocks)
            .checked_mul(u128::from(sub_data.rate_per_block))
            .ok_or(ExecError::Overflow)?;
        let accrued_fee = accrued_gross
            .checked_mul(MARKETPLACE_FEE_BPS)
            .ok_or(ExecError::Overflow)?
            / BPS_DENOMINATOR;
        let accrued_net = accrued_gross
            .checked_sub(accrued_fee)
            .ok_or(ExecError::Overflow)?;

        // Unaccrued remainder: total_locked - accrued_gross. The 5% cancel
        // fee on this remainder is paid 100% to the producer (no
        // marketplace cut on the cancel fee, by design). The rest goes
        // back to the subscriber.
        let remaining = sub_data
            .total_locked
            .checked_sub(accrued_gross)
            .ok_or(ExecError::Overflow)?;
        let cancel_fee = remaining
            .checked_mul(SUBSCRIPTION_CANCEL_FEE_BPS)
            .ok_or(ExecError::Overflow)?
            / BPS_DENOMINATOR;
        let refund = remaining
            .checked_sub(cancel_fee)
            .ok_or(ExecError::Overflow)?;

        let producer_credit = accrued_net
            .checked_add(cancel_fee)
            .ok_or(ExecError::Overflow)?;

        let mut producer = read_ai_entity(db, &sub_data.producer_entity_id)?
            .ok_or(ExecError::SubscriptionProducerNotFound)?;
        // Producer being inactive does not block settlement; funds owed
        // for already-rendered service must still flow.
        producer.economic_balance = producer
            .economic_balance
            .checked_add(producer_credit)
            .ok_or(ExecError::Overflow)?;
        entity.economic_balance = entity
            .economic_balance
            .checked_add(refund)
            .ok_or(ExecError::Overflow)?;

        if accrued_fee > 0 {
            let new_treasury = read_treasury_balance(db, KEY_MARKETPLACE_TREASURY)?
                .checked_add(accrued_fee)
                .ok_or(ExecError::Overflow)?;
            ops.push(WriteOp::Put(
                KEY_MARKETPLACE_TREASURY.to_vec(),
                encode_fee_pool_v1(&FeePoolV1 {
                    balance: new_treasury,
                })
                .to_vec(),
            ));
        }

        // Mark the record settled and inactive. Rewriting in place keeps
        // the existing object_id (it was hashed at create time over the
        // original data); the subscriber can address it by the same id
        // for inspection or later DELETE_MEMORY_OBJECT cleanup.
        sub_data.last_settled_height = cap;
        sub_data.is_active = false;
        let updated_data = sub_data.encode().to_vec();
        let updated_obj = MemoryObject {
            data: updated_data,
            updated_at: current_height,
            ..sub_obj
        };
        let updated_encoded = encode_memory_object_v1(&updated_obj);
        ops.push(WriteOp::Put(
            ai_memory_object_key(&entity.id, &updated_obj.object_id),
            updated_encoded,
        ));

        ops.push(write_ai_entity_op(&producer));
    }

    // PaymentRequest branch (Week 28): native x402-style settlement. The
    // issuer (payer) pays `amount` to the payee and `fee = amount *
    // PAYMENT_FEE_BPS / BPS_DENOMINATOR` to the marketplace treasury.
    // Replay protection is enforced at signal-hash granularity by the
    // by_hash record: any subsequent PaymentRequest carrying the same
    // signal_hash is rejected with PaymentAlreadySettled.
    if payload.signal_type == AiSignalType::PaymentRequest {
        let extra = payload
            .payment_request
            .as_ref()
            .ok_or_else(|| ExecError::CodecDecode("PaymentRequest missing extra".into()))?;

        if extra.payee_entity_id == entity.id {
            return Err(ExecError::PaymentSelfReferential);
        }
        if extra.amount == 0 {
            return Err(ExecError::PaymentAmountZero);
        }
        if current_height > extra.max_block_height {
            return Err(ExecError::PaymentExpired {
                current_height,
                max_block_height: extra.max_block_height,
            });
        }

        let mut payee = read_ai_entity(db, &extra.payee_entity_id)?
            .ok_or(ExecError::PaymentPayeeNotFound)?;
        if !payee.is_active {
            return Err(ExecError::PaymentPayeeNotActive);
        }

        // Replay guard. The by_hash record is the canonical seen-set
        // entry; its presence rejects every duplicate before any state
        // mutation. Storing the record at the end of this branch closes
        // the window for the same signal_hash going forward.
        let by_hash_key = payment_by_hash_key(&payload.signal_hash);
        if db.get(&by_hash_key).map_err(ExecError::Db)?.is_some() {
            return Err(ExecError::PaymentAlreadySettled {
                signal_hash: payload.signal_hash,
            });
        }

        let amount_u128 = u128::from(extra.amount);
        let fee = amount_u128
            .checked_mul(PAYMENT_FEE_BPS)
            .ok_or(ExecError::Overflow)?
            / BPS_DENOMINATOR;
        let total_debit = amount_u128.checked_add(fee).ok_or(ExecError::Overflow)?;

        if entity.economic_balance < total_debit {
            return Err(ExecError::PaymentInsufficientBalance {
                required: total_debit,
                available: entity.economic_balance,
            });
        }
        entity.economic_balance = entity
            .economic_balance
            .checked_sub(total_debit)
            .ok_or(ExecError::Overflow)?;
        payee.economic_balance = payee
            .economic_balance
            .checked_add(amount_u128)
            .ok_or(ExecError::Overflow)?;

        // Treasury credit only when fee > 0. amount > 0 is already
        // enforced, but the fee is still zero for amounts below
        // BPS_DENOMINATOR / PAYMENT_FEE_BPS (i.e., below 50 base units).
        // Skipping the treasury write in that case avoids dead state
        // churn and mirrors the SignalPurchase pattern at line 3562.
        if fee > 0 {
            let new_treasury = read_treasury_balance(db, KEY_MARKETPLACE_TREASURY)?
                .checked_add(fee)
                .ok_or(ExecError::Overflow)?;
            ops.push(WriteOp::Put(
                KEY_MARKETPLACE_TREASURY.to_vec(),
                encode_fee_pool_v1(&FeePoolV1 {
                    balance: new_treasury,
                })
                .to_vec(),
            ));
        }

        entity.total_transactions = entity.total_transactions.saturating_add(1);
        payee.total_transactions = payee.total_transactions.saturating_add(1);

        // Persist canonical payment record + two scan indexes. The by_hash
        // value carries the full record; the by_payer / by_payee entries
        // are zero-byte markers (the canonical data lives in by_hash).
        let record = PaymentRecord {
            payer: entity.id,
            payee: payee.id,
            amount: extra.amount,
            service_descriptor_hash: extra.service_descriptor_hash,
            request_hash: extra.request_hash,
            payment_height: current_height,
            max_block_height: extra.max_block_height,
            attested_status: PAYMENT_ATTESTATION_STATUS_NONE,
            attested_height: 0,
        };
        ops.push(WriteOp::Put(
            by_hash_key,
            encode_payment_record_v1(&record).to_vec(),
        ));
        ops.push(WriteOp::Put(
            payment_by_payer_key(&entity.id, current_height, &payload.signal_hash),
            Vec::new(),
        ));
        ops.push(WriteOp::Put(
            payment_by_payee_key(&payee.id, current_height, &payload.signal_hash),
            Vec::new(),
        ));

        ops.push(write_ai_entity_op(&payee));
    }

    // ServiceAttestation branch (Week 28): the payer of a prior
    // PaymentRequest attests to delivery status. The handler loads the
    // PaymentRecord from by_hash, pins the issuer to the recorded payer,
    // and applies REP_DELTA_PAYMENT_DELIVERED (+1) or
    // REP_DELTA_PAYMENT_FAILED (-3) to the payee with the standard
    // [0, MAX_REPUTATION_SCORE] clamp. The record is rewritten in place
    // with the attested status and height; a second attestation against
    // the same record is rejected.
    if payload.signal_type == AiSignalType::ServiceAttestation {
        let extra = payload
            .service_attestation
            .as_ref()
            .ok_or_else(|| ExecError::CodecDecode("ServiceAttestation missing extra".into()))?;

        // The decoder validates status <= PAYMENT_ATTESTATION_STATUS_MAX,
        // but the handler-side check is kept as a defense-in-depth guard
        // for any future path that constructs a SignalCommitmentPayloadV1
        // without round-tripping through decode.
        if extra.status > PAYMENT_ATTESTATION_STATUS_MAX {
            return Err(ExecError::ServiceAttestationInvalidStatus {
                status: extra.status,
            });
        }

        let by_hash_key = payment_by_hash_key(&extra.payment_signal_hash);
        let record_bytes = db
            .get(&by_hash_key)
            .map_err(ExecError::Db)?
            .ok_or(ExecError::ServiceAttestationPaymentNotFound)?;
        let mut record = decode_payment_record_v1(&record_bytes)
            .map_err(|_| ExecError::PaymentRecordDecodeFailed)?;

        if record.payer != entity.id {
            return Err(ExecError::ServiceAttestationNotPayer);
        }
        if record.payee != extra.payee_entity_id {
            return Err(ExecError::ServiceAttestationPayeeMismatch);
        }
        if record.attested_status != PAYMENT_ATTESTATION_STATUS_NONE {
            return Err(ExecError::ServiceAttestationAlreadyAttested);
        }

        // The payee record MUST exist at this point: it was loaded
        // successfully when the PaymentRequest was processed, and
        // entities are not deleted (only deactivated). A miss here
        // indicates state corruption and is surfaced as
        // PaymentRecordDecodeFailed - the error variant most aligned
        // with "the payment audit trail no longer matches the entity
        // store".
        let mut payee = read_ai_entity(db, &record.payee)?
            .ok_or(ExecError::PaymentRecordDecodeFailed)?;

        let delta = if extra.status == PAYMENT_ATTESTATION_STATUS_DELIVERED {
            REP_DELTA_PAYMENT_DELIVERED
        } else {
            REP_DELTA_PAYMENT_FAILED
        };
        let new_score: u16 = (i32::from(payee.reputation_score) + delta)
            .clamp(0, i32::from(MAX_REPUTATION_SCORE))
            .try_into()
            .map_err(|_| ExecError::Overflow)?;
        payee.reputation_score = new_score;
        payee.reputation_events_count = payee.reputation_events_count.saturating_add(1);

        // Rewrite the record in place. The same by_hash key holds the
        // updated bytes; future attestation attempts will see
        // attested_status != NONE and be rejected.
        record.attested_status = extra.status;
        record.attested_height = current_height;
        ops.push(WriteOp::Put(
            by_hash_key,
            encode_payment_record_v1(&record).to_vec(),
        ));

        ops.push(write_ai_entity_op(&payee));
    }

    ops.push(write_ai_entity_op(&entity));

    // Apply all changes atomically
    db.apply_batch(&ops).map_err(ExecError::Db)?;

    Ok(())
}

// ============================================================================
// REGISTER AI ENTITY EXECUTION (Audit Gap Fix)
// ============================================================================

/// Apply a `RegisterAiEntity` transaction.
///
/// Creates a new AI entity on-chain, funded by a normal account (the creator).
///
/// # Validation
/// 1. Payload must decode correctly
/// 2. Autonomous mode is rejected (reserved for future ZK proof support)
/// 3. Creator nonce must match `tx.nonce`
/// 4. Creator balance must cover `initial_balance + fee`
/// 5. Entity must not already exist (duplicate rejection)
///
/// # State Changes
/// - Debit creator by `initial_balance + fee`
/// - Increment creator nonce
/// - Credit fee pool by `fee`
/// - Create new AI entity with `initial_balance`
///
/// # Returns
/// The 32-byte entity ID on success.
///
/// # Errors
/// Returns error if validation fails or DB error occurs.
pub fn apply_register_ai_entity_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<[u8; 32], ExecError<K::Error>> {
    // Decode payload
    let payload = decode_register_ai_entity_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        _ => ExecError::Overflow,
    })?;

    // Reject Autonomous mode (reserved, requires ZK proofs)
    if payload.autonomy_mode == novai_ai_entities::AutonomyMode::Autonomous {
        return Err(ExecError::AutonomousModeReserved);
    }

    // Read creator account (sender is a normal account)
    let mut creator_acct = read_account_or_default(db, &tx.from)?;

    // Validate nonce
    if tx.nonce != creator_acct.nonce {
        return Err(ExecError::NonceMismatch {
            expected: creator_acct.nonce,
            got: tx.nonce,
        });
    }

    // Compute total cost: initial_balance + fee
    let fee_u128 = u128::from(tx.fee);
    let total_cost = payload
        .initial_balance
        .checked_add(fee_u128)
        .ok_or(ExecError::Overflow)?;

    if creator_acct.balance < total_cost {
        return Err(ExecError::InsufficientFunds {
            balance: creator_acct.balance,
            needed: total_cost,
        });
    }

    // Compute entity ID
    let entity_id = AiEntity::compute_id(&payload.code_hash, &tx.from);

    // Check no duplicate
    if read_ai_entity(db, &entity_id)?.is_some() {
        return Err(ExecError::EntityAlreadyExists);
    }

    // Create entity
    let mut entity = AiEntity::new(
        payload.code_hash,
        tx.from,
        payload.autonomy_mode,
        payload.capabilities,
        current_height,
    );
    entity.economic_balance = payload.initial_balance;

    // Debit creator
    creator_acct.balance = creator_acct
        .balance
        .checked_sub(total_cost)
        .ok_or(ExecError::Overflow)?;
    creator_acct.nonce = creator_acct
        .nonce
        .checked_add(1)
        .ok_or(ExecError::NonceOverflow)?;

    // Credit fee pool
    let mut fee_pool = read_fee_pool_or_default(db)?;
    fee_pool.balance = fee_pool
        .balance
        .checked_add(fee_u128)
        .ok_or(ExecError::Overflow)?;

    // Build atomic batch
    let ops = vec![
        WriteOp::Put(
            account_key(&tx.from),
            encode_account_v1(&creator_acct).to_vec(),
        ),
        WriteOp::Put(
            KEY_FEE_POOL.to_vec(),
            encode_fee_pool_v1(&fee_pool).to_vec(),
        ),
        write_ai_entity_op(&entity),
    ];

    db.apply_batch(&ops).map_err(ExecError::Db)?;

    Ok(entity_id)
}

// ============================================================================
// CREDIT AI ENTITY EXECUTION (Audit Gap Fix)
// ============================================================================

/// Apply a `CreditAiEntity` transaction.
///
/// Transfers funds from a normal account to an AI entity's balance.
///
/// # Validation
/// 1. Payload must decode correctly
/// 2. Sender nonce must match `tx.nonce`
/// 3. Sender balance must cover `amount + fee`
/// 4. Target entity must exist
/// 5. Target entity must be active
/// 6. Credit must not overflow entity balance
///
/// # State Changes
/// - Debit sender by `amount + fee`
/// - Increment sender nonce
/// - Credit entity by `amount`
/// - Credit fee pool by `fee`
///
/// # Errors
/// Returns error if validation fails or DB error occurs.
pub fn apply_credit_ai_entity_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    _current_height: u64,
) -> Result<(), ExecError<K::Error>> {
    // Decode payload
    let payload = decode_credit_ai_entity_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        _ => ExecError::Overflow,
    })?;

    // Read sender account (normal account)
    let mut sender_acct = read_account_or_default(db, &tx.from)?;

    // Validate nonce
    if tx.nonce != sender_acct.nonce {
        return Err(ExecError::NonceMismatch {
            expected: sender_acct.nonce,
            got: tx.nonce,
        });
    }

    // Compute total cost: amount + fee
    let fee_u128 = u128::from(tx.fee);
    let total_cost = payload
        .amount
        .checked_add(fee_u128)
        .ok_or(ExecError::Overflow)?;

    if sender_acct.balance < total_cost {
        return Err(ExecError::InsufficientFunds {
            balance: sender_acct.balance,
            needed: total_cost,
        });
    }

    // Load entity — must exist
    let mut entity = read_ai_entity(db, &payload.entity_id)?.ok_or(ExecError::EntityNotFound)?;

    // Entity must be active
    if !entity.is_active {
        return Err(ExecError::EntityNotActive);
    }

    // Debit sender
    sender_acct.balance = sender_acct
        .balance
        .checked_sub(total_cost)
        .ok_or(ExecError::Overflow)?;
    sender_acct.nonce = sender_acct
        .nonce
        .checked_add(1)
        .ok_or(ExecError::NonceOverflow)?;

    // Credit entity
    entity.economic_balance = entity
        .economic_balance
        .checked_add(payload.amount)
        .ok_or(ExecError::Overflow)?;

    // Credit fee pool
    let mut fee_pool = read_fee_pool_or_default(db)?;
    fee_pool.balance = fee_pool
        .balance
        .checked_add(fee_u128)
        .ok_or(ExecError::Overflow)?;

    // Build atomic batch
    let ops = vec![
        WriteOp::Put(
            account_key(&tx.from),
            encode_account_v1(&sender_acct).to_vec(),
        ),
        WriteOp::Put(
            KEY_FEE_POOL.to_vec(),
            encode_fee_pool_v1(&fee_pool).to_vec(),
        ),
        write_ai_entity_op(&entity),
    ];

    db.apply_batch(&ops).map_err(ExecError::Db)?;

    Ok(())
}

// ============================================================================
// REGISTER AI ENTITY WITH KEY EXECUTION (Type 10)
// ============================================================================

use novai_state::ai_entity_by_address_key;

/// Apply a `RegisterAiEntityWithKey` transaction (type 10).
///
/// Like type 8 but includes an ed25519 public key that enables the entity
/// to sign transactions. Also writes a reverse index from entity address to `entity_id`.
///
/// # Validation
/// 1. Payload must decode correctly (83 bytes)
/// 2. Autonomous mode rejected (reserved)
/// 3. AI kill switch must not be active
/// 4. Sender nonce must match `tx.nonce`
/// 5. Sender balance must cover `initial_balance + fee`
/// 6. Entity must not already exist
/// 7. No other entity may already be registered at the derived address
///
/// # State Changes
/// - Debit creator by `initial_balance + fee`
/// - Increment creator nonce
/// - Write entity (V3 with pubkey)
/// - Write reverse index `ai/entities_by_addr/{address} → entity_id`
/// - Credit fee pool by `fee`
///
/// # Errors
/// Returns error on decode failure, autonomous mode, kill switch active, nonce mismatch,
/// insufficient funds, duplicate entity, or duplicate address.
pub fn apply_register_ai_entity_with_key_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<[u8; 32], ExecError<K::Error>> {
    // Decode payload
    let payload =
        decode_register_ai_entity_with_key_payload_v1(&tx.payload).map_err(|e| match e {
            ExecError::BadPayloadLength { expected, got } => {
                ExecError::BadPayloadLength { expected, got }
            }
            ExecError::BadPayloadVersion { expected, got } => {
                ExecError::BadPayloadVersion { expected, got }
            }
            _ => ExecError::Overflow,
        })?;

    // Reject Autonomous mode (reserved, requires ZK proofs)
    if payload.autonomy_mode == novai_ai_entities::AutonomyMode::Autonomous {
        return Err(ExecError::AutonomousModeReserved);
    }

    // Check kill switch
    if read_ai_kill_switch(db)? {
        return Err(ExecError::AiKillSwitchActive);
    }

    // Read creator account (sender is a normal account)
    let mut creator_acct = read_account_or_default(db, &tx.from)?;

    // Validate nonce
    if tx.nonce != creator_acct.nonce {
        return Err(ExecError::NonceMismatch {
            expected: creator_acct.nonce,
            got: tx.nonce,
        });
    }

    // Compute total cost: initial_balance + fee
    let fee_u128 = u128::from(tx.fee);
    let total_cost = payload
        .initial_balance
        .checked_add(fee_u128)
        .ok_or(ExecError::Overflow)?;

    if creator_acct.balance < total_cost {
        return Err(ExecError::InsufficientFunds {
            balance: creator_acct.balance,
            needed: total_cost,
        });
    }

    // Compute entity ID
    let entity_id = AiEntity::compute_id(&payload.code_hash, &tx.from);

    // Check no duplicate entity
    if read_ai_entity(db, &entity_id)?.is_some() {
        return Err(ExecError::EntityAlreadyExists);
    }

    // Derive address from pubkey: address = blake3("NOVAI_ADDRESS_V1" || pubkey)
    let entity_addr = derive_address_from_pubkey_bytes(&payload.pubkey);

    // Check no existing entity registered at this address
    let addr_key = ai_entity_by_address_key(&entity_addr);
    if db.get(&addr_key).map_err(ExecError::Db)?.is_some() {
        return Err(ExecError::EntityAlreadyExists);
    }

    // Create entity with pubkey
    let mut entity = AiEntity::new_with_pubkey(
        payload.code_hash,
        tx.from,
        payload.autonomy_mode,
        payload.capabilities,
        payload.pubkey,
        current_height,
    );
    entity.economic_balance = payload.initial_balance;

    // Debit creator
    creator_acct.balance = creator_acct
        .balance
        .checked_sub(total_cost)
        .ok_or(ExecError::Overflow)?;
    creator_acct.nonce = creator_acct
        .nonce
        .checked_add(1)
        .ok_or(ExecError::NonceOverflow)?;

    // Credit fee pool
    let mut fee_pool = read_fee_pool_or_default(db)?;
    fee_pool.balance = fee_pool
        .balance
        .checked_add(fee_u128)
        .ok_or(ExecError::Overflow)?;

    // Build atomic batch (entity + reverse index + account + fee pool)
    let ops = vec![
        WriteOp::Put(
            account_key(&tx.from),
            encode_account_v1(&creator_acct).to_vec(),
        ),
        WriteOp::Put(
            KEY_FEE_POOL.to_vec(),
            encode_fee_pool_v1(&fee_pool).to_vec(),
        ),
        write_ai_entity_op(&entity),
        // Reverse index: address → entity_id
        WriteOp::Put(addr_key, entity_id.to_vec()),
    ];

    db.apply_batch(&ops).map_err(ExecError::Db)?;

    Ok(entity_id)
}

/// Look up an AI entity by sender address via reverse index.
///
/// Returns `Some(entity)` if the address maps to a registered AI entity, `None` otherwise.
///
/// # Errors
/// Returns error on DB failure or corrupt reverse index data.
pub fn lookup_ai_entity_by_address<K: Kv>(
    db: &K,
    addr: &Address,
) -> Result<Option<AiEntity>, ExecError<K::Error>> {
    let addr_key = ai_entity_by_address_key(addr);
    match db.get(&addr_key).map_err(ExecError::Db)? {
        None => Ok(None),
        Some(entity_id_bytes) => {
            if entity_id_bytes.len() != 32 {
                return Err(ExecError::CodecDecode(
                    "invalid entity_id in reverse index".into(),
                ));
            }
            let mut entity_id = [0u8; 32];
            entity_id.copy_from_slice(&entity_id_bytes);
            read_ai_entity(db, &entity_id)
        }
    }
}

// ============================================================================
// DELEGATION-AWARE CAPABILITY RESOLUTION (Feature 8)
// ============================================================================

use novai_ai_entities::{Capabilities, DelegationGrantData};
use novai_state::ai_delegations_by_delegate_prefix;

/// Resolve effective capabilities for an AI entity by merging its static
/// capabilities with active delegation grants targeting it.
///
/// Walks `ai_delegations_by_delegate_prefix(entity.id)`. For each entry:
///
/// 1. Extracts the trailing 32-byte grant id from the key and reads the
///    delegator id from the 32-byte value.
/// 2. Loads the underlying `DelegationGrant` memory object from
///    `ai_memory_object_key(delegator_id, grant_id)`.
/// 3. Decodes its `DelegationGrantData` payload.
/// 4. Skips the grant unless it is currently active (`is_active_at`) AND
///    the delegator entity exists and is itself active.
/// 5. ORs `granted_capabilities` into the accumulator on success.
///
/// Stale or corrupt index entries (missing primary, wrong object type,
/// decode failure, malformed key/value lengths) are silently skipped,
/// mirroring the policy in `get_memory_objects_by_entity_and_type`.
///
/// # Errors
/// Returns `ExecError::Db` on DB I/O failure during the prefix scan or
/// while loading delegator/grant records.
pub fn resolve_effective_capabilities<K: Kv>(
    db: &K,
    entity: &AiEntity,
    current_height: u64,
) -> Result<Capabilities, ExecError<K::Error>> {
    let mut effective = entity.capabilities;
    let prefix = ai_delegations_by_delegate_prefix(&entity.id);
    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;
    for (key, value) in entries {
        if key.len() < 32 || value.len() != 32 {
            continue;
        }
        let mut grant_id = [0u8; 32];
        grant_id.copy_from_slice(&key[key.len() - 32..]);
        let mut delegator_id = [0u8; 32];
        delegator_id.copy_from_slice(&value);

        let Some(memobj) = read_memory_object(db, &delegator_id, &grant_id)? else {
            continue;
        };
        if memobj.object_type != MemoryObjectType::DelegationGrant {
            continue;
        }
        let Some(grant) = DelegationGrantData::decode(&memobj.data) else {
            continue;
        };
        if !grant.is_active_at(current_height) {
            continue;
        }
        let Some(delegator) = read_ai_entity(db, &delegator_id)? else {
            continue;
        };
        if !delegator.is_active {
            continue;
        }

        let granted = Capabilities::from_byte(grant.granted_capabilities);
        effective = effective.or(&granted);
    }
    Ok(effective)
}

/// Verify that an entity satisfies a capability requirement either statically
/// or via an active delegation grant.
///
/// Fast path: if `selector(&entity.capabilities)` is true, returns `Ok(())`
/// without consulting the delegation index. Slow path: scans the by-delegate
/// index, builds the merged effective set, and re-evaluates the selector.
///
/// # Errors
/// Returns `ExecError::IssuerMissingCapability` when neither static nor
/// delegated capabilities satisfy the selector. Propagates `ExecError::Db`
/// or decode failures from the slow-path scan; corrupt individual entries
/// are silently skipped (a delegate cannot be denied service by a single
/// stale index row).
pub fn requires_capability<K, F>(
    db: &K,
    entity: &AiEntity,
    current_height: u64,
    selector: F,
) -> Result<(), ExecError<K::Error>>
where
    K: Kv,
    F: Fn(&Capabilities) -> bool,
{
    if selector(&entity.capabilities) {
        return Ok(());
    }
    let resolved = resolve_effective_capabilities(db, entity, current_height)?;
    if selector(&resolved) {
        Ok(())
    } else {
        Err(ExecError::IssuerMissingCapability)
    }
}

/// Check if a transaction sender is an AI entity and enforce restrictions.
///
/// Returns `Err` if the sender is an AI entity that is not allowed to submit this tx type.
/// Returns `Ok(Some(entity))` if the sender is a valid AI entity allowed to submit this type.
/// Returns `Ok(None)` if the sender is not an AI entity (normal account).
///
/// # Errors
/// Returns error if entity is inactive or tx type is restricted for AI entities.
pub fn check_ai_entity_sender<K: Kv>(
    db: &K,
    tx: &TxV1,
    current_height: u64,
) -> Result<Option<AiEntity>, ExecError<K::Error>> {
    let Some(entity) = lookup_ai_entity_by_address(db, &tx.from)? else {
        return Ok(None);
    };

    // Entity must be active
    if !entity.is_active {
        return Err(ExecError::EntityNotActive);
    }

    let tx_type = tx
        .payload
        .first()
        .copied()
        .ok_or(ExecError::UnknownPayloadVersion { version: 0 })?;

    match tx_type {
        // ALLOW: Transfer (type 1)
        TRANSFER_PAYLOAD_V1 => Ok(Some(entity)),

        // ALLOW: Signal Commitment (type 2) — if emit_proposals (static or delegated)
        SIGNAL_COMMITMENT_PAYLOAD_V1 => {
            requires_capability(db, &entity, current_height, |c| c.emit_proposals)?;
            Ok(Some(entity))
        }

        // ALLOW: Memory CRUD (types 3, 4, 5) — if read_memory_objects (static or delegated)
        CREATE_MEMORY_OBJECT_PAYLOAD_V1
        | UPDATE_MEMORY_OBJECT_PAYLOAD_V1
        | DELETE_MEMORY_OBJECT_PAYLOAD_V1 => {
            requires_capability(db, &entity, current_height, |c| c.read_memory_objects)?;
            Ok(Some(entity))
        }

        // DENY: Governance (6,7), entity registration (8,10), credit entity (9),
        // and all unknown types — AI entities cannot perform these operations.
        _ => Err(ExecError::IssuerMissingCapability),
    }
}

// ============================================================================
// MEMORY OBJECT EXECUTION (Week 21 - D21.4)
// ============================================================================

use novai_ai_entities::{
    decode_memory_object_v1, MAX_DELEGATION_GRANTS, MAX_MEMORY_OBJECTS_PER_ENTITY,
    MAX_MEMORY_OBJECT_SIZE,
};
use novai_state::{
    ai_delegation_by_delegate_key, ai_memory_count_key, decode_memory_count, encode_memory_count,
    KEY_PREFIX_AI_MEMORY_BY_TYPE, KEY_PREFIX_AI_MEMORY_OBJECTS,
};

/// Read memory object count for an entity.
fn read_memory_count<K: Kv>(db: &K, entity_id: &[u8; 32]) -> Result<u32, ExecError<K::Error>> {
    let key = ai_memory_count_key(entity_id);
    Ok(db
        .get(&key)
        .map_err(ExecError::Db)?
        .map_or(0, |bytes| decode_memory_count(&bytes)))
}

/// Read a memory object from storage.
///
/// # Errors
/// Returns error if DB read fails or stored data is malformed.
pub fn read_memory_object<K: Kv>(
    db: &K,
    entity_id: &[u8; 32],
    object_id: &[u8; 32],
) -> Result<Option<MemoryObject>, ExecError<K::Error>> {
    let key = ai_memory_object_key(entity_id, object_id);
    match db.get(&key).map_err(ExecError::Db)? {
        None => Ok(None),
        Some(bytes) => {
            let obj = decode_memory_object_v1(&bytes)
                .map_err(|e| ExecError::CodecDecode(format!("{e:?}")))?;
            Ok(Some(obj))
        }
    }
}

/// Apply a `CreateMemoryObject` transaction.
///
/// # Validation (D21.4)
/// 1. Entity must exist and match tx.from
/// 2. Entity must have `read_memory_objects` capability
/// 3. Data size must not exceed `MAX_MEMORY_OBJECT_SIZE`
/// 4. Entity must not exceed `MAX_MEMORY_OBJECTS_PER_ENTITY`
/// 5. Nonce must be correct
/// 6. Sufficient balance for fee
///
/// # State Changes
/// - Create memory object with computed ID
/// - Store at `ai/memory_objects/{entity}/{object_id}`
/// - Create type index at `ai/memory_by_type/{type}/{entity}/{object_id}`
/// - Increment memory count
/// - Deduct fee, increment nonce, update `last_active_at`
///
/// # Errors
/// Returns error if validation fails or DB error occurs.
pub fn apply_create_memory_object_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<[u8; 32], ExecError<K::Error>> {
    let entity = lookup_ai_entity_by_address(db, &tx.from)?.ok_or(ExecError::IssuerNotFound)?;
    apply_create_memory_object_tx_inner(db, tx, entity, current_height)
}

/// Inner create-memory-object implementation taking a pre-resolved AI entity.
///
/// Storage keys (object key, type index, count) are derived from `entity.id`
/// rather than `tx.from`, which equals `address_from_pubkey(entity.pubkey)` and
/// would not match what `get_memory_objects_by_entity(entity_id)` reads.
/// Validate a `CompositionGraph` payload against the owning entity.
///
/// Decodes the payload as `CompositionGraphData` and rejects any
/// dependency whose `source_entity_id` equals the owner — entities cannot
/// depend on themselves. Used by both create and update handlers so the
/// invariant cannot be bypassed by creating a clean graph and then
/// updating it to include a self-reference.
///
/// Returns `Ok(())` if `object_type != CompositionGraph` (no-op on
/// non-composition types) or the graph passes validation.
fn validate_composition_graph_payload<E>(
    object_type: MemoryObjectType,
    data: &[u8],
    owner_id: &[u8; 32],
) -> Result<(), ExecError<E>> {
    if object_type != MemoryObjectType::CompositionGraph {
        return Ok(());
    }
    let graph = CompositionGraphData::decode(data)
        .ok_or_else(|| ExecError::CodecDecode("malformed CompositionGraph payload".into()))?;
    for dep in &graph.dependencies {
        if &dep.source_entity_id == owner_id {
            return Err(ExecError::SelfDependency);
        }
    }
    Ok(())
}

/// Per-type structural and semantic validation for a `DelegationGrant`
/// memory object payload. No-op for non-`DelegationGrant` types so the
/// CREATE handler can call it unconditionally alongside the existing
/// `validate_composition_graph_payload`.
///
/// Validation rules (Feature 8):
/// 1. `DelegationGrantData` decodes cleanly and the version byte matches
///    `DELEGATION_GRANT_VERSION`.
/// 2. `delegate_entity_id != delegator.id` (no self-delegation).
/// 3. Every bit set in `granted_capabilities` is also set in the
///    delegator's static capabilities. An entity cannot grant authority
///    it does not currently hold.
/// 4. The delegator currently holds fewer than `MAX_DELEGATION_GRANTS`
///    open `DelegationGrant` memory objects. Cancelled or expired grants
///    still count toward the cap until they are deleted.
///
/// Returns the decoded `DelegationGrantData` on success so the create
/// handler can derive the `delegate_entity_id` for the by-delegate
/// secondary index without re-decoding.
fn validate_delegation_grant_payload<K: Kv>(
    db: &K,
    object_type: MemoryObjectType,
    data: &[u8],
    delegator: &AiEntity,
) -> Result<Option<DelegationGrantData>, ExecError<K::Error>> {
    if object_type != MemoryObjectType::DelegationGrant {
        return Ok(None);
    }
    let grant = DelegationGrantData::decode(data).ok_or(ExecError::InvalidDelegationGrant)?;
    if grant.delegate_entity_id == delegator.id {
        return Err(ExecError::InvalidDelegationSelf);
    }
    let delegator_caps = delegator.capabilities.to_byte();
    if (grant.granted_capabilities & !delegator_caps) != 0 {
        return Err(ExecError::DelegationCapabilityNotHeld);
    }

    // Count existing DelegationGrant memory objects owned by this
    // delegator via the ai_memory_by_type index. Bounded scan over the
    // (type, delegator) prefix; the cap is small so the linear walk is
    // cheaper than maintaining a dedicated counter.
    let mut count_prefix = Vec::with_capacity(KEY_PREFIX_AI_MEMORY_BY_TYPE.len() + 1 + 1 + 32 + 1);
    count_prefix.extend_from_slice(KEY_PREFIX_AI_MEMORY_BY_TYPE);
    count_prefix.push(MemoryObjectType::DelegationGrant.to_byte());
    count_prefix.push(b'/');
    count_prefix.extend_from_slice(&delegator.id);
    count_prefix.push(b'/');
    let entries = db.scan_prefix(&count_prefix).map_err(ExecError::Db)?;
    #[allow(clippy::cast_possible_truncation)]
    let current = entries.len() as u32;
    if current >= MAX_DELEGATION_GRANTS {
        return Err(ExecError::DelegationCountExceeded {
            current,
            max: MAX_DELEGATION_GRANTS,
        });
    }
    Ok(Some(grant))
}

fn apply_create_memory_object_tx_inner<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    mut entity: AiEntity,
    current_height: u64,
) -> Result<[u8; 32], ExecError<K::Error>> {
    // Kill switch: block all AI entity operations when active
    if read_ai_kill_switch(db)? {
        return Err(ExecError::AiKillSwitchActive);
    }

    // Decode payload
    let payload = decode_create_memory_object_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        ExecError::InvalidMemoryObjectType { byte } => ExecError::InvalidMemoryObjectType { byte },
        _ => ExecError::Overflow,
    })?;

    // Validate data size
    if payload.data.len() > MAX_MEMORY_OBJECT_SIZE {
        return Err(ExecError::MemoryObjectTooLarge {
            size: payload.data.len(),
            max: MAX_MEMORY_OBJECT_SIZE,
        });
    }

    // Per-type structural validation. CompositionGraph rejects
    // self-dependencies; other types are no-op.
    validate_composition_graph_payload(payload.object_type, &payload.data, &entity.id)?;

    // Per-type structural validation for DelegationGrant (Feature 8).
    // Decodes the grant once so the by-delegate secondary index Put below
    // can address the delegate without redecoding the payload.
    let delegation_grant =
        validate_delegation_grant_payload(db, payload.object_type, &payload.data, &entity)?;

    // W5-06: Reject operations from deactivated entities
    if !entity.is_active {
        return Err(ExecError::EntityNotActive);
    }

    // Validate capability (static or via active delegation grant)
    requires_capability(db, &entity, current_height, |c| c.read_memory_objects)?;

    // Validate nonce
    if tx.nonce != entity.nonce {
        return Err(ExecError::NonceMismatch {
            expected: entity.nonce,
            got: tx.nonce,
        });
    }

    // Validate balance
    let fee_u128 = u128::from(tx.fee);
    if entity.economic_balance < fee_u128 {
        return Err(ExecError::InsufficientFunds {
            balance: entity.economic_balance,
            needed: fee_u128,
        });
    }

    // Check memory object count limit (keyed by canonical entity.id)
    let current_count = read_memory_count(db, &entity.id)?;
    if current_count >= MAX_MEMORY_OBJECTS_PER_ENTITY {
        return Err(ExecError::MemoryObjectCountExceeded {
            count: current_count,
            max: MAX_MEMORY_OBJECTS_PER_ENTITY,
        });
    }

    // Create memory object owned by canonical entity.id
    let memory_object =
        MemoryObject::new(entity.id, payload.object_type, current_height, payload.data);
    let object_id = memory_object.object_id;
    let encoded = encode_memory_object_v1(&memory_object);

    // Update entity state
    entity.economic_balance = entity
        .economic_balance
        .checked_sub(fee_u128)
        .ok_or(ExecError::Overflow)?;
    entity.nonce = entity
        .nonce
        .checked_add(1)
        .ok_or(ExecError::NonceOverflow)?;
    entity.last_active_at = current_height;

    // Build atomic batch — all storage keys use canonical entity.id
    let mut ops = Vec::new();

    let obj_key = ai_memory_object_key(&entity.id, &object_id);
    ops.push(WriteOp::Put(obj_key, encoded));

    let type_key = ai_memory_by_type_key(payload.object_type.to_byte(), &entity.id, &object_id);
    ops.push(WriteOp::Put(type_key, vec![])); // Presence-only index

    // Feature 8: secondary index by delegate so capability resolution can
    // discover grants without scanning every memory object owned by every
    // delegator. Value carries the delegator id; key carries the grant id.
    if let Some(grant) = &delegation_grant {
        let delegate_index_key =
            ai_delegation_by_delegate_key(&grant.delegate_entity_id, &object_id);
        ops.push(WriteOp::Put(delegate_index_key, entity.id.to_vec()));
    }

    let count_key = ai_memory_count_key(&entity.id);
    ops.push(WriteOp::Put(
        count_key,
        encode_memory_count(current_count + 1).to_vec(),
    ));

    ops.push(write_ai_entity_op(&entity));

    // Apply atomically
    db.apply_batch(&ops).map_err(ExecError::Db)?;

    Ok(object_id)
}

/// Apply an `UpdateMemoryObject` transaction.
///
/// # Validation (D21.4)
/// 1. Entity must exist and match tx.from
/// 2. Memory object must exist and be owned by entity
/// 3. New data size must not exceed `MAX_MEMORY_OBJECT_SIZE`
/// 4. Nonce must be correct
/// 5. Sufficient balance for fee
///
/// # State Changes
/// - Update memory object data and `updated_at`
/// - Deduct fee, increment nonce, update `last_active_at`
///
/// # Errors
/// Returns error if validation fails or DB error occurs.
pub fn apply_update_memory_object_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<(), ExecError<K::Error>> {
    let entity = lookup_ai_entity_by_address(db, &tx.from)?.ok_or(ExecError::IssuerNotFound)?;
    apply_update_memory_object_tx_inner(db, tx, entity, current_height)
}

/// Inner update-memory-object implementation taking a pre-resolved AI entity.
fn apply_update_memory_object_tx_inner<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    mut entity: AiEntity,
    current_height: u64,
) -> Result<(), ExecError<K::Error>> {
    // Kill switch: block all AI entity operations when active
    if read_ai_kill_switch(db)? {
        return Err(ExecError::AiKillSwitchActive);
    }

    // Decode payload
    let payload = decode_update_memory_object_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        _ => ExecError::Overflow,
    })?;

    // Validate data size
    if payload.new_data.len() > MAX_MEMORY_OBJECT_SIZE {
        return Err(ExecError::MemoryObjectTooLarge {
            size: payload.new_data.len(),
            max: MAX_MEMORY_OBJECT_SIZE,
        });
    }

    // W5-06: Reject operations from deactivated entities
    if !entity.is_active {
        return Err(ExecError::EntityNotActive);
    }

    // Validate read_memory_objects capability (static or via active delegation grant)
    requires_capability(db, &entity, current_height, |c| c.read_memory_objects)?;

    // Validate nonce
    if tx.nonce != entity.nonce {
        return Err(ExecError::NonceMismatch {
            expected: entity.nonce,
            got: tx.nonce,
        });
    }

    // Validate balance
    let fee_u128 = u128::from(tx.fee);
    if entity.economic_balance < fee_u128 {
        return Err(ExecError::InsufficientFunds {
            balance: entity.economic_balance,
            needed: fee_u128,
        });
    }

    // Load memory object — keyed by canonical entity.id
    let mut memory_object = read_memory_object(db, &entity.id, &payload.object_id)?
        .ok_or(ExecError::MemoryObjectNotFound)?;

    // Validate ownership against canonical entity.id
    if memory_object.owner_entity != entity.id {
        return Err(ExecError::MemoryObjectOwnerMismatch);
    }

    // Feature 8: DelegationGrant memory objects are immutable. Mutation
    // could quietly add capabilities a delegator no longer holds, change
    // the delegate, or extend expiry past the original audit trail.
    // Force delete-and-recreate instead.
    if memory_object.object_type == MemoryObjectType::DelegationGrant {
        return Err(ExecError::DelegationGrantNotUpdatable);
    }

    // Per-type structural validation on the NEW payload bytes.
    // CompositionGraph rejects self-dependencies even when introduced via
    // update, not just at create time.
    validate_composition_graph_payload(memory_object.object_type, &payload.new_data, &entity.id)?;

    // Update memory object
    memory_object.data = payload.new_data;
    memory_object.updated_at = current_height;
    let encoded = encode_memory_object_v1(&memory_object);

    // Update entity state
    entity.economic_balance = entity
        .economic_balance
        .checked_sub(fee_u128)
        .ok_or(ExecError::Overflow)?;
    entity.nonce = entity
        .nonce
        .checked_add(1)
        .ok_or(ExecError::NonceOverflow)?;
    entity.last_active_at = current_height;

    // Build atomic batch
    let mut ops = Vec::new();

    let obj_key = ai_memory_object_key(&entity.id, &payload.object_id);
    ops.push(WriteOp::Put(obj_key, encoded));

    ops.push(write_ai_entity_op(&entity));

    // Apply atomically
    db.apply_batch(&ops).map_err(ExecError::Db)?;

    Ok(())
}

/// Apply a `DeleteMemoryObject` transaction.
///
/// # Validation (D21.4)
/// 1. Entity must exist and match tx.from
/// 2. Memory object must exist and be owned by entity
/// 3. Nonce must be correct
/// 4. Sufficient balance for fee
///
/// # State Changes
/// - Delete memory object
/// - Delete type index
/// - Decrement memory count
/// - Deduct fee, increment nonce, update `last_active_at`
///
/// # Errors
/// Returns error if validation fails or DB error occurs.
pub fn apply_delete_memory_object_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<(), ExecError<K::Error>> {
    let entity = lookup_ai_entity_by_address(db, &tx.from)?.ok_or(ExecError::IssuerNotFound)?;
    apply_delete_memory_object_tx_inner(db, tx, entity, current_height)
}

/// Inner delete-memory-object implementation taking a pre-resolved AI entity.
fn apply_delete_memory_object_tx_inner<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    mut entity: AiEntity,
    current_height: u64,
) -> Result<(), ExecError<K::Error>> {
    // Kill switch: block all AI entity operations when active
    if read_ai_kill_switch(db)? {
        return Err(ExecError::AiKillSwitchActive);
    }

    // Decode payload
    let payload = decode_delete_memory_object_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        _ => ExecError::Overflow,
    })?;

    // W5-06: Reject operations from deactivated entities
    if !entity.is_active {
        return Err(ExecError::EntityNotActive);
    }

    // Validate read_memory_objects capability (static or via active delegation grant)
    requires_capability(db, &entity, current_height, |c| c.read_memory_objects)?;

    // Validate nonce
    if tx.nonce != entity.nonce {
        return Err(ExecError::NonceMismatch {
            expected: entity.nonce,
            got: tx.nonce,
        });
    }

    // Validate balance
    let fee_u128 = u128::from(tx.fee);
    if entity.economic_balance < fee_u128 {
        return Err(ExecError::InsufficientFunds {
            balance: entity.economic_balance,
            needed: fee_u128,
        });
    }

    // Load memory object — keyed by canonical entity.id
    let memory_object = read_memory_object(db, &entity.id, &payload.object_id)?
        .ok_or(ExecError::MemoryObjectNotFound)?;

    // Validate ownership against canonical entity.id
    if memory_object.owner_entity != entity.id {
        return Err(ExecError::MemoryObjectOwnerMismatch);
    }

    // Get current count (keyed by entity.id)
    let current_count = read_memory_count(db, &entity.id)?;

    // Update entity state
    entity.economic_balance = entity
        .economic_balance
        .checked_sub(fee_u128)
        .ok_or(ExecError::Overflow)?;
    entity.nonce = entity
        .nonce
        .checked_add(1)
        .ok_or(ExecError::NonceOverflow)?;
    entity.last_active_at = current_height;

    // Build atomic batch — all storage keys use canonical entity.id
    let mut ops = Vec::new();

    let obj_key = ai_memory_object_key(&entity.id, &payload.object_id);
    ops.push(WriteOp::Delete(obj_key));

    let type_key = ai_memory_by_type_key(
        memory_object.object_type.to_byte(),
        &entity.id,
        &payload.object_id,
    );
    ops.push(WriteOp::Delete(type_key));

    // Feature 8: tear down the by-delegate secondary index when a
    // DelegationGrant is deleted. The grant payload is decoded from the
    // primary record (loaded above) to recover the delegate id; a stale
    // payload causes the index Delete to be skipped silently rather than
    // failing the whole tx, mirroring the resolver's tolerance policy.
    if memory_object.object_type == MemoryObjectType::DelegationGrant {
        if let Some(grant) = DelegationGrantData::decode(&memory_object.data) {
            let delegate_index_key =
                ai_delegation_by_delegate_key(&grant.delegate_entity_id, &payload.object_id);
            ops.push(WriteOp::Delete(delegate_index_key));
        }
    }

    // Update count (decrement, but don't go below 0)
    let count_key = ai_memory_count_key(&entity.id);
    ops.push(WriteOp::Put(
        count_key,
        encode_memory_count(current_count.saturating_sub(1)).to_vec(),
    ));

    ops.push(write_ai_entity_op(&entity));

    // Apply atomically
    db.apply_batch(&ops).map_err(ExecError::Db)?;

    Ok(())
}

/// Query all memory objects for an entity.
///
/// Returns all memory objects owned by the entity.
///
/// # Errors
/// Returns error if DB read fails or stored data is malformed.
pub fn get_memory_objects_by_entity<K: Kv>(
    db: &K,
    entity_id: &[u8; 32],
) -> Result<Vec<MemoryObject>, ExecError<K::Error>> {
    // Build prefix: "ai/memory_objects/" ++ entity_id ++ "/"
    let mut prefix = Vec::with_capacity(KEY_PREFIX_AI_MEMORY_OBJECTS.len() + 32 + 1);
    prefix.extend_from_slice(KEY_PREFIX_AI_MEMORY_OBJECTS);
    prefix.extend_from_slice(entity_id);
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::with_capacity(entries.len());
    for (_key, value) in entries {
        let obj = decode_memory_object_v1(&value)
            .map_err(|e| ExecError::CodecDecode(format!("{e:?}")))?;
        results.push(obj);
    }

    Ok(results)
}

/// Query memory objects of a specific type owned by an entity.
///
/// Walks the `ai/memory_by_type/{type}/{entity_id}/` index, extracts the
/// trailing 32-byte object id from each indexed key, and reads the object
/// record via `read_memory_object`. Stale index entries (object record
/// missing) are silently skipped so a partial index does not break callers.
///
/// # Errors
/// Returns error if DB read fails, stored data is malformed, or an
/// indexed key has fewer than 32 trailing bytes (corruption).
pub fn get_memory_objects_by_entity_and_type<K: Kv>(
    db: &K,
    entity_id: &[u8; 32],
    object_type: u8,
) -> Result<Vec<MemoryObject>, ExecError<K::Error>> {
    // Build prefix: "ai/memory_by_type/" ++ type_byte ++ "/" ++ entity_id ++ "/"
    let mut prefix = Vec::with_capacity(KEY_PREFIX_AI_MEMORY_BY_TYPE.len() + 1 + 1 + 32 + 1);
    prefix.extend_from_slice(KEY_PREFIX_AI_MEMORY_BY_TYPE);
    prefix.push(object_type);
    prefix.push(b'/');
    prefix.extend_from_slice(entity_id);
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::with_capacity(entries.len());
    for (key, _value) in entries {
        if key.len() < 32 {
            return Err(ExecError::CodecDecode(format!(
                "memory_by_type key too short: {} bytes",
                key.len()
            )));
        }
        let mut object_id = [0u8; 32];
        object_id.copy_from_slice(&key[key.len() - 32..]);
        if let Some(obj) = read_memory_object(db, entity_id, &object_id)? {
            results.push(obj);
        }
    }

    Ok(results)
}

// ============================================================================
// SIGNAL QUERY FUNCTIONS (Week 14 - D14.5)
// ============================================================================

use novai_codec::decode_signal_commitment_v1;
use novai_state::{
    KEY_PREFIX_AI_SIGNALS, KEY_PREFIX_AI_SIGNALS_BY_ISSUER, KEY_PREFIX_AI_SIGNALS_BY_TYPE,
};

/// Query all signal commitments at a specific height (D14.5).
///
/// Returns all signals stored at the given height, ordered by issuer.
///
/// # Errors
/// Returns error if DB read fails or stored data is malformed.
pub fn get_signals_by_height<K: Kv>(
    db: &K,
    height: u64,
) -> Result<Vec<SignalCommitment>, ExecError<K::Error>> {
    // Build prefix: "ai/signals/" ++ height_be8 ++ "/"
    let mut prefix = Vec::with_capacity(KEY_PREFIX_AI_SIGNALS.len() + 8 + 1);
    prefix.extend_from_slice(KEY_PREFIX_AI_SIGNALS);
    prefix.extend_from_slice(&height.to_be_bytes());
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::with_capacity(entries.len());
    for (_key, value) in entries {
        let commitment = decode_signal_commitment_v1(&value)
            .map_err(|e| ExecError::CodecDecode(format!("{e:?}")))?;
        results.push(commitment);
    }

    Ok(results)
}

/// Query signal commitments by issuer in height range [`start_height`, `end_height`] (D14.5).
///
/// Returns all signals from the given issuer within the height range.
///
/// # Errors
/// Returns error if DB read fails or stored data is malformed.
pub fn get_signals_by_issuer<K: Kv>(
    db: &K,
    issuer: &[u8; 32],
    start_height: u64,
    end_height: u64,
) -> Result<Vec<SignalCommitment>, ExecError<K::Error>> {
    // Build prefix: "ai/signals/by_issuer/" ++ issuer32 ++ "/"
    let mut prefix = Vec::with_capacity(KEY_PREFIX_AI_SIGNALS_BY_ISSUER.len() + 32 + 1);
    prefix.extend_from_slice(KEY_PREFIX_AI_SIGNALS_BY_ISSUER);
    prefix.extend_from_slice(issuer);
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::new();
    for (_key, value) in entries {
        let commitment = decode_signal_commitment_v1(&value)
            .map_err(|e| ExecError::CodecDecode(format!("{e:?}")))?;
        // Filter by height range
        if commitment.height >= start_height && commitment.height <= end_height {
            results.push(commitment);
        }
    }

    Ok(results)
}

/// Query signal commitments by type in height range [`start_height`, `end_height`] (D14.5).
///
/// Returns all signals of the given type within the height range.
///
/// # Errors
/// Returns error if DB read fails or stored data is malformed.
pub fn get_signals_by_type<K: Kv>(
    db: &K,
    signal_type: novai_ai_entities::AiSignalType,
    start_height: u64,
    end_height: u64,
) -> Result<Vec<SignalCommitment>, ExecError<K::Error>> {
    // Build prefix: "ai/signals/by_type/" ++ type_u8 ++ "/"
    let mut prefix = Vec::with_capacity(KEY_PREFIX_AI_SIGNALS_BY_TYPE.len() + 1 + 1);
    prefix.extend_from_slice(KEY_PREFIX_AI_SIGNALS_BY_TYPE);
    prefix.push(signal_type.to_byte());
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::new();
    for (_key, value) in entries {
        let commitment = decode_signal_commitment_v1(&value)
            .map_err(|e| ExecError::CodecDecode(format!("{e:?}")))?;
        // Filter by height range
        if commitment.height >= start_height && commitment.height <= end_height {
            results.push(commitment);
        }
    }

    Ok(results)
}

// ============================================================================
// NNPX PRIVACY BOUNDARY ENFORCEMENT (Week 22 - D22.4)
// ============================================================================

use novai_state::{is_nnpx_key, nnpx_nullifier_key};

/// Caller context for privacy boundary checks.
///
/// Used to determine if an operation is initiated by an AI entity
/// (which is blocked from NNPX access) or by a human-controlled account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// Human-controlled account (address).
    Account([u8; 32]),
    /// AI entity (entity ID).
    AiEntity([u8; 32]),
}

/// Validate that the caller has permission to access a key.
///
/// # Privacy Boundary (D22.4)
///
/// AI entities are NEVER allowed to directly access NNPX private data.
/// This is a hard security boundary enforced at the execution layer.
///
/// # Errors
///
/// Returns `NnpxAccessDenied` if:
/// - Key starts with `b"nnpx/"` AND caller is an AI entity
#[inline]
pub fn validate_nnpx_access<E>(key: &[u8], caller: &Caller) -> Result<(), ExecError<E>> {
    if is_nnpx_key(key) {
        if let Caller::AiEntity(_) = caller {
            return Err(ExecError::NnpxAccessDenied);
        }
    }
    Ok(())
}

/// Check if a nullifier has already been spent.
///
/// # Errors
///
/// Returns `NullifierAlreadySpent` if the nullifier exists in state.
pub fn validate_nullifier_unspent<K: Kv>(
    db: &K,
    nullifier: &[u8; 32],
) -> Result<(), ExecError<K::Error>> {
    let key = nnpx_nullifier_key(nullifier);
    if db.get(&key).map_err(ExecError::Db)?.is_some() {
        return Err(ExecError::NullifierAlreadySpent);
    }
    Ok(())
}

/// Mark a nullifier as spent by writing it to the nullifier set.
///
/// Returns a `WriteOp` for inclusion in an atomic batch.
#[must_use]
pub fn mark_nullifier_spent(nullifier: &[u8; 32]) -> WriteOp {
    let key = nnpx_nullifier_key(nullifier);
    // Value is empty - presence in the set is what matters
    WriteOp::Put(key, vec![])
}

/// Validate that an AI entity does NOT have NNPX-derived capability.
///
/// # Privacy Invariant
///
/// AI entities must NEVER be registered with `read_nnpx_derived: true`.
/// This function is used during entity registration to enforce this invariant.
///
/// # Errors
///
/// Returns `NnpxAccessDenied` if the entity has `read_nnpx_derived` capability.
#[inline]
#[allow(clippy::missing_const_for_fn)] // Generic functions cannot be const in stable Rust
pub fn validate_ai_entity_no_nnpx_capability<E>(
    capabilities: &novai_ai_entities::Capabilities,
) -> Result<(), ExecError<E>> {
    if capabilities.read_nnpx_derived {
        return Err(ExecError::NnpxAccessDenied);
    }
    Ok(())
}

/// Check if a key is in the NNPX private store.
///
/// Convenience re-export from `novai_state` for use in execution logic.
#[inline]
#[must_use]
pub fn is_private_key(key: &[u8]) -> bool {
    is_nnpx_key(key)
}

// ============================================================================
// DERIVED VIEW ACCESS CONTROL (Week 23 - D23.5)
// ============================================================================

use novai_ai_entities::{
    decode_derived_view_v1, encode_derived_view_v1, DerivedView, DerivedViewDecodeError,
};
use novai_state::{
    derived_view_audit_key, derived_view_by_creator_key, derived_view_by_schema_key,
    derived_view_key, is_derived_view_key, KEY_PREFIX_DERIVED_VIEWS,
};

/// Validate that an AI entity can read derived views.
///
/// # Access Control (D23.5)
///
/// AI entities can ONLY read derived views if they have the `read_nnpx_derived` capability.
/// This capability grants access to privacy-safe aggregates without exposing raw private data.
///
/// # Arguments
///
/// - `entity`: The AI entity attempting to read
///
/// # Errors
///
/// Returns `DerivedViewAccessDenied` if the entity lacks the `read_nnpx_derived` capability.
#[inline]
#[allow(clippy::missing_const_for_fn)] // Generic functions cannot be const in stable Rust
pub fn validate_derived_view_access<E>(
    entity: &novai_ai_entities::AiEntity,
) -> Result<(), ExecError<E>> {
    if !entity.capabilities.read_nnpx_derived {
        return Err(ExecError::DerivedViewAccessDenied);
    }
    Ok(())
}

/// Read a derived view from storage with access control.
///
/// # Access Control (D23.5)
///
/// 1. Validates the AI entity has `read_nnpx_derived` capability
/// 2. Reads the derived view from storage
/// 3. Creates an audit log entry (returned as `WriteOp`)
///
/// # Arguments
///
/// - `db`: Database handle
/// - `entity`: AI entity performing the read
/// - `view_id`: ID of the derived view to read
/// - `current_height`: Current block height (for audit log)
///
/// # Returns
///
/// On success, returns `(DerivedView, WriteOp)` where `WriteOp` is the audit log entry.
///
/// # Errors
///
/// - `DerivedViewAccessDenied`: Entity lacks capability
/// - `DerivedViewNotFound`: View doesn't exist
/// - `Db`: Database error
pub fn read_derived_view_with_audit<K: Kv>(
    db: &K,
    entity: &novai_ai_entities::AiEntity,
    view_id: &[u8; 32],
    current_height: u64,
) -> Result<(DerivedView, WriteOp), ExecError<K::Error>> {
    // Step 1: Validate capability
    validate_derived_view_access(entity)?;

    // Step 2: Read derived view
    let key = derived_view_key(view_id);
    let bytes = db
        .get(&key)
        .map_err(ExecError::Db)?
        .ok_or(ExecError::DerivedViewNotFound)?;

    let view = decode_derived_view_v1(&bytes).map_err(|e| match e {
        DerivedViewDecodeError::InvalidSchemaId { id } => {
            ExecError::InvalidDerivedViewSchema { schema_id: id }
        }
        _ => ExecError::Overflow, // Map other decode errors
    })?;

    // Step 3: Create audit log entry
    let audit_op = create_derived_view_audit_entry(&entity.id, view_id, current_height);

    Ok((view, audit_op))
}

/// Read a derived view without access control (for internal use).
///
/// Use this only for protocol-level operations that don't require capability checks.
///
/// # Errors
///
/// - `DerivedViewNotFound`: View doesn't exist
/// - `Db`: Database error
pub fn read_derived_view<K: Kv>(
    db: &K,
    view_id: &[u8; 32],
) -> Result<Option<DerivedView>, ExecError<K::Error>> {
    let key = derived_view_key(view_id);
    match db.get(&key).map_err(ExecError::Db)? {
        None => Ok(None),
        Some(bytes) => {
            let view = decode_derived_view_v1(&bytes).map_err(|e| match e {
                DerivedViewDecodeError::InvalidSchemaId { id } => {
                    ExecError::InvalidDerivedViewSchema { schema_id: id }
                }
                _ => ExecError::Overflow,
            })?;
            Ok(Some(view))
        }
    }
}

/// Create a `WriteOp` to store a derived view.
///
/// Also creates index entries for schema and creator lookups.
///
/// # Returns
///
/// Vector of `WriteOps` for atomic batch:
/// 1. Primary view storage
/// 2. Schema index entry
/// 3. Creator index entry
#[must_use]
pub fn write_derived_view_ops(view: &DerivedView) -> Vec<WriteOp> {
    let mut ops = Vec::with_capacity(3);

    // Primary storage
    let primary_key = derived_view_key(&view.view_id);
    let encoded = encode_derived_view_v1(view);
    ops.push(WriteOp::Put(primary_key, encoded));

    // Schema index
    let schema_key = derived_view_by_schema_key(view.schema_id, &view.view_id);
    ops.push(WriteOp::Put(schema_key, vec![])); // Presence-only index

    // Creator index
    let creator_key = derived_view_by_creator_key(&view.creator, &view.view_id);
    ops.push(WriteOp::Put(creator_key, vec![])); // Presence-only index

    ops
}

/// Create an audit log entry for a derived view read.
///
/// # Audit Log Format (D23.5)
///
/// Key: `derived_views/audit/{entity_id}/{height}`
/// Value: `{view_id}` (32 bytes)
///
/// This records that the given AI entity read a derived view at the given height.
#[must_use]
pub fn create_derived_view_audit_entry(
    entity_id: &[u8; 32],
    view_id: &[u8; 32],
    height: u64,
) -> WriteOp {
    let key = derived_view_audit_key(entity_id, height);
    WriteOp::Put(key, view_id.to_vec())
}

/// Query derived views by schema ID.
///
/// Returns all derived views with the given schema.
///
/// # Errors
///
/// Returns error if DB read fails or stored data is malformed.
pub fn get_derived_views_by_schema<K: Kv>(
    db: &K,
    schema_id: u32,
) -> Result<Vec<DerivedView>, ExecError<K::Error>> {
    // Build prefix: "derived_views/by_schema/" ++ schema_id_be4 ++ "/"
    let mut prefix =
        Vec::with_capacity(KEY_PREFIX_DERIVED_VIEWS.len() + "by_schema/".len() + 4 + 1);
    prefix.extend_from_slice(b"derived_views/by_schema/");
    prefix.extend_from_slice(&schema_id.to_be_bytes());
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::with_capacity(entries.len());
    for (key, _value) in entries {
        // Extract view_id from key (last 32 bytes)
        if key.len() >= 32 {
            let mut view_id = [0u8; 32];
            view_id.copy_from_slice(&key[key.len() - 32..]);

            // Read the actual view
            if let Some(view) = read_derived_view(db, &view_id)? {
                results.push(view);
            }
        }
    }

    Ok(results)
}

/// Query derived views by creator.
///
/// Returns all derived views created by the given address/entity.
///
/// # Errors
///
/// Returns error if DB read fails or stored data is malformed.
pub fn get_derived_views_by_creator<K: Kv>(
    db: &K,
    creator: &[u8; 32],
) -> Result<Vec<DerivedView>, ExecError<K::Error>> {
    // Build prefix: "derived_views/by_creator/" ++ creator32 ++ "/"
    let mut prefix =
        Vec::with_capacity(KEY_PREFIX_DERIVED_VIEWS.len() + "by_creator/".len() + 32 + 1);
    prefix.extend_from_slice(b"derived_views/by_creator/");
    prefix.extend_from_slice(creator);
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::with_capacity(entries.len());
    for (key, _value) in entries {
        // Extract view_id from key (last 32 bytes)
        if key.len() >= 32 {
            let mut view_id = [0u8; 32];
            view_id.copy_from_slice(&key[key.len() - 32..]);

            // Read the actual view
            if let Some(view) = read_derived_view(db, &view_id)? {
                results.push(view);
            }
        }
    }

    Ok(results)
}

/// Check if a key is a derived view key.
///
/// Convenience re-export for use in execution logic.
#[inline]
#[must_use]
pub fn is_derived_view(key: &[u8]) -> bool {
    is_derived_view_key(key)
}

// ============================================================================
// FEE SCHEDULE (Tiered Minimum Fee Enforcement)
// ============================================================================

/// M-06: Minimum balance for new account creation via transfer.
/// Prevents state spam with dust accounts. Existing accounts are not affected.
pub const MIN_ACCOUNT_BALANCE: u128 = 1_000;

/// Minimum fee for a base transfer transaction.
pub const MIN_FEE_TRANSFER: u64 = 100;

/// Minimum fee for a signal commitment transaction (10x base).
pub const MIN_FEE_SIGNAL_COMMITMENT: u64 = 1_000;

/// Minimum fee for memory object operations (5x base).
pub const MIN_FEE_MEMORY_OBJECT: u64 = 500;

/// Minimum fee for submitting a governance proposal (20x base).
pub const MIN_FEE_GOVERNANCE_SUBMIT: u64 = 2_000;

/// Minimum fee for executing a governance proposal (5x base).
pub const MIN_FEE_GOVERNANCE_EXECUTE: u64 = 500;

/// Minimum fee for registering an AI entity (50x base).
pub const MIN_FEE_REGISTER_AI_ENTITY: u64 = 5_000;

/// Minimum fee for crediting an AI entity balance (same as base).
pub const MIN_FEE_CREDIT_AI_ENTITY: u64 = 100;

/// Minimum fee for registering an AI entity with key (same as register, 50x base).
pub const MIN_FEE_REGISTER_AI_ENTITY_WITH_KEY: u64 = 5_000;

/// Tiered fee schedule with minimum fees per transaction type.
///
/// All values are floor minimums — senders can pay MORE but never less.
/// The fee hierarchy reflects operational cost and spam resistance needs:
/// - Base transfers are cheapest
/// - AI operations cost more (signals, memory, registration)
/// - Governance proposals are expensive to deter spam
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeSchedule {
    pub transfer: u64,
    pub signal_commitment: u64,
    pub memory_object: u64,
    pub governance_submit: u64,
    pub governance_execute: u64,
    pub register_ai_entity: u64,
    pub credit_ai_entity: u64,
}

impl FeeSchedule {
    /// Returns the default fee schedule with production values.
    #[must_use]
    pub const fn default() -> Self {
        Self {
            transfer: MIN_FEE_TRANSFER,
            signal_commitment: MIN_FEE_SIGNAL_COMMITMENT,
            memory_object: MIN_FEE_MEMORY_OBJECT,
            governance_submit: MIN_FEE_GOVERNANCE_SUBMIT,
            governance_execute: MIN_FEE_GOVERNANCE_EXECUTE,
            register_ai_entity: MIN_FEE_REGISTER_AI_ENTITY,
            credit_ai_entity: MIN_FEE_CREDIT_AI_ENTITY,
        }
    }
}

/// Returns the minimum fee required for a transaction based on its payload version byte.
///
/// # Errors
///
/// Returns `UnknownPayloadVersion` if the payload is empty or unrecognized.
pub fn minimum_fee_for_tx(tx: &TxV1) -> Result<u64, ExecError<()>> {
    let version = tx
        .payload
        .first()
        .copied()
        .ok_or(ExecError::UnknownPayloadVersion { version: 0 })?;

    match version {
        TRANSFER_PAYLOAD_V1 => Ok(MIN_FEE_TRANSFER),
        SIGNAL_COMMITMENT_PAYLOAD_V1 => Ok(MIN_FEE_SIGNAL_COMMITMENT),
        CREATE_MEMORY_OBJECT_PAYLOAD_V1
        | UPDATE_MEMORY_OBJECT_PAYLOAD_V1
        | DELETE_MEMORY_OBJECT_PAYLOAD_V1 => Ok(MIN_FEE_MEMORY_OBJECT),
        SUBMIT_PROPOSAL_PAYLOAD_V1 => Ok(MIN_FEE_GOVERNANCE_SUBMIT),
        EXECUTE_PROPOSAL_PAYLOAD_V1 => Ok(MIN_FEE_GOVERNANCE_EXECUTE),
        REGISTER_AI_ENTITY_PAYLOAD_V1 => Ok(MIN_FEE_REGISTER_AI_ENTITY),
        CREDIT_AI_ENTITY_PAYLOAD_V1 => Ok(MIN_FEE_CREDIT_AI_ENTITY),
        REGISTER_AI_ENTITY_WITH_KEY_PAYLOAD_V1 => Ok(MIN_FEE_REGISTER_AI_ENTITY_WITH_KEY),
        other => Err(ExecError::UnknownPayloadVersion { version: other }),
    }
}

// ============================================================================
// TREASURY STATE KEYS (Fee Distribution)
// ============================================================================

/// Canonical key for the AI treasury balance record.
///
/// Receives the non-base portion of fees from AI-related transactions
/// (signal commitments, memory objects, entity registration).
pub const KEY_AI_TREASURY: &[u8] = b"treasury/ai";

/// Canonical key for the NNPX privacy treasury balance record.
///
/// Reserved for future use — will receive fees from NNPX privacy transactions.
pub const KEY_PRIVACY_TREASURY: &[u8] = b"treasury/privacy";

/// Canonical key for the marketplace treasury balance record.
///
/// Receives the protocol-fee portion (`MARKETPLACE_FEE_BPS` basis points)
/// of every signal purchase. Reuses the `FeePoolV1` codec used by the
/// other treasury keys.
pub const KEY_MARKETPLACE_TREASURY: &[u8] = b"treasury/marketplace";

/// Storage key for the slash treasury (accumulates funds slashed from
/// misbehaving entities' `stake_balance` via `StakeSlash` signals).
pub const KEY_SLASH_TREASURY: &[u8] = b"treasury/slash";

/// Marketplace protocol fee, in basis points (1 bp = 0.01%).
/// 200 bps = 2% on every signal purchase.
pub const MARKETPLACE_FEE_BPS: u128 = 200;

/// Subscription early-cancellation fee, in basis points (Feature 9).
///
/// 500 bps = 5% of the unaccrued (refundable) portion of a subscription.
/// Paid 100% to the producer as compensation for early termination; no
/// marketplace cut is taken from this fee.
pub const SUBSCRIPTION_CANCEL_FEE_BPS: u128 = 500;

/// Minimum allowed `duration_blocks` for a `SubscriptionCreate` signal
/// (Feature 9). Subscriptions below this floor are rejected.
pub const MIN_SUBSCRIPTION_DURATION: u64 = 100;

/// Basis-points denominator (`10_000` = 100%).
pub const BPS_DENOMINATOR: u128 = 10_000;

/// Distribute a fee between the validator fee pool and the AI treasury.
///
/// For AI-related transactions (signal commitment, memory objects, entity registration):
/// - The base portion (`MIN_FEE_TRANSFER`) goes to the validator fee pool
/// - The remainder goes to the AI treasury
///
/// For base transfers and governance: all goes to the fee pool.
///
/// # Arguments
///
/// - `db`: Database handle
/// - `tx`: Transaction (payload[0] determines fee category)
/// - `fee`: Total fee amount (as u128)
///
/// # Returns
///
/// `WriteOp`s for the fee pool and (optionally) AI treasury updates.
///
/// # Errors
///
/// Returns error if DB read fails or arithmetic overflows.
pub fn distribute_fee<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    fee: u128,
) -> Result<Vec<WriteOp>, ExecError<K::Error>> {
    let version = tx.payload.first().copied().unwrap_or(0);

    let is_ai_tx = matches!(
        version,
        SIGNAL_COMMITMENT_PAYLOAD_V1
            | CREATE_MEMORY_OBJECT_PAYLOAD_V1
            | UPDATE_MEMORY_OBJECT_PAYLOAD_V1
            | DELETE_MEMORY_OBJECT_PAYLOAD_V1
            | REGISTER_AI_ENTITY_PAYLOAD_V1
            | REGISTER_AI_ENTITY_WITH_KEY_PAYLOAD_V1
    );

    let mut ops = Vec::with_capacity(2);

    if is_ai_tx && fee > u128::from(MIN_FEE_TRANSFER) {
        // Split: base portion to fee pool, remainder to AI treasury
        let base_portion = u128::from(MIN_FEE_TRANSFER);
        let ai_portion = fee.checked_sub(base_portion).ok_or(ExecError::Overflow)?;

        // Fee pool gets base portion
        let mut fee_pool = read_fee_pool_or_default(db)?;
        fee_pool.balance = fee_pool
            .balance
            .checked_add(base_portion)
            .ok_or(ExecError::Overflow)?;
        ops.push(WriteOp::Put(
            KEY_FEE_POOL.to_vec(),
            encode_fee_pool_v1(&fee_pool).to_vec(),
        ));

        // AI treasury gets remainder
        let mut ai_treasury_balance = read_treasury_balance(db, KEY_AI_TREASURY)?;
        ai_treasury_balance = ai_treasury_balance
            .checked_add(ai_portion)
            .ok_or(ExecError::Overflow)?;
        ops.push(WriteOp::Put(
            KEY_AI_TREASURY.to_vec(),
            encode_fee_pool_v1(&FeePoolV1 {
                balance: ai_treasury_balance,
            })
            .to_vec(),
        ));
    } else {
        // All to fee pool
        let mut fee_pool = read_fee_pool_or_default(db)?;
        fee_pool.balance = fee_pool
            .balance
            .checked_add(fee)
            .ok_or(ExecError::Overflow)?;
        ops.push(WriteOp::Put(
            KEY_FEE_POOL.to_vec(),
            encode_fee_pool_v1(&fee_pool).to_vec(),
        ));
    }

    Ok(ops)
}

/// Read treasury balance from a given key.
///
/// Treasury records reuse the `FeePoolV1` encoding (version byte + u128 big-endian).
fn read_treasury_balance<K: Kv>(db: &K, key: &[u8]) -> Result<u128, ExecError<K::Error>> {
    match db.get(key).map_err(ExecError::Db)? {
        None => Ok(0),
        Some(bytes) => Ok(decode_fee_pool_v1(&bytes)?.balance),
    }
}

// ============================================================================
// TRANSACTION DISPATCH (routes TxV1 by payload version byte)
// ============================================================================

/// H-07: Purge expired/executed governance proposals from the database.
///
/// Scans all proposals and deletes those that are:
/// - Executed and finalized (`current_height > executed_at + finality_window`)
/// - Expired and finalized (`current_height > expires_at + finality_window`)
///
/// Returns the number of proposals purged.
pub fn purge_expired_proposals<K: KvBatch>(
    db: &mut K,
    current_height: u64,
    finality_window: u64,
) -> usize {
    let Ok(proposals) = db.scan_prefix(novai_state::KEY_PREFIX_GOVERNANCE_PROPOSALS) else {
        return 0;
    };

    let mut purged = 0;
    for (key, value) in &proposals {
        // Decode proposal to check state
        let Ok(proposal) = novai_governance::codec::decode_proposal_v1(value) else {
            continue; // Skip malformed proposals
        };

        let should_purge = match proposal.state {
            novai_governance::ProposalState::Executed => {
                proposal.executed_at > 0 && current_height > proposal.executed_at + finality_window
            }
            _ => {
                proposal.is_expired(current_height)
                    && current_height > proposal.expires_at + finality_window
            }
        };

        if should_purge {
            // Delete proposal record
            if db.delete(key).is_ok() {
                purged += 1;
            }
            // Delete by-state index entries (best-effort)
            let state_key = novai_state::governance_proposal_by_state_key(
                proposal.state.to_byte(),
                &proposal.id,
            );
            let _ = db.delete(&state_key);
        }
    }

    purged
}

/// Dispatch a `TxV1` to the correct apply function based on its payload version byte.
///
/// The first byte of `tx.payload` identifies the transaction type:
///
/// | Byte | Type                       | Apply function                        |
/// |------|----------------------------|---------------------------------------|
/// |  1   | Transfer                   | `apply_tx_v1_transfer_inner`          |
/// |  2   | Signal Commitment          | `apply_signal_commitment_tx_inner`    |
/// |  3   | Create Memory Object       | `apply_create_memory_object_tx_inner` |
/// |  4   | Update Memory Object       | `apply_update_memory_object_tx_inner` |
/// |  5   | Delete Memory Object       | `apply_delete_memory_object_tx_inner` |
/// |  6   | Submit Governance Proposal | `apply_governance_submit_tx`          |
/// |  7   | Execute Governance Proposal| `apply_governance_execute_tx`         |
/// |  8   | Register AI Entity         | `apply_register_ai_entity_tx`         |
/// |  9   | Credit AI Entity           | `apply_credit_ai_entity_tx`           |
/// | 10   | Register AI Entity w/ Key  | `apply_register_ai_entity_with_key_tx`|
///
/// For tx types 2–5 (entity-signed), the dispatcher requires `tx.from` to
/// resolve to a registered AI entity via the address→id reverse index. The
/// resolved entity is passed into the inner handler so storage keys can use
/// the canonical `entity.id`.
///
/// # Errors
///
/// Returns `UnknownPayloadVersion` if the payload is empty or the version byte
/// does not match any known transaction type. Returns `IssuerNotFound` if a
/// signal or memory tx's sender is not a registered AI entity. All other
/// errors are forwarded from the underlying apply function.
pub fn dispatch_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<(), ExecError<K::Error>> {
    // Enforce minimum fee before routing to any apply function.
    let min_fee = minimum_fee_for_tx(tx).map_err(|e| match e {
        ExecError::UnknownPayloadVersion { version } => {
            ExecError::UnknownPayloadVersion { version }
        }
        _ => ExecError::Overflow,
    })?;
    if tx.fee < min_fee {
        return Err(ExecError::FeeBelowMinimum {
            minimum: min_fee,
            provided: tx.fee,
        });
    }

    // Check AI entity sender restrictions before routing.
    // If sender is an AI entity, verify it is allowed to submit this tx type.
    // The returned entity is passed to apply functions that need it, avoiding
    // a redundant lookup_ai_entity_by_address call. Capability resolution is
    // delegation-aware: an entity may pass via a static capability or via an
    // active delegation grant naming it as the delegate.
    let ai_entity = check_ai_entity_sender(db, tx, current_height)?;

    let version = tx
        .payload
        .first()
        .copied()
        .ok_or(ExecError::UnknownPayloadVersion { version: 0 })?;

    match version {
        TRANSFER_PAYLOAD_V1 => apply_tx_v1_transfer_inner(db, tx, ai_entity),
        SIGNAL_COMMITMENT_PAYLOAD_V1 => {
            let entity = ai_entity.ok_or(ExecError::IssuerNotFound)?;
            apply_signal_commitment_tx_inner(db, tx, entity, current_height)
        }
        CREATE_MEMORY_OBJECT_PAYLOAD_V1 => {
            let entity = ai_entity.ok_or(ExecError::IssuerNotFound)?;
            apply_create_memory_object_tx_inner(db, tx, entity, current_height).map(|_| ())
        }
        UPDATE_MEMORY_OBJECT_PAYLOAD_V1 => {
            let entity = ai_entity.ok_or(ExecError::IssuerNotFound)?;
            apply_update_memory_object_tx_inner(db, tx, entity, current_height)
        }
        DELETE_MEMORY_OBJECT_PAYLOAD_V1 => {
            let entity = ai_entity.ok_or(ExecError::IssuerNotFound)?;
            apply_delete_memory_object_tx_inner(db, tx, entity, current_height)
        }
        SUBMIT_PROPOSAL_PAYLOAD_V1 => {
            apply_governance_submit_tx(db, tx, current_height).map(|_| ())
        }
        EXECUTE_PROPOSAL_PAYLOAD_V1 => apply_governance_execute_tx(db, tx, current_height),
        REGISTER_AI_ENTITY_PAYLOAD_V1 => {
            apply_register_ai_entity_tx(db, tx, current_height).map(|_| ())
        }
        CREDIT_AI_ENTITY_PAYLOAD_V1 => apply_credit_ai_entity_tx(db, tx, current_height),
        REGISTER_AI_ENTITY_WITH_KEY_PAYLOAD_V1 => {
            apply_register_ai_entity_with_key_tx(db, tx, current_height).map(|_| ())
        }
        other => Err(ExecError::UnknownPayloadVersion { version: other }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use novai_ai_entities::{AutonomyMode, Capabilities};
    use novai_state::{ai_signal_key, KvBatch, MemKv};

    #[test]
    fn ai_entity_write_read_roundtrip() {
        let mut db = MemKv::new();

        // Create an AI entity
        let entity = AiEntity::new(
            [0x42u8; 32], // code_hash
            [0x01u8; 32], // creator
            AutonomyMode::Gated,
            Capabilities::gated(),
            1000, // registered_at
        );

        // Write entity to DB
        let op = write_ai_entity_op(&entity);
        db.apply_batch(&[op]).unwrap();

        // Read it back
        let read_back = read_ai_entity(&db, &entity.id).unwrap();
        assert!(read_back.is_some(), "Entity should exist after write");

        let read_entity = read_back.unwrap();
        assert_eq!(read_entity.id, entity.id);
        assert_eq!(read_entity.code_hash, entity.code_hash);
        assert_eq!(read_entity.creator, entity.creator);
        assert_eq!(read_entity.autonomy_mode, entity.autonomy_mode);
        assert_eq!(
            read_entity.capabilities.to_byte(),
            entity.capabilities.to_byte()
        );
        assert_eq!(read_entity.economic_balance, entity.economic_balance);
        assert_eq!(read_entity.nonce, entity.nonce);
        assert_eq!(read_entity.memory_root, entity.memory_root);
        assert_eq!(read_entity.params_root, entity.params_root);
        assert_eq!(read_entity.registered_at, entity.registered_at);
        assert_eq!(read_entity.last_active_at, entity.last_active_at);
    }

    #[test]
    fn ai_memory_isolation() {
        let mut db = MemKv::new();

        let entity_a = [0xAAu8; 32];
        let entity_b = [0xBBu8; 32];
        let slot = b"config";

        // Write different values for same slot name, different entities
        let op_a = write_ai_memory_op(&entity_a, slot, b"value_for_a".to_vec());
        let op_b = write_ai_memory_op(&entity_b, slot, b"value_for_b".to_vec());
        db.apply_batch(&[op_a, op_b]).unwrap();

        // Read back and verify isolation
        let val_a = read_ai_memory(&db, &entity_a, slot).unwrap();
        let val_b = read_ai_memory(&db, &entity_b, slot).unwrap();

        assert_eq!(val_a, Some(b"value_for_a".to_vec()));
        assert_eq!(val_b, Some(b"value_for_b".to_vec()));

        // Verify they are different
        assert_ne!(val_a, val_b, "Memory must be isolated between entities");
    }

    #[test]
    fn ai_key_ordering() {
        // Signal keys for heights 100, 200 must be lexicographically ordered
        // because we use big-endian encoding for height
        let issuer = [0x01u8; 32];

        let key_100 = ai_signal_key(100, &issuer);
        let key_200 = ai_signal_key(200, &issuer);

        assert!(
            key_100 < key_200,
            "ai_signal_key(100, x) must be < ai_signal_key(200, x) for range scans"
        );

        // Also test edge cases
        let key_0 = ai_signal_key(0, &issuer);
        let key_max = ai_signal_key(u64::MAX, &issuer);

        assert!(key_0 < key_100);
        assert!(key_200 < key_max);

        // Different issuers at same height should not affect height ordering
        let issuer_2 = [0xFFu8; 32];
        let key_100_issuer2 = ai_signal_key(100, &issuer_2);
        let key_200_issuer1 = ai_signal_key(200, &issuer);

        assert!(
            key_100_issuer2 < key_200_issuer1,
            "Height ordering must take precedence over issuer"
        );
    }

    #[test]
    fn ai_entity_not_found_returns_none() {
        let db = MemKv::new();
        let nonexistent = [0xDEu8; 32];

        let result = read_ai_entity(&db, &nonexistent).unwrap();
        assert!(result.is_none(), "Non-existent entity should return None");
    }

    #[test]
    fn ai_memory_not_found_returns_none() {
        let db = MemKv::new();
        let entity = [0xAAu8; 32];

        let result = read_ai_memory(&db, &entity, b"nonexistent").unwrap();
        assert!(
            result.is_none(),
            "Non-existent memory slot should return None"
        );
    }

    #[test]
    fn ai_memory_delete_works() {
        let mut db = MemKv::new();
        let entity = [0xAAu8; 32];
        let slot = b"temp_data";

        // Write
        let write_op = write_ai_memory_op(&entity, slot, b"some_value".to_vec());
        db.apply_batch(&[write_op]).unwrap();

        // Verify exists
        let val = read_ai_memory(&db, &entity, slot).unwrap();
        assert!(val.is_some());

        // Delete
        let delete_op = delete_ai_memory_op(&entity, slot);
        db.apply_batch(&[delete_op]).unwrap();

        // Verify gone
        let val_after = read_ai_memory(&db, &entity, slot).unwrap();
        assert!(val_after.is_none(), "Memory slot should be deleted");
    }

    // ========================================================================
    // MEMORY OBJECT PAYLOAD TESTS (Week 21 - D21.4)
    // ========================================================================

    #[test]
    fn create_memory_object_payload_roundtrip() {
        use novai_ai_entities::MemoryObjectType;

        let payload = CreateMemoryObjectPayloadV1 {
            object_type: MemoryObjectType::ChainSummary,
            data: b"test chain summary data".to_vec(),
        };

        let encoded = encode_create_memory_object_payload_v1(&payload);
        let decoded = decode_create_memory_object_payload_v1(&encoded).unwrap();

        assert_eq!(decoded, payload);
    }

    #[test]
    fn create_memory_object_payload_all_types() {
        use novai_ai_entities::MemoryObjectType;

        for obj_type in [
            MemoryObjectType::ChainSummary,
            MemoryObjectType::LabelIndex,
            MemoryObjectType::EmbeddingCommitment,
            MemoryObjectType::AnomalyLog,
            MemoryObjectType::StatisticsSnapshot,
        ] {
            let payload = CreateMemoryObjectPayloadV1 {
                object_type: obj_type,
                data: vec![0xAA, 0xBB, 0xCC],
            };

            let encoded = encode_create_memory_object_payload_v1(&payload);
            let decoded = decode_create_memory_object_payload_v1(&encoded).unwrap();

            assert_eq!(decoded.object_type, obj_type);
            assert_eq!(decoded.data, payload.data);
        }
    }

    #[test]
    fn update_memory_object_payload_roundtrip() {
        let payload = UpdateMemoryObjectPayloadV1 {
            object_id: [0xABu8; 32],
            new_data: b"updated data content".to_vec(),
        };

        let encoded = encode_update_memory_object_payload_v1(&payload);
        let decoded = decode_update_memory_object_payload_v1(&encoded).unwrap();

        assert_eq!(decoded, payload);
    }

    #[test]
    fn delete_memory_object_payload_roundtrip() {
        let payload = DeleteMemoryObjectPayloadV1 {
            object_id: [0xCDu8; 32],
        };

        let encoded = encode_delete_memory_object_payload_v1(&payload);
        let decoded = decode_delete_memory_object_payload_v1(&encoded).unwrap();

        assert_eq!(decoded, payload);
    }

    #[test]
    fn create_memory_object_payload_empty_data() {
        use novai_ai_entities::MemoryObjectType;

        let payload = CreateMemoryObjectPayloadV1 {
            object_type: MemoryObjectType::LabelIndex,
            data: vec![], // Empty data
        };

        let encoded = encode_create_memory_object_payload_v1(&payload);
        assert_eq!(encoded.len(), 6); // version(1) + type(1) + data_len(4)

        let decoded = decode_create_memory_object_payload_v1(&encoded).unwrap();
        assert_eq!(decoded.data.len(), 0);
    }

    // ========================================================================
    // MEMORY OBJECT CRUD EXECUTION TESTS (Week 21 - D21.4)
    // ========================================================================

    /// Helper to create an AI entity with memory capabilities.
    fn setup_test_entity_for_memory(db: &mut MemKv) -> AiEntity {
        let mut entity = AiEntity::new(
            [0x42u8; 32], // code_hash
            [0x01u8; 32], // creator
            AutonomyMode::Gated,
            Capabilities::gated(),
            1000, // registered_at
        );
        entity.economic_balance = 1_000_000u128;

        // In production, the reverse index is keyed on
        // `address_from_pubkey(entity.pubkey)`. In these tests txs use
        // `tx.from = entity.id`, so we index the entity at its own id so the
        // public wrapper's `lookup_ai_entity_by_address(tx.from)` succeeds.
        db.apply_batch(&[
            write_ai_entity_op(&entity),
            WriteOp::Put(
                novai_state::ai_entity_by_address_key(&entity.id),
                entity.id.to_vec(),
            ),
        ])
        .unwrap();

        entity
    }

    /// Helper to build a create memory object tx.
    fn mk_create_memory_tx(
        entity_id: [u8; 32],
        nonce: u64,
        fee: u64,
        object_type: novai_ai_entities::MemoryObjectType,
        data: Vec<u8>,
    ) -> TxV1 {
        use novai_types::TxVersion;

        let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
            object_type,
            data,
        });

        TxV1 {
            version: TxVersion::V1,
            from: entity_id,
            pubkey: entity_id,
            nonce,
            fee,
            payload,
            sig: [0u8; 64],
        }
    }

    /// Helper to build an update memory object tx.
    fn mk_update_memory_tx(
        entity_id: [u8; 32],
        nonce: u64,
        fee: u64,
        object_id: [u8; 32],
        new_data: Vec<u8>,
    ) -> TxV1 {
        use novai_types::TxVersion;

        let payload = encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
            object_id,
            new_data,
        });

        TxV1 {
            version: TxVersion::V1,
            from: entity_id,
            pubkey: entity_id,
            nonce,
            fee,
            payload,
            sig: [0u8; 64],
        }
    }

    /// Helper to build a delete memory object tx.
    fn mk_delete_memory_tx(entity_id: [u8; 32], nonce: u64, fee: u64, object_id: [u8; 32]) -> TxV1 {
        use novai_types::TxVersion;

        let payload =
            encode_delete_memory_object_payload_v1(&DeleteMemoryObjectPayloadV1 { object_id })
                .to_vec();

        TxV1 {
            version: TxVersion::V1,
            from: entity_id,
            pubkey: entity_id,
            nonce,
            fee,
            payload,
            sig: [0u8; 64],
        }
    }

    #[test]
    fn create_memory_object_success() {
        use novai_ai_entities::MemoryObjectType;

        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        let tx = mk_create_memory_tx(
            entity.id,
            0, // nonce
            1, // fee
            MemoryObjectType::ChainSummary,
            b"chain summary data".to_vec(),
        );

        let object_id = apply_create_memory_object_tx(&mut db, &tx, 2000).unwrap();

        // Verify object was created
        let obj = read_memory_object(&db, &entity.id, &object_id)
            .unwrap()
            .expect("Object should exist");
        assert_eq!(obj.object_type, MemoryObjectType::ChainSummary);
        assert_eq!(obj.data, b"chain summary data".to_vec());
        assert_eq!(obj.owner_entity, entity.id);
        assert_eq!(obj.created_at, 2000);
        assert_eq!(obj.updated_at, 2000);

        // Verify count was incremented
        let count = read_memory_count(&db, &entity.id).unwrap();
        assert_eq!(count, 1);

        // Verify entity state was updated
        let updated_entity = read_ai_entity(&db, &entity.id).unwrap().unwrap();
        assert_eq!(updated_entity.nonce, 1);
        assert_eq!(updated_entity.last_active_at, 2000);
    }

    #[test]
    fn create_memory_object_size_limit_enforced() {
        use novai_ai_entities::{MemoryObjectType, MAX_MEMORY_OBJECT_SIZE};

        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        // Try to create object exceeding size limit
        let oversized_data = vec![0xAAu8; MAX_MEMORY_OBJECT_SIZE + 1];
        let tx = mk_create_memory_tx(
            entity.id,
            0,
            1,
            MemoryObjectType::ChainSummary,
            oversized_data,
        );

        let result = apply_create_memory_object_tx(&mut db, &tx, 2000);
        assert!(matches!(
            result,
            Err(ExecError::MemoryObjectTooLarge { .. })
        ));
    }

    #[test]
    fn create_memory_object_count_limit_enforced() {
        use novai_ai_entities::{MemoryObjectType, MAX_MEMORY_OBJECTS_PER_ENTITY};
        use novai_state::{ai_memory_count_key, encode_memory_count};

        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        // Set count to max - this simulates having max objects already
        let count_key = ai_memory_count_key(&entity.id);
        db.put(
            &count_key,
            &encode_memory_count(MAX_MEMORY_OBJECTS_PER_ENTITY),
        )
        .unwrap();

        let tx = mk_create_memory_tx(
            entity.id,
            0,
            1,
            MemoryObjectType::ChainSummary,
            b"data".to_vec(),
        );

        let result = apply_create_memory_object_tx(&mut db, &tx, 2000);
        assert!(matches!(
            result,
            Err(ExecError::MemoryObjectCountExceeded { .. })
        ));
    }

    #[test]
    fn update_memory_object_success() {
        use novai_ai_entities::MemoryObjectType;

        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        // Create an object first
        let create_tx = mk_create_memory_tx(
            entity.id,
            0,
            1,
            MemoryObjectType::StatisticsSnapshot,
            b"initial data".to_vec(),
        );
        let object_id = apply_create_memory_object_tx(&mut db, &create_tx, 2000).unwrap();

        // Update the object
        let update_tx = mk_update_memory_tx(
            entity.id,
            1, // nonce incremented
            1,
            object_id,
            b"updated data".to_vec(),
        );
        apply_update_memory_object_tx(&mut db, &update_tx, 3000).unwrap();

        // Verify update
        let obj = read_memory_object(&db, &entity.id, &object_id)
            .unwrap()
            .expect("Object should exist");
        assert_eq!(obj.data, b"updated data".to_vec());
        assert_eq!(obj.created_at, 2000); // unchanged
        assert_eq!(obj.updated_at, 3000); // updated
    }

    #[test]
    fn update_memory_object_not_found() {
        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        // Try to update non-existent object
        let update_tx = mk_update_memory_tx(
            entity.id,
            0,
            1,
            [0xFFu8; 32], // non-existent object ID
            b"data".to_vec(),
        );

        let result = apply_update_memory_object_tx(&mut db, &update_tx, 2000);
        assert!(matches!(result, Err(ExecError::MemoryObjectNotFound)));
    }

    #[test]
    fn delete_memory_object_success() {
        use novai_ai_entities::MemoryObjectType;

        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        // Create an object first
        let create_tx = mk_create_memory_tx(
            entity.id,
            0,
            1,
            MemoryObjectType::AnomalyLog,
            b"anomaly data".to_vec(),
        );
        let object_id = apply_create_memory_object_tx(&mut db, &create_tx, 2000).unwrap();

        // Verify it exists
        let count_before = read_memory_count(&db, &entity.id).unwrap();
        assert_eq!(count_before, 1);

        // Delete the object
        let delete_tx = mk_delete_memory_tx(
            entity.id, 1, // nonce incremented
            1, object_id,
        );
        apply_delete_memory_object_tx(&mut db, &delete_tx, 3000).unwrap();

        // Verify deletion
        let obj = read_memory_object(&db, &entity.id, &object_id).unwrap();
        assert!(obj.is_none(), "Object should be deleted");

        // Verify count was decremented
        let count_after = read_memory_count(&db, &entity.id).unwrap();
        assert_eq!(count_after, 0);
    }

    #[test]
    fn delete_memory_object_not_found() {
        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        // Try to delete non-existent object
        let delete_tx = mk_delete_memory_tx(
            entity.id,
            0,
            1,
            [0xFFu8; 32], // non-existent object ID
        );

        let result = apply_delete_memory_object_tx(&mut db, &delete_tx, 2000);
        assert!(matches!(result, Err(ExecError::MemoryObjectNotFound)));
    }

    #[test]
    fn memory_object_crud_full_lifecycle() {
        use novai_ai_entities::MemoryObjectType;

        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);
        let mut nonce = 0u64;

        // 1. Create
        let create_tx = mk_create_memory_tx(
            entity.id,
            nonce,
            1,
            MemoryObjectType::EmbeddingCommitment,
            b"embedding v1".to_vec(),
        );
        nonce += 1;
        let object_id = apply_create_memory_object_tx(&mut db, &create_tx, 1000).unwrap();

        // 2. Read
        let obj = read_memory_object(&db, &entity.id, &object_id)
            .unwrap()
            .unwrap();
        assert_eq!(obj.data, b"embedding v1".to_vec());

        // 3. Update
        let update_tx =
            mk_update_memory_tx(entity.id, nonce, 1, object_id, b"embedding v2".to_vec());
        nonce += 1;
        apply_update_memory_object_tx(&mut db, &update_tx, 2000).unwrap();

        let obj = read_memory_object(&db, &entity.id, &object_id)
            .unwrap()
            .unwrap();
        assert_eq!(obj.data, b"embedding v2".to_vec());

        // 4. Delete
        let delete_tx = mk_delete_memory_tx(entity.id, nonce, 1, object_id);
        apply_delete_memory_object_tx(&mut db, &delete_tx, 3000).unwrap();

        let obj = read_memory_object(&db, &entity.id, &object_id).unwrap();
        assert!(obj.is_none());
    }

    // ========================================================================
    // NNPX PRIVACY SECURITY TESTS (Week 22 - D22.5)
    // ========================================================================

    #[test]
    fn ai_cannot_read_nnpx() {
        // D22.5: AI entity operations are rejected for nnpx/ keys

        let ai_entity_id = [0x42u8; 32];
        let account_addr = [0x01u8; 32];

        let ai_caller = Caller::AiEntity(ai_entity_id);
        let account_caller = Caller::Account(account_addr);

        // AI entity trying to access NNPX keys should be denied
        let nnpx_key = b"nnpx/commitments/abc123";
        let result: Result<(), ExecError<()>> = validate_nnpx_access(nnpx_key, &ai_caller);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "AI entity must be denied access to NNPX keys"
        );

        // Human account can access NNPX keys
        let result: Result<(), ExecError<()>> = validate_nnpx_access(nnpx_key, &account_caller);
        assert!(
            result.is_ok(),
            "Human account should be allowed to access NNPX keys"
        );

        // AI entity can access public keys
        let public_key = b"accounts/alice";
        let result: Result<(), ExecError<()>> = validate_nnpx_access(public_key, &ai_caller);
        assert!(result.is_ok(), "AI entity can access public keys");

        // Test various NNPX prefixes
        for key in [
            b"nnpx/".as_slice(),
            b"nnpx/nullifiers/null1",
            b"nnpx/encrypted/payload1",
            b"nnpx/commitments/commit1",
        ] {
            let result: Result<(), ExecError<()>> = validate_nnpx_access(key, &ai_caller);
            assert!(
                matches!(result, Err(ExecError::NnpxAccessDenied)),
                "AI entity must be denied access to all NNPX keys"
            );
        }
    }

    #[test]
    fn commitment_hides_payload() {
        // D22.5: Same payload with different encryption produces different commitments
        use novai_ai_entities::PrivatePayloadCommitment;

        // Simulate same logical payload encrypted with different randomness
        let payload_encrypted_v1 = b"encrypted_with_random_nonce_1";
        let payload_encrypted_v2 = b"encrypted_with_random_nonce_2";

        let hash1 = PrivatePayloadCommitment::compute_commitment_hash(payload_encrypted_v1);
        let hash2 = PrivatePayloadCommitment::compute_commitment_hash(payload_encrypted_v2);

        // Different encrypted payloads must produce different commitments
        // This is the "hiding" property - observer cannot determine original content
        assert_ne!(
            hash1, hash2,
            "Different encryptions must produce different commitments (hiding property)"
        );

        // Same payload must produce same commitment (deterministic)
        let hash1_again = PrivatePayloadCommitment::compute_commitment_hash(payload_encrypted_v1);
        assert_eq!(
            hash1, hash1_again,
            "Same payload must produce same commitment (deterministic)"
        );
    }

    #[test]
    fn nullifier_prevents_reuse() {
        // D22.5: Duplicate nullifiers are detected and rejected
        use novai_ai_entities::PrivatePayloadCommitment;

        let mut db = MemKv::new();

        let spending_secret = [0xABu8; 32];
        let counter = 42u64;

        // Compute nullifier
        let nullifier = PrivatePayloadCommitment::compute_nullifier(&spending_secret, counter);

        // First spend: nullifier is unspent
        let result = validate_nullifier_unspent(&db, &nullifier);
        assert!(result.is_ok(), "First spend should succeed");

        // Mark as spent
        let spend_op = mark_nullifier_spent(&nullifier);
        db.apply_batch(&[spend_op]).unwrap();

        // Second spend: nullifier is already spent (double-spend attempt)
        let result = validate_nullifier_unspent(&db, &nullifier);
        assert!(
            matches!(result, Err(ExecError::NullifierAlreadySpent)),
            "Double-spend must be rejected"
        );

        // Different counter = different nullifier = valid new spend
        let nullifier2 = PrivatePayloadCommitment::compute_nullifier(&spending_secret, counter + 1);
        let result = validate_nullifier_unspent(&db, &nullifier2);
        assert!(
            result.is_ok(),
            "Different nullifier (new spend) should succeed"
        );
    }

    #[test]
    fn ai_entity_cannot_have_nnpx_capability() {
        // D22.4: AI entities must not be registered with read_nnpx_derived capability

        // Valid AI capabilities (no NNPX access)
        let valid_caps = Capabilities::gated();
        assert!(!valid_caps.read_nnpx_derived);
        let result: Result<(), ExecError<()>> = validate_ai_entity_no_nnpx_capability(&valid_caps);
        assert!(result.is_ok(), "AI without NNPX capability should pass");

        // Invalid: AI trying to get NNPX access
        let mut invalid_caps = Capabilities::gated();
        invalid_caps.read_nnpx_derived = true;
        let result: Result<(), ExecError<()>> =
            validate_ai_entity_no_nnpx_capability(&invalid_caps);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "AI with NNPX capability must be rejected"
        );
    }

    #[test]
    fn is_private_key_identifies_nnpx_keys() {
        // Verify the is_private_key helper works correctly

        // NNPX keys are private
        assert!(is_private_key(b"nnpx/"));
        assert!(is_private_key(b"nnpx/commitments/abc"));
        assert!(is_private_key(b"nnpx/nullifiers/xyz"));
        assert!(is_private_key(b"nnpx/encrypted/data"));

        // Non-NNPX keys are public
        assert!(!is_private_key(b"accounts/alice"));
        assert!(!is_private_key(b"ai/entities/entity1"));
        assert!(!is_private_key(b"consensus/blocks/100"));
        assert!(!is_private_key(b"governance/proposals/prop1"));
    }

    #[test]
    fn nullifier_storage_key_format() {
        // Verify nullifier keys are stored in the NNPX namespace
        let nullifier = [0xABu8; 32];
        let key = nnpx_nullifier_key(&nullifier);

        // Must be an NNPX key
        assert!(
            is_private_key(&key),
            "Nullifier key must be in NNPX namespace"
        );

        // Must start with correct prefix
        assert!(
            key.starts_with(b"nnpx/nullifiers/"),
            "Nullifier key must have correct prefix"
        );
    }

    #[test]
    fn caller_enum_equality() {
        let ai1 = Caller::AiEntity([0x01u8; 32]);
        let ai2 = Caller::AiEntity([0x02u8; 32]);
        let ai1_copy = Caller::AiEntity([0x01u8; 32]);
        let account = Caller::Account([0x01u8; 32]);

        assert_eq!(ai1, ai1_copy, "Same AI entity should be equal");
        assert_ne!(ai1, ai2, "Different AI entities should not be equal");
        assert_ne!(ai1, account, "AI entity and account should not be equal");
    }

    // ========================================================================
    // DERIVED VIEWS BOUNDARY TESTS (Week 23 - D23.6)
    // ========================================================================

    use novai_ai_entities::{AggregateVolumeData, DerivedSourceType, DerivedView};

    /// Helper: Create an AI entity with derived view read capability.
    fn create_entity_with_derived_capability() -> novai_ai_entities::AiEntity {
        let code_hash = [0xAAu8; 32];
        let creator = [0xBBu8; 32];
        let caps = Capabilities {
            read_nnpx_derived: true,
            read_public_chain: true,
            ..Capabilities::default()
        };

        novai_ai_entities::AiEntity::new(code_hash, creator, AutonomyMode::Advisory, caps, 1000)
    }

    /// Helper: Create an AI entity WITHOUT derived view read capability.
    fn create_entity_without_derived_capability() -> novai_ai_entities::AiEntity {
        let code_hash = [0xCCu8; 32];
        let creator = [0xDDu8; 32];
        let caps = Capabilities {
            read_nnpx_derived: false,
            read_public_chain: true,
            ..Capabilities::default()
        };

        novai_ai_entities::AiEntity::new(code_hash, creator, AutonomyMode::Advisory, caps, 1000)
    }

    /// Helper: Create a test derived view.
    fn create_test_derived_view(creator: [u8; 32]) -> DerivedView {
        let data = AggregateVolumeData {
            start_height: 100,
            end_height: 200,
            total_volume: 1_000_000,
        }
        .encode();

        DerivedView::new(
            DerivedSourceType::ChainAggregate,
            1, // AggregateVolume schema
            1000,
            creator,
            data,
        )
        .expect("Valid derived view")
    }

    #[test]
    fn ai_reads_derived_view_with_capability() {
        // D23.6: AI entity with read_nnpx_derived capability can read derived views
        let mut db = MemKv::new();

        // Create an entity WITH derived view capability
        let entity = create_entity_with_derived_capability();
        assert!(
            entity.capabilities.read_nnpx_derived,
            "Test entity should have capability"
        );

        // Create and store a derived view
        let creator = [0x42u8; 32];
        let view = create_test_derived_view(creator);
        let view_id = view.view_id;

        // Store the view
        let ops = write_derived_view_ops(&view);
        db.apply_batch(&ops).unwrap();

        // Entity with capability can read the view
        let result = read_derived_view_with_audit(&db, &entity, &view_id, 2000);
        assert!(result.is_ok(), "Entity with capability should read view");

        let (read_view, audit_op) = result.unwrap();
        assert_eq!(read_view.view_id, view.view_id);
        assert_eq!(read_view.schema_id, 1);
        assert_eq!(read_view.source_type, DerivedSourceType::ChainAggregate);

        // Verify audit op was created
        match audit_op {
            WriteOp::Put(key, value) => {
                assert!(
                    key.starts_with(b"derived_views/audit/"),
                    "Audit key should have correct prefix"
                );
                assert_eq!(value.len(), 32, "Audit value should be view_id");
            }
            WriteOp::Delete(_) => panic!("Audit should be a Put, not Delete"),
        }
    }

    #[test]
    fn ai_cannot_read_derived_view_without_capability() {
        // D23.6: AI entity WITHOUT read_nnpx_derived capability is denied
        let mut db = MemKv::new();

        // Create an entity WITHOUT derived view capability
        let entity = create_entity_without_derived_capability();
        assert!(
            !entity.capabilities.read_nnpx_derived,
            "Test entity should NOT have capability"
        );

        // Create and store a derived view
        let creator = [0x42u8; 32];
        let view = create_test_derived_view(creator);
        let view_id = view.view_id;

        let ops = write_derived_view_ops(&view);
        db.apply_batch(&ops).unwrap();

        // Entity without capability is denied
        let result = read_derived_view_with_audit(&db, &entity, &view_id, 2000);
        assert!(
            matches!(result, Err(ExecError::DerivedViewAccessDenied)),
            "Entity without capability should be denied"
        );
    }

    #[test]
    fn ai_cannot_read_raw_nnpx_even_with_derived_capability() {
        // D23.6: AI entity with read_nnpx_derived still cannot read raw NNPX data
        let entity = create_entity_with_derived_capability();
        assert!(
            entity.capabilities.read_nnpx_derived,
            "Entity should have derived capability"
        );

        let ai_caller = Caller::AiEntity(entity.id);

        // AI entity is still blocked from raw NNPX keys
        let nnpx_key = b"nnpx/commitments/secret123";
        let result: Result<(), ExecError<()>> = validate_nnpx_access(nnpx_key, &ai_caller);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "AI must STILL be blocked from raw NNPX even with derived capability"
        );

        // But derived_views/ keys are NOT nnpx keys
        let derived_key = b"derived_views/view123";
        assert!(!is_private_key(derived_key), "Derived views are not NNPX");
        assert!(is_derived_view(derived_key), "Should be a derived view key");
    }

    #[test]
    fn derived_view_schema_validated() {
        // D23.6: Invalid schema is rejected
        let creator = [0x42u8; 32];

        // Valid schema (AggregateVolume = 1)
        let valid_data = AggregateVolumeData {
            start_height: 0,
            end_height: 100,
            total_volume: 0,
        }
        .encode();

        let view = DerivedView::new(
            DerivedSourceType::ChainAggregate,
            1, // Valid schema
            1000,
            creator,
            valid_data.clone(),
        );
        assert!(view.is_some(), "Valid schema should create view");

        // Invalid schema ID (99 doesn't exist)
        let view = DerivedView::new(
            DerivedSourceType::ChainAggregate,
            99, // Invalid schema
            1000,
            creator,
            valid_data,
        );
        assert!(view.is_none(), "Invalid schema should fail");

        // Invalid data length for schema
        let wrong_length_data = vec![0u8; 10]; // AggregateVolume needs 32 bytes
        let view = DerivedView::new(
            DerivedSourceType::ChainAggregate,
            1,
            1000,
            creator,
            wrong_length_data,
        );
        assert!(view.is_none(), "Wrong data length should fail");
    }

    #[test]
    fn derived_view_not_found_returns_error() {
        let db = MemKv::new();
        let entity = create_entity_with_derived_capability();

        // Try to read non-existent view
        let missing_id = [0xFFu8; 32];
        let result = read_derived_view_with_audit(&db, &entity, &missing_id, 1000);
        assert!(
            matches!(result, Err(ExecError::DerivedViewNotFound)),
            "Missing view should return NotFound error"
        );
    }

    #[test]
    fn derived_view_write_creates_indices() {
        let mut db = MemKv::new();

        let creator = [0x42u8; 32];
        let view = create_test_derived_view(creator);

        // Write the view
        let ops = write_derived_view_ops(&view);
        assert_eq!(
            ops.len(),
            3,
            "Should create 3 WriteOps (primary + 2 indices)"
        );

        db.apply_batch(&ops).unwrap();

        // Verify primary storage
        let primary_key = derived_view_key(&view.view_id);
        assert!(
            db.get(&primary_key).unwrap().is_some(),
            "Primary storage should exist"
        );

        // Verify schema index
        let schema_key = derived_view_by_schema_key(view.schema_id, &view.view_id);
        assert!(
            db.get(&schema_key).unwrap().is_some(),
            "Schema index should exist"
        );

        // Verify creator index
        let creator_key = derived_view_by_creator_key(&view.creator, &view.view_id);
        assert!(
            db.get(&creator_key).unwrap().is_some(),
            "Creator index should exist"
        );
    }

    #[test]
    fn query_derived_views_by_schema() {
        let mut db = MemKv::new();

        let creator = [0x42u8; 32];

        // Create multiple views with same schema
        let view1 = create_test_derived_view(creator);

        // Create another view with different created_at to get different ID
        let data2 = AggregateVolumeData {
            start_height: 200,
            end_height: 300,
            total_volume: 2_000_000,
        }
        .encode();
        let view2 = DerivedView::new(
            DerivedSourceType::ChainAggregate,
            1, // Same schema
            2000,
            creator,
            data2,
        )
        .unwrap();

        // Store both views
        db.apply_batch(&write_derived_view_ops(&view1)).unwrap();
        db.apply_batch(&write_derived_view_ops(&view2)).unwrap();

        // Query by schema
        let results = get_derived_views_by_schema(&db, 1).unwrap();
        assert_eq!(results.len(), 2, "Should find 2 views with schema 1");

        // Query non-existent schema
        let results = get_derived_views_by_schema(&db, 99).unwrap();
        assert_eq!(results.len(), 0, "Should find 0 views with schema 99");
    }

    #[test]
    fn query_derived_views_by_creator() {
        let mut db = MemKv::new();

        let creator1 = [0x42u8; 32];
        let creator2 = [0x43u8; 32];

        // Create views from different creators
        let view1 = create_test_derived_view(creator1);

        let data2 = AggregateVolumeData {
            start_height: 200,
            end_height: 300,
            total_volume: 2_000_000,
        }
        .encode();
        let view2 = DerivedView::new(
            DerivedSourceType::UserAuthorized,
            1,
            2000,
            creator2, // Different creator
            data2,
        )
        .unwrap();

        // Store both views
        db.apply_batch(&write_derived_view_ops(&view1)).unwrap();
        db.apply_batch(&write_derived_view_ops(&view2)).unwrap();

        // Query by creator1
        let results = get_derived_views_by_creator(&db, &creator1).unwrap();
        assert_eq!(results.len(), 1, "Should find 1 view from creator1");
        assert_eq!(results[0].creator, creator1);

        // Query by creator2
        let results = get_derived_views_by_creator(&db, &creator2).unwrap();
        assert_eq!(results.len(), 1, "Should find 1 view from creator2");
        assert_eq!(results[0].creator, creator2);

        // Query by non-existent creator
        let creator3 = [0x44u8; 32];
        let results = get_derived_views_by_creator(&db, &creator3).unwrap();
        assert_eq!(results.len(), 0, "Should find 0 views from creator3");
    }

    #[test]
    fn validate_derived_view_access_function() {
        // Test the access validation function directly
        let entity_with = create_entity_with_derived_capability();
        let entity_without = create_entity_without_derived_capability();

        let result: Result<(), ExecError<()>> = validate_derived_view_access(&entity_with);
        assert!(result.is_ok(), "Entity with capability should pass");

        let result: Result<(), ExecError<()>> = validate_derived_view_access(&entity_without);
        assert!(
            matches!(result, Err(ExecError::DerivedViewAccessDenied)),
            "Entity without capability should fail"
        );
    }

    #[test]
    fn is_derived_view_function() {
        // Test the is_derived_view helper
        assert!(is_derived_view(b"derived_views/"));
        assert!(is_derived_view(b"derived_views/view123"));
        assert!(is_derived_view(b"derived_views/audit/entity/100"));

        assert!(!is_derived_view(b"nnpx/"));
        assert!(!is_derived_view(b"accounts/"));
        assert!(!is_derived_view(b"ai/entities/"));
    }

    #[test]
    fn all_schemas_are_valid() {
        // Verify all predefined schemas can create views
        let creator = [0x42u8; 32];

        // Schema 1: AggregateVolume
        let data1 = AggregateVolumeData {
            start_height: 0,
            end_height: 100,
            total_volume: 0,
        }
        .encode();
        assert!(
            DerivedView::new(DerivedSourceType::ChainAggregate, 1, 0, creator, data1).is_some()
        );

        // Schema 2: ActivityCount
        let data2 = novai_ai_entities::ActivityCountData {
            start_height: 0,
            end_height: 100,
            tx_count: 0,
        }
        .encode();
        assert!(
            DerivedView::new(DerivedSourceType::ChainAggregate, 2, 0, creator, data2).is_some()
        );

        // Schema 3: PoolSize
        let data3 = novai_ai_entities::PoolSizeData {
            snapshot_height: 0,
            pool_size: 0,
        }
        .encode();
        assert!(
            DerivedView::new(DerivedSourceType::ChainAggregate, 3, 0, creator, data3).is_some()
        );
    }

    // ========================================================================
    // PHASE 3: AI ENTITY ORIGINATE TRANSACTIONS — Type 10 Registration Tests
    // ========================================================================

    /// Helper: build a type 10 register-entity-with-key tx.
    #[allow(clippy::too_many_arguments)]
    fn mk_register_with_key_tx(
        from: Address,
        nonce: u64,
        fee: u64,
        code_hash: [u8; 32],
        pubkey: [u8; 32],
        autonomy_mode: AutonomyMode,
        capabilities: Capabilities,
        initial_balance: u128,
    ) -> TxV1 {
        use novai_types::TxVersion;
        let payload =
            encode_register_ai_entity_with_key_payload_v1(&RegisterAiEntityWithKeyPayloadV1 {
                code_hash,
                pubkey,
                autonomy_mode,
                capabilities,
                initial_balance,
            });
        TxV1 {
            version: TxVersion::V1,
            from,
            pubkey: from,
            nonce,
            fee,
            payload: payload.to_vec(),
            sig: [0u8; 64],
        }
    }

    /// Helper: fund an account in `MemKv`.
    fn fund_account(db: &mut MemKv, addr: &Address, balance: u128) {
        let acct = AccountStateV1 { balance, nonce: 0 };
        db.apply_batch(&[WriteOp::Put(
            account_key(addr),
            encode_account_v1(&acct).to_vec(),
        )])
        .unwrap();
    }

    #[test]
    fn type10_encode_decode_roundtrip() {
        let p = RegisterAiEntityWithKeyPayloadV1 {
            code_hash: [0x42; 32],
            pubkey: [0xAB; 32],
            autonomy_mode: AutonomyMode::Gated,
            capabilities: Capabilities::gated(),
            initial_balance: 500_000,
        };
        let encoded = encode_register_ai_entity_with_key_payload_v1(&p);
        assert_eq!(encoded.len(), 83);
        assert_eq!(encoded[0], REGISTER_AI_ENTITY_WITH_KEY_PAYLOAD_V1);

        let decoded = decode_register_ai_entity_with_key_payload_v1(&encoded).unwrap();
        assert_eq!(decoded.code_hash, p.code_hash);
        assert_eq!(decoded.pubkey, p.pubkey);
        assert_eq!(decoded.autonomy_mode, p.autonomy_mode);
        assert_eq!(decoded.capabilities.to_byte(), p.capabilities.to_byte());
        assert_eq!(decoded.initial_balance, p.initial_balance);
    }

    #[test]
    fn type10_payload_bad_length_rejected() {
        let result = decode_register_ai_entity_with_key_payload_v1(&[10u8; 50]);
        assert!(matches!(
            result,
            Err(ExecError::BadPayloadLength {
                expected: 83,
                got: 50
            })
        ));
    }

    #[test]
    fn type10_register_stores_pubkey_and_reverse_index() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 1_000_000);

        let entity_pubkey = [0xABu8; 32];
        let tx = mk_register_with_key_tx(
            creator,
            0,
            5_000,
            [0x42; 32],
            entity_pubkey,
            AutonomyMode::Gated,
            Capabilities::gated(),
            100_000,
        );

        let entity_id = apply_register_ai_entity_with_key_tx(&mut db, &tx, 42).unwrap();

        // Verify entity stored with pubkey
        let entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
        assert_eq!(entity.pubkey, entity_pubkey);
        assert_eq!(entity.economic_balance, 100_000);
        assert_eq!(entity.registered_at, 42);
        assert!(entity.is_active);

        // Verify reverse index exists
        let entity_addr = derive_address_from_pubkey_bytes(&entity_pubkey);
        let addr_key = novai_state::ai_entity_by_address_key(&entity_addr);
        let stored_id = db.get(&addr_key).unwrap().unwrap();
        assert_eq!(stored_id, entity_id.to_vec());

        // Verify lookup by address works
        let looked_up = lookup_ai_entity_by_address(&db, &entity_addr)
            .unwrap()
            .unwrap();
        assert_eq!(looked_up.id, entity_id);
        assert_eq!(looked_up.pubkey, entity_pubkey);
    }

    #[test]
    fn type10_register_debits_creator() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 1_000_000);

        let tx = mk_register_with_key_tx(
            creator,
            0,
            5_000,
            [0x42; 32],
            [0xAB; 32],
            AutonomyMode::Gated,
            Capabilities::gated(),
            100_000,
        );

        apply_register_ai_entity_with_key_tx(&mut db, &tx, 1).unwrap();

        // Creator debited: initial_balance(100_000) + fee(5_000) = 105_000
        let acct = read_account_or_default::<MemKv>(&db, &creator).unwrap();
        assert_eq!(acct.balance, 1_000_000 - 105_000);
        assert_eq!(acct.nonce, 1);
    }

    #[test]
    fn type10_register_rejects_autonomous_mode() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 1_000_000);

        let tx = mk_register_with_key_tx(
            creator,
            0,
            5_000,
            [0x42; 32],
            [0xAB; 32],
            AutonomyMode::Autonomous,
            Capabilities::gated(),
            100_000,
        );

        let result = apply_register_ai_entity_with_key_tx(&mut db, &tx, 1);
        assert!(matches!(result, Err(ExecError::AutonomousModeReserved)));
    }

    #[test]
    fn type10_register_rejects_duplicate_entity() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 2_000_000);

        let tx1 = mk_register_with_key_tx(
            creator,
            0,
            5_000,
            [0x42; 32],
            [0xAB; 32],
            AutonomyMode::Gated,
            Capabilities::gated(),
            100_000,
        );
        apply_register_ai_entity_with_key_tx(&mut db, &tx1, 1).unwrap();

        // Same code_hash + creator → same entity_id → duplicate
        let tx2 = mk_register_with_key_tx(
            creator,
            1,
            5_000,
            [0x42; 32],
            [0xCC; 32], // different pubkey but same entity_id
            AutonomyMode::Gated,
            Capabilities::gated(),
            100_000,
        );
        let result = apply_register_ai_entity_with_key_tx(&mut db, &tx2, 2);
        assert!(matches!(result, Err(ExecError::EntityAlreadyExists)));
    }

    #[test]
    fn type10_register_rejects_duplicate_address() {
        let mut db = MemKv::new();
        let creator1 = [0x01u8; 32];
        let creator2 = [0x02u8; 32];
        fund_account(&mut db, &creator1, 2_000_000);
        fund_account(&mut db, &creator2, 2_000_000);

        let same_pubkey = [0xAB; 32];

        // First registration succeeds
        let tx1 = mk_register_with_key_tx(
            creator1,
            0,
            5_000,
            [0x42; 32],
            same_pubkey,
            AutonomyMode::Gated,
            Capabilities::gated(),
            100_000,
        );
        apply_register_ai_entity_with_key_tx(&mut db, &tx1, 1).unwrap();

        // Second with same pubkey (same address) from different creator → rejected
        let tx2 = mk_register_with_key_tx(
            creator2,
            0,
            5_000,
            [0x99; 32],  // different code_hash → different entity_id
            same_pubkey, // same pubkey → same address
            AutonomyMode::Gated,
            Capabilities::gated(),
            100_000,
        );
        let result = apply_register_ai_entity_with_key_tx(&mut db, &tx2, 2);
        assert!(matches!(result, Err(ExecError::EntityAlreadyExists)));
    }

    #[test]
    fn type10_register_rejects_insufficient_funds() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 50_000); // not enough for 100_000 + 5_000

        let tx = mk_register_with_key_tx(
            creator,
            0,
            5_000,
            [0x42; 32],
            [0xAB; 32],
            AutonomyMode::Gated,
            Capabilities::gated(),
            100_000,
        );
        let result = apply_register_ai_entity_with_key_tx(&mut db, &tx, 1);
        assert!(matches!(result, Err(ExecError::InsufficientFunds { .. })));
    }

    // ========================================================================
    // PHASE 3: AI ENTITY TRANSFER — Entity as Sender
    // ========================================================================

    /// Helper: register an entity with key and return (`entity_id`, `entity_addr`).
    fn register_entity_with_key(
        db: &mut MemKv,
        creator: &Address,
        nonce: u64,
        pubkey: [u8; 32],
        initial_balance: u128,
    ) -> ([u8; 32], Address) {
        let tx = mk_register_with_key_tx(
            *creator,
            nonce,
            5_000,
            [0x42; 32],
            pubkey,
            AutonomyMode::Gated,
            Capabilities::gated(),
            initial_balance,
        );
        let entity_id = apply_register_ai_entity_with_key_tx(db, &tx, 1).unwrap();
        let entity_addr = derive_address_from_pubkey_bytes(&pubkey);
        (entity_id, entity_addr)
    }

    /// Helper: build a transfer tx from an AI entity address.
    fn mk_entity_transfer_tx(
        entity_addr: Address,
        nonce: u64,
        fee: u64,
        to: Address,
        amount: u64,
    ) -> TxV1 {
        use novai_types::TxVersion;
        let payload = encode_transfer_payload_v1(&TransferPayloadV1 { to, amount });
        TxV1 {
            version: TxVersion::V1,
            from: entity_addr,
            pubkey: entity_addr, // in reality this is the entity pubkey; simplified for tests
            nonce,
            fee,
            payload: payload.to_vec(),
            sig: [0u8; 64],
        }
    }

    #[test]
    fn ai_entity_transfer_debits_entity_balance() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 10_000_000);

        let entity_pubkey = [0xAB; 32];
        let (entity_id, entity_addr) =
            register_entity_with_key(&mut db, &creator, 0, entity_pubkey, 500_000);

        let recipient = [0xFFu8; 32];
        let tx = mk_entity_transfer_tx(entity_addr, 0, 1_000, recipient, 50_000);
        apply_tx_v1_transfer(&mut db, &tx).unwrap();

        // Entity balance: 500_000 - 50_000 - 1_000 = 449_000
        let entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
        assert_eq!(entity.economic_balance, 449_000);
        assert_eq!(entity.nonce, 1);

        // Recipient credited
        let recip_acct = read_account_or_default::<MemKv>(&db, &recipient).unwrap();
        assert_eq!(recip_acct.balance, 50_000u128);
    }

    #[test]
    fn ai_entity_transfer_nonce_mismatch_rejected() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 10_000_000);

        let (_, entity_addr) = register_entity_with_key(&mut db, &creator, 0, [0xAB; 32], 500_000);

        // Entity nonce is 0, but we send nonce 5
        let tx = mk_entity_transfer_tx(entity_addr, 5, 1_000, [0xFF; 32], 10_000);
        let result = apply_tx_v1_transfer(&mut db, &tx);
        assert!(matches!(
            result,
            Err(ExecError::NonceMismatch {
                expected: 0,
                got: 5
            })
        ));
    }

    #[test]
    fn ai_entity_transfer_insufficient_funds_rejected() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 10_000_000);

        let (_, entity_addr) = register_entity_with_key(&mut db, &creator, 0, [0xAB; 32], 10_000);

        // Try to transfer more than entity balance
        let tx = mk_entity_transfer_tx(entity_addr, 0, 1_000, [0xFF; 32], 100_000);
        let result = apply_tx_v1_transfer(&mut db, &tx);
        assert!(matches!(result, Err(ExecError::InsufficientFunds { .. })));
    }

    #[test]
    fn ai_entity_transfer_inactive_entity_rejected() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 10_000_000);

        let entity_pubkey = [0xAB; 32];
        let (entity_id, entity_addr) =
            register_entity_with_key(&mut db, &creator, 0, entity_pubkey, 500_000);

        // Deactivate entity
        let mut entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
        entity.is_active = false;
        let op = write_ai_entity_op(&entity);
        db.apply_batch(&[op]).unwrap();

        let tx = mk_entity_transfer_tx(entity_addr, 0, 1_000, [0xFF; 32], 10_000);
        let result = apply_tx_v1_transfer(&mut db, &tx);
        assert!(matches!(result, Err(ExecError::EntityNotActive)));
    }

    #[test]
    fn ai_entity_sequential_transfers() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 10_000_000);

        let (entity_id, entity_addr) =
            register_entity_with_key(&mut db, &creator, 0, [0xAB; 32], 1_000_000);

        // Transfer 1
        let tx1 = mk_entity_transfer_tx(entity_addr, 0, 100, [0xFF; 32], 10_000);
        apply_tx_v1_transfer(&mut db, &tx1).unwrap();

        // Transfer 2 (nonce must be 1)
        let tx2 = mk_entity_transfer_tx(entity_addr, 1, 100, [0xEE; 32], 20_000);
        apply_tx_v1_transfer(&mut db, &tx2).unwrap();

        let entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
        // 1_000_000 - 10_000 - 100 - 20_000 - 100 = 969_800
        assert_eq!(entity.economic_balance, 969_800);
        assert_eq!(entity.nonce, 2);
    }

    // ========================================================================
    // PHASE 3: AI ENTITY RESTRICTION ENFORCEMENT
    // ========================================================================

    #[test]
    fn ai_entity_cannot_submit_governance() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 10_000_000);

        let (_, entity_addr) = register_entity_with_key(&mut db, &creator, 0, [0xAB; 32], 500_000);

        // Build a fake governance payload (type 6)
        let mut payload = vec![SUBMIT_PROPOSAL_PAYLOAD_V1];
        payload.extend_from_slice(&[0u8; 200]); // dummy data

        let tx = TxV1 {
            version: novai_types::TxVersion::V1,
            from: entity_addr,
            pubkey: entity_addr,
            nonce: 0,
            fee: 10_000,
            payload,
            sig: [0u8; 64],
        };

        let result = check_ai_entity_sender(&db, &tx, 100);
        assert!(matches!(result, Err(ExecError::IssuerMissingCapability)));
    }

    #[test]
    fn ai_entity_cannot_register_entities() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 10_000_000);

        let (_, entity_addr) = register_entity_with_key(&mut db, &creator, 0, [0xAB; 32], 500_000);

        // Type 8: register AI entity
        let mut payload = vec![REGISTER_AI_ENTITY_PAYLOAD_V1];
        payload.extend_from_slice(&[0u8; 100]);

        let tx = TxV1 {
            version: novai_types::TxVersion::V1,
            from: entity_addr,
            pubkey: entity_addr,
            nonce: 0,
            fee: 10_000,
            payload,
            sig: [0u8; 64],
        };

        let result = check_ai_entity_sender(&db, &tx, 100);
        assert!(matches!(result, Err(ExecError::IssuerMissingCapability)));
    }

    #[test]
    fn ai_entity_cannot_register_entities_with_key() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 10_000_000);

        let (_, entity_addr) = register_entity_with_key(&mut db, &creator, 0, [0xAB; 32], 500_000);

        // Type 10: register AI entity with key
        let mut payload = vec![REGISTER_AI_ENTITY_WITH_KEY_PAYLOAD_V1];
        payload.extend_from_slice(&[0u8; 100]);

        let tx = TxV1 {
            version: novai_types::TxVersion::V1,
            from: entity_addr,
            pubkey: entity_addr,
            nonce: 0,
            fee: 10_000,
            payload,
            sig: [0u8; 64],
        };

        let result = check_ai_entity_sender(&db, &tx, 100);
        assert!(matches!(result, Err(ExecError::IssuerMissingCapability)));
    }

    #[test]
    fn ai_entity_transfer_allowed_by_restrictions() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 10_000_000);

        let (_, entity_addr) = register_entity_with_key(&mut db, &creator, 0, [0xAB; 32], 500_000);

        // Type 1: transfer
        let payload = encode_transfer_payload_v1(&TransferPayloadV1 {
            to: [0xFF; 32],
            amount: 1_000,
        });

        let tx = TxV1 {
            version: novai_types::TxVersion::V1,
            from: entity_addr,
            pubkey: entity_addr,
            nonce: 0,
            fee: 1_000,
            payload: payload.to_vec(),
            sig: [0u8; 64],
        };

        let result = check_ai_entity_sender(&db, &tx, 100);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some()); // returns Some(entity)
    }

    #[test]
    fn normal_account_passes_restrictions_as_none() {
        let db = MemKv::new();
        let normal_addr = [0xDD; 32]; // not registered as entity

        let payload = encode_transfer_payload_v1(&TransferPayloadV1 {
            to: [0xFF; 32],
            amount: 1_000,
        });

        let tx = TxV1 {
            version: novai_types::TxVersion::V1,
            from: normal_addr,
            pubkey: normal_addr,
            nonce: 0,
            fee: 1_000,
            payload: payload.to_vec(),
            sig: [0u8; 64],
        };

        let result = check_ai_entity_sender(&db, &tx, 100);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none()); // not an entity
    }

    // ========================================================================
    // PHASE 3: DISPATCH INTEGRATION — Type 10 via dispatch_tx
    // ========================================================================

    #[test]
    fn dispatch_type10_via_dispatch_tx() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 10_000_000);

        let tx = mk_register_with_key_tx(
            creator,
            0,
            5_000,
            [0x42; 32],
            [0xAB; 32],
            AutonomyMode::Gated,
            Capabilities::gated(),
            100_000,
        );

        let result = dispatch_tx(&mut db, &tx, 1);
        assert!(result.is_ok());

        // Verify entity was created
        let entity_id = AiEntity::compute_id(&[0x42; 32], &creator);
        let entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
        assert_eq!(entity.pubkey, [0xAB; 32]);
    }

    #[test]
    fn dispatch_type10_below_min_fee_rejected() {
        let mut db = MemKv::new();
        let creator = [0x01u8; 32];
        fund_account(&mut db, &creator, 10_000_000);

        let tx = mk_register_with_key_tx(
            creator,
            0,
            100, // below MIN_FEE_REGISTER_AI_ENTITY_WITH_KEY (5_000)
            [0x42; 32],
            [0xAB; 32],
            AutonomyMode::Gated,
            Capabilities::gated(),
            100_000,
        );

        let result = dispatch_tx(&mut db, &tx, 1);
        assert!(matches!(result, Err(ExecError::FeeBelowMinimum { .. })));
    }

    // ========================================================================
    // PHASE 3: ADDRESS DERIVATION CONSISTENCY
    // ========================================================================

    #[test]
    fn derive_address_is_deterministic() {
        let pubkey = [0xAB; 32];
        let addr1 = derive_address_from_pubkey_bytes(&pubkey);
        let addr2 = derive_address_from_pubkey_bytes(&pubkey);
        assert_eq!(addr1, addr2);
    }

    #[test]
    fn derive_address_matches_crypto_module() {
        // Verify our inline derivation matches novai_crypto::address_from_pubkey
        // Generate a real ed25519 keypair to get a valid public key
        let (_sk, vk) = novai_crypto::generate_keypair();
        let pubkey_bytes: [u8; 32] = *vk.as_bytes();

        // Inline derivation
        let inline_addr = derive_address_from_pubkey_bytes(&pubkey_bytes);

        // Verify via novai_crypto (available as dev-dependency)
        let crypto_addr = novai_crypto::address_from_pubkey(&vk);

        assert_eq!(
            inline_addr, crypto_addr,
            "Inline address derivation must match novai_crypto"
        );
    }

    #[test]
    fn different_pubkeys_produce_different_addresses() {
        let addr1 = derive_address_from_pubkey_bytes(&[0xAB; 32]);
        let addr2 = derive_address_from_pubkey_bytes(&[0xCD; 32]);
        assert_ne!(addr1, addr2);
    }

    #[test]
    fn lookup_nonexistent_address_returns_none() {
        let db = MemKv::new();
        let addr = [0xFF; 32];
        let result = lookup_ai_entity_by_address(&db, &addr).unwrap();
        assert!(result.is_none());
    }

    // ========================================================================
    // Delegation-aware capability resolution (Feature 8)
    // ========================================================================

    fn make_entity(code_byte: u8, creator_byte: u8, caps: Capabilities) -> AiEntity {
        AiEntity::new(
            [code_byte; 32],
            [creator_byte; 32],
            AutonomyMode::Gated,
            caps,
            0,
        )
    }

    fn write_entity(db: &mut MemKv, entity: &AiEntity) {
        db.apply_batch(&[write_ai_entity_op(entity)]).unwrap();
    }

    fn write_grant(
        db: &mut MemKv,
        delegator_id: &[u8; 32],
        delegate_id: &[u8; 32],
        granted_byte: u8,
        expires_at: u64,
        created_height: u64,
    ) -> [u8; 32] {
        let grant = DelegationGrantData {
            version: novai_ai_entities::DELEGATION_GRANT_VERSION,
            delegate_entity_id: *delegate_id,
            granted_capabilities: granted_byte,
            expires_at,
        };
        let payload = grant.encode().to_vec();
        let memobj = MemoryObject::new(
            *delegator_id,
            MemoryObjectType::DelegationGrant,
            created_height,
            payload,
        );
        let grant_id = memobj.object_id;
        let primary_key = ai_memory_object_key(delegator_id, &grant_id);
        let primary_val = encode_memory_object_v1(&memobj);
        let index_key = novai_state::ai_delegation_by_delegate_key(delegate_id, &grant_id);
        db.apply_batch(&[
            WriteOp::Put(primary_key, primary_val),
            WriteOp::Put(index_key, delegator_id.to_vec()),
        ])
        .unwrap();
        grant_id
    }

    #[test]
    fn requires_capability_fast_path_when_static_present() {
        let db = MemKv::new();
        let entity = make_entity(0x10, 0x01, Capabilities::advisory());
        let r = requires_capability(&db, &entity, 100, |c| c.emit_proposals);
        assert!(r.is_ok(), "static capability should pass the fast path");
    }

    #[test]
    fn requires_capability_rejects_when_static_missing_and_no_grants() {
        let db = MemKv::new();
        let entity = make_entity(0x11, 0x01, Capabilities::read_only());
        let r = requires_capability(&db, &entity, 100, |c| c.emit_proposals);
        assert!(matches!(r, Err(ExecError::IssuerMissingCapability)));
    }

    #[test]
    fn resolve_returns_static_when_no_grants() {
        let db = MemKv::new();
        let entity = make_entity(0x12, 0x01, Capabilities::read_only());
        let resolved = resolve_effective_capabilities(&db, &entity, 100).unwrap();
        assert_eq!(resolved.to_byte(), entity.capabilities.to_byte());
    }

    #[test]
    fn resolve_active_grant_extends_capability() {
        let mut db = MemKv::new();
        let delegator = make_entity(0x21, 0x01, Capabilities::advisory());
        let delegate = make_entity(0x22, 0x02, Capabilities::read_only());
        write_entity(&mut db, &delegator);
        write_entity(&mut db, &delegate);
        write_grant(&mut db, &delegator.id, &delegate.id, 0x04, 0, 1);

        let resolved = resolve_effective_capabilities(&db, &delegate, 100).unwrap();
        assert!(resolved.emit_proposals);
        assert!(!delegate.capabilities.emit_proposals);
    }

    #[test]
    fn resolve_expired_grant_does_not_extend() {
        let mut db = MemKv::new();
        let delegator = make_entity(0x31, 0x01, Capabilities::advisory());
        let delegate = make_entity(0x32, 0x02, Capabilities::read_only());
        write_entity(&mut db, &delegator);
        write_entity(&mut db, &delegate);
        write_grant(&mut db, &delegator.id, &delegate.id, 0x04, 50, 1);

        let resolved = resolve_effective_capabilities(&db, &delegate, 50).unwrap();
        assert!(
            !resolved.emit_proposals,
            "current_height >= expires_at must skip the grant"
        );
        let resolved_before = resolve_effective_capabilities(&db, &delegate, 49).unwrap();
        assert!(resolved_before.emit_proposals);
    }

    #[test]
    fn resolve_grant_from_inactive_delegator_is_skipped() {
        let mut db = MemKv::new();
        let mut delegator = make_entity(0x41, 0x01, Capabilities::advisory());
        delegator.is_active = false;
        let delegate = make_entity(0x42, 0x02, Capabilities::read_only());
        write_entity(&mut db, &delegator);
        write_entity(&mut db, &delegate);
        write_grant(&mut db, &delegator.id, &delegate.id, 0x04, 0, 1);

        let resolved = resolve_effective_capabilities(&db, &delegate, 100).unwrap();
        assert!(!resolved.emit_proposals);
    }

    #[test]
    fn resolve_combines_multiple_grants() {
        let mut db = MemKv::new();
        let delegator_a = make_entity(0x51, 0x01, Capabilities::advisory());
        let sub_caps = Capabilities {
            read_public_chain: true,
            submit_reputation_updates: true,
            ..Capabilities::default()
        };
        let delegator_b = make_entity(0x52, 0x02, sub_caps);
        let delegate = make_entity(0x53, 0x03, Capabilities::read_only());
        write_entity(&mut db, &delegator_a);
        write_entity(&mut db, &delegator_b);
        write_entity(&mut db, &delegate);
        write_grant(&mut db, &delegator_a.id, &delegate.id, 0x04, 0, 1);
        write_grant(&mut db, &delegator_b.id, &delegate.id, 0x20, 0, 2);

        let resolved = resolve_effective_capabilities(&db, &delegate, 100).unwrap();
        assert!(resolved.emit_proposals);
        assert!(resolved.submit_reputation_updates);
        assert!(resolved.read_public_chain, "static cap preserved");
    }

    #[test]
    fn requires_capability_via_delegation_succeeds() {
        let mut db = MemKv::new();
        let delegator = make_entity(0x61, 0x01, Capabilities::advisory());
        let delegate = make_entity(0x62, 0x02, Capabilities::read_only());
        write_entity(&mut db, &delegator);
        write_entity(&mut db, &delegate);
        write_grant(&mut db, &delegator.id, &delegate.id, 0x04, 0, 1);

        let r = requires_capability(&db, &delegate, 100, |c| c.emit_proposals);
        assert!(r.is_ok(), "delegated emit_proposals should pass");
    }

    #[test]
    fn resolve_skips_corrupt_index_value_length() {
        let mut db = MemKv::new();
        let delegate = make_entity(0x71, 0x02, Capabilities::read_only());
        write_entity(&mut db, &delegate);

        let mut bad_key = ai_delegations_by_delegate_prefix(&delegate.id);
        bad_key.extend_from_slice(&[0xCDu8; 32]);
        db.apply_batch(&[WriteOp::Put(bad_key, vec![0u8; 8])])
            .unwrap();

        let resolved = resolve_effective_capabilities(&db, &delegate, 100).unwrap();
        assert_eq!(resolved.to_byte(), delegate.capabilities.to_byte());
    }

    #[test]
    fn resolve_grant_for_no_expiry_remains_active_at_high_heights() {
        let mut db = MemKv::new();
        let delegator = make_entity(0x81, 0x01, Capabilities::advisory());
        let delegate = make_entity(0x82, 0x02, Capabilities::read_only());
        write_entity(&mut db, &delegator);
        write_entity(&mut db, &delegate);
        write_grant(&mut db, &delegator.id, &delegate.id, 0x04, 0, 1);

        let resolved = resolve_effective_capabilities(&db, &delegate, u64::MAX - 1).unwrap();
        assert!(resolved.emit_proposals);
    }

    // ========================================================================
    // validate_delegation_grant_payload tests (Feature 8 / Phase 6)
    // ========================================================================

    fn make_grant_bytes(delegate_id: [u8; 32], granted: u8, expires_at: u64) -> Vec<u8> {
        let g = DelegationGrantData {
            version: novai_ai_entities::DELEGATION_GRANT_VERSION,
            delegate_entity_id: delegate_id,
            granted_capabilities: granted,
            expires_at,
        };
        g.encode().to_vec()
    }

    #[test]
    fn validate_grant_passes_for_subset_capabilities() {
        let db = MemKv::new();
        let delegator = make_entity(0xA1, 0x01, Capabilities::advisory()); // 0x07
        let delegate = make_entity(0xA2, 0x02, Capabilities::read_only());
        let bytes = make_grant_bytes(delegate.id, 0x04, 0); // emit_proposals subset
        let r = validate_delegation_grant_payload::<MemKv>(
            &db,
            MemoryObjectType::DelegationGrant,
            &bytes,
            &delegator,
        );
        assert!(matches!(r, Ok(Some(_))));
    }

    #[test]
    fn validate_grant_no_op_for_other_object_types() {
        let db = MemKv::new();
        let delegator = make_entity(0xA3, 0x01, Capabilities::advisory());
        let r = validate_delegation_grant_payload::<MemKv>(
            &db,
            MemoryObjectType::ChainSummary,
            &[0u8; 16],
            &delegator,
        );
        assert!(matches!(r, Ok(None)));
    }

    #[test]
    fn validate_grant_rejects_self_delegation() {
        let db = MemKv::new();
        let delegator = make_entity(0xA4, 0x01, Capabilities::advisory());
        let bytes = make_grant_bytes(delegator.id, 0x04, 0);
        let r = validate_delegation_grant_payload::<MemKv>(
            &db,
            MemoryObjectType::DelegationGrant,
            &bytes,
            &delegator,
        );
        assert!(matches!(r, Err(ExecError::InvalidDelegationSelf)));
    }

    #[test]
    fn validate_grant_rejects_superset_capability() {
        let db = MemKv::new();
        // delegator only has read_only (0x03); tries to grant emit_proposals (0x04).
        let delegator = make_entity(0xA5, 0x01, Capabilities::read_only());
        let delegate = make_entity(0xA6, 0x02, Capabilities::default());
        let bytes = make_grant_bytes(delegate.id, 0x04, 0);
        let r = validate_delegation_grant_payload::<MemKv>(
            &db,
            MemoryObjectType::DelegationGrant,
            &bytes,
            &delegator,
        );
        assert!(matches!(r, Err(ExecError::DelegationCapabilityNotHeld)));
    }

    #[test]
    fn validate_grant_rejects_bad_version_byte() {
        let db = MemKv::new();
        let delegator = make_entity(0xA7, 0x01, Capabilities::advisory());
        let mut bytes = make_grant_bytes([0xAAu8; 32], 0x04, 0);
        bytes[0] = 99; // wrong version
        let r = validate_delegation_grant_payload::<MemKv>(
            &db,
            MemoryObjectType::DelegationGrant,
            &bytes,
            &delegator,
        );
        assert!(matches!(r, Err(ExecError::InvalidDelegationGrant)));
    }

    #[test]
    fn validate_grant_rejects_at_max_count() {
        let mut db = MemKv::new();
        let delegator = make_entity(0xA8, 0x01, Capabilities::advisory());
        let delegate = make_entity(0xA9, 0x02, Capabilities::default());
        write_entity(&mut db, &delegator);
        write_entity(&mut db, &delegate);
        // Pre-populate the by-type index with MAX_DELEGATION_GRANTS sentinel
        // entries so the count check trips.
        for i in 0..MAX_DELEGATION_GRANTS {
            let mut fake_id = [0u8; 32];
            fake_id[..4].copy_from_slice(&i.to_be_bytes());
            let key = ai_memory_by_type_key(
                MemoryObjectType::DelegationGrant.to_byte(),
                &delegator.id,
                &fake_id,
            );
            db.apply_batch(&[WriteOp::Put(key, vec![])]).unwrap();
        }
        let bytes = make_grant_bytes(delegate.id, 0x04, 0);
        let r = validate_delegation_grant_payload::<MemKv>(
            &db,
            MemoryObjectType::DelegationGrant,
            &bytes,
            &delegator,
        );
        assert!(matches!(
            r,
            Err(ExecError::DelegationCountExceeded { current, max })
                if current == MAX_DELEGATION_GRANTS && max == MAX_DELEGATION_GRANTS
        ));
    }

    // ========================================================================
    // Week 28 - native x402 payment rail (Phase 1: types and constants)
    // ========================================================================

    fn make_payment_request_extra() -> PaymentRequestExtraV1 {
        PaymentRequestExtraV1 {
            payee_entity_id: [0xAAu8; 32],
            amount: 0x0102_0304_0506_0708,
            service_descriptor_hash: [0xBBu8; 32],
            request_hash: [0xCCu8; 32],
            max_block_height: 0x1112_1314_1516_1718,
        }
    }

    fn make_payment_request_payload(issuer: [u8; 32], signal_hash: [u8; 32]) -> Vec<u8> {
        let p = SignalCommitmentPayloadV1 {
            signal_hash,
            signal_type: novai_ai_entities::AiSignalType::PaymentRequest,
            issuer_entity_id: issuer,
            reputation: None,
            purchase: None,
            stake_deposit: None,
            stake_withdraw: None,
            stake_slash: None,
            composition_check: None,
            proof_submission: None,
            subscription_create: None,
            subscription_cancel: None,
            payment_request: Some(make_payment_request_extra()),
            service_attestation: None,
        };
        encode_signal_commitment_payload_v1(&p)
    }

    fn make_service_attestation_extra() -> ServiceAttestationExtraV1 {
        ServiceAttestationExtraV1 {
            payment_signal_hash: [0xDDu8; 32],
            payee_entity_id: [0xEEu8; 32],
            status: PAYMENT_ATTESTATION_STATUS_DELIVERED,
        }
    }

    fn make_service_attestation_payload(issuer: [u8; 32], signal_hash: [u8; 32]) -> Vec<u8> {
        let p = SignalCommitmentPayloadV1 {
            signal_hash,
            signal_type: novai_ai_entities::AiSignalType::ServiceAttestation,
            issuer_entity_id: issuer,
            reputation: None,
            purchase: None,
            stake_deposit: None,
            stake_withdraw: None,
            stake_slash: None,
            composition_check: None,
            proof_submission: None,
            subscription_create: None,
            subscription_cancel: None,
            payment_request: None,
            service_attestation: Some(make_service_attestation_extra()),
        };
        encode_signal_commitment_payload_v1(&p)
    }

    #[test]
    fn payment_request_payload_roundtrip() {
        let issuer = [0x11u8; 32];
        let signal_hash = [0x22u8; 32];
        let bytes = make_payment_request_payload(issuer, signal_hash);
        assert_eq!(
            bytes.len(),
            SIGNAL_COMMITMENT_PAYLOAD_V1_PAYMENT_REQUEST_LEN
        );
        assert_eq!(bytes.len(), 178);
        let decoded = decode_signal_commitment_payload_v1(&bytes).expect("decode succeeds");
        assert_eq!(decoded.signal_hash, signal_hash);
        assert_eq!(
            decoded.signal_type,
            novai_ai_entities::AiSignalType::PaymentRequest
        );
        assert_eq!(decoded.issuer_entity_id, issuer);
        let extra = decoded
            .payment_request
            .expect("payment_request tail present");
        assert_eq!(extra, make_payment_request_extra());
        assert!(decoded.service_attestation.is_none());
    }

    #[test]
    fn service_attestation_payload_roundtrip() {
        let issuer = [0x33u8; 32];
        let signal_hash = [0x44u8; 32];
        let bytes = make_service_attestation_payload(issuer, signal_hash);
        assert_eq!(
            bytes.len(),
            SIGNAL_COMMITMENT_PAYLOAD_V1_SERVICE_ATTESTATION_LEN
        );
        assert_eq!(bytes.len(), 131);
        let decoded = decode_signal_commitment_payload_v1(&bytes).expect("decode succeeds");
        assert_eq!(decoded.signal_hash, signal_hash);
        assert_eq!(
            decoded.signal_type,
            novai_ai_entities::AiSignalType::ServiceAttestation
        );
        assert_eq!(decoded.issuer_entity_id, issuer);
        let extra = decoded
            .service_attestation
            .expect("service_attestation tail present");
        assert_eq!(extra, make_service_attestation_extra());
        assert!(decoded.payment_request.is_none());
    }

    #[test]
    fn golden_vector_payment_request_payload_178_bytes() {
        let issuer = [0x55u8; 32];
        let signal_hash = [0x66u8; 32];
        let bytes = make_payment_request_payload(issuer, signal_hash);
        assert_eq!(bytes.len(), 178);
        assert_eq!(bytes[0], SIGNAL_COMMITMENT_PAYLOAD_V1);
        assert_eq!(&bytes[1..33], &signal_hash, "signal_hash at 1..33");
        assert_eq!(
            bytes[33],
            novai_ai_entities::AiSignalType::PaymentRequest.to_byte(),
            "signal_type byte = 16"
        );
        assert_eq!(bytes[33], 16);
        assert_eq!(&bytes[34..66], &issuer, "issuer_entity_id at 34..66");
        assert_eq!(&bytes[66..98], &[0xAAu8; 32], "payee_entity_id at 66..98");
        assert_eq!(
            &bytes[98..106],
            &0x0102_0304_0506_0708u64.to_be_bytes(),
            "amount_be at 98..106"
        );
        assert_eq!(
            &bytes[106..138],
            &[0xBBu8; 32],
            "service_descriptor_hash at 106..138"
        );
        assert_eq!(&bytes[138..170], &[0xCCu8; 32], "request_hash at 138..170");
        assert_eq!(
            &bytes[170..178],
            &0x1112_1314_1516_1718u64.to_be_bytes(),
            "max_block_height_be at 170..178"
        );
    }

    #[test]
    fn golden_vector_service_attestation_payload_131_bytes() {
        let issuer = [0x77u8; 32];
        let signal_hash = [0x88u8; 32];
        let bytes = make_service_attestation_payload(issuer, signal_hash);
        assert_eq!(bytes.len(), 131);
        assert_eq!(bytes[0], SIGNAL_COMMITMENT_PAYLOAD_V1);
        assert_eq!(&bytes[1..33], &signal_hash, "signal_hash at 1..33");
        assert_eq!(
            bytes[33],
            novai_ai_entities::AiSignalType::ServiceAttestation.to_byte(),
            "signal_type byte = 17"
        );
        assert_eq!(bytes[33], 17);
        assert_eq!(&bytes[34..66], &issuer, "issuer_entity_id at 34..66");
        assert_eq!(
            &bytes[66..98],
            &[0xDDu8; 32],
            "payment_signal_hash at 66..98"
        );
        assert_eq!(&bytes[98..130], &[0xEEu8; 32], "payee_entity_id at 98..130");
        assert_eq!(
            bytes[130], PAYMENT_ATTESTATION_STATUS_DELIVERED,
            "status byte at offset 130"
        );
    }

    #[test]
    fn payment_request_bad_length_rejected() {
        let issuer = [0x99u8; 32];
        let signal_hash = [0xAAu8; 32];
        let mut bytes = make_payment_request_payload(issuer, signal_hash);
        // Drop the last byte so length becomes 177 instead of 178.
        bytes.pop();
        let err = decode_signal_commitment_payload_v1(&bytes).unwrap_err();
        assert!(
            matches!(
                err,
                ExecError::BadPayloadLength {
                    expected: SIGNAL_COMMITMENT_PAYLOAD_V1_PAYMENT_REQUEST_LEN,
                    got: 177
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn service_attestation_bad_length_rejected() {
        let issuer = [0xBBu8; 32];
        let signal_hash = [0xCCu8; 32];
        let mut bytes = make_service_attestation_payload(issuer, signal_hash);
        // Append a stray byte so length becomes 132 instead of 131.
        bytes.push(0);
        let err = decode_signal_commitment_payload_v1(&bytes).unwrap_err();
        assert!(
            matches!(
                err,
                ExecError::BadPayloadLength {
                    expected: SIGNAL_COMMITMENT_PAYLOAD_V1_SERVICE_ATTESTATION_LEN,
                    got: 132
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn service_attestation_invalid_status_rejected_at_decode() {
        let issuer = [0xDDu8; 32];
        let signal_hash = [0xEEu8; 32];
        let mut bytes = make_service_attestation_payload(issuer, signal_hash);
        // Force the status byte to a value above PAYMENT_ATTESTATION_STATUS_MAX.
        bytes[130] = 99;
        let err = decode_signal_commitment_payload_v1(&bytes).unwrap_err();
        assert!(
            matches!(
                err,
                ExecError::ServiceAttestationInvalidStatus { status: 99 }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn payment_record_roundtrip() {
        let rec = PaymentRecord {
            payer: [0x01u8; 32],
            payee: [0x02u8; 32],
            amount: 0xDEAD_BEEF_CAFE_BABE,
            service_descriptor_hash: [0x03u8; 32],
            request_hash: [0x04u8; 32],
            payment_height: 12_345,
            max_block_height: 12_500,
            attested_status: PAYMENT_ATTESTATION_STATUS_NONE,
            attested_height: 0,
        };
        let bytes = encode_payment_record_v1(&rec);
        assert_eq!(bytes.len(), PAYMENT_RECORD_LEN);
        assert_eq!(bytes.len(), 162);
        let decoded = decode_payment_record_v1(&bytes).expect("decode succeeds");
        assert_eq!(decoded, rec);
    }

    #[test]
    fn payment_record_roundtrip_after_attestation() {
        let rec = PaymentRecord {
            payer: [0x05u8; 32],
            payee: [0x06u8; 32],
            amount: 1_000,
            service_descriptor_hash: [0x07u8; 32],
            request_hash: [0x08u8; 32],
            payment_height: 500,
            max_block_height: 600,
            attested_status: PAYMENT_ATTESTATION_STATUS_FAILED,
            attested_height: 510,
        };
        let bytes = encode_payment_record_v1(&rec);
        let decoded = decode_payment_record_v1(&bytes).expect("decode succeeds");
        assert_eq!(decoded, rec);
        assert_eq!(decoded.attested_status, PAYMENT_ATTESTATION_STATUS_FAILED);
        assert_eq!(decoded.attested_height, 510);
    }

    #[test]
    fn golden_vector_payment_record_162_bytes() {
        let rec = PaymentRecord {
            payer: [0xA1u8; 32],
            payee: [0xA2u8; 32],
            amount: 0x1122_3344_5566_7788,
            service_descriptor_hash: [0xA3u8; 32],
            request_hash: [0xA4u8; 32],
            payment_height: 0x01_02_03_04_05_06_07_08,
            max_block_height: 0x11_12_13_14_15_16_17_18,
            attested_status: PAYMENT_ATTESTATION_STATUS_DELIVERED,
            attested_height: 0x21_22_23_24_25_26_27_28,
        };
        let bytes = encode_payment_record_v1(&rec);
        assert_eq!(bytes.len(), 162);
        assert_eq!(bytes[0], PAYMENT_RECORD_V1);
        assert_eq!(&bytes[1..33], &[0xA1u8; 32], "payer at 1..33");
        assert_eq!(&bytes[33..65], &[0xA2u8; 32], "payee at 33..65");
        assert_eq!(
            &bytes[65..73],
            &0x1122_3344_5566_7788u64.to_be_bytes(),
            "amount_be at 65..73"
        );
        assert_eq!(
            &bytes[73..105],
            &[0xA3u8; 32],
            "service_descriptor_hash at 73..105"
        );
        assert_eq!(&bytes[105..137], &[0xA4u8; 32], "request_hash at 105..137");
        assert_eq!(
            &bytes[137..145],
            &0x01_02_03_04_05_06_07_08u64.to_be_bytes(),
            "payment_height_be at 137..145"
        );
        assert_eq!(
            &bytes[145..153],
            &0x11_12_13_14_15_16_17_18u64.to_be_bytes(),
            "max_block_height_be at 145..153"
        );
        assert_eq!(
            bytes[153], PAYMENT_ATTESTATION_STATUS_DELIVERED,
            "attested_status at 153"
        );
        assert_eq!(
            &bytes[154..162],
            &0x21_22_23_24_25_26_27_28u64.to_be_bytes(),
            "attested_height_be at 154..162"
        );
    }

    #[test]
    fn payment_record_decode_rejects_wrong_length() {
        let mut bytes = encode_payment_record_v1(&PaymentRecord {
            payer: [0u8; 32],
            payee: [0u8; 32],
            amount: 0,
            service_descriptor_hash: [0u8; 32],
            request_hash: [0u8; 32],
            payment_height: 0,
            max_block_height: 0,
            attested_status: PAYMENT_ATTESTATION_STATUS_NONE,
            attested_height: 0,
        })
        .to_vec();
        bytes.push(0);
        let err = decode_payment_record_v1(&bytes).unwrap_err();
        assert!(
            matches!(
                err,
                ExecError::BadPayloadLength {
                    expected: PAYMENT_RECORD_LEN,
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn payment_record_decode_rejects_wrong_version() {
        let rec = PaymentRecord {
            payer: [0u8; 32],
            payee: [0u8; 32],
            amount: 0,
            service_descriptor_hash: [0u8; 32],
            request_hash: [0u8; 32],
            payment_height: 0,
            max_block_height: 0,
            attested_status: PAYMENT_ATTESTATION_STATUS_NONE,
            attested_height: 0,
        };
        let mut bytes = encode_payment_record_v1(&rec);
        bytes[0] = 99;
        let err = decode_payment_record_v1(&bytes).unwrap_err();
        assert!(
            matches!(
                err,
                ExecError::BadPayloadVersion {
                    expected: PAYMENT_RECORD_V1,
                    got: 99
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    #[allow(clippy::similar_names)]
    fn payment_kv_keys_are_deterministic_and_prefixed() {
        let payer = [0x11u8; 32];
        let payee = [0x22u8; 32];
        let signal_hash = [0x33u8; 32];
        let height = 0x44_45_46_47_48_49_4A_4Bu64;

        let by_hash = payment_by_hash_key(&signal_hash);
        let by_payer = payment_by_payer_key(&payer, height, &signal_hash);
        let by_payee = payment_by_payee_key(&payee, height, &signal_hash);

        // Determinism: same inputs produce identical key bytes.
        assert_eq!(by_hash, payment_by_hash_key(&signal_hash));
        assert_eq!(by_payer, payment_by_payer_key(&payer, height, &signal_hash));
        assert_eq!(by_payee, payment_by_payee_key(&payee, height, &signal_hash));

        // Prefix correctness.
        assert!(by_hash.starts_with(KEY_PREFIX_AI_PAYMENTS_BY_HASH));
        assert!(by_payer.starts_with(KEY_PREFIX_AI_PAYMENTS_BY_PAYER));
        assert!(by_payee.starts_with(KEY_PREFIX_AI_PAYMENTS_BY_PAYEE));

        // Layout: by_hash is prefix + 32 bytes.
        assert_eq!(by_hash.len(), KEY_PREFIX_AI_PAYMENTS_BY_HASH.len() + 32);
        assert_eq!(
            &by_hash[KEY_PREFIX_AI_PAYMENTS_BY_HASH.len()..],
            &signal_hash
        );

        // Layout: by_payer / by_payee are prefix + 32 + 8 + 32 (entity || height_be || hash).
        let by_payer_body = &by_payer[KEY_PREFIX_AI_PAYMENTS_BY_PAYER.len()..];
        assert_eq!(by_payer_body.len(), 32 + 8 + 32);
        assert_eq!(&by_payer_body[..32], &payer);
        assert_eq!(&by_payer_body[32..40], &height.to_be_bytes());
        assert_eq!(&by_payer_body[40..72], &signal_hash);

        let by_payee_body = &by_payee[KEY_PREFIX_AI_PAYMENTS_BY_PAYEE.len()..];
        assert_eq!(by_payee_body.len(), 32 + 8 + 32);
        assert_eq!(&by_payee_body[..32], &payee);
        assert_eq!(&by_payee_body[32..40], &height.to_be_bytes());
        assert_eq!(&by_payee_body[40..72], &signal_hash);

        // Scan-order property: same (entity, signal_hash), earlier height
        // sorts before later height under lexicographic comparison.
        let earlier = payment_by_payer_key(&payer, height - 1, &signal_hash);
        assert!(earlier < by_payer, "earlier height must sort before later");
    }

    #[test]
    fn payment_rail_constants_have_expected_values() {
        // Lock the wire-level layout sizes so accidental encoding changes
        // are caught at the type level.
        assert_eq!(PAYMENT_REQUEST_EXTRA_LEN, 112);
        assert_eq!(SERVICE_ATTESTATION_EXTRA_LEN, 65);
        assert_eq!(SIGNAL_COMMITMENT_PAYLOAD_V1_PAYMENT_REQUEST_LEN, 178);
        assert_eq!(SIGNAL_COMMITMENT_PAYLOAD_V1_SERVICE_ATTESTATION_LEN, 131);
        assert_eq!(PAYMENT_RECORD_LEN, 162);

        // Fee identical to MARKETPLACE_FEE_BPS for v1, but tracked
        // independently to allow future tuning.
        assert_eq!(PAYMENT_FEE_BPS, 200);
        assert_eq!(PAYMENT_FEE_BPS, MARKETPLACE_FEE_BPS);

        // Reputation event discriminants and bounds.
        assert_eq!(REP_EVENT_PAYMENT_DELIVERED, 10);
        assert_eq!(REP_EVENT_PAYMENT_FAILED, 11);
        assert_eq!(REP_EVENT_MAX, REP_EVENT_PAYMENT_FAILED);

        // Calibrated against existing magnitudes: PROOF_VERIFIED inlines
        // +3, COMPOSITION_FAILURE inlines -1. Payment attestations sit
        // between these: smaller positive (+1) than a verified proof,
        // larger negative (-3) than a composition failure.
        assert_eq!(REP_DELTA_PAYMENT_DELIVERED, 1);
        assert_eq!(REP_DELTA_PAYMENT_FAILED, -3);

        // Status discriminants.
        assert_eq!(PAYMENT_ATTESTATION_STATUS_DELIVERED, 0);
        assert_eq!(PAYMENT_ATTESTATION_STATUS_FAILED, 1);
        assert_eq!(
            PAYMENT_ATTESTATION_STATUS_MAX,
            PAYMENT_ATTESTATION_STATUS_FAILED
        );
        assert_eq!(PAYMENT_ATTESTATION_STATUS_NONE, 0xFF);
        // 0xFF is unambiguously outside the valid status range
        // [0, PAYMENT_ATTESTATION_STATUS_MAX=1], guaranteeing the
        // "no-attestation" sentinel can never collide with a real status.

        // Storage prefixes.
        assert_eq!(KEY_PREFIX_AI_PAYMENTS_BY_HASH, b"ai/payments/by_hash/");
        assert_eq!(KEY_PREFIX_AI_PAYMENTS_BY_PAYER, b"ai/payments/by_payer/");
        assert_eq!(KEY_PREFIX_AI_PAYMENTS_BY_PAYEE, b"ai/payments/by_payee/");

        assert_eq!(PAYMENT_RECORD_V1, 1);
    }

    #[test]
    fn signal_type_byte_17_decodes_to_service_attestation_from_execution_view() {
        // Smoke test that the execution crate's view of AiSignalType
        // matches the ai_entities crate after the Phase 1 enum bump.
        assert_eq!(
            novai_ai_entities::AiSignalType::from_byte(16),
            Some(novai_ai_entities::AiSignalType::PaymentRequest),
        );
        assert_eq!(
            novai_ai_entities::AiSignalType::from_byte(17),
            Some(novai_ai_entities::AiSignalType::ServiceAttestation),
        );
        assert_eq!(novai_ai_entities::AiSignalType::from_byte(18), None);
    }
}
