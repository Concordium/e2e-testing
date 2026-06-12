#![allow(dead_code)] // placeholder module — used by tasks 04+

//! Genesis fixture — programmatic genesis block generation.
//!
//! This module will call into the SDK's genesis creation API (task 01) to
//! produce a fresh single-validator genesis block and the accompanying
//! credentials entirely in memory.
//!
//! # Planned API
//!
//! ```ignore
//! let genesis = GenesisFixture::build(GenesisConfig::default()).await?;
//! // genesis.block_bytes  — serialised genesis.dat bytes
//! // genesis.validator    — validator keys / credentials
//! // genesis.accounts     — funded account key-pairs
//! ```

/// Configuration for the programmatic genesis block.
///
/// All fields have sensible defaults suitable for a minimal single-validator
/// private chain.
#[derive(Debug, Clone)]
pub struct GenesisConfig {
    /// Protocol version to target (1–11). Defaults to 11 (latest).
    pub protocol_version: u32,
    /// Number of validator accounts to generate (minimum 1).
    pub num_validators: u32,
    /// Number of additional funded (non-validator) accounts.
    pub num_funded_accounts: u32,
}

impl Default for GenesisConfig {
    fn default() -> Self {
        Self {
            protocol_version: 11,
            num_validators: 1,
            num_funded_accounts: 1,
        }
    }
}

/// Output of the genesis fixture: everything needed to start the private chain
/// and interact with it during testing.
///
/// No data is written to disk; all values live in memory for the duration of
/// the suite.
#[derive(Debug)]
pub struct GenesisFixture {
    /// Serialised genesis block (`genesis.dat` bytes).
    pub block_bytes: Vec<u8>,
    /// Generated validator credentials (keys + baker ID).
    pub validator: ValidatorCredentials,
    /// Generated funded accounts (address + signing keys).
    pub accounts: Vec<FundedAccount>,
}

/// Placeholder: validator key material produced alongside the genesis block.
#[derive(Debug)]
pub struct ValidatorCredentials {
    // TODO(task-01): populate with typed key material from the SDK genesis API.
    pub _placeholder: (),
}

/// Placeholder: a funded account produced alongside the genesis block.
#[derive(Debug)]
pub struct FundedAccount {
    // TODO(task-01): address + signing keys from the SDK genesis API.
    pub _placeholder: (),
}

impl GenesisFixture {
    /// Build a fresh genesis block from the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if genesis generation fails (e.g. unsupported protocol
    /// version).
    pub async fn build(_config: GenesisConfig) -> anyhow::Result<Self> {
        // TODO(task-01): replace with the real SDK genesis creation call once
        // the `genesis` module is available on `concordium-rust-sdk`.
        anyhow::bail!("genesis fixture not yet implemented — blocked on task 01")
    }
}
