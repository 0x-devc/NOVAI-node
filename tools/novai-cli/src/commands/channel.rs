//! PaymentChannel commands (Week 32): propose, accept, sign-update,
//! close, finalize, cancel, show, list-by-party-a, list-by-party-b,
//! dispute-status.
//!
//! Propose / cancel wrap `CreateMemoryObject` / `DeleteMemoryObject`
//! payloads so operators do not hand-roll the 222-byte
//! `PaymentChannelData`. Accept / close / finalize wrap the three
//! channel-specific signal types (19 / 20 / 21). `sign-update` is a
//! pure off-chain helper that produces an ed25519 signature over the
//! canonical channel state bytes via
//! `novai_crypto::sign_channel_state`; it does NOT touch the chain.
//! Lookups route through the four
//! `novai_getPaymentChannel` / `novai_listChannelsByPartyA` /
//! `novai_listChannelsByPartyB` / `novai_getChannelDisputeStatus`
//! RPC methods shipped in Phase 6.
//!
//! All u128 amounts (deposits, balances) accept their decimal-string
//! representation in CLI args to match the on-chain encoding and
//! avoid f64 precision loss in `clap` parsing.

use crate::commands::account::sign_and_submit;
use crate::commands::keygen::{load_keypair, parse_hex32};
use crate::rpc_client::RpcClient;
use novai_ai_entities::{
    AiSignalType, MemoryObjectType, PaymentChannelData, PAYMENT_CHANNEL_RESERVED_LEN,
    PAYMENT_CHANNEL_STATUS_PROPOSED, PAYMENT_CHANNEL_V1,
};
use novai_crypto::sign_channel_state;

/// Numeric chain id mixed into every off-chain channel state
/// signature. MUST match `novai_execution::NOVAI_CHANNEL_CHAIN_ID`.
/// Hardcoded for v1; the chain rejects signatures that bind any
/// other chain id with `ChannelCloseInvalidSignatureA` /
/// `ChannelCloseInvalidSignatureB`.
const NOVAI_CHANNEL_CHAIN_ID: u64 = 1;

const CHANNEL_CLOSE_IS_FINAL: u8 = 1;
const CHANNEL_CLOSE_NOT_FINAL: u8 = 0;

/// Build a canonical PROPOSED `PaymentChannelData` from typed CLI
/// args. All runtime-only fields (`accepted_at_height`,
/// `closing_at_height`, `dispute_deadline_height`, `nonce`,
/// `balance_b`) are zeroed; `balance_a` is initialized to
/// `deposit_a` to satisfy the create-handler's initial-state
/// invariant.
#[allow(clippy::too_many_arguments)]
fn build_channel(
    party_a_hex: &str,
    party_b_hex: &str,
    sla_object_id_hex: &str,
    deposit_a: u128,
    deposit_b: u128,
    dispute_window_blocks: u32,
) -> Result<PaymentChannelData, String> {
    Ok(PaymentChannelData {
        version: PAYMENT_CHANNEL_V1,
        party_a_entity_id: parse_hex32(party_a_hex, "party_a_entity_id")?,
        party_b_entity_id: parse_hex32(party_b_hex, "party_b_entity_id")?,
        sla_object_id: parse_hex32(sla_object_id_hex, "sla_object_id")?,
        status: PAYMENT_CHANNEL_STATUS_PROPOSED,
        deposit_a,
        deposit_b,
        balance_a: deposit_a,
        balance_b: 0,
        nonce: 0,
        proposed_at_height: 0,
        accepted_at_height: 0,
        closing_at_height: 0,
        dispute_deadline_height: 0,
        dispute_window_blocks,
        reserved: [0u8; PAYMENT_CHANNEL_RESERVED_LEN],
    })
}

