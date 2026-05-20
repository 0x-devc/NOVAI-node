//! Signal commands: publish, by-height, by-issuer, by-type.

use crate::commands::account::sign_and_submit;
use crate::commands::keygen::parse_hex32;
use crate::rpc_client::RpcClient;
use clap::Args;
use novai_ai_entities::AiSignalType;

/// Optional extended-payload arguments for signal types 7-15.
///
/// Required flags depend on signal type; see per-field docs for which
/// signal type each flag applies to. Signal types 0-6 (the original seven)
/// take no extended flags.
#[derive(Args, Default, Debug, Clone)]
pub struct ExtendedSignalArgs {
    /// Hex-encoded 32-byte target entity ID (reputation-update, stake-slash, composition-check).
    #[arg(long)]
    pub target_entity_id: Option<String>,

    /// Reputation event type byte (reputation-update, stake-slash).
    #[arg(long)]
    pub event_type: Option<u8>,

    /// Reputation points delta (reputation-update, stake-slash).
    #[arg(long, allow_hyphen_values = true)]
    pub points_delta: Option<i16>,

    /// Hex-encoded 32-byte seller entity ID (signal-purchase).
    #[arg(long)]
    pub seller_entity_id: Option<String>,

    /// Purchased signal type byte (signal-purchase).
    #[arg(long)]
    pub purchased_signal_type: Option<u8>,

    /// Maximum purchase price (signal-purchase).
    #[arg(long)]
    pub max_price: Option<u64>,

    /// Stake deposit amount (stake-deposit).
    #[arg(long)]
    pub stake_amount: Option<u128>,

    /// Stake withdrawal amount (stake-withdraw).
    #[arg(long)]
    pub withdraw_amount: Option<u128>,

    /// Slash amount (stake-slash).
    #[arg(long)]
    pub slash_amount: Option<u128>,

    /// Index of the failed dependency (composition-check).
    #[arg(long)]
    pub failed_dependency_idx: Option<u8>,

    /// Failure reason byte (composition-check).
    #[arg(long)]
    pub failure_reason: Option<u8>,

    /// Proof type byte (proof-submission).
    #[arg(long)]
    pub proof_type: Option<u8>,

    /// Hex-encoded 32-byte code hash (proof-submission).
    #[arg(long)]
    pub code_hash: Option<String>,

    /// Hex-encoded 32-byte computation hash (proof-submission).
    #[arg(long)]
    pub computation_hash: Option<String>,

    /// Hex-encoded 32-byte producer entity ID (subscription-create).
    #[arg(long)]
    pub producer_entity_id: Option<String>,

    /// Covered signal type byte (subscription-create); identifies which
    /// producer signal type the subscription pays for.
    #[arg(long)]
    pub covered_signal_type: Option<u8>,

    /// Per-block payment rate in base units (subscription-create).
    #[arg(long)]
    pub rate_per_block: Option<u64>,

    /// Subscription duration in blocks (subscription-create).
    #[arg(long)]
    pub duration_blocks: Option<u64>,

    /// Hex-encoded 32-byte subscription memory object ID (subscription-cancel).
    #[arg(long)]
    pub subscription_id: Option<String>,

    /// Hex-encoded 32-byte payee entity ID (payment-request, service-attestation).
    #[arg(long)]
    pub payee_entity_id: Option<String>,

    /// Payment amount in base units of `economic_balance` (payment-request).
    #[arg(long)]
    pub payment_amount: Option<u64>,

    /// Hex-encoded 32-byte opaque service identifier (payment-request).
    #[arg(long)]
    pub service_descriptor_hash: Option<String>,

    /// Hex-encoded 32-byte opaque per-request commitment (payment-request).
    #[arg(long)]
    pub request_hash: Option<String>,

    /// Absolute block height past which the payment is invalid (payment-request).
    #[arg(long)]
    pub max_block_height: Option<u64>,

    /// Hex-encoded 32-byte signal hash of the PaymentRequest being attested
    /// (service-attestation).
    #[arg(long)]
    pub payment_signal_hash: Option<String>,

    /// Attestation status byte (service-attestation): 0=delivered, 1=failed.
    #[arg(long)]
    pub attestation_status: Option<u8>,

    /// Hex-encoded 32-byte VK registry handle (proof-submission, when
    /// proof_type = 3 = PROOF_TYPE_GROTH16_REGISTERED). Resolves the
    /// VK bytes on-chain via the `ai/vk_registry/by_id/` index;
    /// callers do not pass `--vk-file` in this mode.
    #[arg(long)]
    pub vk_id: Option<String>,

