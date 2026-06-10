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
            "post_oracle_anchors" => caps.post_oracle_anchors = true,
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

/// Build the EntityUpgrade payload bytes (type 11, 97 bytes). Pure helper.
fn build_upgrade_payload(
    entity_id: &[u8; 32],
    new_code_hash: &[u8; 32],
    reason_hash: &[u8; 32],
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(97);
    payload.push(11);
    payload.extend_from_slice(entity_id);
    payload.extend_from_slice(new_code_hash);
    payload.extend_from_slice(reason_hash);
    payload
}

/// Upgrade an AI entity's code hash (payload type 11, 97 bytes).
///
/// Preserves the entity id and all id-keyed state; only the code hash changes.
pub async fn run_upgrade(
    rpc: &RpcClient,
    key_file: &str,
    entity_id_hex: &str,
    new_code_hash_hex: &str,
    reason_hash_hex: Option<&str>,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let entity_id = parse_hex32(entity_id_hex, "entity_id")?;
    let new_code_hash = parse_hex32(new_code_hash_hex, "new_code_hash")?;
    let reason_hash = match reason_hash_hex {
        Some(h) => parse_hex32(h, "reason_hash")?,
        None => [0u8; 32],
    };

    let payload = build_upgrade_payload(&entity_id, &new_code_hash, &reason_hash);
    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "entity_id": entity_id_hex,
                "new_code_hash": new_code_hash_hex,
                "txid": txid,
            })
        );
    } else {
        println!("AI entity upgrade submitted");
        println!("Entity ID:     {entity_id_hex}");
        println!("New Code Hash: {new_code_hash_hex}");
        println!("TxID:          {txid}");
    }
    Ok(())
}

/// Query an AI entity's upgrade history.
pub async fn run_upgrade_history(
    rpc: &RpcClient,
    entity_id_hex: &str,
    start_height: u64,
    end_height: u64,
    json: bool,
) -> Result<(), String> {
    let upgrades = rpc
        .get_upgrade_history(entity_id_hex, start_height, end_height)
        .await?;

    if json {
        println!("{}", serde_json::json!({ "upgrades": upgrades }));
    } else if upgrades.is_empty() {
        println!("No upgrades for entity {entity_id_hex} in [{start_height}, {end_height}]");
    } else {
        println!("Upgrade history for {entity_id_hex}:");
        for u in &upgrades {
            println!(
                "  #{} at height {}: {} -> {}",
                u["upgrade_count"].as_u64().unwrap_or(0),
                u["upgrade_height"].as_u64().unwrap_or(0),
                u["old_code_hash"].as_str().unwrap_or("?"),
                u["new_code_hash"].as_str().unwrap_or("?"),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_payload_layout_is_97_bytes() {
        let p = build_upgrade_payload(&[0x11; 32], &[0x22; 32], &[0x33; 32]);
        assert_eq!(p.len(), 97);
        assert_eq!(p[0], 11);
        assert_eq!(&p[1..33], &[0x11; 32]);
        assert_eq!(&p[33..65], &[0x22; 32]);
        assert_eq!(&p[65..97], &[0x33; 32]);
    }

    #[test]
    fn upgrade_default_reason_is_zero() {
        let p = build_upgrade_payload(&[0x11; 32], &[0x22; 32], &[0u8; 32]);
        assert_eq!(&p[65..97], &[0u8; 32]);
    }

    #[test]
    fn upgrade_bad_hex_entity_id_rejected() {
        assert!(parse_hex32("not-hex", "entity_id").is_err());
    }
}