/// Propose a new payment channel (wraps `CreateMemoryObject` payload
/// v3 with type 15). Party A's deposit is debited at create time;
/// party B accepts via `run_accept` to escrow their own deposit.
#[allow(clippy::too_many_arguments)]
pub async fn run_propose(
    rpc: &RpcClient,
    key_file: &str,
    party_a_entity_id: &str,
    party_b_entity_id: &str,
    sla_object_id: &str,
    deposit_a: u128,
    deposit_b: u128,
    dispute_window_blocks: u32,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let channel = build_channel(
        party_a_entity_id,
        party_b_entity_id,
        sla_object_id,
        deposit_a,
        deposit_b,
        dispute_window_blocks,
    )?;
    let data = channel.encode();

    let data_len =
        u32::try_from(data.len()).map_err(|_| "data length overflows u32".to_string())?;
    let mut payload = Vec::with_capacity(6 + data.len());
    payload.push(3);
    payload.push(MemoryObjectType::PaymentChannel.to_byte());
    payload.extend_from_slice(&data_len.to_be_bytes());
    payload.extend_from_slice(&data);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "party_a": party_a_entity_id,
                "party_b": party_b_entity_id,
                "deposit_a": deposit_a.to_string(),
                "deposit_b": deposit_b.to_string(),
                "dispute_window_blocks": dispute_window_blocks,
            })
        );
    } else {
        println!("Channel proposal submitted");
        println!("Party A:              {party_a_entity_id}");
        println!("Party B:              {party_b_entity_id}");
        println!("Deposit A:            {deposit_a}");
        println!("Deposit B:            {deposit_b}");
        println!("Dispute window:       {dispute_window_blocks} blocks");
        println!("TxID:                 {txid}");
    }
    Ok(())
}

/// Accept a PROPOSED payment channel (wraps the `ChannelAccept`
/// signal type 19). Party B's deposit is debited at this step; the
/// channel transitions to OPEN.
pub async fn run_accept(
    rpc: &RpcClient,
    key_file: &str,
    channel_object_id: &str,
    party_a_entity_id: &str,
    party_b_entity_id: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let channel_id_bytes = parse_hex32(channel_object_id, "channel_object_id")?;
    let party_a_bytes = parse_hex32(party_a_entity_id, "party_a_entity_id")?;
    let party_b_bytes = parse_hex32(party_b_entity_id, "party_b_entity_id")?;

    // Deterministic signal_hash so the caller does not have to
    // invent one. ChannelAccept does not write a by_hash seen-set
    // (replay protection is via tx.nonce), so the hash is purely
    // informational and a domain-tagged blake3 over the targeting
    // inputs is sufficient.
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"novai-channel-accept-v1");
    hasher.update(&channel_id_bytes);
    hasher.update(&party_a_bytes);
    let signal_hash = *hasher.finalize().as_bytes();

    // Payload: [2][signal_hash:32][signal_type:1=19][issuer:32=party_b]
    //          [channel_object_id:32][party_a_entity_id:32]
    let mut payload = Vec::with_capacity(2 + 32 + 1 + 32 + 32 + 32);
    payload.push(2);
    payload.extend_from_slice(&signal_hash);
    payload.push(AiSignalType::ChannelAccept.to_byte());
    payload.extend_from_slice(&party_b_bytes); // issuer = party B (the accepter)
    payload.extend_from_slice(&channel_id_bytes);
    payload.extend_from_slice(&party_a_bytes);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "signal_hash": hex::encode(signal_hash),
                "channel_object_id": channel_object_id,
                "party_a": party_a_entity_id,
                "party_b": party_b_entity_id,
            })
        );
    } else {
        println!("Channel acceptance submitted");
        println!("Channel:   {channel_object_id}");
        println!("Party A:   {party_a_entity_id}");
        println!("Party B:   {party_b_entity_id}");
        println!("TxID:      {txid}");
    }
    Ok(())
}

