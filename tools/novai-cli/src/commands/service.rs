//! Agent Discovery Registry commands: publish, update, delete, list (Week 29).
//!
//! Wraps the existing memory-object create/update/delete signal payloads
//! with typed flags so operators do not have to hand-roll the 144-byte
//! `ServiceDescriptorData`. Discovery queries route through the new
//! `novai_getServiceDescriptorsByCategory` RPC method.

use crate::commands::account::sign_and_submit;
use crate::commands::keygen::parse_hex32;
use crate::rpc_client::RpcClient;
use novai_ai_entities::{
    MemoryObjectType, ServiceDescriptorData, SERVICE_CATEGORY_COMPUTE,
    SERVICE_CATEGORY_DATA_ORACLE, SERVICE_CATEGORY_GATEWAY, SERVICE_CATEGORY_GENERIC,
    SERVICE_CATEGORY_INDEXER, SERVICE_CATEGORY_INFERENCE, SERVICE_CATEGORY_MONITORING,
    SERVICE_CATEGORY_SIGNAL_PROVIDER, SERVICE_CATEGORY_STORAGE, SERVICE_CATEGORY_VERIFICATION,
    SERVICE_DESCRIPTOR_V1, SERVICE_STATUS_ACTIVE, SERVICE_STATUS_DEPRECATED, SERVICE_STATUS_PAUSED,
};

/// Parse a category-name string to its canonical byte.
fn parse_category(s: &str) -> Result<u8, String> {
    match s {
        "generic" => Ok(SERVICE_CATEGORY_GENERIC),
        "data-oracle" => Ok(SERVICE_CATEGORY_DATA_ORACLE),
        "inference" => Ok(SERVICE_CATEGORY_INFERENCE),
        "compute" => Ok(SERVICE_CATEGORY_COMPUTE),
        "storage" => Ok(SERVICE_CATEGORY_STORAGE),
        "indexer" => Ok(SERVICE_CATEGORY_INDEXER),
        "signal-provider" => Ok(SERVICE_CATEGORY_SIGNAL_PROVIDER),
        "verification" => Ok(SERVICE_CATEGORY_VERIFICATION),
        "monitoring" => Ok(SERVICE_CATEGORY_MONITORING),
        "gateway" => Ok(SERVICE_CATEGORY_GATEWAY),
        _ => Err(format!(
            "Unknown service category '{s}'. Valid: generic, data-oracle, inference, compute, storage, indexer, signal-provider, verification, monitoring, gateway"
        )),
    }
}

/// Parse a status-name string to its canonical byte.
fn parse_status(s: &str) -> Result<u8, String> {
    match s {
        "active" => Ok(SERVICE_STATUS_ACTIVE),
        "paused" => Ok(SERVICE_STATUS_PAUSED),
        "deprecated" => Ok(SERVICE_STATUS_DEPRECATED),
        _ => Err(format!(
            "Unknown service status '{s}'. Valid: active, paused, deprecated"
        )),
    }
}

/// Build the canonical 144-byte ServiceDescriptorData from typed args.
#[allow(clippy::too_many_arguments)]
fn build_descriptor(
    service_name_hash: &str,
    service_url_hash: &str,
    description_hash: &str,
    category_str: &str,
    price_per_call: u64,
    subscription_rate_per_block: u64,
    min_reputation_score: u16,
    min_stake: u128,
    capability_tags: u32,
    status_str: &str,
) -> Result<ServiceDescriptorData, String> {
    Ok(ServiceDescriptorData {
        version: SERVICE_DESCRIPTOR_V1,
        service_name_hash: parse_hex32(service_name_hash, "service_name_hash")?,
        service_url_hash: parse_hex32(service_url_hash, "service_url_hash")?,
        description_hash: parse_hex32(description_hash, "description_hash")?,
        category: parse_category(category_str)?,
        price_per_call,
        subscription_rate_per_block,
        min_reputation_score,
        min_stake,
        capability_tags,
        status: parse_status(status_str)?,
        reserved: [0u8; 7],
    })
}

/// Publish a new ServiceDescriptor (wraps CreateMemoryObject payload v3).
#[allow(clippy::too_many_arguments)]
pub async fn run_publish(
    rpc: &RpcClient,
    key_file: &str,
    service_name_hash: &str,
    service_url_hash: &str,
    description_hash: &str,
    category: &str,
    price_per_call: u64,
    subscription_rate: u64,
    min_reputation: u16,
    min_stake: u128,
    capability_tags: u32,
    status: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let descriptor = build_descriptor(
        service_name_hash,
        service_url_hash,
        description_hash,
        category,
        price_per_call,
        subscription_rate,
        min_reputation,
        min_stake,
        capability_tags,
        status,
    )?;
    let data = descriptor.encode();

    // Payload: [3][object_type:1][data_len_be:4][data:144]
    let mut payload = Vec::with_capacity(6 + data.len());
    payload.push(3);
    payload.push(MemoryObjectType::ServiceDescriptor.to_byte());
    payload.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
    payload.extend_from_slice(&data);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "category": category,
                "status": status,
            })
        );
    } else {
        println!("Service descriptor publish submitted");
        println!("Category: {category}");
        println!("Status:   {status}");
        println!("TxID:     {txid}");
    }
    Ok(())
}