    /// Path to compressed verification-key bytes (proof-submission,
    /// when proof_type = 1 = PROOF_TYPE_GROTH16). Mutually exclusive
    /// with `--vk-id`.
    #[arg(long)]
    pub vk_file: Option<String>,

    /// Path to compressed proof bytes (proof-submission, for any
    /// real verifier proof_type). Required for proof_type = 1 or 3.
    #[arg(long)]
    pub proof_file: Option<String>,

    /// Hex-encoded 32-byte SLA memory object id (sla-accept). The
    /// signal issuer must equal the SLA's seller.
    #[arg(long)]
    pub sla_object_id: Option<String>,

    /// Hex-encoded 32-byte buyer entity id (sla-accept). The buyer
    /// owns the SLA memory object; supplying the id here lets the
    /// handler load it without a global scan.
    #[arg(long)]
    pub buyer_entity_id: Option<String>,
}

/// Parse signal type string.
fn parse_signal_type(s: &str) -> Result<AiSignalType, String> {
    match s {
        "anomaly" => Ok(AiSignalType::Anomaly),
        "optimization" => Ok(AiSignalType::Optimization),
        "prediction" => Ok(AiSignalType::Prediction),
        "risk-score" => Ok(AiSignalType::RiskScore),
        "audit-report" => Ok(AiSignalType::AuditReport),
        "spam-risk" => Ok(AiSignalType::SpamRisk),
        "congestion-forecast" => Ok(AiSignalType::CongestionForecast),
        "reputation-update" => Ok(AiSignalType::ReputationUpdate),
        "signal-purchase" => Ok(AiSignalType::SignalPurchase),
        "stake-deposit" => Ok(AiSignalType::StakeDeposit),
        "stake-withdraw" => Ok(AiSignalType::StakeWithdraw),
        "stake-slash" => Ok(AiSignalType::StakeSlash),
        "composition-check" => Ok(AiSignalType::CompositionCheck),
        "proof-submission" => Ok(AiSignalType::ProofSubmission),
        "subscription-create" => Ok(AiSignalType::SubscriptionCreate),
        "subscription-cancel" => Ok(AiSignalType::SubscriptionCancel),
        "payment-request" => Ok(AiSignalType::PaymentRequest),
        "service-attestation" => Ok(AiSignalType::ServiceAttestation),
        "sla-accept" => Ok(AiSignalType::SlaAccept),
        _ => Err(format!(
            "Unknown signal type '{s}'. Valid: anomaly, optimization, prediction, risk-score, audit-report, spam-risk, congestion-forecast, reputation-update, signal-purchase, stake-deposit, stake-withdraw, stake-slash, composition-check, proof-submission, subscription-create, subscription-cancel, payment-request, service-attestation, sla-accept"
        )),
    }
}

fn require_str<'a>(
    value: &'a Option<String>,
    flag: &str,
    sig_type: &str,
) -> Result<&'a str, String> {
    value
        .as_deref()
        .ok_or_else(|| format!("{flag} is required for signal type {sig_type}"))
}

fn require_some<T>(value: Option<T>, flag: &str, sig_type: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("{flag} is required for signal type {sig_type}"))
}

/// Proof-system discriminants the CLI knows about. Duplicates the
/// runtime constants in `novai_execution` so the signal binary does not
/// have to depend on the execution crate just for three byte literals.
const PROOF_TYPE_STUB: u8 = 0;
const PROOF_TYPE_GROTH16: u8 = 1;
const PROOF_TYPE_GROTH16_REGISTERED: u8 = 3;