/// OFFLINE: produce an ed25519 signature over the canonical channel
/// state signing bytes. The signing key file is read from disk and
/// MUST correspond to whichever party's `pubkey` is registered
/// on-chain (party A or party B). The output signature is hex-
/// encoded; pass it to `run_close` via `--sig-a` or `--sig-b`.
///
/// This subcommand does NOT touch the chain. It exists so two
/// parties can exchange doubly-signed channel state off-chain
/// without manually computing the canonical signing bytes.
#[allow(clippy::too_many_arguments)]
pub fn run_sign_update(
    signing_key_file: &str,
    channel_object_id: &str,
    party_a_entity_id: &str,
    party_b_entity_id: &str,
    nonce: u64,
    balance_a: u128,
    balance_b: u128,
    is_final: bool,
    json: bool,
) -> Result<(), String> {
    let channel_id_bytes = parse_hex32(channel_object_id, "channel_object_id")?;
    let party_a_bytes = parse_hex32(party_a_entity_id, "party_a_entity_id")?;
    let party_b_bytes = parse_hex32(party_b_entity_id, "party_b_entity_id")?;
    let (sk, _pk) = load_keypair(signing_key_file)?;

    let sig = sign_channel_state(
        &sk,
        NOVAI_CHANNEL_CHAIN_ID,
        &channel_id_bytes,
        &party_a_bytes,
        &party_b_bytes,
        nonce,
        balance_a,
        balance_b,
        is_final,
    );
    let sig_hex = hex::encode(sig);

    if json {
        println!(
            "{}",
            serde_json::json!({
                "signature": sig_hex,
                "channel_object_id": channel_object_id,
                "party_a": party_a_entity_id,
                "party_b": party_b_entity_id,
                "nonce": nonce,
                "balance_a": balance_a.to_string(),
                "balance_b": balance_b.to_string(),
                "is_final": is_final,
            })
        );
    } else {
        println!("Channel state signature:");
        println!("  Signature:  {sig_hex}");
        println!("  Channel:    {channel_object_id}");
        println!("  Party A:    {party_a_entity_id}");
        println!("  Party B:    {party_b_entity_id}");
        println!("  Nonce:      {nonce}");
        println!("  Balance A:  {balance_a}");
        println!("  Balance B:  {balance_b}");
        println!("  Is final:   {is_final}");
    }
    Ok(())
}

/// Submit a `ChannelClose` signal (type 20) with the supplied
/// doubly-signed state. Both `sig_a` and `sig_b` MUST verify on-
/// chain; the canonical signing bytes are produced by
/// `run_sign_update`. The submitter (the `key_file` owner) MUST be
/// party A or party B per
/// `ChannelCloseSubmitterNotParticipant`.
#[allow(clippy::too_many_arguments)]
pub async fn run_close(
    rpc: &RpcClient,
    key_file: &str,
    channel_object_id: &str,
    party_a_entity_id: &str,
    party_b_entity_id: &str,
    nonce: u64,
    balance_a: u128,
    balance_b: u128,
    is_final: bool,
    sig_a_hex: &str,
    sig_b_hex: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let channel_id_bytes = parse_hex32(channel_object_id, "channel_object_id")?;
    let party_a_bytes = parse_hex32(party_a_entity_id, "party_a_entity_id")?;

    let sig_a_raw =
        hex::decode(sig_a_hex).map_err(|e| format!("Invalid sig_a hex: {e}"))?;
    if sig_a_raw.len() != 64 {
        return Err(format!("sig_a must be 64 bytes, got {}", sig_a_raw.len()));
    }
    let mut sig_a = [0u8; 64];
    sig_a.copy_from_slice(&sig_a_raw);

    let sig_b_raw =
        hex::decode(sig_b_hex).map_err(|e| format!("Invalid sig_b hex: {e}"))?;
    if sig_b_raw.len() != 64 {
        return Err(format!("sig_b must be 64 bytes, got {}", sig_b_raw.len()));
    }
    let mut sig_b = [0u8; 64];
    sig_b.copy_from_slice(&sig_b_raw);

    // The submitter's pubkey is whatever load_keypair returns. We
    // derive `issuer_entity_id` for the signal payload from
    // tx.from, which sign_and_submit fills from the keypair's
    // address. To compute `issuer_entity_id` at this layer we would
    // need the party's entity id; the signal-payload's
    // `issuer_entity_id` is checked against `entity.id` resolved
    // from `tx.from` on-chain, so we set it equal to whichever
    // party is submitting. The caller supplies which party they
    // are implicitly via the key file; we infer it by reading the
    // pubkey and matching against party_a / party_b.
    //
    // Simpler approach for v1: require the caller to pass their
    // own entity id explicitly via `submitter_entity_id`. But
    // since the chain already cross-checks issuer_entity_id ==
    // entity.id resolved from tx.from, we can read it back from
    // the keypair's address. To keep the wire format minimal we
    // require the caller to identify themselves via the key file,
    // and we set issuer_entity_id = address_from_pubkey(loaded
    // pubkey) which on-chain MUST equal entity.id. This matches
    // the SLA accept flow's convention.
    let (_, pk) = load_keypair(key_file)?;
    let submitter_entity_id = novai_crypto::address_from_pubkey(&pk);

    // signal_hash: deterministic over the closing state so callers
    // do not have to invent one. The on-chain handler does not
    // check signal_hash uniqueness for ChannelClose (no by_hash
    // seen-set); replay protection is tx.nonce + status gate.
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"novai-channel-close-v1");
    hasher.update(&channel_id_bytes);
    hasher.update(&nonce.to_be_bytes());
    hasher.update(&balance_a.to_be_bytes());
    hasher.update(&balance_b.to_be_bytes());
    hasher.update(&[u8::from(is_final)]);
    let signal_hash = *hasher.finalize().as_bytes();

    let is_final_byte = if is_final {
        CHANNEL_CLOSE_IS_FINAL
    } else {
        CHANNEL_CLOSE_NOT_FINAL
    };

    // Payload: [2][signal_hash:32][signal_type:1=20][issuer:32]
    //          [channel_object_id:32][party_a_entity_id:32]
    //          [nonce_be:8][balance_a_be:16][balance_b_be:16]
    //          [is_final:1][sig_a:64][sig_b:64]
    // Total = 66 (base) + 233 (tail) = 299 bytes.
    let mut payload = Vec::with_capacity(299);
    payload.push(2);
    payload.extend_from_slice(&signal_hash);
    payload.push(AiSignalType::ChannelClose.to_byte());
    payload.extend_from_slice(&submitter_entity_id);
    payload.extend_from_slice(&channel_id_bytes);
    payload.extend_from_slice(&party_a_bytes);
    payload.extend_from_slice(&nonce.to_be_bytes());
    payload.extend_from_slice(&balance_a.to_be_bytes());
    payload.extend_from_slice(&balance_b.to_be_bytes());
    payload.push(is_final_byte);
    payload.extend_from_slice(&sig_a);
    payload.extend_from_slice(&sig_b);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "signal_hash": hex::encode(signal_hash),
                "channel_object_id": channel_object_id,
                "party_a": party_a_entity_id,
                "party_b": party_b_entity_id,
                "nonce": nonce,
                "balance_a": balance_a.to_string(),
                "balance_b": balance_b.to_string(),
                "is_final": is_final,
            })
        );
    } else {
        println!(
            "Channel close submitted ({})",
            if is_final { "cooperative settle" } else { "unilateral / dispute" }
        );
        println!("Channel:    {channel_object_id}");
        println!("Party A:    {party_a_entity_id}");
        println!("Party B:    {party_b_entity_id}");
        println!("Nonce:      {nonce}");
        println!("Balance A:  {balance_a}");
        println!("Balance B:  {balance_b}");
        println!("TxID:       {txid}");
    }
    Ok(())
}

