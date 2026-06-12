#![allow(dead_code)] // placeholder module — used by tasks 05+

//! Node fixture — Docker container lifecycle management.
//!
//! This module will start a Concordium node in a Docker container, wait until
//! it is healthy, and tear it down on completion (or on drop).
//!
//! # Planned API
//!
//! ```ignore
//! let node = NodeFixture::start(NodeConfig {
//!     image: "concordium-node:7.0.4".into(),
//!     grpc_port: 20000,
//!     genesis_bytes: fixture.block_bytes.clone(),
//!     validator_credentials: fixture.validator,
//! }).await?;
//!
//! node.wait_until_ready(Duration::from_secs(60)).await?;
//! // ... run tests using node.client() ...
//! node.stop().await?;
//! ```

/// Configuration for a node container started by the suite.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// Docker image to run (e.g. `concordium-node:7.0.4`).
    pub image: String,
    /// Host port to map to the node's gRPC endpoint.
    pub grpc_port: u16,
    /// Serialised genesis block bytes to mount into the container.
    pub genesis_bytes: Vec<u8>,
}

/// A running Concordium node managed by the suite.
///
/// The container is stopped and removed when this value is dropped or when
/// [`NodeFixture::stop`] is called explicitly.
#[derive(Debug)]
pub struct NodeFixture {
    config: NodeConfig,
    // TODO(task-05): Docker container handle + gRPC client.
    _placeholder: (),
}

impl NodeFixture {
    /// Start a node container with the given configuration.
    ///
    /// Returns once the container process has started; call
    /// [`wait_until_ready`](NodeFixture::wait_until_ready) to block until the
    /// node is serving gRPC.
    pub async fn start(_config: NodeConfig) -> anyhow::Result<Self> {
        // TODO(task-05): implement Docker container startup.
        anyhow::bail!("node fixture not yet implemented — blocked on task 05")
    }

    /// Block until the node reports itself healthy and has produced at least
    /// one block, or until `timeout` elapses.
    pub async fn wait_until_ready(&self, _timeout: std::time::Duration) -> anyhow::Result<()> {
        // TODO(task-05): poll health + block height.
        anyhow::bail!("node fixture not yet implemented — blocked on task 05")
    }

    /// Stop and remove the node container.
    pub async fn stop(self) -> anyhow::Result<()> {
        // TODO(task-05): stop container.
        anyhow::bail!("node fixture not yet implemented — blocked on task 05")
    }
}
