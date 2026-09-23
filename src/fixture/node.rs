//! Node fixture — Docker container lifecycle management via bollard.
//!
//! Starts the Concordium node as an isolated Docker container, mounts the
//! genesis block and validator credentials from a temporary directory, waits
//! for readiness, and tears everything down on exit.

use anyhow::{bail, Context, Result};
use bollard::{
    models::{ContainerCreateBody, HostConfig, PortBinding},
    query_parameters::{
        CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder, StopContainerOptionsBuilder,
    },
    Docker,
};
use concordium_rust_sdk::v2::{Client, Endpoint};
use std::{collections::HashMap, str::FromStr, time::Duration};
use tempfile::TempDir;

const READINESS_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

// Paths inside the container
const CONTAINER_DATA_DIR: &str = "/mnt/data";
const CONTAINER_GENESIS_PATH: &str = "/mnt/data/genesis.dat";
const CONTAINER_BAKER_CREDS_PATH: &str = "/mnt/data/validator-credentials.json";
const CONTAINER_GRPC_PORT: u16 = 20000;

/// Configuration for a node container.
pub struct NodeConfig {
    /// Docker image to run (e.g. `concordium-node:7.0.4`).
    pub image: String,
    /// Host port mapped to the node's gRPC endpoint.
    pub grpc_port: u16,
    /// Serialized genesis block (`genesis.dat` contents).
    pub genesis_bytes: Vec<u8>,
    /// Baker credentials
    pub validator_credentials: Option<Vec<u8>>,
}

/// A running Concordium node managed by the suite.
///
/// Call [`NodeFixture::stop`] when done; the container and temporary directory
/// are cleaned up at that point.
pub struct NodeFixture {
    container_id: String,
    endpoint: Endpoint,
    _temp_dir: TempDir, // kept alive until stop(); dropped after container removal
}

impl NodeFixture {
    /// Open gRPC connection to this node.
    pub async fn connect(&self) -> Result<Client> {
        Client::new(self.endpoint.clone()).await.with_context(|| {
            format!(
                "Failed opening connection to node at {}",
                self.endpoint.uri()
            )
        })
    }

