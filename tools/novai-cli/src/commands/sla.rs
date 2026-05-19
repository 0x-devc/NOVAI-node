//! SLA Agreement commands: propose, accept, cancel, show, active,
//! list-by-buyer, list-by-seller (Week 31).
//!
//! Propose / cancel wrap the existing `CreateMemoryObject` /
//! `DeleteMemoryObject` signal payloads so operators do not have to
//! hand-roll the 210-byte `SlaAgreementData`. Acceptance is the new
//! `SlaAccept` signal type 18. Lookups route through the four
//! `novai_getSlaAgreement` / `novai_getActiveSla` /
//! `novai_listSlasBy{Buyer,Seller}` RPC methods.

use crate::commands::account::sign_and_submit;
use crate::commands::keygen::parse_hex32;
use crate::rpc_client::RpcClient;
use novai_ai_entities::{
    AiSignalType, MemoryObjectType, SlaAgreementData, SLA_AGREEMENT_V1, SLA_RESERVED_LEN,
    SLA_STATUS_PROPOSED,
};

/// Build a canonical `SlaAgreementData` from typed CLI args.
///
/// The buyer is the proposer (identified via `--buyer-entity-id`).
/// All runtime-only fields (`accepted_at_height`, `violation_count`,
/// `terminated_at_height`, `slashed_amount`) are zeroed: the chain
/// rejects pre-seeded values with `SlaAgreementInitialFieldsNotZero`,
/// and `created_at_height` is informational only (the canonical
/// index height is the memory object envelope's `created_at`).
#[allow(clippy::too_many_arguments)]
fn build_agreement(
    buyer_entity_id_hex: &str,
    seller_entity_id_hex: &str,
    service_descriptor_hash_hex: &str,
    start_height: u64,
    end_height: u64,
    violation_threshold: u32,
    slash_amount: u128,
    price_per_call: u64,
    max_response_time_blocks: u32,
    min_uptime_bps: u16,
    min_delivery_success_bps: u16,
) -> Result<SlaAgreementData, String> {
    Ok(SlaAgreementData {
        version: SLA_AGREEMENT_V1,
        buyer_entity_id: parse_hex32(buyer_entity_id_hex, "buyer_entity_id")?,
        seller_entity_id: parse_hex32(seller_entity_id_hex, "seller_entity_id")?,
        service_descriptor_hash: parse_hex32(
            service_descriptor_hash_hex,
            "service_descriptor_hash",
        )?,
        status: SLA_STATUS_PROPOSED,
        created_at_height: 0,
        accepted_at_height: 0,
        start_height,
        end_height,
        violation_count: 0,
        violation_threshold,
        max_response_time_blocks,
        min_uptime_bps,
        min_delivery_success_bps,
        price_per_call,
        slash_amount,
        terminated_at_height: 0,
        slashed_amount: 0,
        reserved: [0u8; SLA_RESERVED_LEN],
    })
}

/// Propose a new SLA (wraps CreateMemoryObject payload v3).
#[allow(clippy::too_many_arguments)]
pub async fn run_propose(
    rpc: &RpcClient,
    key_file: &str,
    buyer_entity_id: &str,
    seller_entity_id: &str,
    service_descriptor_hash: &str,
    start_height: u64,
    end_height: u64,
    violation_threshold: u32,
    slash_amount: u128,
    price_per_call: u64,
    max_response_time_blocks: u32,
    min_uptime_bps: u16,
    min_delivery_success_bps: u16,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let agreement = build_agreement(
        buyer_entity_id,
        seller_entity_id,
        service_descriptor_hash,
        start_height,
        end_height,
        violation_threshold,
        slash_amount,
        price_per_call,
        max_response_time_blocks,
        min_uptime_bps,
        min_delivery_success_bps,
    )?;
    let data = agreement.encode();

    // Payload: [3][object_type:1][data_len_be:4][data]
    let data_len =
        u32::try_from(data.len()).map_err(|_| "data length overflows u32".to_string())?;
    let mut payload = Vec::with_capacity(6 + data.len());
    payload.push(3);
    payload.push(MemoryObjectType::SlaAgreement.to_byte());
    payload.extend_from_slice(&data_len.to_be_bytes());
    payload.extend_from_slice(&data);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "buyer": buyer_entity_id,
                "seller": seller_entity_id,
                "start_height": start_height,
                "end_height": end_height,
                "violation_threshold": violation_threshold,
                "slash_amount": slash_amount.to_string(),
            })
        );
    } else {
        println!("SLA proposal submitted");
        println!("Buyer:               {buyer_entity_id}");
        println!("Seller:              {seller_entity_id}");
        println!("Window:              {start_height} .. {end_height}");
        println!("Violation threshold: {violation_threshold}");
        println!("Slash amount:        {slash_amount}");
        println!("TxID:                {txid}");
    }
    Ok(())
}

