mod commands;
mod rpc_client;

use clap::{Parser, Subcommand};
use commands::signal::ExtendedSignalArgs;
use commands::{account, ai, channel, keygen, memory, oracle, service, signal, sla, vk};
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
#[allow(clippy::large_enum_variant)]
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
    /// Agent Discovery Registry operations (Week 29).
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// VK Registry operations (Week 30): publish, update label, delete,
    /// show, and list zero-knowledge proof verification keys on chain.
    Vk {
        #[command(subcommand)]
        command: VkCommand,
    },
    /// SLA Agreement operations (Week 31): propose, accept, cancel,
    /// show, and list two-party service level agreements with
    /// auto-slash on threshold breach.
    Sla {
        #[command(subcommand)]
        command: SlaCommand,
    },
    /// Payment Channel operations (Week 32): propose, accept,
    /// sign-update, close, finalize, cancel, show, list, and
    /// dispute-status for bidirectional payment channels between
    /// two AI entities.
    Channel {
        #[command(subcommand)]
        command: ChannelCommand,
    },
    /// Oracle anchoring operations (Week 35): post-anchor and query
    /// commitments to external off-chain data (price feeds, API
    /// responses) from registered, reputation-bearing oracle entities.
    Oracle {
        #[command(subcommand)]
        command: OracleCommand,
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
        /// Comma-separated capabilities: read_chain, read_memory, emit_proposals, request_execution, read_nnpx, post_oracle_anchors.
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
    /// Upgrade an AI entity's code hash (preserves the entity id and all state).
    Upgrade {
        /// Path to creator's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte entity ID to upgrade.
        #[arg(long)]
        entity_id: String,
        /// Hex-encoded 32-byte new code hash.
        #[arg(long)]
        new_code_hash: String,
        /// Optional hex-encoded 32-byte reason commitment.
        #[arg(long)]
        reason_hash: Option<String>,
        /// Transaction fee (default: 5000).
        #[arg(long, default_value_t = 5000)]
        fee: u64,
    },
    /// Query an AI entity's upgrade history.
    UpgradeHistory {
        /// Hex-encoded 32-byte entity ID.
        #[arg(long)]
        entity_id: String,
        /// Inclusive start height.
        #[arg(long, default_value_t = 0)]
        start_height: u64,
        /// Inclusive end height.
        #[arg(long, default_value_t = 10_000)]
        end_height: u64,
    },
}

