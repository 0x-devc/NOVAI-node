use futures::StreamExt;
use libp2p::{noise, ping, swarm::SwarmEvent, tcp, yamux, Multiaddr};

use mempool::Mempool;
use novai_types::Tx;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // --- Simple CLI parsing ---
    // Examples:
    //   cargo run -p novai-node -- submit-tx hello
    //   cargo run -p novai-node -- /ip4/1.2.3.4/tcp/1234
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().map(|s| s.as_str()) == Some("submit-tx") {
        let payload = args.get(1).cloned().unwrap_or_else(|| "hello".to_string());

        // For now this is an in-memory mempool demo (fresh each run).
        // Later we'll keep it alive in the node and add drain commands.
        let mut mp: Mempool<u64, Tx> = Mempool::new(|tx: &Tx| tx.id);

        let id = 1u64;
        let tx = Tx {
            id,
            payload: payload.into_bytes(),
        };

        match mp.insert(tx) {
            Ok(()) => println!("submitted tx id={id} (mempool size={})", mp.len()),
            Err(e) => eprintln!("submit failed: {e:?}"),
        }

        return Ok(());
    }

    // --- Existing libp2p ping demo ---
    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|_| ping::Behaviour::default())?
        .build();

    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    // If the first arg is not "submit-tx", treat it as an optional multiaddr to dial.
    if let Some(addr) = args.first() {
        let addr: Multiaddr = addr.parse()?;
        swarm.dial(addr)?;
    }

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("Listening on {address}");
            }
            event => {
                println!("{event:?}");
            }
        }
    }
}