    /// Create, start, and return a handle to a node container.
    ///
    /// The container is fully isolated (no network peers) and has the genesis
    /// block and baker credentials bind-mounted from a temporary directory.
    /// Call [`wait_until_ready`](NodeFixture::wait_until_ready) next.
    pub async fn start(docker: &Docker, config: NodeConfig) -> Result<Self> {
        // ── Write fixture files to a temp directory ───────────────────────────
        let temp_dir = TempDir::new().context("Failed to create temp dir")?;
        std::fs::write(temp_dir.path().join("genesis.dat"), config.genesis_bytes)
            .context("Failed to write genesis.dat")?;

        if let Some(validator_credentials) = config.validator_credentials.as_ref() {
            std::fs::write(
                temp_dir.path().join("validator-credentials.json"),
                validator_credentials,
            )
            .context("Failed to write validator-credentials.json")?;
        };

        // ── Port binding: host:grpc_port → container:20000/tcp ────────────────
        let port_key = format!("{CONTAINER_GRPC_PORT}/tcp");
        let port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::from([(
            port_key.clone(),
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some(config.grpc_port.to_string()),
            }]),
        )]);

        // ── Create container ──────────────────────────────────────────────────
        let options = CreateContainerOptionsBuilder::default().build();
        let env = {
            let mut env = vec![
                format!("CONCORDIUM_NODE_DATA_DIR={CONTAINER_DATA_DIR}"),
                format!("CONCORDIUM_NODE_CONFIG_DIR={CONTAINER_DATA_DIR}"),
                format!("CONCORDIUM_NODE_CONSENSUS_GENESIS_DATA_FILE={CONTAINER_GENESIS_PATH}"),
                format!("CONCORDIUM_NODE_BAKER_CREDENTIALS_FILE={CONTAINER_BAKER_CREDS_PATH}"),
                format!("CONCORDIUM_NODE_GRPC2_LISTEN_ADDRESS=0.0.0.0"),
                format!("CONCORDIUM_NODE_GRPC2_LISTEN_PORT={CONTAINER_GRPC_PORT}"),
                "CONCORDIUM_NODE_CONNECTION_BOOTSTRAP_NODES=".to_string(),
            ];
            if config.validator_credentials.is_some() {
                env.push(format!(
                    "CONCORDIUM_NODE_BAKER_CREDENTIALS_FILE={CONTAINER_BAKER_CREDS_PATH}"
                ))
            }
            env
        };

        let container_config = ContainerCreateBody {
            image: Some(config.image.clone()),
            // Official Concordium network images default to an interactive
            // shell and ship the node executable at this fixed path.
            cmd: Some(vec!["/concordium-node".to_string()]),
            env: Some(env),
            exposed_ports: Some(vec![port_key]),
            host_config: Some(HostConfig {
                binds: Some(vec![format!(
                    "{}:{CONTAINER_DATA_DIR}",
                    temp_dir.path().display()
                )]),
                // Use Docker's bridge so the published host gRPC port is
                // reachable. Peer discovery remains disabled by node config.
                network_mode: Some("bridge".to_string()),
                port_bindings: Some(port_bindings),
                auto_remove: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };

        let response = docker
            .create_container(Some(options), container_config)
            .await
            .context("Failed to create container")?;

        let container_id = response.id;

        for warning in &response.warnings {
            tracing::warn!(container_id = %container_id, "docker: {warning}");
        }

        // ── Start container ───────────────────────────────────────────────────
        docker
            .start_container(&container_id, None)
            .await
            .context("Failed to start container")?;

        tracing::info!(
            container_id = %&container_id[..12],
            image = %config.image,
            grpc_port = config.grpc_port,
            "node container started"
        );

        let endpoint = Endpoint::from_str(&format!("http://localhost:{}", config.grpc_port))
            .context("Invalid gRPC endpoint")?;

        Ok(Self {
            container_id,
            endpoint,
            _temp_dir: temp_dir,
        })
    }

    /// Poll the node's health endpoint until it reports healthy, then wait
    /// until at least one block has been produced past genesis.
    ///
    /// Each step has a hard 30-second timeout.
    pub async fn wait_until_ready(&mut self) -> Result<()> {
        self.wait_for_health().await?;
        self.wait_for_first_block().await?;
        Ok(())
    }

    /// Stop and remove the node container and delete all temporary files.
    pub async fn stop(self, docker: &Docker) -> Result<()> {
        tracing::info!(container_id = %&self.container_id[..12], "stopping node container");

        let stop_opts = StopContainerOptionsBuilder::default()
            .t(10i32) // 10-second grace period before SIGKILL
            .build();

        if let Err(e) = docker
            .stop_container(&self.container_id, Some(stop_opts))
            .await
        {
            tracing::warn!(container_id = %&self.container_id[..12], "stop error: {e}");
        }

        let remove_options = RemoveContainerOptionsBuilder::default().force(true).build();
        if let Err(error) = docker
            .remove_container(&self.container_id, Some(remove_options))
            .await
        {
            tracing::warn!(container_id = %&self.container_id[..12], "remove error: {error}");
        }

        // _temp_dir is dropped here, deleting genesis.dat + baker-credentials.json
        Ok(())
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    async fn wait_for_health(&mut self) -> Result<()> {
        tracing::info!("waiting for node health check...");
        tokio::time::timeout(READINESS_TIMEOUT, async {
            loop {
                let result = match self.connect().await {
                    Ok(mut client) => client.get_node_info().await.map(|_| ()),
                    Err(error) => {
                        tracing::debug!("connect: {error}");
                        tokio::time::sleep(POLL_INTERVAL).await;
                        continue;
                    }
                };
                match result {
                    Ok(()) => {
                        tracing::info!("node is healthy");
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::debug!("health check: {e}");
                        tokio::time::sleep(POLL_INTERVAL).await;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            bail!(
                "Node did not become healthy within {} seconds",
                READINESS_TIMEOUT.as_secs()
            )
        })
    }

    async fn wait_for_first_block(&mut self) -> Result<()> {
        tracing::info!("waiting for first block...");
        let mut client = self.connect().await?;
        tokio::time::timeout(READINESS_TIMEOUT, async {
            loop {
                match client.get_consensus_info().await {
                    Ok(info) if info.best_block_height.height >= 1 => {
                        tracing::info!(
                            height = info.best_block_height.height,
                            "first block produced"
                        );
                        return Ok(());
                    }
                    Ok(_) => {
                        tokio::time::sleep(POLL_INTERVAL).await;
                    }
                    Err(e) => {
                        tracing::debug!("get_consensus_info: {e}");
                        tokio::time::sleep(POLL_INTERVAL).await;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            bail!(
                "No block produced within {} seconds of node becoming healthy",
                READINESS_TIMEOUT.as_secs()
            )
        })
    }
}
