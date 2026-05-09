//! AI entity commands: register, register-with-key, credit, info.

use crate::commands::account::sign_and_submit;
use crate::commands::keygen::{load_keypair, parse_hex32};
use crate::rpc_client::RpcClient;
use novai_ai_entities::{AiEntity, AutonomyMode, Capabilities};
use novai_crypto::address_from_pubkey;

/// Parse autonomy mode string.
fn parse_autonomy(s: &str) -> Result<AutonomyMode, String> {
    match s {
        "advisory" => Ok(AutonomyMode::Advisory),
        "gated" => Ok(AutonomyMode::Gated),
        _ => Err(format!(
            "Invalid autonomy mode '{s}': must be 'advisory' or 'gated'"
        )),
    }
}

/// Parse comma-separated capabilities string into a `Capabilities` struct.
fn parse_capabilities(s: &str) -> Result<Capabilities, String> {
    let mut caps = Capabilities::default();
    for part in s.split(',') {
        match part.trim() {
            "read_chain" => caps.read_public_chain = true,
            "read_memory" => caps.read_memory_objects = true,
            "emit_proposals" => caps.emit_proposals = true,
            "request_execution" => caps.request_execution = true,
            "read_nnpx" => caps.read_nnpx_derived = true,
            other => return Err(format!("Unknown capability '{other}'")),
        }
    }
    Ok(caps)
}

/// Register a new AI entity (payload type 8, 51 bytes).
#[allow(clippy::too_many_arguments)]
pub async fn run_register(
    rpc: &RpcClient,
    key_file: &str,
    code_hash_hex: &str,
    autonomy: &str,
    capabilities_str: &str,
    initial_balance: u128,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let code_hash = parse_hex32(code_hash_hex, "code_hash")?;
    let autonomy_mode = parse_autonomy(autonomy)?;
    let caps = parse_capabilities(capabilities_str)?;

    // Compute entity ID client-side
    let (_, pk) = load_keypair(key_file)?;
    let creator = address_from_pubkey(&pk);
    let entity_id = AiEntity::compute_id(&code_hash, &creator);

    // Build payload: [8][code_hash:32][autonomy:1][capabilities:1][initial_balance_be:16]
    let mut payload = Vec::with_capacity(51);
    payload.push(8);
    payload.extend_from_slice(&code_hash);
    payload.push(autonomy_mode.to_byte());
    payload.push(caps.to_byte());
    payload.extend_from_slice(&initial_balance.to_be_bytes());

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "entity_id": hex::encode(entity_id),
                "txid": txid,
            })
        );
    } else {
        println!("AI entity registration submitted");
        println!("Entity ID: {}", hex::encode(entity_id));
        println!("TxID:      {txid}");
    }
    Ok(())
}

/// Register a new AI entity with its own signing key (payload type 10, 83 bytes).
#[allow(clippy::too_many_arguments)]
pub async fn run_register_with_key(
    rpc: &RpcClient,
    key_file: &str,
    entity_key_file: &str,
    code_hash_hex: &str,
    autonomy: &str,
    capabilities_str: &str,
    initial_balance: u128,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let code_hash = parse_hex32(code_hash_hex, "code_hash")?;
    let autonomy_mode = parse_autonomy(autonomy)?;
    let caps = parse_capabilities(capabilities_str)?;

    // Load entity's public key
    let (_, entity_pk) = load_keypair(entity_key_file)?;

    // Compute entity ID client-side
    let (_, creator_pk) = load_keypair(key_file)?;
    let creator = address_from_pubkey(&creator_pk);
    let entity_id = AiEntity::compute_id(&code_hash, &creator);

    // Build payload: [10][code_hash:32][pubkey:32][autonomy:1][capabilities:1][initial_balance_be:16]
    let mut payload = Vec::with_capacity(83);
    payload.push(10);
    payload.extend_from_slice(&code_hash);
    payload.extend_from_slice(&entity_pk.to_bytes());
    payload.push(autonomy_mode.to_byte());
    payload.push(caps.to_byte());
    payload.extend_from_slice(&initial_balance.to_be_bytes());

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    let entity_addr = address_from_pubkey(&entity_pk);
    if json {
        println!(
            "{}",
            serde_json::json!({
                "entity_id": hex::encode(entity_id),
                "entity_address": hex::encode(entity_addr),
                "txid": txid,
            })
        );
    } else {
        println!("AI entity registration (with key) submitted");
        println!("Entity ID:      {}", hex::encode(entity_id));
        println!("Entity Address: {}", hex::encode(entity_addr));
        println!("TxID:           {txid}");
    }
    Ok(())
}