/// Submit a `ChannelFinalize` signal (type 21). Permissionless: the
/// submitter does not need to be a participant. Valid only when the
/// channel is in CLOSING and `current_height >
/// dispute_deadline_height`; the chain rejects otherwise with
/// `ChannelFinalizeNotClosing` or `ChannelFinalizeBeforeDeadline`.
pub async fn run_finalize(
    rpc: &RpcClient,
    key_file: &str,
    channel_object_id: &str,
    party_a_entity_id: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let channel_id_bytes = parse_hex32(channel_object_id, "channel_object_id")?;
    let party_a_bytes = parse_hex32(party_a_entity_id, "party_a_entity_id")?;
    let (_, pk) = load_keypair(key_file)?;
    let submitter_entity_id = novai_crypto::address_from_pubkey(&pk);

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"novai-channel-finalize-v1");
    hasher.update(&channel_id_bytes);
    hasher.update(&party_a_bytes);
    let signal_hash = *hasher.finalize().as_bytes();

    // Payload: [2][signal_hash:32][signal_type:1=21][issuer:32]
    //          [channel_object_id:32][party_a_entity_id:32]
    let mut payload = Vec::with_capacity(2 + 32 + 1 + 32 + 32 + 32);
    payload.push(2);
    payload.extend_from_slice(&signal_hash);
    payload.push(AiSignalType::ChannelFinalize.to_byte());
    payload.extend_from_slice(&submitter_entity_id);
    payload.extend_from_slice(&channel_id_bytes);
    payload.extend_from_slice(&party_a_bytes);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "signal_hash": hex::encode(signal_hash),
                "channel_object_id": channel_object_id,
                "party_a": party_a_entity_id,
            })
        );
    } else {
        println!("Channel finalize submitted");
        println!("Channel:   {channel_object_id}");
        println!("Party A:   {party_a_entity_id}");
        println!("TxID:      {txid}");
    }
    Ok(())
}

