//! Signal commands: publish, by-height, by-issuer, by-type.

use crate::commands::account::sign_and_submit;
use crate::commands::keygen::parse_hex32;
use crate::rpc_client::RpcClient;
use novai_ai_entities::AiSignalType;

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
        _ => Err(format!(
            "Unknown signal type '{s}'. Valid: anomaly, optimization, prediction, risk-score, audit-report, spam-risk, congestion-forecast"
        )),
    }
}

/// Publish a signal commitment (payload type 2, 66 bytes).
#[allow(clippy::too_many_arguments)]
pub async fn run_publish(
    rpc: &RpcClient,
    key_file: &str,
    signal_hash_hex: &str,
    signal_type_str: &str,
    issuer_entity_id_hex: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let signal_hash = parse_hex32(signal_hash_hex, "signal_hash")?;
    let signal_type = parse_signal_type(signal_type_str)?;
    let issuer_entity_id = parse_hex32(issuer_entity_id_hex, "issuer_entity_id")?;

    // Build payload: [2][signal_hash:32][signal_type:1][issuer_entity_id:32]
    let mut payload = Vec::with_capacity(66);
    payload.push(2);
    payload.extend_from_slice(&signal_hash);
    payload.push(signal_type.to_byte());
    payload.extend_from_slice(&issuer_entity_id);

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
