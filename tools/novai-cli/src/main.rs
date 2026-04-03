mod commands;
mod rpc_client;

use clap::{Parser, Subcommand};
use commands::{account, ai, keygen, memory, signal};
use rpc_client::RpcClient;

/// NOVAI CLI — interact with NOVAI blockchain nodes.
#[derive(Parser)]
#[command(name = "novai-cli", version, about)]
struct Cli {
    /// RPC endpoint URL.
    #[arg(long, default_value = "http://localhost:3030", global = true)]
    endpoint: String,

    /// Output as JSON instead of human-readable text.
    #[arg(long, default_value_t = false, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a new Ed25519 keypair.
    Keygen {
        /// Path to save the key file.
        #[arg(long)]
        output: String,
    },
    /// Show address and public key from an existing key file.
    KeyInfo {
        /// Path to the key file.
        #[arg(long)]
        key_file: String,
    },
    /// Query account balance and nonce.
    Balance {
        /// Hex-encoded 32-byte address.
        #[arg(long)]
        address: String,
    },
    /// Query expected nonce for an address.
    Nonce {
        /// Hex-encoded 32-byte address.
        #[arg(long)]
        address: String,
    },
    /// Request testnet tokens from the faucet.
    Faucet {
        /// Hex-encoded 32-byte address to fund.
        #[arg(long)]
        address: String,
    },
    /// Transfer tokens to another address.
    Transfer {
        /// Path to sender's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte recipient address.
        #[arg(long)]
        to: String,
        /// Amount to transfer.
        #[arg(long)]
        amount: u64,
        /// Transaction fee (default: 100).
        #[arg(long, default_value_t = 100)]
        fee: u64,
    },
    /// AI entity operations.
    Ai {
        #[command(subcommand)]
        command: AiCommand,
    },
    /// Memory object operations.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Signal operations.
    Signal {
        #[command(subcommand)]
        command: SignalCommand,
    },
}

#[derive(Subcommand)]
enum AiCommand {
    /// Register a new AI entity.
    Register {
        /// Path to creator's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte code hash.
        #[arg(long)]
        code_hash: String,
        /// Autonomy mode: advisory or gated.
        #[arg(long, default_value = "advisory")]
        autonomy: String,
        /// Comma-separated capabilities: read_chain, read_memory, emit_proposals, request_execution, read_nnpx.
        #[arg(long, default_value = "read_chain,read_memory,emit_proposals")]
        capabilities: String,
        /// Initial balance to fund the entity.
        #[arg(long)]
        initial_balance: u128,
        /// Transaction fee (default: 5000).
        #[arg(long, default_value_t = 5000)]
        fee: u64,
    },
    /// Register a new AI entity with its own signing key.
    RegisterWithKey {
        /// Path to creator's key file.
        #[arg(long)]
        key_file: String,
        /// Path to entity's key file.
        #[arg(long)]
        entity_key_file: String,
        /// Hex-encoded 32-byte code hash.
        #[arg(long)]
        code_hash: String,
        /// Autonomy mode: advisory or gated.
        #[arg(long, default_value = "advisory")]
        autonomy: String,
        /// Comma-separated capabilities.
        #[arg(long, default_value = "read_chain,read_memory,emit_proposals")]
        capabilities: String,
        /// Initial balance to fund the entity.
        #[arg(long)]
        initial_balance: u128,
        /// Transaction fee (default: 5000).
        #[arg(long, default_value_t = 5000)]
        fee: u64,
    },
    /// Credit (fund) an existing AI entity.
    Credit {
        /// Path to sender's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte entity ID to credit.
        #[arg(long)]
        entity_id: String,
        /// Amount to credit.
        #[arg(long)]
        amount: u128,
        /// Transaction fee (default: 100).
        #[arg(long, default_value_t = 100)]
        fee: u64,
    },
    /// Query AI entity state.
    Info {
        /// Hex-encoded 32-byte entity ID.
        #[arg(long)]
        entity_id: String,
    },
}

#[derive(Subcommand)]
enum MemoryCommand {
    /// Create a new memory object.
    Create {
        /// Path to entity's key file.
        #[arg(long)]
        key_file: String,
        /// Memory object type: chain-summary, label-index, embedding-commitment, anomaly-log, statistics-snapshot.
        #[arg(long, name = "type")]
        object_type: String,
        /// Data as UTF-8 string (mutually exclusive with --data-file).
        #[arg(long, group = "data_source")]
        data: Option<String>,
        /// Path to file containing data bytes (mutually exclusive with --data).
        #[arg(long, group = "data_source")]
        data_file: Option<String>,
        /// Transaction fee (default: 500).
        #[arg(long, default_value_t = 500)]
        fee: u64,
    },
    /// Update an existing memory object.
    Update {
        /// Path to entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte object ID.
        #[arg(long)]
        object_id: String,
        /// Data as UTF-8 string.
        #[arg(long, group = "data_source")]
        data: Option<String>,
        /// Path to file containing data bytes.
        #[arg(long, group = "data_source")]
        data_file: Option<String>,
        /// Transaction fee (default: 500).
        #[arg(long, default_value_t = 500)]
        fee: u64,
    },
    /// Delete a memory object.
    Delete {
        /// Path to entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte object ID.
        #[arg(long)]
        object_id: String,
        /// Transaction fee (default: 500).
        #[arg(long, default_value_t = 500)]
        fee: u64,
    },
    /// List memory objects for an entity.
    List {
        /// Hex-encoded 32-byte entity ID.
        #[arg(long)]
        entity_id: String,
    },
}