/// Credit (fund) an existing AI entity (payload type 9, 49 bytes).
pub async fn run_credit(
    rpc: &RpcClient,
    key_file: &str,
    entity_id_hex: &str,
    amount: u128,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let entity_id = parse_hex32(entity_id_hex, "entity_id")?;

    // Build payload: [9][entity_id:32][amount_be:16]
    let mut payload = Vec::with_capacity(49);
    payload.push(9);
    payload.extend_from_slice(&entity_id);
    payload.extend_from_slice(&amount.to_be_bytes());

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({ "txid": txid, "amount": amount.to_string() })
        );
    } else {
        println!("AI entity credited");
        println!("Entity: {entity_id_hex}");
        println!("Amount: {amount}");
        println!("TxID:   {txid}");
    }
    Ok(())
}

/// Query AI entity state.
pub async fn run_info(rpc: &RpcClient, entity_id_hex: &str, json: bool) -> Result<(), String> {
    let entity = rpc.get_ai_entity(entity_id_hex).await?;

    match entity {
        None => {
            if json {
                println!("{}", serde_json::json!({ "entity": null }));
            } else {
                println!("Entity not found: {entity_id_hex}");
            }
        }
        Some(e) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&e).unwrap());
            } else {
                println!("ID:              {}", e["id"].as_str().unwrap_or("?"));
                println!(
                    "Code Hash:       {}",
                    e["code_hash"].as_str().unwrap_or("?")
                );
                println!("Creator:         {}", e["creator"].as_str().unwrap_or("?"));
                println!(
                    "Autonomy Mode:   {}",
                    match e["autonomy_mode"].as_u64() {
                        Some(0) => "Advisory",
                        Some(1) => "Gated",
                        Some(2) => "Autonomous",
                        _ => "Unknown",
                    }
                );
                println!(
                    "Capabilities:    0x{:02x}",
                    e["capabilities"].as_u64().unwrap_or(0)
                );
                println!(
                    "Balance:         {}",
                    e["economic_balance"].as_str().unwrap_or("0")
                );
                println!("Nonce:           {}", e["nonce"].as_u64().unwrap_or(0));
                println!("Pubkey:          {}", e["pubkey"].as_str().unwrap_or("?"));
                println!(
                    "Registered At:   {}",
                    e["registered_at"].as_u64().unwrap_or(0)
                );
                println!(
                    "Last Active At:  {}",
                    e["last_active_at"].as_u64().unwrap_or(0)
                );
                println!(
                    "Active:          {}",
                    e["is_active"].as_bool().unwrap_or(false)
                );
                println!(
                    "Reputation:      {} / 100",
                    e["reputation_score"].as_u64().unwrap_or(0)
                );
                println!(
                    "Total Txs:       {}",
                    e["total_transactions"].as_u64().unwrap_or(0)
                );
                println!(
                    "Rep Events:      {}",
                    e["reputation_events_count"].as_u64().unwrap_or(0)
                );
                println!(
                    "Stake:           {}",
                    e["stake_balance"].as_str().unwrap_or("0")
                );
                println!(
                    "Stake Locked:    {}",
                    e["stake_locked_until"].as_u64().unwrap_or(0)
                );
            }
        }
    }
    Ok(())
}
