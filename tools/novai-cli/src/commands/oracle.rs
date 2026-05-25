//! Oracle anchoring commands (Week 35): post-anchor and queries.
//!
//! `post-anchor` builds an `OracleAnchor` signal (commitment type 2, signal
//! type 22) and submits it from the issuing entity's signing key. The
//! signal hash is content-addressed so identical data posts collide (and
//! are rejected as duplicates) while distinct data does not. The
//! `get-anchors-by-entity` / `get-anchors-by-tag` / `show` verbs query the
//! Week 35 RPC surface.

use crate::commands::account::sign_and_submit;
use crate::commands::keygen::parse_hex32;
use crate::rpc_client::RpcClient;

const ORACLE_ANCHOR_SIGNAL_TYPE: u8 = 22;
const SIGNAL_COMMITMENT_PAYLOAD_VERSION: u8 = 2;
const DATA_TAG_MAX_LEN: usize = 32;

/// Derive a content-addressed signal hash for an anchor:
/// `blake3("novai-oracle-anchor-v1" || issuer || data_hash || ts_be ||
/// source_hash || tag_len_be || data_tag)`. Identical content yields the
/// same hash (rejected on-chain as a duplicate); distinct content differs.
fn derive_signal_hash(
    issuer: &[u8; 32],
    data_hash: &[u8; 32],
    external_timestamp: u64,
    source_hash: &[u8; 32],
    data_tag: &[u8],
) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"novai-oracle-anchor-v1");
    h.update(issuer);
    h.update(data_hash);
    h.update(&external_timestamp.to_be_bytes());
    h.update(source_hash);
    h.update(
        &u32::try_from(data_tag.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    h.update(data_tag);
    *h.finalize().as_bytes()
}

/// Build the OracleAnchor signal commitment payload (type 2, signal type 22).
///
/// Layout: `[2][signal_hash:32][22][issuer:32][data_hash:32][ts_be:8]
/// [source_hash:32][expiry_be:8][tag_len:1][data_tag]`.
fn build_anchor_payload(
    signal_hash: &[u8; 32],
    issuer: &[u8; 32],
    data_hash: &[u8; 32],
    external_timestamp: u64,
    source_hash: &[u8; 32],
    expiry_height: u64,
    data_tag: &[u8],
) -> Result<Vec<u8>, String> {
    if data_tag.is_empty() || data_tag.len() > DATA_TAG_MAX_LEN {
        return Err(format!(
            "data_tag must be 1..={DATA_TAG_MAX_LEN} bytes, got {}",
            data_tag.len()
        ));
    }
    let tag_len = u8::try_from(data_tag.len()).map_err(|_| "data_tag too long".to_string())?;
    let mut p = Vec::with_capacity(66 + 81 + data_tag.len());
    p.push(SIGNAL_COMMITMENT_PAYLOAD_VERSION);
    p.extend_from_slice(signal_hash);
    p.push(ORACLE_ANCHOR_SIGNAL_TYPE);
    p.extend_from_slice(issuer);
    p.extend_from_slice(data_hash);
    p.extend_from_slice(&external_timestamp.to_be_bytes());
    p.extend_from_slice(source_hash);
    p.extend_from_slice(&expiry_height.to_be_bytes());
    p.push(tag_len);
    p.extend_from_slice(data_tag);
    Ok(p)
}

/// Post an oracle anchor (signal type 22). The signing key must belong to
/// the issuing entity (registered via `ai register-with-key`).
#[allow(clippy::too_many_arguments)]
pub async fn run_post_anchor(
    rpc: &RpcClient,
    key_file: &str,
    issuer_entity_id_hex: &str,
    data_hash_hex: &str,
    external_timestamp: u64,
    source_hash_hex: Option<&str>,
    expiry_height: u64,
    data_tag: &str,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let issuer = parse_hex32(issuer_entity_id_hex, "issuer_entity_id")?;
    let data_hash = parse_hex32(data_hash_hex, "data_hash")?;
    let source_hash = match source_hash_hex {
        Some(h) => parse_hex32(h, "source_hash")?,
        None => [0u8; 32],
    };
    let tag_bytes = data_tag.as_bytes();
    let signal_hash = derive_signal_hash(
        &issuer,
        &data_hash,
        external_timestamp,
        &source_hash,
        tag_bytes,
    );
    let payload = build_anchor_payload(
        &signal_hash,
        &issuer,
        &data_hash,
        external_timestamp,
        &source_hash,
        expiry_height,
        tag_bytes,
    )?;
    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "signal_hash": hex::encode(signal_hash),
                "issuer_entity_id": issuer_entity_id_hex,
                "data_tag": data_tag,
                "txid": txid,
            })
        );
    } else {
        println!("Oracle anchor submitted");
        println!("Signal Hash: {}", hex::encode(signal_hash));
        println!("Issuer:      {issuer_entity_id_hex}");
        println!("Tag:         {data_tag}");
        println!("TxID:        {txid}");
    }
    Ok(())
}

