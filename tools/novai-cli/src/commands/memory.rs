//! Memory object commands: create, update, delete, list.

use crate::commands::account::sign_and_submit;
use crate::commands::keygen::parse_hex32;
use crate::rpc_client::RpcClient;
use novai_ai_entities::MemoryObjectType;

/// Parse memory object type string.
fn parse_memory_type(s: &str) -> Result<MemoryObjectType, String> {
    match s {
        "chain-summary" => Ok(MemoryObjectType::ChainSummary),
        "label-index" => Ok(MemoryObjectType::LabelIndex),
        "embedding-commitment" => Ok(MemoryObjectType::EmbeddingCommitment),
        "anomaly-log" => Ok(MemoryObjectType::AnomalyLog),
        "statistics-snapshot" => Ok(MemoryObjectType::StatisticsSnapshot),
        _ => Err(format!(
            "Unknown memory object type '{s}'. Valid: chain-summary, label-index, embedding-commitment, anomaly-log, statistics-snapshot"
        )),
    }
}

/// Resolve data from --data or --data-file flags.
fn resolve_data(data: Option<String>, data_file: Option<String>) -> Result<Vec<u8>, String> {
    match (data, data_file) {
        (Some(s), None) => Ok(s.into_bytes()),
        (None, Some(path)) => {
            std::fs::read(&path).map_err(|e| format!("Failed to read data file '{path}': {e}"))
        }
        (None, None) => Err("Either --data or --data-file must be provided".to_string()),
        (Some(_), Some(_)) => Err("Cannot specify both --data and --data-file".to_string()),
    }
}

/// Create a new memory object (payload type 3).
#[allow(clippy::too_many_arguments)]
pub async fn run_create(
    rpc: &RpcClient,
    key_file: &str,
    object_type_str: &str,
    data: Option<String>,
    data_file: Option<String>,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let obj_type = parse_memory_type(object_type_str)?;
    let data_bytes = resolve_data(data, data_file)?;

    let data_len = u32::try_from(data_bytes.len())
        .map_err(|_| format!("Data too large: {} bytes", data_bytes.len()))?;

    // Build payload: [3][object_type:1][data_len_be:4][data:var]
    let mut payload = Vec::with_capacity(6 + data_bytes.len());
    payload.push(3);
    payload.push(obj_type.to_byte());
    payload.extend_from_slice(&data_len.to_be_bytes());
    payload.extend_from_slice(&data_bytes);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "object_type": object_type_str,
                "data_size": data_bytes.len(),
            })
        );
    } else {
        println!("Memory object creation submitted");
        println!("Type: {object_type_str}");
        println!("Size: {} bytes", data_bytes.len());
        println!("TxID: {txid}");
    }
    Ok(())
}

/// Update an existing memory object (payload type 4).
#[allow(clippy::too_many_arguments)]
pub async fn run_update(
    rpc: &RpcClient,
    key_file: &str,
    object_id_hex: &str,
    data: Option<String>,
    data_file: Option<String>,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let object_id = parse_hex32(object_id_hex, "object_id")?;
    let data_bytes = resolve_data(data, data_file)?;

    let data_len = u32::try_from(data_bytes.len())
        .map_err(|_| format!("Data too large: {} bytes", data_bytes.len()))?;

    // Build payload: [4][object_id:32][data_len_be:4][new_data:var]
    let mut payload = Vec::with_capacity(37 + data_bytes.len());
    payload.push(4);
    payload.extend_from_slice(&object_id);
    payload.extend_from_slice(&data_len.to_be_bytes());
    payload.extend_from_slice(&data_bytes);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({ "txid": txid, "object_id": object_id_hex })
        );
    } else {
        println!("Memory object update submitted");
        println!("Object: {object_id_hex}");
        println!("TxID:   {txid}");
    }
    Ok(())
}

/// Delete a memory object (payload type 5, 33 bytes).
pub async fn run_delete(
    rpc: &RpcClient,
    key_file: &str,
    object_id_hex: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let object_id = parse_hex32(object_id_hex, "object_id")?;

    // Build payload: [5][object_id:32]
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
        println!("Memory object deletion submitted");
        println!("Object: {object_id_hex}");
        println!("TxID:   {txid}");
    }
    Ok(())
}

/// List memory objects for an entity.
pub async fn run_list(rpc: &RpcClient, entity_id_hex: &str, json: bool) -> Result<(), String> {
    let objects = rpc.get_memory_objects(entity_id_hex).await?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "objects": objects })).unwrap()
        );
    } else if objects.is_empty() {
        println!("No memory objects found for entity {entity_id_hex}");
    } else {
        println!(
            "{:<64}  {:>4}  {:>8}  {:>10}  {:>10}",
            "OBJECT ID", "TYPE", "SIZE", "CREATED", "UPDATED"
        );
        for obj in &objects {
            println!(
                "{:<64}  {:>4}  {:>8}  {:>10}  {:>10}",
                obj["object_id"].as_str().unwrap_or("?"),
                obj["object_type"].as_u64().unwrap_or(0),
                obj["data_size"].as_u64().unwrap_or(0),
                obj["created_at"].as_u64().unwrap_or(0),
                obj["updated_at"].as_u64().unwrap_or(0),
            );
        }
        println!("\n{} object(s)", objects.len());
    }
    Ok(())
}
