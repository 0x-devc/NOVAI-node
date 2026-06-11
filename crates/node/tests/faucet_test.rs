//! E2E test for the public faucet HTTP route.
//!
//! Spins the RPC server in-process with minimal state, issues a raw HTTP/1.1
//! GET to /faucet/<address>, and asserts the public contract: status 200 with
//! a JSON body advertising `amount=100000`.
//!
//! This guards two call sites in crates/node/src/rpc.rs against silent drift:
//!   - The /faucet/ short-circuit inside start_rpc_server_with_state that
//!     branches on method == GET and url.starts_with("/faucet/")
//!   - The resolve_client_ip invocation that feeds handle_public_faucet
//!
//! The test deliberately uses std::net::TcpStream rather than reqwest so the
//! workspace dev-dependency surface stays unchanged.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use mempool::{NonceProvider, TxMempool};
use novai_node::consensus_node::Storage;
use novai_node::rpc::{start_rpc_server_with_state, BlockchainIndex, CidrBlock};
use novai_state::MemKv;
use novai_types::Address;
use rand_core::OsRng;

/// Always-zero nonce provider. The faucet keypair is freshly generated per
/// run so no prior nonce state can exist for its derived address.
struct ZeroNonceProvider;

impl NonceProvider for ZeroNonceProvider {
    fn expected_nonce(&self, _from: &Address) -> u64 {
        0
    }
}

/// Bind on an ephemeral port, capture the kernel-assigned port, drop the
/// listener. There is a tiny race window before the RPC server rebinds, but
/// it is acceptable for a single in-process test.
fn find_free_port() -> u16 {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .expect("bind ephemeral port to discover a free one");
    listener.local_addr().expect("local_addr").port()
}

/// Send a raw HTTP/1.1 GET and return (status_code, body). std-only so the
/// test does not pull reqwest or hyper into the workspace.
fn raw_http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut stream =
        TcpStream::connect_timeout(&addr, Duration::from_secs(5)).expect("connect to RPC server");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");

    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("write HTTP request");

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).expect("read HTTP response");
    let raw = String::from_utf8_lossy(&buf).into_owned();

    let status_line = raw.lines().next().expect("status line present in response");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .expect("status code present in status line")
        .parse()
        .expect("status code parses as u16");

    let body_offset = raw
        .find("\r\n\r\n")
        .expect("CRLF body separator present in response")
        + 4;
    (status, raw[body_offset..].to_string())
}

#[test]
fn public_faucet_route_returns_200_and_amount_100k() {
    // Faucet key. handle_public_faucet returns 503 without one.
    let faucet_key = SigningKey::generate(&mut OsRng);

    // Minimal RPC state. No consensus, no gossip, no peers. The public
    // faucet path responds before the dispensed tx is committed, so none of
    // those are needed for the observable the test asserts.
    let mempool = Arc::new(Mutex::new(TxMempool::new(1, 1000)));
    let nonce_provider: Arc<dyn NonceProvider + Send + Sync> = Arc::new(ZeroNonceProvider);
    let db = Arc::new(Mutex::new(Storage::Memory(MemKv::new())));
    let blockchain_index = Arc::new(Mutex::new(BlockchainIndex::new()));

    // Per-IP rate-limit state lives on disk. Use a unique tempdir per run so
    // parallel test runs and reruns do not collide.
    let tmp_dir = std::env::temp_dir().join(format!(
        "novai-faucet-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create faucet rate-limit tmp dir");
    let rate_limit_path = tmp_dir.join("faucet_rate_limit.json");

    // start_rpc_server_with_state does not surface the bound port, so port 0
    // cannot be used directly; discover a port first.
    let port = find_free_port();
    let bind_addr = format!("127.0.0.1:{port}");
    let socket_addr: SocketAddr = bind_addr.parse().expect("parse bind addr");

    start_rpc_server_with_state(
        &bind_addr,
        Arc::clone(&mempool),
        Arc::clone(&nonce_provider),
        Arc::clone(&db),
        false,
        Arc::clone(&blockchain_index),
        Some(faucet_key),
        Vec::<CidrBlock>::new(),
        rate_limit_path.clone(),
        None,
    )
    .expect("start RPC server");

    // Give the spawned thread a moment to enter incoming_requests(). The
    // tiny_http listener is bound inside Server::http before
    // start_rpc_server_with_state returns, so a short sleep suffices.
    thread::sleep(Duration::from_millis(200));

    let addr_hex = "11".repeat(32);
    let (status, body) = raw_http_get(socket_addr, &format!("/faucet/{addr_hex}"));

    assert_eq!(
        status, 200,
        "GET /faucet/<addr> should return 200 OK; body was: {body}",
    );
    assert!(
        body.contains(r#""amount":"100000""#),
        "response body should advertise PUBLIC_FAUCET_AMOUNT of 100000; body was: {body}",
    );
    assert!(
        body.contains(&format!(r#""to":"{addr_hex}""#)),
        "response body should echo the destination address; body was: {body}",
    );
    assert!(
        body.contains(r#""txid":""#),
        "response body should include a txid; body was: {body}",
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}