/// Append the v2 ProofSubmission tail (`vk_len_be:4 | vk_bytes |
/// proof_len_be:4 | proof_bytes`) to `payload`, deriving the contents
/// from `extra` according to `proof_type`.
///
/// - `PROOF_TYPE_STUB` (0): no tail (v1 layout); validates that the
///   v2-only flags are not set so a typo surfaces early instead of
///   producing a confusingly large v2 payload.
/// - `PROOF_TYPE_GROTH16` (1): require `--vk-file` and `--proof-file`;
///   inline the bytes verbatim.
/// - `PROOF_TYPE_GROTH16_REGISTERED` (3): require `--vk-id` and
///   `--proof-file`; vk_bytes is exactly the 32-byte registry handle.
fn append_proof_submission_v2_tail(
    payload: &mut Vec<u8>,
    proof_type: u8,
    extra: &ExtendedSignalArgs,
) -> Result<(), String> {
    match proof_type {
        PROOF_TYPE_STUB => {
            if extra.vk_id.is_some() || extra.vk_file.is_some() || extra.proof_file.is_some() {
                return Err(
                    "proof_type 0 (stub) does not accept --vk-id / --vk-file / --proof-file"
                        .to_string(),
                );
            }
            Ok(())
        }
        PROOF_TYPE_GROTH16 => {
            if extra.vk_id.is_some() {
                return Err(
                    "--vk-id only applies to proof_type 3 (groth16-registered); use --vk-file with proof_type 1"
                        .to_string(),
                );
            }
            let vk_path = require_str(&extra.vk_file, "--vk-file", "proof-submission")?;
            let proof_path = require_str(&extra.proof_file, "--proof-file", "proof-submission")?;
            let vk_bytes = std::fs::read(vk_path)
                .map_err(|e| format!("Failed to read VK file '{vk_path}': {e}"))?;
            let proof_bytes = std::fs::read(proof_path)
                .map_err(|e| format!("Failed to read proof file '{proof_path}': {e}"))?;
            append_v2_tail(payload, &vk_bytes, &proof_bytes)
        }
        PROOF_TYPE_GROTH16_REGISTERED => {
            if extra.vk_file.is_some() {
                return Err(
                    "--vk-file does not apply to proof_type 3 (groth16-registered); use --vk-id"
                        .to_string(),
                );
            }
            let vk_id_hex = require_str(&extra.vk_id, "--vk-id", "proof-submission")?;
            let vk_id = parse_hex32(vk_id_hex, "vk_id")?;
            let proof_path = require_str(&extra.proof_file, "--proof-file", "proof-submission")?;
            let proof_bytes = std::fs::read(proof_path)
                .map_err(|e| format!("Failed to read proof file '{proof_path}': {e}"))?;
            append_v2_tail(payload, &vk_id, &proof_bytes)
        }
        other => Err(format!(
            "proof_type {other} is not supported. Valid: 0 (stub), 1 (groth16), 3 (groth16-registered)"
        )),
    }
}

fn append_v2_tail(payload: &mut Vec<u8>, vk: &[u8], proof: &[u8]) -> Result<(), String> {
    let vk_len = u32::try_from(vk.len()).map_err(|_| "vk length overflows u32".to_string())?;
    let proof_len =
        u32::try_from(proof.len()).map_err(|_| "proof length overflows u32".to_string())?;
    payload.extend_from_slice(&vk_len.to_be_bytes());
    payload.extend_from_slice(vk);
    payload.extend_from_slice(&proof_len.to_be_bytes());
    payload.extend_from_slice(proof);
    Ok(())
}