#[derive(Subcommand)]
enum SignalCommand {
    /// Publish a signal commitment.
    Publish {
        /// Path to issuer entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte signal hash.
        #[arg(long)]
        signal_hash: String,
        /// Signal type: anomaly, optimization, prediction, risk-score, audit-report, spam-risk, congestion-forecast.
        #[arg(long)]
        signal_type: String,
        /// Hex-encoded 32-byte issuer entity ID.
        #[arg(long)]
        issuer_entity_id: String,
        /// Transaction fee (default: 1000).
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Query signals by block height.
    ByHeight {
        /// Block height.
        #[arg(long)]
        height: u64,
    },
    /// Query signals by issuer.
    ByIssuer {
        /// Hex-encoded 32-byte issuer entity ID.
        #[arg(long)]
        issuer: String,
        /// Start height (inclusive).
        #[arg(long)]
        start: u64,
        /// End height (inclusive).
        #[arg(long)]
        end: u64,
    },
    /// Query signals by type.
    ByType {
        /// Signal type name.
        #[arg(long, name = "type")]
        signal_type: String,
        /// Start height (inclusive).
        #[arg(long)]
        start: u64,
        /// End height (inclusive).
        #[arg(long)]
        end: u64,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let rpc = RpcClient::new(&cli.endpoint);

    let result = match cli.command {
        Command::Keygen { output } => keygen::run_keygen(&output),
        Command::KeyInfo { key_file } => keygen::run_key_info(&key_file),
        Command::Balance { address } => account::run_balance(&rpc, &address, cli.json).await,
        Command::Nonce { address } => account::run_nonce(&rpc, &address, cli.json).await,
        Command::Faucet { address } => account::run_faucet(&rpc, &address, cli.json).await,
        Command::Transfer {
            key_file,
            to,
            amount,
            fee,
        } => account::run_transfer(&rpc, &key_file, &to, amount, fee, cli.json).await,
        Command::Ai { command } => match command {
            AiCommand::Register {
                key_file,
                code_hash,
                autonomy,
                capabilities,
                initial_balance,
                fee,
            } => {
                ai::run_register(
                    &rpc,
                    &key_file,
                    &code_hash,
                    &autonomy,
                    &capabilities,
                    initial_balance,
                    fee,
                    cli.json,
                )
                .await
            }
            AiCommand::RegisterWithKey {
                key_file,
                entity_key_file,
                code_hash,
                autonomy,
                capabilities,
                initial_balance,
                fee,
            } => {
                ai::run_register_with_key(
                    &rpc,
                    &key_file,
                    &entity_key_file,
                    &code_hash,
                    &autonomy,
                    &capabilities,
                    initial_balance,
                    fee,
                    cli.json,
                )
                .await
            }
            AiCommand::Credit {
                key_file,
                entity_id,
                amount,
                fee,
            } => ai::run_credit(&rpc, &key_file, &entity_id, amount, fee, cli.json).await,
            AiCommand::Info { entity_id } => ai::run_info(&rpc, &entity_id, cli.json).await,
        },
        Command::Memory { command } => match command {
            MemoryCommand::Create {
                key_file,
                object_type,
                data,
                data_file,
                fee,
            } => {
                memory::run_create(
                    &rpc,
                    &key_file,
                    &object_type,
                    data,
                    data_file,
                    fee,
                    cli.json,
                )
                .await
            }
            MemoryCommand::Update {
                key_file,
                object_id,
                data,
                data_file,
                fee,
            } => {
                memory::run_update(&rpc, &key_file, &object_id, data, data_file, fee, cli.json)
                    .await
            }
            MemoryCommand::Delete {
                key_file,
                object_id,
                fee,
            } => memory::run_delete(&rpc, &key_file, &object_id, fee, cli.json).await,
            MemoryCommand::List { entity_id } => memory::run_list(&rpc, &entity_id, cli.json).await,
        },
        Command::Signal { command } => match command {
            SignalCommand::Publish {
                key_file,
                signal_hash,
                signal_type,
                issuer_entity_id,
                fee,
            } => {
                signal::run_publish(
                    &rpc,
                    &key_file,
                    &signal_hash,
                    &signal_type,
                    &issuer_entity_id,
                    fee,
                    cli.json,
                )
                .await
            }
            SignalCommand::ByHeight { height } => {
                signal::run_by_height(&rpc, height, cli.json).await
            }
            SignalCommand::ByIssuer { issuer, start, end } => {
                signal::run_by_issuer(&rpc, &issuer, start, end, cli.json).await
            }
            SignalCommand::ByType {
                signal_type,
                start,
                end,
            } => signal::run_by_type(&rpc, &signal_type, start, end, cli.json).await,
        },
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