/// Update an existing ServiceDescriptor (wraps UpdateMemoryObject payload v4).
///
/// All descriptor fields must be re-supplied; the chain rewrites the full
/// 144-byte payload. `category` is immutable - the on-chain handler
/// rejects updates whose new category differs from the stored value, so
/// the caller MUST pass the same category the descriptor was published
/// under.
#[allow(clippy::too_many_arguments)]
pub async fn run_update(
    rpc: &RpcClient,
    key_file: &str,
    object_id_hex: &str,
    service_name_hash: &str,
    service_url_hash: &str,
    description_hash: &str,
    category: &str,
    price_per_call: u64,
    subscription_rate: u64,
    min_reputation: u16,
    min_stake: u128,
    capability_tags: u32,
    status: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let object_id = parse_hex32(object_id_hex, "object_id")?;
    let descriptor = build_descriptor(
        service_name_hash,
        service_url_hash,
        description_hash,
        category,
        price_per_call,
        subscription_rate,
        min_reputation,
        min_stake,
        capability_tags,
        status,
    )?;
    let data = descriptor.encode();

    // Payload: [4][object_id:32][data_len_be:4][new_data:144]
    let mut payload = Vec::with_capacity(37 + data.len());
    payload.push(4);
    payload.extend_from_slice(&object_id);
    payload.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
    payload.extend_from_slice(&data);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({ "txid": txid, "object_id": object_id_hex })
        );
    } else {
        println!("Service descriptor update submitted");
        println!("Object: {object_id_hex}");
        println!("Status: {status}");
        println!("TxID:   {txid}");
    }
    Ok(())
}

/// Delete a published ServiceDescriptor (wraps DeleteMemoryObject payload v5).
pub async fn run_delete(
    rpc: &RpcClient,
    key_file: &str,
    object_id_hex: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let object_id = parse_hex32(object_id_hex, "object_id")?;

    // Payload: [5][object_id:32]
    let mut payload = Vec::with_capacity(33);
    payload.push(5);
    payload.extend_from_slice(&object_id);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({ "txid": txid, "object_id": object_id_hex })
        );
    } else {
        println!("Service descriptor delete submitted");
        println!("Object: {object_id_hex}");
        println!("TxID:   {txid}");
    }
    Ok(())
}

/// List all published ServiceDescriptors in a given category.
pub async fn run_list(rpc: &RpcClient, category_str: &str, json: bool) -> Result<(), String> {
    let category = parse_category(category_str)?;
    let descriptors = rpc.get_service_descriptors_by_category(category).await?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "descriptors": descriptors })
            )
            .unwrap()
        );
    } else if descriptors.is_empty() {
        println!("No descriptors published in category '{category_str}'");
    } else {
        println!(
            "{:<64}  {:<64}  {:>10}  {:>10}",
            "OBJECT ID", "OWNER", "PRICE", "STATUS"
        );
        for d in &descriptors {
            println!(
                "{:<64}  {:<64}  {:>10}  {:>10}",
                d["object_id"].as_str().unwrap_or("?"),
                d["owner_entity"].as_str().unwrap_or("?"),
                d["price_per_call"].as_str().unwrap_or("?"),
                d["status_label"].as_str().unwrap_or("?"),
            );
        }
        println!("\n{} descriptor(s)", descriptors.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_category_all_well_known() {
        let cases = [
            ("generic", SERVICE_CATEGORY_GENERIC),
            ("data-oracle", SERVICE_CATEGORY_DATA_ORACLE),
            ("inference", SERVICE_CATEGORY_INFERENCE),
            ("compute", SERVICE_CATEGORY_COMPUTE),
            ("storage", SERVICE_CATEGORY_STORAGE),
            ("indexer", SERVICE_CATEGORY_INDEXER),
            ("signal-provider", SERVICE_CATEGORY_SIGNAL_PROVIDER),
            ("verification", SERVICE_CATEGORY_VERIFICATION),
            ("monitoring", SERVICE_CATEGORY_MONITORING),
            ("gateway", SERVICE_CATEGORY_GATEWAY),
        ];
        for (name, expected) in cases {
            assert_eq!(parse_category(name).unwrap(), expected, "input {name}");
        }
    }

    #[test]
    fn test_parse_category_rejects_unknown() {
        let err = parse_category("translation").unwrap_err();
        assert!(err.contains("translation"));
        assert!(err.contains("Valid:"));
    }

    #[test]
    fn test_parse_status_all_well_known() {
        assert_eq!(parse_status("active").unwrap(), SERVICE_STATUS_ACTIVE);
        assert_eq!(parse_status("paused").unwrap(), SERVICE_STATUS_PAUSED);
        assert_eq!(
            parse_status("deprecated").unwrap(),
            SERVICE_STATUS_DEPRECATED
        );
        assert!(parse_status("ACTIVE").is_err());
    }

    #[test]
    fn test_build_descriptor_round_trip_bytes() {
        let sd = build_descriptor(
            &"aa".repeat(32),
            &"bb".repeat(32),
            &"cc".repeat(32),
            "data-oracle",
            12_345,
            42,
            50,
            1_000_000_000_000_u128,
            0x0F,
            "active",
        )
        .unwrap();
        let bytes = sd.encode();
        assert_eq!(bytes.len(), 144);
        let decoded = ServiceDescriptorData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded.category, SERVICE_CATEGORY_DATA_ORACLE);
        assert_eq!(decoded.price_per_call, 12_345);
        assert_eq!(decoded.subscription_rate_per_block, 42);
        assert_eq!(decoded.min_reputation_score, 50);
        assert_eq!(decoded.min_stake, 1_000_000_000_000_u128);
        assert_eq!(decoded.capability_tags, 0x0F);
        assert_eq!(decoded.status, SERVICE_STATUS_ACTIVE);
        assert_eq!(decoded.reserved, [0u8; 7]);
    }

    #[test]
    fn test_build_descriptor_rejects_bad_hex() {
        let err = build_descriptor(
            "not_hex",
            &"bb".repeat(32),
            &"cc".repeat(32),
            "data-oracle",
            0,
            0,
            0,
            0,
            0,
            "active",
        )
        .unwrap_err();
        assert!(err.contains("service_name_hash"));
    }
}
