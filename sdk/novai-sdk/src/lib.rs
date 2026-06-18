//! NOVAI SDK — programmatic interface for interacting with NOVAI blockchain nodes.
//!
//! # Overview
//!
//! This SDK provides key management, transaction construction for all 11 NOVAI
//! transaction types, and an async RPC client for node communication.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use novai_sdk::{keys, tx, Client};
//! use novai_sdk::types::{AutonomyMode, Capabilities};
//!
//! # async fn example() -> Result<(), novai_sdk::Error> {
//! // Generate a keypair
//! let (sk, pk) = keys::generate();
//! let addr = keys::address(&pk);
//!
//! // Connect to a node
//! let client = Client::new("http://localhost:3030");
//!
//! // Get tokens from faucet
//! client.faucet(&addr).await?;
//!
//! // Query nonce and build a transfer
//! let nonce = client.get_nonce(&addr).await?;
//! let transfer = tx::transfer(&sk, nonce, 100, &[0u8; 32], 1000)?;
//! client.submit_tx(&transfer).await?;
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod error;
pub mod keys;
pub mod signals;
pub mod tx;

pub use client::{AiEntityInfo, Client, MemoryObjectInfo, SignalInfo};
pub use error::Error;

/// Re-exported protocol types for convenience.
pub mod types {
    pub use novai_ai_entities::{
        AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType,
    };
    pub use novai_types::{Address, Fee, Hash32, Nonce, SignatureBytes, TxId, TxV1, TxVersion};
}