/// Accept a proposed SLA (wraps the SlaAccept signal type 18).
///
/// The signal-publish payload layout is `[2][signal_hash:32]
/// [signal_type:1=18][issuer:32][sla_object_id:32][buyer_entity_id:32]`
/// = 130 bytes total. `signal_hash` is derived locally as
/// `blake3("novai-sla-accept" || sla_object_id || buyer_entity_id)`
/// for determinism; replay protection is handled by tx.nonce because
/// SlaAccept does not write to the seen-set keyed `by_hash` index.
pub async fn run_accept(
    rpc: &RpcClient,
    key_file: &str,
    sla_object_id: &str,
    buyer_entity_id: &str,
    seller_entity_id: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let sla_id_bytes = parse_hex32(sla_object_id, "sla_object_id")?;
    let buyer_bytes = parse_hex32(buyer_entity_id, "buyer_entity_id")?;
    let seller_bytes = parse_hex32(seller_entity_id, "seller_entity_id")?;

    // Derive a deterministic signal_hash from the SLA + buyer so callers
    // do not have to invent one. The signal hash is purely
    // informational for SlaAccept (no by_hash dedup); deriving it
    // locally keeps the CLI side-effect-free.
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"novai-sla-accept-v1");
    hasher.update(&sla_id_bytes);
    hasher.update(&buyer_bytes);
    let signal_hash = *hasher.finalize().as_bytes();

    // Payload: [2][signal_hash:32][signal_type:1][issuer:32]
    //          [sla_object_id:32][buyer_entity_id:32]
    let mut payload = Vec::with_capacity(2 + 32 + 1 + 32 + 32 + 32);
    payload.push(2);
    payload.extend_from_slice(&signal_hash);
    payload.push(AiSignalType::SlaAccept.to_byte());
    payload.extend_from_slice(&seller_bytes); // issuer entity id = seller
    payload.extend_from_slice(&sla_id_bytes);
    payload.extend_from_slice(&buyer_bytes);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "signal_hash": hex::encode(signal_hash),
                "sla_object_id": sla_object_id,
                "buyer": buyer_entity_id,
                "seller": seller_entity_id,
            })
        );
    } else {
        println!("SLA acceptance submitted");
        println!("Object:    {sla_object_id}");
        println!("Buyer:     {buyer_entity_id}");
        println!("Seller:    {seller_entity_id}");
        println!("TxID:      {txid}");
    }
    Ok(())
}

/// Cancel a still-Proposed SLA (wraps DeleteMemoryObject payload v5).
///
/// The buyer is the memory-object owner, so the cancel transaction is
/// just a delete signed by the buyer's key. The chain rejects delete
/// of an in-window Active SLA with `SlaAgreementDeleteWhileActive`.
pub async fn run_cancel(
    rpc: &RpcClient,
    key_file: &str,
    sla_object_id: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let object_id = parse_hex32(sla_object_id, "sla_object_id")?;

    // Payload: [5][object_id:32]
    let mut payload = Vec::with_capacity(33);
    payload.push(5);
    payload.extend_from_slice(&object_id);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({ "txid": txid, "sla_object_id": sla_object_id })
        );
    } else {
        println!("SLA cancel submitted");
        println!("Object: {sla_object_id}");
        println!("TxID:   {txid}");
    }
    Ok(())
}

/// Show a single SLA by `(owner, object_id)`.
pub async fn run_show(
    rpc: &RpcClient,
    owner: &str,
    object_id: &str,
    json: bool,
) -> Result<(), String> {
    let agreement = rpc.get_sla_agreement(owner, object_id).await?;
    print_single(agreement, json);
    Ok(())
}

/// Resolve the currently-open SLA between `(buyer, seller)`.
pub async fn run_active(
    rpc: &RpcClient,
    buyer: &str,
    seller: &str,
    json: bool,
) -> Result<(), String> {
    let agreement = rpc.get_active_sla(buyer, seller).await?;
    print_single(agreement, json);
    Ok(())
}

