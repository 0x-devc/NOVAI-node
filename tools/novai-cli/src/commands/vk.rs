//! VK Registry commands: register, update (label only), delete, show, list
//! (Week 30).
//!
//! Wraps the `CreateMemoryObject` / `UpdateMemoryObject` /
//! `DeleteMemoryObject` signal payloads so operators do not have to
//! hand-roll the `VkRegistrationData` bytes. Lookups route through the
//! new `novai_getVkRegistration` and `novai_listVkRegistrations` RPC
//! methods.

use crate::commands::account::sign_and_submit;
use crate::commands::keygen::parse_hex32;
use crate::rpc_client::RpcClient;
use novai_ai_entities::{
    MemoryObjectType, VkRegistrationData, VK_REGISTRATION_LABEL_MAX, VK_REGISTRATION_VERSION,
};

/// Translate a user-facing proof-system name to its on-chain
/// `PROOF_TYPE_*` discriminant. The registry handler only accepts
/// `PROOF_TYPE_GROTH16` at the moment; other names are surfaced here so
/// the error message lists the accepted set rather than letting the
/// chain reject with an opaque `VkRegistrationUnsupportedProofType`.
fn parse_proof_type(s: &str) -> Result<u8, String> {
    match s {
        "groth16" => Ok(1u8), // PROOF_TYPE_GROTH16
        "stub" | "plonk" | "groth16-registered" | "plonk-registered" => Err(format!(
            "Proof system '{s}' is not accepted at registration. Only 'groth16' is wired in v1.",
        )),
        _ => Err(format!("Unknown proof system '{s}'. Valid: groth16",)),
    }
}

/// Load compressed VK bytes from a file path.
fn load_vk_file(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("Failed to read VK file '{path}': {e}"))
}

/// Build a canonical `VkRegistrationData` from typed args, enforcing
/// the same label cap the chain enforces so the CLI surfaces the
/// failure before paying for a rejected tx.
fn build_registration(
    proof_type_str: &str,
    code_hash_hex: &str,
    label: &str,
    vk_bytes: Vec<u8>,
) -> Result<VkRegistrationData, String> {
    if label.len() > VK_REGISTRATION_LABEL_MAX {
        return Err(format!(
            "label exceeds {VK_REGISTRATION_LABEL_MAX} bytes (got {})",
            label.len()
        ));
    }
    if vk_bytes.is_empty() {
        return Err("VK file is empty".to_string());
    }
    Ok(VkRegistrationData {
        version: VK_REGISTRATION_VERSION,
        proof_type: parse_proof_type(proof_type_str)?,
        code_hash: parse_hex32(code_hash_hex, "code_hash")?,
        label: label.as_bytes().to_vec(),
        vk_bytes,
    })
}

/// Register a new VK (wraps CreateMemoryObject payload v3).
#[allow(clippy::too_many_arguments)]
pub async fn run_register(
    rpc: &RpcClient,
    key_file: &str,
    code_hash: &str,
    vk_file: &str,
    proof_type: &str,
    label: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let vk_bytes = load_vk_file(vk_file)?;
    let registration = build_registration(proof_type, code_hash, label, vk_bytes)?;
    let data = registration.encode();
    let vk_len = registration.vk_bytes.len();

    // Payload: [3][object_type:1][data_len_be:4][data]
    let data_len =
        u32::try_from(data.len()).map_err(|_| "data length overflows u32".to_string())?;
    let mut payload = Vec::with_capacity(6 + data.len());
    payload.push(3);
    payload.push(MemoryObjectType::VkRegistration.to_byte());
    payload.extend_from_slice(&data_len.to_be_bytes());
    payload.extend_from_slice(&data);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "proof_type": proof_type,
                "code_hash": code_hash,
                "vk_len": vk_len,
                "label": label,
            })
        );
    } else {
        println!("VK registration submitted");
        println!("Proof type: {proof_type}");
        println!("Code hash:  {code_hash}");
        println!("VK length:  {vk_len} bytes");
        if !label.is_empty() {
            println!("Label:      {label}");
        }
        println!("TxID:       {txid}");
    }
    Ok(())
}

/// Update only the `label` field of an existing VK registration
/// (wraps UpdateMemoryObject payload v4).
///
/// The on-chain handler treats `version`, `proof_type`, `code_hash`, and
/// `vk_bytes` as immutable, so this command first fetches the current
/// registration over RPC, mutates the label, and resubmits the full
/// payload with every immutable field preserved verbatim. Callers who
/// want to change anything else must `delete` and re-register.
pub async fn run_update_label(
    rpc: &RpcClient,
    key_file: &str,
    object_id_hex: &str,
    new_label: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    if new_label.len() > VK_REGISTRATION_LABEL_MAX {
        return Err(format!(
            "label exceeds {VK_REGISTRATION_LABEL_MAX} bytes (got {})",
            new_label.len()
        ));
    }
    let object_id = parse_hex32(object_id_hex, "object_id")?;

    // Fetch the current registration so we can preserve immutable fields.
    let existing = rpc
        .get_vk_registration(object_id_hex)
        .await?
        .ok_or_else(|| format!("VK registration {object_id_hex} not found"))?;
    let vk_bytes_hex = existing["vk_bytes_hex"]
        .as_str()
        .ok_or_else(|| "vk_bytes_hex missing from RPC response".to_string())?;
    let vk_bytes = hex::decode(vk_bytes_hex)
        .map_err(|e| format!("invalid vk_bytes_hex in RPC response: {e}"))?;
    let proof_type = u8::try_from(
        existing["proof_type"]
            .as_u64()
            .ok_or_else(|| "proof_type missing from RPC response".to_string())?,
    )
    .map_err(|_| "proof_type out of u8 range".to_string())?;
    let code_hash_hex = existing["code_hash"]
        .as_str()
        .ok_or_else(|| "code_hash missing from RPC response".to_string())?;
    let code_hash = parse_hex32(code_hash_hex, "code_hash")?;
    let version = u8::try_from(
        existing["version"]
            .as_u64()
            .ok_or_else(|| "version missing from RPC response".to_string())?,
    )
    .map_err(|_| "version out of u8 range".to_string())?;

    let registration = VkRegistrationData {
        version,
        proof_type,
        code_hash,
        label: new_label.as_bytes().to_vec(),
        vk_bytes,
    };
    let data = registration.encode();
    let data_len =
        u32::try_from(data.len()).map_err(|_| "data length overflows u32".to_string())?;

    // Payload: [4][object_id:32][data_len_be:4][new_data]
    let mut payload = Vec::with_capacity(37 + data.len());
    payload.push(4);
    payload.extend_from_slice(&object_id);
    payload.extend_from_slice(&data_len.to_be_bytes());
    payload.extend_from_slice(&data);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "object_id": object_id_hex,
                "label": new_label,
            })
        );
    } else {
        println!("VK registration label updated");
        println!("Object: {object_id_hex}");
        println!("Label:  {new_label}");
        println!("TxID:   {txid}");
    }
    Ok(())
}