#[derive(Subcommand)]
enum MemoryCommand {
    /// Create a new memory object.
    Create {
        /// Path to entity's key file.
        #[arg(long)]
        key_file: String,
        /// Memory object type: chain-summary, label-index, embedding-commitment, anomaly-log,
        /// statistics-snapshot, reputation-event, rating, signal-catalog, composition-graph,
        /// verification-record.
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
#[allow(clippy::large_enum_variant)]
enum ServiceCommand {
    /// Publish a new service descriptor in the Agent Discovery Registry.
    Publish {
        /// Path to publisher entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte off-chain service name commitment.
        #[arg(long)]
        service_name_hash: String,
        /// Hex-encoded 32-byte off-chain endpoint URL commitment.
        #[arg(long)]
        service_url_hash: String,
        /// Hex-encoded 32-byte off-chain long description commitment.
        #[arg(long)]
        description_hash: String,
        /// Service category: generic, data-oracle, inference, compute,
        /// storage, indexer, signal-provider, verification, monitoring,
        /// gateway.
        #[arg(long)]
        category: String,
        /// Per-call price in base units (0 = free).
        #[arg(long)]
        price_per_call: u64,
        /// Per-block subscription rate (0 = no subscription offered).
        #[arg(long, default_value_t = 0)]
        subscription_rate: u64,
        /// Minimum caller reputation score (0..=100).
        #[arg(long, default_value_t = 0)]
        min_reputation: u16,
        /// Minimum caller stake balance (0 = no stake required).
        #[arg(long, default_value_t = 0)]
        min_stake: u128,
        /// Capability tags bitfield.
        #[arg(long, default_value_t = 0)]
        capability_tags: u32,
        /// Initial status: active, paused, or deprecated.
        #[arg(long, default_value = "active")]
        status: String,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Update an existing service descriptor. All fields must be re-
    /// supplied; `category` is immutable and must match the published
    /// value.
    Update {
        /// Path to publisher entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte object id of the descriptor to update.
        #[arg(long)]
        object_id: String,
        #[arg(long)]
        service_name_hash: String,
        #[arg(long)]
        service_url_hash: String,
        #[arg(long)]
        description_hash: String,
        #[arg(long)]
        category: String,
        #[arg(long)]
        price_per_call: u64,
        #[arg(long, default_value_t = 0)]
        subscription_rate: u64,
        #[arg(long, default_value_t = 0)]
        min_reputation: u16,
        #[arg(long, default_value_t = 0)]
        min_stake: u128,
        #[arg(long, default_value_t = 0)]
        capability_tags: u32,
        #[arg(long, default_value = "active")]
        status: String,
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Delete a service descriptor.
    Delete {
        /// Path to publisher entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte object id of the descriptor to delete.
        #[arg(long)]
        object_id: String,
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// List all service descriptors in a category.
    List {
        /// Category name (see Publish for valid values).
        #[arg(long)]
        category: String,
    },
}

#[derive(Subcommand)]
enum VkCommand {
    /// Register a Groth16 verification key on chain.
    Register {
        /// Path to publisher entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte canonical code hash the VK verifies.
        #[arg(long)]
        code_hash: String,
        /// Path to the compressed VK bytes (ark-serialize format).
        #[arg(long)]
        vk_file: String,
        /// Proof system name. Only 'groth16' is wired in v1.
        #[arg(long, default_value = "groth16")]
        proof_type: String,
        /// Optional free-form label (max 32 bytes UTF-8).
        #[arg(long, default_value = "")]
        label: String,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Update only the `label` field of an existing VK registration.
    /// `proof_type`, `code_hash`, and `vk_bytes` are immutable; use
    /// `Delete` + `Register` to change them.
    UpdateLabel {
        /// Path to publisher entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte object id of the registration to update.
        #[arg(long)]
        object_id: String,
        /// New label (max 32 bytes UTF-8). Pass `""` to clear it.
        #[arg(long)]
        label: String,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Delete a VK registration. Subsequent ProofSubmission signals
    /// referencing this handle will fail with `VkRegistrationNotFound`.
    Delete {
        /// Path to publisher entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte object id of the registration to delete.
        #[arg(long)]
        object_id: String,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Show a single VK registration by id.
    Show {
        /// Hex-encoded 32-byte object id.
        #[arg(long)]
        id: String,
    },
    /// List all VK registrations owned by an entity.
    List {
        /// Hex-encoded 32-byte entity id.
        #[arg(long)]
        entity_id: String,
    },
}

#[derive(Subcommand)]
enum SlaCommand {
    /// Propose a new SLA against a seller (wraps CreateMemoryObject
    /// payload v3 with type 14).
    Propose {
        /// Path to buyer entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte buyer entity id (must equal the signer).
        #[arg(long)]
        buyer_entity_id: String,
        /// Hex-encoded 32-byte seller entity id.
        #[arg(long)]
        seller_entity_id: String,
        /// Hex-encoded 32-byte service descriptor reference. Zero
        /// (`00..`) means no reference; informational only.
        #[arg(
            long,
            default_value = "0000000000000000000000000000000000000000000000000000000000000000"
        )]
        service_descriptor_hash: String,
        /// First block height inside the violation window.
        #[arg(long)]
        start_height: u64,
        /// Last block height inside the violation window (must be
        /// strictly greater than start_height; window <= 604800 blocks).
        #[arg(long)]
        end_height: u64,
        /// Number of in-window FAILED attestations that must accumulate
        /// before auto-slash fires. Must be >= 1.
        #[arg(long)]
        violation_threshold: u32,
        /// Penalty paid on threshold breach (saturating against the
        /// seller's stake_balance). Must be > 0.
        #[arg(long)]
        slash_amount: u128,
        /// Per-call price (informational; NAP enforces actual payments).
        #[arg(long, default_value_t = 0)]
        price_per_call: u64,
        /// RESERVED v1: maximum acceptable response time in blocks.
        #[arg(long, default_value_t = 0)]
        max_response_time_blocks: u32,
        /// RESERVED v1: minimum uptime in basis points (<= 10000).
        #[arg(long, default_value_t = 0)]
        min_uptime_bps: u16,
        /// RESERVED v1: minimum delivery success in basis points
        /// (<= 10000).
        #[arg(long, default_value_t = 0)]
        min_delivery_success_bps: u16,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Accept a proposed SLA (wraps the SlaAccept signal type 18).
    /// The signer must be the SLA's seller. Rejected if the seller's
    /// stake_balance is below the SLA's slash_amount.
    Accept {
        /// Path to seller entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte SLA memory object id.
        #[arg(long)]
        sla_object_id: String,
        /// Hex-encoded 32-byte buyer entity id (memory-object owner).
        #[arg(long)]
        buyer_entity_id: String,
        /// Hex-encoded 32-byte seller entity id (must equal the signer).
        #[arg(long)]
        seller_entity_id: String,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Cancel a still-Proposed SLA by deleting the memory object.
    /// Active SLAs inside their window cannot be cancelled.
    Cancel {
        /// Path to buyer entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte SLA memory object id.
        #[arg(long)]
        sla_object_id: String,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Show a single SLA by `(owner, object_id)`.
    Show {
        /// Hex-encoded 32-byte buyer entity id (memory-object owner).
        #[arg(long)]
        owner: String,
        /// Hex-encoded 32-byte SLA memory object id.
        #[arg(long)]
        object_id: String,
    },
    /// Resolve the currently-open SLA between a buyer and a seller
    /// via the active-between singleton.
    Active {
        /// Hex-encoded 32-byte buyer entity id.
        #[arg(long)]
        buyer: String,
        /// Hex-encoded 32-byte seller entity id.
        #[arg(long)]
        seller: String,
    },
    /// List SLAs where the entity is the buyer (memory-object owner).
    ListByBuyer {
        /// Hex-encoded 32-byte buyer entity id.
        #[arg(long)]
        entity_id: String,
        /// Inclusive lower bound on `created_at` height.
        #[arg(long, default_value_t = 0)]
        start_height: u64,
        /// Inclusive upper bound on `created_at` height. The runtime
        /// caps the span at 10_000 heights per query.
        #[arg(long, default_value_t = 10_000)]
        end_height: u64,
    },
    /// List SLAs where the entity is the seller.
    ListBySeller {
        /// Hex-encoded 32-byte seller entity id.
        #[arg(long)]
        entity_id: String,
        /// Inclusive lower bound on `created_at` height.
        #[arg(long, default_value_t = 0)]
        start_height: u64,
        /// Inclusive upper bound on `created_at` height.
        #[arg(long, default_value_t = 10_000)]
        end_height: u64,
    },
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum ChannelCommand {
    /// Propose a new payment channel against a counterparty (wraps
    /// CreateMemoryObject payload v3 with type 15). Party A's
    /// deposit is debited at create time; party B accepts later via
    /// `channel accept`.
    Propose {
        /// Path to party A's key file (the proposer).
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte party A entity id (must equal the
        /// signer derived from `--key-file`).
        #[arg(long)]
        party_a_entity_id: String,
        /// Hex-encoded 32-byte party B entity id (the counterparty).
        #[arg(long)]
        party_b_entity_id: String,
        /// Hex-encoded 32-byte SLA reference. Zero (`00..`) means no
        /// reference; informational only in v1.
        #[arg(
            long,
            default_value = "0000000000000000000000000000000000000000000000000000000000000000"
        )]
        sla_object_id: String,
        /// Party A's deposit (decimal u128).
        #[arg(long)]
        deposit_a: u128,
        /// Party B's deposit (decimal u128). Debited from party B at
        /// accept time.
        #[arg(long)]
        deposit_b: u128,
        /// Dispute window length in blocks. Must be in
        /// `[100, 10_000]`. Default = 256.
        #[arg(long, default_value_t = 256)]
        dispute_window_blocks: u32,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Accept a PROPOSED payment channel (wraps the ChannelAccept
    /// signal type 19). Party B's deposit is debited at this step.
    Accept {
        /// Path to party B's key file (the accepter).
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte channel memory object id.
        #[arg(long)]
        channel_object_id: String,
        /// Hex-encoded 32-byte party A entity id (memory object owner).
        #[arg(long)]
        party_a_entity_id: String,
        /// Hex-encoded 32-byte party B entity id (must equal the
        /// signer).
        #[arg(long)]
        party_b_entity_id: String,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// OFFLINE: produce an ed25519 signature over the canonical
    /// channel state signing bytes. The signing key file MUST
    /// correspond to the entity pubkey registered on-chain. Outputs
    /// a hex-encoded 64-byte signature for use with `channel close`.
    SignUpdate {
        /// Path to the signer's channel-state key file (32-byte seed).
        #[arg(long)]
        signing_key: String,
        /// Hex-encoded 32-byte channel memory object id.
        #[arg(long)]
        channel_object_id: String,
        /// Hex-encoded 32-byte party A entity id.
        #[arg(long)]
        party_a_entity_id: String,
        /// Hex-encoded 32-byte party B entity id.
        #[arg(long)]
        party_b_entity_id: String,
        /// Off-chain state nonce being signed.
        #[arg(long)]
        nonce: u64,
        /// Party A's balance in this state (decimal u128).
        #[arg(long)]
        balance_a: u128,
        /// Party B's balance in this state (decimal u128).
        #[arg(long)]
        balance_b: u128,
        /// Mark this state as the cooperative-settle final state.
        /// When set, both parties must sign `is_final = true` and
        /// the close transition skips the dispute window.
        #[arg(long)]
        final_state: bool,
    },
    /// Submit a ChannelClose signal (type 20). Both `--sig-a` and
    /// `--sig-b` (hex-encoded 64-byte signatures) MUST verify on-
    /// chain; produce them with `channel sign-update`. When
    /// `--final` is set the close is cooperative (instant settle);
    /// otherwise it opens or extends the dispute window.
    Close {
        /// Path to the submitter's tx-signing key file (must be
        /// party A or party B per
        /// `ChannelCloseSubmitterNotParticipant`).
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte channel memory object id.
        #[arg(long)]
        channel_object_id: String,
        /// Hex-encoded 32-byte party A entity id.
        #[arg(long)]
        party_a_entity_id: String,
        /// Hex-encoded 32-byte party B entity id.
        #[arg(long)]
        party_b_entity_id: String,
        /// Off-chain state nonce being applied.
        #[arg(long)]
        nonce: u64,
        /// Party A's balance in this state (decimal u128).
        #[arg(long)]
        balance_a: u128,
        /// Party B's balance in this state (decimal u128).
        #[arg(long)]
        balance_b: u128,
        /// Submit as a cooperative settle (`is_final = 1`).
        #[arg(long)]
        final_state: bool,
        /// Hex-encoded 64-byte signature by party A over the
        /// canonical state bytes.
        #[arg(long)]
        sig_a: String,
        /// Hex-encoded 64-byte signature by party B over the
        /// canonical state bytes.
        #[arg(long)]
        sig_b: String,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Submit a ChannelFinalize signal (type 21). Permissionless:
    /// any active AI entity may submit. Valid only when the channel
    /// is CLOSING and the dispute deadline has passed.
    Finalize {
        /// Path to the submitter's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte channel memory object id.
        #[arg(long)]
        channel_object_id: String,
        /// Hex-encoded 32-byte party A entity id (memory object owner).
        #[arg(long)]
        party_a_entity_id: String,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Cancel a still-PROPOSED channel (wraps DeleteMemoryObject
    /// payload v5). Party A's deposit is refunded; rejected if the
    /// channel is OPEN or CLOSING.
    Cancel {
        /// Path to party A's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte channel memory object id.
        #[arg(long)]
        channel_object_id: String,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// Show a single channel by `(owner, object_id)`.
    Show {
        /// Hex-encoded 32-byte party A entity id (memory object owner).
        #[arg(long)]
        owner: String,
        /// Hex-encoded 32-byte channel memory object id.
        #[arg(long)]
        object_id: String,
    },
    /// List channels where the entity is party A (memory object owner).
    ListByPartyA {
        /// Hex-encoded 32-byte entity id.
        #[arg(long)]
        entity_id: String,
        /// Inclusive lower bound on `created_at` height.
        #[arg(long, default_value_t = 0)]
        start_height: u64,
        /// Inclusive upper bound on `created_at` height.
        #[arg(long, default_value_t = 10_000)]
        end_height: u64,
    },
    /// List channels where the entity is party B (counterparty).
    ListByPartyB {
        /// Hex-encoded 32-byte entity id.
        #[arg(long)]
        entity_id: String,
        /// Inclusive lower bound on `created_at` height.
        #[arg(long, default_value_t = 0)]
        start_height: u64,
        /// Inclusive upper bound on `created_at` height.
        #[arg(long, default_value_t = 10_000)]
        end_height: u64,
    },
    /// Show dispute-window status for a channel: closing height,
    /// dispute deadline, current chain height, blocks remaining,
    /// and a finalize-ready flag.
    DisputeStatus {
        /// Hex-encoded 32-byte party A entity id (memory object owner).
        #[arg(long)]
        owner: String,
        /// Hex-encoded 32-byte channel memory object id.
        #[arg(long)]
        object_id: String,
    },
}

#[derive(Subcommand)]
enum OracleCommand {
    /// Post an oracle data anchor (signal type 22). The signing key must
    /// belong to the issuing entity, which must hold the
    /// `post_oracle_anchors` capability. The signal hash is derived from
    /// the anchor content, so re-posting identical data is rejected.
    PostAnchor {
        /// Path to the issuing entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte issuing entity id.
        #[arg(long)]
        issuer_entity_id: String,
        /// Hex-encoded 32-byte blake3 commitment to the off-chain data.
        #[arg(long)]
        data_hash: String,
        /// External (oracle-attested) timestamp of the data; must be non-zero.
        #[arg(long)]
        external_timestamp: u64,
        /// Optional hex-encoded 32-byte commitment to the data source.
        #[arg(long)]
        source_hash: Option<String>,
        /// Advisory intended-valid-until chain height (0 = none).
        #[arg(long, default_value_t = 0)]
        expiry_height: u64,
        /// Category tag, 1..=32 bytes (e.g. "price/ETH-USD").
        #[arg(long)]
        data_tag: String,
        /// Transaction fee.
        #[arg(long, default_value_t = 1000)]
        fee: u64,
    },
    /// List an entity's anchors within an inclusive chain-height window.
    GetAnchorsByEntity {
        /// Hex-encoded 32-byte entity id.
        #[arg(long)]
        entity_id: String,
        /// Inclusive lower bound on anchor height.
        #[arg(long, default_value_t = 0)]
        start_height: u64,
        /// Inclusive upper bound on anchor height.
        #[arg(long, default_value_t = 10_000)]
        end_height: u64,
    },
    /// List anchors posted under a tag within an inclusive height window.
    GetAnchorsByTag {
        /// Category tag (1..=32 bytes).
        #[arg(long)]
        data_tag: String,
        /// Inclusive lower bound on anchor height.
        #[arg(long, default_value_t = 0)]
        start_height: u64,
        /// Inclusive upper bound on anchor height.
        #[arg(long, default_value_t = 10_000)]
        end_height: u64,
    },
    /// Show a single anchor by its signal hash.
    Show {
        /// Hex-encoded 32-byte anchor signal hash.
        #[arg(long)]
        signal_hash: String,
    },
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum SignalCommand {
    /// Publish a signal commitment.
    Publish {
        /// Path to issuer entity's key file.
        #[arg(long)]
        key_file: String,
        /// Hex-encoded 32-byte signal hash.
        #[arg(long)]
        signal_hash: String,
        /// Signal type: anomaly, optimization, prediction, risk-score, audit-report,
        /// spam-risk, congestion-forecast, reputation-update, signal-purchase,
        /// stake-deposit, stake-withdraw, stake-slash, composition-check, proof-submission.
        #[arg(long)]
        signal_type: String,
        /// Hex-encoded 32-byte issuer entity ID.
        #[arg(long)]
        issuer_entity_id: String,
        /// Transaction fee (default: 1000).
        #[arg(long, default_value_t = 1000)]
        fee: u64,
        /// Extended payload arguments (required for signal types 7-13).
        #[command(flatten)]
        extra: ExtendedSignalArgs,
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
            AiCommand::Upgrade {
                key_file,
                entity_id,
                new_code_hash,
                reason_hash,
                fee,
            } => {
                ai::run_upgrade(
                    &rpc,
                    &key_file,
                    &entity_id,
                    &new_code_hash,
                    reason_hash.as_deref(),
                    fee,
                    cli.json,
                )
                .await
            }
            AiCommand::UpgradeHistory {
                entity_id,
                start_height,
                end_height,
            } => {
                ai::run_upgrade_history(&rpc, &entity_id, start_height, end_height, cli.json).await
            }
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
                extra,
            } => {
                signal::run_publish(
                    &rpc,
                    &key_file,
                    &signal_hash,
                    &signal_type,
                    &issuer_entity_id,
                    fee,
                    &extra,
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
        Command::Service { command } => match command {
            ServiceCommand::Publish {
                key_file,
                service_name_hash,
                service_url_hash,
                description_hash,
                category,
                price_per_call,
                subscription_rate,
                min_reputation,
                min_stake,
                capability_tags,
                status,
                fee,
            } => {
                service::run_publish(
                    &rpc,
                    &key_file,
                    &service_name_hash,
                    &service_url_hash,
                    &description_hash,
                    &category,
                    price_per_call,
                    subscription_rate,
                    min_reputation,
                    min_stake,
                    capability_tags,
                    &status,
                    fee,
                    cli.json,
                )
                .await
            }
            ServiceCommand::Update {
                key_file,
                object_id,
                service_name_hash,
                service_url_hash,
                description_hash,
                category,
                price_per_call,
                subscription_rate,
                min_reputation,
                min_stake,
                capability_tags,
                status,
                fee,
            } => {
                service::run_update(
                    &rpc,
                    &key_file,
                    &object_id,
                    &service_name_hash,
                    &service_url_hash,
                    &description_hash,
                    &category,
                    price_per_call,
                    subscription_rate,
                    min_reputation,
                    min_stake,
                    capability_tags,
                    &status,
                    fee,
                    cli.json,
                )
                .await
            }
            ServiceCommand::Delete {
                key_file,
                object_id,
                fee,
            } => service::run_delete(&rpc, &key_file, &object_id, fee, cli.json).await,
            ServiceCommand::List { category } => service::run_list(&rpc, &category, cli.json).await,
        },
        Command::Vk { command } => match command {
            VkCommand::Register {
                key_file,
                code_hash,
                vk_file,
                proof_type,
                label,
                fee,
            } => {
                vk::run_register(
                    &rpc,
                    &key_file,
                    &code_hash,
                    &vk_file,
                    &proof_type,
                    &label,
                    fee,
                    cli.json,
                )
                .await
            }
            VkCommand::UpdateLabel {
                key_file,
                object_id,
                label,
                fee,
            } => vk::run_update_label(&rpc, &key_file, &object_id, &label, fee, cli.json).await,
            VkCommand::Delete {
                key_file,
                object_id,
                fee,
            } => vk::run_delete(&rpc, &key_file, &object_id, fee, cli.json).await,
            VkCommand::Show { id } => vk::run_show(&rpc, &id, cli.json).await,
            VkCommand::List { entity_id } => vk::run_list(&rpc, &entity_id, cli.json).await,
        },
        Command::Sla { command } => match command {
            SlaCommand::Propose {
                key_file,
                buyer_entity_id,
                seller_entity_id,
                service_descriptor_hash,
                start_height,
                end_height,
                violation_threshold,
                slash_amount,
                price_per_call,
                max_response_time_blocks,
                min_uptime_bps,
                min_delivery_success_bps,
                fee,
            } => {
                sla::run_propose(
                    &rpc,
                    &key_file,
                    &buyer_entity_id,
                    &seller_entity_id,
                    &service_descriptor_hash,
                    start_height,
                    end_height,
                    violation_threshold,
                    slash_amount,
                    price_per_call,
                    max_response_time_blocks,
                    min_uptime_bps,
                    min_delivery_success_bps,
                    fee,
                    cli.json,
                )
                .await
            }
            SlaCommand::Accept {
                key_file,
                sla_object_id,
                buyer_entity_id,
                seller_entity_id,
                fee,
            } => {
                sla::run_accept(
                    &rpc,
                    &key_file,
                    &sla_object_id,
                    &buyer_entity_id,
                    &seller_entity_id,
                    fee,
                    cli.json,
                )
                .await
            }
            SlaCommand::Cancel {
                key_file,
                sla_object_id,
                fee,
            } => sla::run_cancel(&rpc, &key_file, &sla_object_id, fee, cli.json).await,
            SlaCommand::Show { owner, object_id } => {
                sla::run_show(&rpc, &owner, &object_id, cli.json).await
            }
            SlaCommand::Active { buyer, seller } => {
                sla::run_active(&rpc, &buyer, &seller, cli.json).await
            }
            SlaCommand::ListByBuyer {
                entity_id,
                start_height,
                end_height,
            } => sla::run_list_by_buyer(&rpc, &entity_id, start_height, end_height, cli.json).await,
            SlaCommand::ListBySeller {
                entity_id,
                start_height,
                end_height,
            } => {
                sla::run_list_by_seller(&rpc, &entity_id, start_height, end_height, cli.json).await
            }
        },
        Command::Channel { command } => match command {
            ChannelCommand::Propose {
                key_file,
                party_a_entity_id,
                party_b_entity_id,
                sla_object_id,
                deposit_a,
                deposit_b,
                dispute_window_blocks,
                fee,
            } => {
                channel::run_propose(
                    &rpc,
                    &key_file,
                    &party_a_entity_id,
                    &party_b_entity_id,
                    &sla_object_id,
                    deposit_a,
                    deposit_b,
                    dispute_window_blocks,
                    fee,
                    cli.json,
                )
                .await
            }
            ChannelCommand::Accept {
                key_file,
                channel_object_id,
                party_a_entity_id,
                party_b_entity_id,
                fee,
            } => {
                channel::run_accept(
                    &rpc,
                    &key_file,
                    &channel_object_id,
                    &party_a_entity_id,
                    &party_b_entity_id,
                    fee,
                    cli.json,
                )
                .await
            }
            ChannelCommand::SignUpdate {
                signing_key,
                channel_object_id,
                party_a_entity_id,
                party_b_entity_id,
                nonce,
                balance_a,
                balance_b,
                final_state,
            } => channel::run_sign_update(
                &signing_key,
                &channel_object_id,
                &party_a_entity_id,
                &party_b_entity_id,
                nonce,
                balance_a,
                balance_b,
                final_state,
                cli.json,
            ),
            ChannelCommand::Close {
                key_file,
                channel_object_id,
                party_a_entity_id,
                party_b_entity_id,
                nonce,
                balance_a,
                balance_b,
                final_state,
                sig_a,
                sig_b,
                fee,
            } => {
                channel::run_close(
                    &rpc,
                    &key_file,
                    &channel_object_id,
                    &party_a_entity_id,
                    &party_b_entity_id,
                    nonce,
                    balance_a,
                    balance_b,
                    final_state,
                    &sig_a,
                    &sig_b,
                    fee,
                    cli.json,
                )
                .await
            }
            ChannelCommand::Finalize {
                key_file,
                channel_object_id,
                party_a_entity_id,
                fee,
            } => {
                channel::run_finalize(
                    &rpc,
                    &key_file,
                    &channel_object_id,
                    &party_a_entity_id,
                    fee,
                    cli.json,
                )
                .await
            }
            ChannelCommand::Cancel {
                key_file,
                channel_object_id,
                fee,
            } => channel::run_cancel(&rpc, &key_file, &channel_object_id, fee, cli.json).await,
            ChannelCommand::Show { owner, object_id } => {
                channel::run_show(&rpc, &owner, &object_id, cli.json).await
            }
            ChannelCommand::ListByPartyA {
                entity_id,
                start_height,
                end_height,
            } => {
                channel::run_list_by_party_a(&rpc, &entity_id, start_height, end_height, cli.json)
                    .await
            }
            ChannelCommand::ListByPartyB {
                entity_id,
                start_height,
                end_height,
            } => {
                channel::run_list_by_party_b(&rpc, &entity_id, start_height, end_height, cli.json)
                    .await
            }
            ChannelCommand::DisputeStatus { owner, object_id } => {
                channel::run_dispute_status(&rpc, &owner, &object_id, cli.json).await
            }
        },
        Command::Oracle { command } => match command {
            OracleCommand::PostAnchor {
                key_file,
                issuer_entity_id,
                data_hash,
                external_timestamp,
                source_hash,
                expiry_height,
                data_tag,
                fee,
            } => {
                oracle::run_post_anchor(
                    &rpc,
                    &key_file,
                    &issuer_entity_id,
                    &data_hash,
                    external_timestamp,
                    source_hash.as_deref(),
                    expiry_height,
                    &data_tag,
                    fee,
                    cli.json,
                )
                .await
            }
            OracleCommand::GetAnchorsByEntity {
                entity_id,
                start_height,
                end_height,
            } => {
                oracle::run_get_anchors_by_entity(
                    &rpc,
                    &entity_id,
                    start_height,
                    end_height,
                    cli.json,
                )
                .await
            }
            OracleCommand::GetAnchorsByTag {
                data_tag,
                start_height,
                end_height,
            } => {
                oracle::run_get_anchors_by_tag(&rpc, &data_tag, start_height, end_height, cli.json)
                    .await
            }
            OracleCommand::Show { signal_hash } => {
                oracle::run_show(&rpc, &signal_hash, cli.json).await
            }
        },
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