/// Build the signal commitment payload bytes.
///
/// Layout: `[2][signal_hash:32][signal_type:1][issuer_entity_id:32]` (66 bytes)
/// optionally followed by a type-specific tail for signal types 7-13.
fn build_signal_payload(
    signal_hash: [u8; 32],
    signal_type: AiSignalType,
    issuer_entity_id: [u8; 32],
    extra: &ExtendedSignalArgs,
) -> Result<Vec<u8>, String> {
    let mut payload = Vec::with_capacity(131);
    payload.push(2);
    payload.extend_from_slice(&signal_hash);
    payload.push(signal_type.to_byte());
    payload.extend_from_slice(&issuer_entity_id);

    match signal_type {
        AiSignalType::Anomaly
        | AiSignalType::Optimization
        | AiSignalType::Prediction
        | AiSignalType::RiskScore
        | AiSignalType::AuditReport
        | AiSignalType::SpamRisk
        | AiSignalType::CongestionForecast => {}
        AiSignalType::ReputationUpdate => {
            let target = parse_hex32(
                require_str(
                    &extra.target_entity_id,
                    "--target-entity-id",
                    "reputation-update",
                )?,
                "target_entity_id",
            )?;
            let event_type = require_some(extra.event_type, "--event-type", "reputation-update")?;
            let points_delta =
                require_some(extra.points_delta, "--points-delta", "reputation-update")?;
            payload.extend_from_slice(&target);
            payload.push(event_type);
            payload.extend_from_slice(&points_delta.to_be_bytes());
        }
        AiSignalType::SignalPurchase => {
            let seller = parse_hex32(
                require_str(
                    &extra.seller_entity_id,
                    "--seller-entity-id",
                    "signal-purchase",
                )?,
                "seller_entity_id",
            )?;
            let purchased = require_some(
                extra.purchased_signal_type,
                "--purchased-signal-type",
                "signal-purchase",
            )?;
            let max_price = require_some(extra.max_price, "--max-price", "signal-purchase")?;
            payload.extend_from_slice(&seller);
            payload.push(purchased);
            payload.extend_from_slice(&max_price.to_be_bytes());
        }
        AiSignalType::StakeDeposit => {
            let amount = require_some(extra.stake_amount, "--stake-amount", "stake-deposit")?;
            payload.extend_from_slice(&amount.to_be_bytes());
        }
        AiSignalType::StakeWithdraw => {
            let amount =
                require_some(extra.withdraw_amount, "--withdraw-amount", "stake-withdraw")?;
            payload.extend_from_slice(&amount.to_be_bytes());
        }
        AiSignalType::StakeSlash => {
            let target = parse_hex32(
                require_str(&extra.target_entity_id, "--target-entity-id", "stake-slash")?,
                "target_entity_id",
            )?;
            let slash_amount = require_some(extra.slash_amount, "--slash-amount", "stake-slash")?;
            let event_type = require_some(extra.event_type, "--event-type", "stake-slash")?;
            let points_delta = require_some(extra.points_delta, "--points-delta", "stake-slash")?;
            payload.extend_from_slice(&target);
            payload.extend_from_slice(&slash_amount.to_be_bytes());
            payload.push(event_type);
            payload.extend_from_slice(&points_delta.to_be_bytes());
        }
        AiSignalType::CompositionCheck => {
            let target = parse_hex32(
                require_str(
                    &extra.target_entity_id,
                    "--target-entity-id",
                    "composition-check",
                )?,
                "target_entity_id",
            )?;
            let failed_idx = require_some(
                extra.failed_dependency_idx,
                "--failed-dependency-idx",
                "composition-check",
            )?;
            let reason = require_some(
                extra.failure_reason,
                "--failure-reason",
                "composition-check",
            )?;
            payload.extend_from_slice(&target);
            payload.push(failed_idx);
            payload.push(reason);
        }
        AiSignalType::ProofSubmission => {
            let proof_type = require_some(extra.proof_type, "--proof-type", "proof-submission")?;
            let code = parse_hex32(
                require_str(&extra.code_hash, "--code-hash", "proof-submission")?,
                "code_hash",
            )?;
            let computation = parse_hex32(
                require_str(
                    &extra.computation_hash,
                    "--computation-hash",
                    "proof-submission",
                )?,
                "computation_hash",
            )?;
            payload.push(proof_type);
            payload.extend_from_slice(&code);
            payload.extend_from_slice(&computation);
            append_proof_submission_v2_tail(&mut payload, proof_type, extra)?;
        }
        AiSignalType::SubscriptionCreate => {
            let producer = parse_hex32(
                require_str(
                    &extra.producer_entity_id,
                    "--producer-entity-id",
                    "subscription-create",
                )?,
                "producer_entity_id",
            )?;
            let covered = require_some(
                extra.covered_signal_type,
                "--covered-signal-type",
                "subscription-create",
            )?;
            let rate = require_some(
                extra.rate_per_block,
                "--rate-per-block",
                "subscription-create",
            )?;
            let duration = require_some(
                extra.duration_blocks,
                "--duration-blocks",
                "subscription-create",
            )?;
            payload.extend_from_slice(&producer);
            payload.push(covered);
            payload.extend_from_slice(&rate.to_be_bytes());
            payload.extend_from_slice(&duration.to_be_bytes());
        }
        AiSignalType::SubscriptionCancel => {
            let sub_id = parse_hex32(
                require_str(
                    &extra.subscription_id,
                    "--subscription-id",
                    "subscription-cancel",
                )?,
                "subscription_id",
            )?;
            payload.extend_from_slice(&sub_id);
        }
        AiSignalType::PaymentRequest => {
            let payee = parse_hex32(
                require_str(
                    &extra.payee_entity_id,
                    "--payee-entity-id",
                    "payment-request",
                )?,
                "payee_entity_id",
            )?;
            let amount = require_some(extra.payment_amount, "--payment-amount", "payment-request")?;
            let service_descriptor = parse_hex32(
                require_str(
                    &extra.service_descriptor_hash,
                    "--service-descriptor-hash",
                    "payment-request",
                )?,
                "service_descriptor_hash",
            )?;
            let request = parse_hex32(
                require_str(&extra.request_hash, "--request-hash", "payment-request")?,
                "request_hash",
            )?;
            let max_block_height = require_some(
                extra.max_block_height,
                "--max-block-height",
                "payment-request",
            )?;
            payload.extend_from_slice(&payee);
            payload.extend_from_slice(&amount.to_be_bytes());
            payload.extend_from_slice(&service_descriptor);
            payload.extend_from_slice(&request);
            payload.extend_from_slice(&max_block_height.to_be_bytes());
        }
        AiSignalType::ServiceAttestation => {
            let payment_signal_hash = parse_hex32(
                require_str(
                    &extra.payment_signal_hash,
                    "--payment-signal-hash",
                    "service-attestation",
                )?,
                "payment_signal_hash",
            )?;
            let payee = parse_hex32(
                require_str(
                    &extra.payee_entity_id,
                    "--payee-entity-id",
                    "service-attestation",
                )?,
                "payee_entity_id",
            )?;
            let status = require_some(
                extra.attestation_status,
                "--attestation-status",
                "service-attestation",
            )?;
            if status > 1 {
                return Err(format!(
                    "--attestation-status must be 0 (delivered) or 1 (failed); got {status}"
                ));
            }
            payload.extend_from_slice(&payment_signal_hash);
            payload.extend_from_slice(&payee);
            payload.push(status);
        }
        AiSignalType::SlaAccept => {
            let sla_object_id = parse_hex32(
                require_str(&extra.sla_object_id, "--sla-object-id", "sla-accept")?,
                "sla_object_id",
            )?;
            let buyer = parse_hex32(
                require_str(&extra.buyer_entity_id, "--buyer-entity-id", "sla-accept")?,
                "buyer_entity_id",
            )?;
            payload.extend_from_slice(&sla_object_id);
            payload.extend_from_slice(&buyer);
        }
        AiSignalType::ChannelAccept
        | AiSignalType::ChannelClose
        | AiSignalType::ChannelFinalize => {
            // Week 32 channel signals carry rich, signature-bearing
            // payloads that do not fit the generic --extra arg shape.
            // The dedicated `novai-cli channel ...` subcommand
            // (wired in Week 32 Phase 7) is the supported entry
            // point; the raw signal-publish path stays explicit
            // about that restriction until then.
            return Err(format!(
                "signal type {:?} must be published via `novai-cli channel ...`, not the raw `signal publish` path",
                signal_type
            ));
        }
    }

    Ok(payload)
}