/// Delete a VK registration (wraps DeleteMemoryObject payload v5).
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
        println!("VK registration delete submitted");
        println!("Object: {object_id_hex}");
        println!("TxID:   {txid}");
    }
    Ok(())
}

/// Show a single VK registration by id.
pub async fn run_show(rpc: &RpcClient, id_hex: &str, json: bool) -> Result<(), String> {
    let registration = rpc.get_vk_registration(id_hex).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "registration": registration }))
                .unwrap()
        );
    } else if let Some(r) = registration {
        println!("Object ID:    {}", r["object_id"].as_str().unwrap_or("?"));
        println!(
            "Owner:        {}",
            r["owner_entity"].as_str().unwrap_or("?")
        );
        println!(
            "Proof type:   {} ({})",
            r["proof_type_label"].as_str().unwrap_or("?"),
            r["proof_type"].as_u64().unwrap_or(0)
        );
        println!("Code hash:    {}", r["code_hash"].as_str().unwrap_or("?"));
        println!("Label:        {}", r["label"].as_str().unwrap_or(""));
        println!("VK length:    {} bytes", r["vk_len"].as_u64().unwrap_or(0));
        println!("Created at:   {}", r["created_at"].as_u64().unwrap_or(0));
        println!("Updated at:   {}", r["updated_at"].as_u64().unwrap_or(0));
    } else {
        println!("No VK registration found for id {id_hex}");
    }
    Ok(())
}

/// List all VK registrations owned by an entity.
pub async fn run_list(rpc: &RpcClient, entity_id_hex: &str, json: bool) -> Result<(), String> {
    let registrations = rpc.list_vk_registrations(entity_id_hex).await?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "registrations": registrations }))
                .unwrap()
        );
    } else if registrations.is_empty() {
        println!("No VK registrations for entity {entity_id_hex}");
    } else {
        println!(
            "{:<64}  {:>10}  {:>12}  {:<32}",
            "OBJECT ID", "PROOF", "VK BYTES", "LABEL"
        );
        for r in &registrations {
            println!(
                "{:<64}  {:>10}  {:>12}  {:<32}",
                r["object_id"].as_str().unwrap_or("?"),
                r["proof_type_label"].as_str().unwrap_or("?"),
                r["vk_len"].as_u64().unwrap_or(0),
                r["label"].as_str().unwrap_or(""),
            );
        }
        println!("\n{} registration(s)", registrations.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_proof_type_accepts_groth16() {
        assert_eq!(parse_proof_type("groth16").unwrap(), 1u8);
    }

    #[test]
    fn parse_proof_type_rejects_stub_and_unwired() {
        for name in ["stub", "plonk", "groth16-registered", "plonk-registered"] {
            let err = parse_proof_type(name).unwrap_err();
            assert!(
                err.contains(name),
                "error must mention rejected name {name}"
            );
        }
    }

    #[test]
    fn parse_proof_type_rejects_unknown() {
        let err = parse_proof_type("zksync").unwrap_err();
        assert!(err.contains("zksync"));
        assert!(err.contains("Valid:"));
    }

    #[test]
    fn build_registration_rejects_label_overflow() {
        let label = "x".repeat(VK_REGISTRATION_LABEL_MAX + 1);
        let err =
            build_registration("groth16", &"c0".repeat(32), &label, vec![0u8; 32]).unwrap_err();
        assert!(err.contains(&VK_REGISTRATION_LABEL_MAX.to_string()));
    }

    #[test]
    fn build_registration_rejects_empty_vk() {
        let err =
            build_registration("groth16", &"c0".repeat(32), "sum-v1", Vec::new()).unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn build_registration_roundtrips_through_codec() {
        let reg =
            build_registration("groth16", &"c0".repeat(32), "sum-v1", (0..16u8).collect()).unwrap();
        assert_eq!(reg.version, VK_REGISTRATION_VERSION);
        assert_eq!(reg.proof_type, 1u8);
        assert_eq!(reg.code_hash, [0xC0u8; 32]);
        assert_eq!(reg.label, b"sum-v1".to_vec());
        assert_eq!(reg.vk_bytes, (0..16u8).collect::<Vec<_>>());

        // Encode/decode roundtrip.
        let bytes = reg.encode();
        let decoded = VkRegistrationData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded, reg);
    }
}