/// Render one JSON anchor row in a human-readable line.
fn print_anchor(a: &serde_json::Value) {
    println!(
        "  [h={}] tag={} data_hash={} ts={} expiry={} issuer={}",
        a["anchor_height"].as_u64().unwrap_or(0),
        a["data_tag"].as_str().unwrap_or("?"),
        a["data_hash"].as_str().unwrap_or("?"),
        a["external_timestamp"].as_u64().unwrap_or(0),
        a["expiry_height"].as_u64().unwrap_or(0),
        a["issuer_entity_id"].as_str().unwrap_or("?"),
    );
}

/// Query anchors by issuing entity within an inclusive height window.
pub async fn run_get_anchors_by_entity(
    rpc: &RpcClient,
    entity_id_hex: &str,
    start_height: u64,
    end_height: u64,
    json: bool,
) -> Result<(), String> {
    let anchors = rpc
        .get_oracle_anchors_by_entity(entity_id_hex, start_height, end_height)
        .await?;
    if json {
        println!("{}", serde_json::json!({ "anchors": anchors }));
    } else if anchors.is_empty() {
        println!("No anchors for entity {entity_id_hex} in [{start_height}, {end_height}]");
    } else {
        println!("Anchors by entity {entity_id_hex}:");
        for a in &anchors {
            print_anchor(a);
        }
    }
    Ok(())
}

/// Query anchors by tag within an inclusive height window.
pub async fn run_get_anchors_by_tag(
    rpc: &RpcClient,
    data_tag: &str,
    start_height: u64,
    end_height: u64,
    json: bool,
) -> Result<(), String> {
    let anchors = rpc
        .get_oracle_anchors_by_tag(data_tag, start_height, end_height)
        .await?;
    if json {
        println!("{}", serde_json::json!({ "anchors": anchors }));
    } else if anchors.is_empty() {
        println!("No anchors for tag '{data_tag}' in [{start_height}, {end_height}]");
    } else {
        println!("Anchors by tag '{data_tag}':");
        for a in &anchors {
            print_anchor(a);
        }
    }
    Ok(())
}

/// Fetch and display a single anchor by signal hash.
pub async fn run_show(rpc: &RpcClient, signal_hash_hex: &str, json: bool) -> Result<(), String> {
    let anchor = rpc.get_oracle_anchor(signal_hash_hex).await?;
    match anchor {
        None => {
            if json {
                println!("{}", serde_json::json!({ "anchor": null }));
            } else {
                println!("Anchor not found: {signal_hash_hex}");
            }
        }
        Some(a) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&a).unwrap());
            } else {
                print_anchor(&a);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_payload_layout_is_correct() {
        let p = build_anchor_payload(
            &[0x10; 32],
            &[0x01; 32],
            &[0xAB; 32],
            0x0102_0304_0506_0708,
            &[0xCD; 32],
            5000,
            b"price/ETH-USD",
        )
        .unwrap();
        assert_eq!(p.len(), 66 + 81 + 13);
        assert_eq!(p[0], 2);
        assert_eq!(&p[1..33], &[0x10; 32]);
        assert_eq!(p[33], 22);
        assert_eq!(&p[34..66], &[0x01; 32]);
        assert_eq!(&p[66..98], &[0xAB; 32]);
        assert_eq!(&p[98..106], &0x0102_0304_0506_0708u64.to_be_bytes());
        assert_eq!(&p[106..138], &[0xCD; 32]);
        assert_eq!(&p[138..146], &5000u64.to_be_bytes());
        assert_eq!(p[146], 13);
        assert_eq!(&p[147..160], b"price/ETH-USD");
    }

    #[test]
    fn anchor_payload_rejects_empty_tag() {
        assert!(build_anchor_payload(&[0; 32], &[1; 32], &[1; 32], 1, &[0; 32], 0, b"").is_err());
    }

    #[test]
    fn anchor_payload_rejects_oversized_tag() {
        let tag = vec![0x5A; 33];
        assert!(build_anchor_payload(&[0; 32], &[1; 32], &[1; 32], 1, &[0; 32], 0, &tag).is_err());
    }

    #[test]
    fn anchor_payload_accepts_min_and_max_tag() {
        assert!(build_anchor_payload(&[0; 32], &[1; 32], &[1; 32], 1, &[0; 32], 0, b"x").is_ok());
        let max = vec![0x5A; 32];
        assert!(build_anchor_payload(&[0; 32], &[1; 32], &[1; 32], 1, &[0; 32], 0, &max).is_ok());
    }

    #[test]
    fn signal_hash_is_content_addressed() {
        let base = derive_signal_hash(&[1; 32], &[2; 32], 100, &[3; 32], b"price/ETH-USD");
        assert_eq!(
            base,
            derive_signal_hash(&[1; 32], &[2; 32], 100, &[3; 32], b"price/ETH-USD"),
            "identical content must collide"
        );
        assert_ne!(
            base,
            derive_signal_hash(&[1; 32], &[9; 32], 100, &[3; 32], b"price/ETH-USD")
        );
        assert_ne!(
            base,
            derive_signal_hash(&[1; 32], &[2; 32], 101, &[3; 32], b"price/ETH-USD")
        );
        assert_ne!(
            base,
            derive_signal_hash(&[1; 32], &[2; 32], 100, &[3; 32], b"price/BTC-USD")
        );
    }

    #[test]
    fn bad_hex_data_hash_rejected() {
        assert!(parse_hex32("zzz", "data_hash").is_err());
    }
}