/// Publish a signal commitment (payload type 2). Total payload length depends
/// on signal type: 66 bytes for types 0-6, larger for types 7-13.
#[allow(clippy::too_many_arguments)]
pub async fn run_publish(
    rpc: &RpcClient,
    key_file: &str,
    signal_hash_hex: &str,
    signal_type_str: &str,
    issuer_entity_id_hex: &str,
    fee: u64,
    extra: &ExtendedSignalArgs,
    json: bool,
) -> Result<(), String> {
    let signal_hash = parse_hex32(signal_hash_hex, "signal_hash")?;
    let signal_type = parse_signal_type(signal_type_str)?;
    let issuer_entity_id = parse_hex32(issuer_entity_id_hex, "issuer_entity_id")?;

    let payload = build_signal_payload(signal_hash, signal_type, issuer_entity_id, extra)?;

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "signal_type": signal_type_str,
                "issuer_entity_id": issuer_entity_id_hex,
            })
        );
    } else {
        println!("Signal commitment submitted");
        println!("Type:   {signal_type_str}");
        println!("Issuer: {issuer_entity_id_hex}");
        println!("TxID:   {txid}");
    }
    Ok(())
}

/// Print a list of signals.
fn print_signals(signals: &[serde_json::Value], json_mode: bool) {
    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "signals": signals })).unwrap()
        );
        return;
    }

    if signals.is_empty() {
        println!("No signals found");
        return;
    }

    println!(
        "{:<10}  {:>4}  {:<64}  {:<64}",
        "HEIGHT", "TYPE", "ISSUER", "COMMITMENT"
    );
    for s in signals {
        println!(
            "{:<10}  {:>4}  {:<64}  {:<64}",
            s["height"].as_u64().unwrap_or(0),
            s["signal_type"].as_u64().unwrap_or(0),
            s["issuer"].as_str().unwrap_or("?"),
            s["commitment_hash"].as_str().unwrap_or("?"),
        );
    }
    println!("\n{} signal(s)", signals.len());
}