/// Cancel a still-PROPOSED channel by deleting its memory object.
/// Party A's deposit is refunded. The chain rejects cancel of an
/// OPEN / CLOSING channel with
/// `PaymentChannelDeleteWhileActive`.
pub async fn run_cancel(
    rpc: &RpcClient,
    key_file: &str,
    channel_object_id: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let object_id = parse_hex32(channel_object_id, "channel_object_id")?;
    // Payload: [5][object_id:32]
    let mut payload = Vec::with_capacity(33);
    payload.push(5);
    payload.extend_from_slice(&object_id);

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({ "txid": txid, "channel_object_id": channel_object_id })
        );
    } else {
        println!("Channel cancel submitted");
        println!("Channel: {channel_object_id}");
        println!("TxID:    {txid}");
    }
    Ok(())
}

/// Show a single channel by `(owner, object_id)`.
pub async fn run_show(
    rpc: &RpcClient,
    owner: &str,
    object_id: &str,
    json: bool,
) -> Result<(), String> {
    let channel = rpc.get_payment_channel(owner, object_id).await?;
    print_single(channel, json);
    Ok(())
}

/// List channels where the entity is party A (memory object owner).
pub async fn run_list_by_party_a(
    rpc: &RpcClient,
    entity_id: &str,
    start_height: u64,
    end_height: u64,
    json: bool,
) -> Result<(), String> {
    let channels = rpc
        .list_channels_by_party_a(entity_id, start_height, end_height)
        .await?;
    print_list(&channels, json);
    Ok(())
}

/// List channels where the entity is party B (counterparty).
pub async fn run_list_by_party_b(
    rpc: &RpcClient,
    entity_id: &str,
    start_height: u64,
    end_height: u64,
    json: bool,
) -> Result<(), String> {
    let channels = rpc
        .list_channels_by_party_b(entity_id, start_height, end_height)
        .await?;
    print_list(&channels, json);
    Ok(())
}

/// Display dispute-window status for a channel.
pub async fn run_dispute_status(
    rpc: &RpcClient,
    owner: &str,
    object_id: &str,
    json: bool,
) -> Result<(), String> {
    let result = rpc.get_channel_dispute_status(owner, object_id).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
        return Ok(());
    }
    let found = result["found"].as_bool().unwrap_or(false);
    if !found {
        println!("Channel not found or wrong type");
        return Ok(());
    }
    println!(
        "Status:               {} ({})",
        result["status_label"].as_str().unwrap_or("?"),
        result["status"].as_u64().unwrap_or(0)
    );
    println!(
        "Closing at:           {}",
        result["closing_at_height"].as_u64().unwrap_or(0)
    );
    println!(
        "Dispute deadline:     {}",
        result["dispute_deadline_height"].as_u64().unwrap_or(0)
    );
    println!(
        "Current height:       {}",
        result["current_height"].as_u64().unwrap_or(0)
    );
    println!(
        "Blocks remaining:     {}",
        result["blocks_remaining"].as_u64().unwrap_or(0)
    );
    println!(
        "Finalize ready:       {}",
        result["finalize_ready"].as_bool().unwrap_or(false)
    );
    Ok(())
}