fn print_single(agreement: Option<serde_json::Value>, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "agreement": agreement })).unwrap()
        );
        return;
    }
    let Some(a) = agreement else {
        println!("No SLA found for the given query");
        return;
    };
    println!(
        "Object ID:           {}",
        a["object_id"].as_str().unwrap_or("?")
    );
    println!(
        "Buyer:               {}",
        a["buyer_entity_id"].as_str().unwrap_or("?")
    );
    println!(
        "Seller:              {}",
        a["seller_entity_id"].as_str().unwrap_or("?")
    );
    println!(
        "Status:              {} ({})",
        a["status_label"].as_str().unwrap_or("?"),
        a["status"].as_u64().unwrap_or(0)
    );
    println!(
        "Window:              {} .. {}",
        a["start_height"].as_u64().unwrap_or(0),
        a["end_height"].as_u64().unwrap_or(0)
    );
    println!(
        "Accepted at:         {}",
        a["accepted_at_height"].as_u64().unwrap_or(0)
    );
    println!(
        "Violations:          {}/{}",
        a["violation_count"].as_u64().unwrap_or(0),
        a["violation_threshold"].as_u64().unwrap_or(0)
    );
    println!(
        "Slash amount:        {}",
        a["slash_amount"].as_str().unwrap_or("0")
    );
    println!(
        "Slashed so far:      {}",
        a["slashed_amount"].as_str().unwrap_or("0")
    );
    println!(
        "Terminated at:       {}",
        a["terminated_at_height"].as_u64().unwrap_or(0)
    );
    println!(
        "Service descriptor:  {}",
        a["service_descriptor_hash"].as_str().unwrap_or("?")
    );
}

/// List SLAs where the entity is the buyer.
pub async fn run_list_by_buyer(
    rpc: &RpcClient,
    entity_id: &str,
    start_height: u64,
    end_height: u64,
    json: bool,
) -> Result<(), String> {
    let agreements = rpc
        .list_slas_by_buyer(entity_id, start_height, end_height)
        .await?;
    print_list(&agreements, json);
    Ok(())
}

/// List SLAs where the entity is the seller.
pub async fn run_list_by_seller(
    rpc: &RpcClient,
    entity_id: &str,
    start_height: u64,
    end_height: u64,
    json: bool,
) -> Result<(), String> {
    let agreements = rpc
        .list_slas_by_seller(entity_id, start_height, end_height)
        .await?;
    print_list(&agreements, json);
    Ok(())
}

fn print_list(agreements: &[serde_json::Value], json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "agreements": agreements })).unwrap()
        );
        return;
    }
    if agreements.is_empty() {
        println!("No SLAs found");
        return;
    }
    println!(
        "{:<64}  {:<10}  {:>10}  {:>10}  {:>14}",
        "OBJECT ID", "STATUS", "START", "END", "SLASH"
    );
    for a in agreements {
        println!(
            "{:<64}  {:<10}  {:>10}  {:>10}  {:>14}",
            a["object_id"].as_str().unwrap_or("?"),
            a["status_label"].as_str().unwrap_or("?"),
            a["start_height"].as_u64().unwrap_or(0),
            a["end_height"].as_u64().unwrap_or(0),
            a["slash_amount"].as_str().unwrap_or("0"),
        );
    }
    println!("\n{} SLA(s)", agreements.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_agreement_round_trips_through_codec() {
        let buyer = "11".repeat(32);
        let seller = "22".repeat(32);
        let desc = "33".repeat(32);
        let sla = build_agreement(
            &buyer, &seller, &desc, 1_000, 5_000, 3, 1_000_000, 100, 0, 0, 0,
        )
        .unwrap();
        let bytes = sla.encode();
        let decoded = SlaAgreementData::decode(&bytes).expect("decode");
        assert_eq!(decoded, sla);
        assert_eq!(decoded.status, SLA_STATUS_PROPOSED);
        assert_eq!(decoded.violation_count, 0);
        assert_eq!(decoded.violation_threshold, 3);
        assert_eq!(decoded.slash_amount, 1_000_000);
    }

    #[test]
    fn build_agreement_rejects_bad_hex_buyer() {
        let err = build_agreement(
            "not-hex",
            "22".repeat(32).as_str(),
            "33".repeat(32).as_str(),
            1_000,
            5_000,
            3,
            1_000_000,
            0,
            0,
            0,
            0,
        )
        .unwrap_err();
        assert!(err.contains("buyer_entity_id"));
    }

    #[test]
    fn build_agreement_rejects_bad_hex_seller() {
        let err = build_agreement(
            "11".repeat(32).as_str(),
            "zz",
            "33".repeat(32).as_str(),
            1_000,
            5_000,
            3,
            1_000_000,
            0,
            0,
            0,
            0,
        )
        .unwrap_err();
        assert!(err.contains("seller_entity_id"));
    }
}