/// Query signals by block height.
pub async fn run_by_height(rpc: &RpcClient, height: u64, json: bool) -> Result<(), String> {
    let signals = rpc.get_signals_by_height(height).await?;
    print_signals(&signals, json);
    Ok(())
}

/// Query signals by issuer.
pub async fn run_by_issuer(
    rpc: &RpcClient,
    issuer_hex: &str,
    start: u64,
    end: u64,
    json: bool,
) -> Result<(), String> {
    let signals = rpc.get_signals_by_issuer(issuer_hex, start, end).await?;
    print_signals(&signals, json);
    Ok(())
}

/// Query signals by type.
pub async fn run_by_type(
    rpc: &RpcClient,
    signal_type_str: &str,
    start: u64,
    end: u64,
    json: bool,
) -> Result<(), String> {
    let signal_type = parse_signal_type(signal_type_str)?;
    let signals = rpc
        .get_signals_by_type(signal_type.to_byte(), start, end)
        .await?;
    print_signals(&signals, json);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_extra() -> ExtendedSignalArgs {
        ExtendedSignalArgs {
            target_entity_id: Some("00".repeat(32)),
            event_type: Some(0),
            points_delta: Some(0),
            seller_entity_id: Some("00".repeat(32)),
            purchased_signal_type: Some(0),
            max_price: Some(0),
            stake_amount: Some(0),
            withdraw_amount: Some(0),
            slash_amount: Some(0),
            failed_dependency_idx: Some(0),
            failure_reason: Some(0),
            proof_type: Some(0),
            code_hash: Some("00".repeat(32)),
            computation_hash: Some("00".repeat(32)),
            producer_entity_id: Some("00".repeat(32)),
            covered_signal_type: Some(0),
            rate_per_block: Some(0),
            duration_blocks: Some(0),
            subscription_id: Some("00".repeat(32)),
            payee_entity_id: Some("00".repeat(32)),
            payment_amount: Some(0),
            service_descriptor_hash: Some("00".repeat(32)),
            request_hash: Some("00".repeat(32)),
            max_block_height: Some(0),
            payment_signal_hash: Some("00".repeat(32)),
            attestation_status: Some(0),
            vk_id: None,
            vk_file: None,
            proof_file: None,
            sla_object_id: Some("00".repeat(32)),
            buyer_entity_id: Some("00".repeat(32)),
        }
    }

    #[test]
    fn test_signal_payload_lengths_for_all_19_types() {
        let extra = full_extra();
        for (sig_type, expected_len) in [
            (AiSignalType::Anomaly, 66),
            (AiSignalType::Optimization, 66),
            (AiSignalType::Prediction, 66),
            (AiSignalType::RiskScore, 66),
            (AiSignalType::AuditReport, 66),
            (AiSignalType::SpamRisk, 66),
            (AiSignalType::CongestionForecast, 66),
            (AiSignalType::ReputationUpdate, 101),
            (AiSignalType::SignalPurchase, 107),
            (AiSignalType::StakeDeposit, 82),
            (AiSignalType::StakeWithdraw, 82),
            (AiSignalType::StakeSlash, 117),
            (AiSignalType::CompositionCheck, 100),
            (AiSignalType::ProofSubmission, 131),
            (AiSignalType::SubscriptionCreate, 115),
            (AiSignalType::SubscriptionCancel, 98),
            (AiSignalType::PaymentRequest, 178),
            (AiSignalType::ServiceAttestation, 131),
            (AiSignalType::SlaAccept, 130),
        ] {
            let payload = build_signal_payload([0u8; 32], sig_type, [0u8; 32], &extra).unwrap();
            assert_eq!(payload.len(), expected_len, "wrong length for {sig_type:?}");
        }
    }

    #[test]
    fn test_parse_signal_type_accepts_all_19_strings() {
        let cases = [
            ("anomaly", AiSignalType::Anomaly),
            ("optimization", AiSignalType::Optimization),
            ("prediction", AiSignalType::Prediction),
            ("risk-score", AiSignalType::RiskScore),
            ("audit-report", AiSignalType::AuditReport),
            ("spam-risk", AiSignalType::SpamRisk),
            ("congestion-forecast", AiSignalType::CongestionForecast),
            ("reputation-update", AiSignalType::ReputationUpdate),
            ("signal-purchase", AiSignalType::SignalPurchase),
            ("stake-deposit", AiSignalType::StakeDeposit),
            ("stake-withdraw", AiSignalType::StakeWithdraw),
            ("stake-slash", AiSignalType::StakeSlash),
            ("composition-check", AiSignalType::CompositionCheck),
            ("proof-submission", AiSignalType::ProofSubmission),
            ("subscription-create", AiSignalType::SubscriptionCreate),
            ("subscription-cancel", AiSignalType::SubscriptionCancel),
            ("payment-request", AiSignalType::PaymentRequest),
            ("service-attestation", AiSignalType::ServiceAttestation),
            ("sla-accept", AiSignalType::SlaAccept),
        ];
        for (s, expected) in cases {
            assert_eq!(parse_signal_type(s).unwrap(), expected, "for input '{s}'");
        }
    }

    #[test]
    fn test_missing_required_flag_returns_clear_error() {
        let mut extra = full_extra();
        extra.target_entity_id = None;
        let err =
            build_signal_payload([0u8; 32], AiSignalType::ReputationUpdate, [0u8; 32], &extra)
                .unwrap_err();
        assert!(err.contains("--target-entity-id"), "err = {err}");
        assert!(err.contains("reputation-update"), "err = {err}");
    }

    #[test]
    fn test_payment_request_missing_payee_returns_clear_error() {
        let mut extra = full_extra();
        extra.payee_entity_id = None;
        let err = build_signal_payload([0u8; 32], AiSignalType::PaymentRequest, [0u8; 32], &extra)
            .unwrap_err();
        assert!(err.contains("--payee-entity-id"), "err = {err}");
        assert!(err.contains("payment-request"), "err = {err}");
    }

    #[test]
    fn test_payment_request_payload_bytes_match_expected_layout() {
        // Verify the CLI-built payload matches the on-chain 178-byte layout
        // byte-for-byte (signal_hash, signal_type, issuer, then tail).
        let extra = ExtendedSignalArgs {
            payee_entity_id: Some("aa".repeat(32)),
            payment_amount: Some(0x0102_0304_0506_0708),
            service_descriptor_hash: Some("bb".repeat(32)),
            request_hash: Some("cc".repeat(32)),
            max_block_height: Some(0x1112_1314_1516_1718),
            ..full_extra()
        };
        let payload = build_signal_payload(
            [0x66u8; 32],
            AiSignalType::PaymentRequest,
            [0x55u8; 32],
            &extra,
        )
        .unwrap();
        assert_eq!(payload.len(), 178);
        assert_eq!(payload[0], 2);
        assert_eq!(&payload[1..33], &[0x66u8; 32]);
        assert_eq!(payload[33], 16);
        assert_eq!(&payload[34..66], &[0x55u8; 32]);
        assert_eq!(&payload[66..98], &[0xAAu8; 32]);
        assert_eq!(&payload[98..106], &0x0102_0304_0506_0708u64.to_be_bytes());
        assert_eq!(&payload[106..138], &[0xBBu8; 32]);
        assert_eq!(&payload[138..170], &[0xCCu8; 32]);
        assert_eq!(&payload[170..178], &0x1112_1314_1516_1718u64.to_be_bytes());
    }

    #[test]
    fn test_service_attestation_rejects_status_above_one() {
        let mut extra = full_extra();
        extra.attestation_status = Some(99);
        let err = build_signal_payload(
            [0u8; 32],
            AiSignalType::ServiceAttestation,
            [0u8; 32],
            &extra,
        )
        .unwrap_err();
        assert!(err.contains("--attestation-status"), "err = {err}");
        assert!(err.contains("99"), "err = {err}");
    }

    #[test]
    fn test_service_attestation_payload_bytes_match_expected_layout() {
        let extra = ExtendedSignalArgs {
            payment_signal_hash: Some("dd".repeat(32)),
            payee_entity_id: Some("ee".repeat(32)),
            attestation_status: Some(0),
            ..full_extra()
        };
        let payload = build_signal_payload(
            [0x88u8; 32],
            AiSignalType::ServiceAttestation,
            [0x77u8; 32],
            &extra,
        )
        .unwrap();
        assert_eq!(payload.len(), 131);
        assert_eq!(payload[0], 2);
        assert_eq!(&payload[1..33], &[0x88u8; 32]);
        assert_eq!(payload[33], 17);
        assert_eq!(&payload[34..66], &[0x77u8; 32]);
        assert_eq!(&payload[66..98], &[0xDDu8; 32]);
        assert_eq!(&payload[98..130], &[0xEEu8; 32]);
        assert_eq!(payload[130], 0);
    }

    #[test]
    fn test_proof_submission_registered_payload_layout() {
        // proof_type = 3 (GROTH16_REGISTERED): vk_bytes carries the
        // 32-byte registry handle from --vk-id; --vk-file must NOT be
        // supplied. We use a dummy proof file via the proof_bytes
        // field on extra (no actual file IO since the test bypasses
        // append_proof_submission_v2_tail by setting proof_file=None,
        // but the helper requires --proof-file). Use a temp file.
        let proof_path = std::env::temp_dir().join("test_proof_bytes.bin");
        std::fs::write(&proof_path, [0xABu8, 0xCD, 0xEF]).unwrap();

        let extra = ExtendedSignalArgs {
            proof_type: Some(3),
            code_hash: Some("c0".repeat(32)),
            computation_hash: Some("c2".repeat(32)),
            vk_id: Some("d1".repeat(32)),
            proof_file: Some(proof_path.to_string_lossy().into_owned()),
            ..full_extra()
        };
        let payload = build_signal_payload(
            [0xC4u8; 32],
            AiSignalType::ProofSubmission,
            [0xE1u8; 32],
            &extra,
        )
        .expect("build registered ProofSubmission payload");

        // Base header: 66 bytes (version + signal_hash + signal_type +
        // issuer_entity_id).
        assert_eq!(payload[0], 2);
        assert_eq!(&payload[1..33], &[0xC4u8; 32]);
        assert_eq!(payload[33], AiSignalType::ProofSubmission.to_byte());
        assert_eq!(&payload[34..66], &[0xE1u8; 32]);

        // ProofSubmission extra fixed part (65 bytes).
        assert_eq!(payload[66], PROOF_TYPE_GROTH16_REGISTERED);
        assert_eq!(&payload[67..99], &[0xC0u8; 32]);
        assert_eq!(&payload[99..131], &[0xC2u8; 32]);

        // v2 tail: vk_len_be(=32) | vk_id(32) | proof_len_be(=3) | proof(3).
        assert_eq!(&payload[131..135], &32u32.to_be_bytes());
        assert_eq!(&payload[135..167], &[0xD1u8; 32]);
        assert_eq!(&payload[167..171], &3u32.to_be_bytes());
        assert_eq!(&payload[171..174], &[0xABu8, 0xCD, 0xEF]);

        let _ = std::fs::remove_file(&proof_path);
    }

    #[test]
    fn test_proof_submission_registered_rejects_vk_file() {
        let extra = ExtendedSignalArgs {
            proof_type: Some(3),
            code_hash: Some("c0".repeat(32)),
            computation_hash: Some("c2".repeat(32)),
            vk_id: Some("d1".repeat(32)),
            vk_file: Some("/tmp/should-not-be-read".to_string()),
            proof_file: Some("/tmp/anything".to_string()),
            ..full_extra()
        };
        let err = build_signal_payload([0u8; 32], AiSignalType::ProofSubmission, [0u8; 32], &extra)
            .unwrap_err();
        assert!(err.contains("--vk-file"), "err = {err}");
        assert!(err.contains("groth16-registered"), "err = {err}");
    }

    #[test]
    fn test_proof_submission_groth16_rejects_vk_id() {
        let extra = ExtendedSignalArgs {
            proof_type: Some(1),
            code_hash: Some("c0".repeat(32)),
            computation_hash: Some("c2".repeat(32)),
            vk_id: Some("d1".repeat(32)),
            vk_file: Some("/tmp/anything".to_string()),
            proof_file: Some("/tmp/anything".to_string()),
            ..full_extra()
        };
        let err = build_signal_payload([0u8; 32], AiSignalType::ProofSubmission, [0u8; 32], &extra)
            .unwrap_err();
        assert!(err.contains("--vk-id"), "err = {err}");
        assert!(err.contains("proof_type 3"), "err = {err}");
    }

    #[test]
    fn test_proof_submission_stub_rejects_v2_flags() {
        let extra = ExtendedSignalArgs {
            proof_type: Some(0),
            code_hash: Some("c0".repeat(32)),
            computation_hash: Some("c2".repeat(32)),
            proof_file: Some("/tmp/anything".to_string()),
            ..full_extra()
        };
        let err = build_signal_payload([0u8; 32], AiSignalType::ProofSubmission, [0u8; 32], &extra)
            .unwrap_err();
        assert!(err.contains("stub"), "err = {err}");
        assert!(err.contains("--proof-file"), "err = {err}");
    }
}