fn print_single(channel: Option<serde_json::Value>, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "channel": channel })).unwrap()
        );
        return;
    }
    let Some(c) = channel else {
        println!("No channel found for the given query");
        return;
    };
    println!(
        "Object ID:               {}",
        c["object_id"].as_str().unwrap_or("?")
    );
    println!(
        "Party A:                 {}",
        c["party_a_entity_id"].as_str().unwrap_or("?")
    );
    println!(
        "Party B:                 {}",
        c["party_b_entity_id"].as_str().unwrap_or("?")
    );
    println!(
        "Status:                  {} ({})",
        c["status_label"].as_str().unwrap_or("?"),
        c["status"].as_u64().unwrap_or(0)
    );
    println!(
        "Deposit A:               {}",
        c["deposit_a"].as_str().unwrap_or("0")
    );
    println!(
        "Deposit B:               {}",
        c["deposit_b"].as_str().unwrap_or("0")
    );
    println!(
        "Balance A:               {}",
        c["balance_a"].as_str().unwrap_or("0")
    );
    println!(
        "Balance B:               {}",
        c["balance_b"].as_str().unwrap_or("0")
    );
    println!("Nonce:                   {}", c["nonce"].as_u64().unwrap_or(0));
    println!(
        "Proposed at:             {}",
        c["proposed_at_height"].as_u64().unwrap_or(0)
    );
    println!(
        "Accepted at:             {}",
        c["accepted_at_height"].as_u64().unwrap_or(0)
    );
    println!(
        "Closing at:              {}",
        c["closing_at_height"].as_u64().unwrap_or(0)
    );
    println!(
        "Dispute deadline:        {}",
        c["dispute_deadline_height"].as_u64().unwrap_or(0)
    );
    println!(
        "Dispute window blocks:   {}",
        c["dispute_window_blocks"].as_u64().unwrap_or(0)
    );
    println!(
        "SLA reference:           {}",
        c["sla_object_id"].as_str().unwrap_or("?")
    );
}

fn print_list(channels: &[serde_json::Value], json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "channels": channels })).unwrap()
        );
        return;
    }
    if channels.is_empty() {
        println!("No channels found");
        return;
    }
    println!(
        "{:<64}  {:<10}  {:>14}  {:>14}  {:>10}",
        "OBJECT ID", "STATUS", "BAL_A", "BAL_B", "NONCE"
    );
    for c in channels {
        println!(
            "{:<64}  {:<10}  {:>14}  {:>14}  {:>10}",
            c["object_id"].as_str().unwrap_or("?"),
            c["status_label"].as_str().unwrap_or("?"),
            c["balance_a"].as_str().unwrap_or("0"),
            c["balance_b"].as_str().unwrap_or("0"),
            c["nonce"].as_u64().unwrap_or(0),
        );
    }
    println!("\n{} channel(s)", channels.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_channel_round_trips_through_codec() {
        let a = "11".repeat(32);
        let b = "22".repeat(32);
        let sla = "00".repeat(32);
        let channel = build_channel(&a, &b, &sla, 100_000, 50_000, 256).unwrap();
        let bytes = channel.encode();
        let decoded = PaymentChannelData::decode(&bytes).expect("decode");
        assert_eq!(decoded, channel);
        assert_eq!(decoded.status, PAYMENT_CHANNEL_STATUS_PROPOSED);
        assert_eq!(decoded.balance_a, 100_000);
        assert_eq!(decoded.balance_b, 0);
        assert_eq!(decoded.nonce, 0);
    }

    #[test]
    fn build_channel_rejects_bad_hex_party_a() {
        let err = build_channel(
            "not-hex",
            "22".repeat(32).as_str(),
            "00".repeat(32).as_str(),
            1,
            1,
            256,
        )
        .unwrap_err();
        assert!(err.contains("party_a_entity_id"));
    }

    #[test]
    fn build_channel_rejects_bad_hex_party_b() {
        let err = build_channel(
            "11".repeat(32).as_str(),
            "zz",
            "00".repeat(32).as_str(),
            1,
            1,
            256,
        )
        .unwrap_err();
        assert!(err.contains("party_b_entity_id"));
    }

    #[test]
    fn close_payload_length_matches_wire_spec() {
        // The wire format requires exactly 299 bytes; the codec
        // rejects anything else. We can't easily test the full
        // run_close because it submits over RPC, but we can mirror
        // the buffer construction to confirm the byte count.
        let mut payload = Vec::new();
        payload.push(2u8); // version
        payload.extend_from_slice(&[0u8; 32]); // signal_hash
        payload.push(20u8); // signal_type
        payload.extend_from_slice(&[0u8; 32]); // issuer
        payload.extend_from_slice(&[0u8; 32]); // channel_object_id
        payload.extend_from_slice(&[0u8; 32]); // party_a_entity_id
        payload.extend_from_slice(&0u64.to_be_bytes()); // nonce
        payload.extend_from_slice(&0u128.to_be_bytes()); // balance_a
        payload.extend_from_slice(&0u128.to_be_bytes()); // balance_b
        payload.push(0u8); // is_final
        payload.extend_from_slice(&[0u8; 64]); // sig_a
        payload.extend_from_slice(&[0u8; 64]); // sig_b
        assert_eq!(payload.len(), 299);
    }
}
