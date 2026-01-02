fn main() {
    let result: Result<(), Box<dyn std::error::Error>> = (|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        rt.block_on(async_main())
    })();

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    use futures::StreamExt;
    use libp2p::{noise, ping, swarm::SwarmEvent, tcp, yamux, Multiaddr};

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

    if let Some(addr) = std::env::args().nth(1) {
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
