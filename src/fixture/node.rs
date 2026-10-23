//! Node fixture — Docker container lifecycle management via bollard.
//!
//! Starts the Concordium node as an isolated Docker container, mounts the
//! genesis block and validator credentials from a temporary directory, waits
//! for readiness, and tears everything down on exit.

use anyhow::{Context, Result, bail};
use bollard::{
    Docker,
    models::{ContainerCreateBody, HostConfig, PortBinding},
    query_parameters::{
        CreateContainerOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
        StopContainerOptionsBuilder,
    },
};
use concordium_rust_sdk::v2::{Client, Endpoint};
use futures_util::StreamExt;
use std::{collections::HashMap, str::FromStr, time::Duration};
use tempfile::TempDir;

/// Maximum wait for each readiness stage: gRPC access, then block production.
const READINESS_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

// Paths inside the container
const CONTAINER_DATA_DIR: &str = "/mnt/data";
const CONTAINER_GENESIS_PATH: &str = "/mnt/data/genesis.dat";
const CONTAINER_BAKER_CREDS_PATH: &str = "/mnt/data/validator-credentials.json";
const CONTAINER_GRPC_PORT: u16 = 20000;

/// Configuration for a node container.
pub struct NodeConfig {
    /// Docker image reference or resolved image ID.
    pub image: String,
    /// Binary genesis data to write to `genesis.dat`.
    pub genesis_bytes: Vec<u8>,
    /// Validator credentials encoded as JSON. `None` starts a node without validator keys.
    pub validator_credentials: Option<Vec<u8>>,
}

/// Owns one node container and its temporary chain files.
///
/// Call [`Self::stop`] to remove the container and its files.
pub struct NodeFixture {
    /// Docker container ID. Changes when the image is replaced.
    container_id: String,
    /// Host-loopback gRPC address. Docker selects its port.
    endpoint: Endpoint,
    /// Genesis data, credentials, and chain database. Keep these files during image replacement.
    temp_dir: TempDir,
    /// Reuse this configuration when the image is replaced.
    config: NodeConfig,
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
    /// Peer discovery is disabled; callers connect peers explicitly. Genesis
    /// data and validator credentials are bind-mounted from a temporary directory.
    /// Call [`wait_until_ready`](NodeFixture::wait_until_ready) next.
    pub async fn start(docker: &Docker, config: NodeConfig) -> Result<Self> {
        // ── Write fixture files to a temp directory ───────────────────────────
        let temp_dir = TempDir::new().context("Failed to create temp dir")?;
        std::fs::write(temp_dir.path().join("genesis.dat"), &config.genesis_bytes)
            .context("Failed to write genesis.dat")?;

        if let Some(validator_credentials) = config.validator_credentials.as_ref() {
            std::fs::write(
                temp_dir.path().join("validator-credentials.json"),
                validator_credentials,
            )
            .context("Failed to write validator-credentials.json")?;
        };

        // Administrative peering is disabled by default. Enable only the
        // endpoints required by the test network's lifecycle.
        std::fs::write(
            temp_dir.path().join("grpc-endpoints.toml"),
            "get_finalized_blocks = true
peer_connect = true
get_peers_info = true
get_node_info = true
get_account_info = true
get_token_info = true
get_consensus_info = true
get_block_info = true
get_block_item_status = true
get_next_account_sequence_number = true
get_next_update_sequence_numbers = true
get_blocks_at_height = true
get_block_special_events = true
send_block_item = true
",
        )?;
        let (container_id, endpoint) = Self::create_container(docker, &config, &temp_dir).await?;
        Ok(Self {
            container_id,
            endpoint,
            temp_dir,
            config,
        })
    }

    /// Start a container with the existing data directory and a new localhost gRPC port.
    /// If startup fails, attempt to remove the container before returning the error.
    async fn create_container(
        docker: &Docker,
        config: &NodeConfig,
        temp_dir: &TempDir,
    ) -> Result<(String, Endpoint)> {
        // Run the container as the owner of the bind-mounted directory. This
        // supports images that default to either root or an unprivileged user.
        #[cfg(unix)]
        let container_user = {
            use std::os::unix::fs::MetadataExt;

            let metadata = std::fs::metadata(temp_dir.path())
                .context("Failed to inspect node data directory")?;
            Some(format!("{}:{}", metadata.uid(), metadata.gid()))
        };
        #[cfg(not(unix))]
        let container_user: Option<String> = None;

        // Let Docker assign a free host port, bound only to loopback.
        let port_key = format!("{CONTAINER_GRPC_PORT}/tcp");
        let port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::from([(
            port_key.clone(),
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some(String::new()),
            }]),
        )]);

        // ── Create container ──────────────────────────────────────────────────
        let options = CreateContainerOptionsBuilder::default().build();
        let env = {
            let mut env = vec![
                format!("CONCORDIUM_NODE_DATA_DIR={CONTAINER_DATA_DIR}"),
                format!("CONCORDIUM_NODE_CONFIG_DIR={CONTAINER_DATA_DIR}"),
                format!("CONCORDIUM_NODE_CONSENSUS_GENESIS_DATA_FILE={CONTAINER_GENESIS_PATH}"),
                "CONCORDIUM_NODE_GRPC2_LISTEN_ADDRESS=0.0.0.0".to_string(),
                format!("CONCORDIUM_NODE_GRPC2_LISTEN_PORT={CONTAINER_GRPC_PORT}"),
                "CONCORDIUM_NODE_CONNECTION_NO_BOOTSTRAP_DNS=true".to_string(),
                "CONCORDIUM_NODE_CONNECTION_BOOTSTRAP_NODES=".to_string(),
                "CONCORDIUM_NODE_LISTEN_ADDRESS=0.0.0.0".to_string(),
                "CONCORDIUM_NODE_LISTEN_PORT=8888".to_string(),
            ];
            env.push(format!(
                "CONCORDIUM_NODE_GRPC2_ENDPOINT_CONFIG={CONTAINER_DATA_DIR}/grpc-endpoints.toml"
            ));
            if config.validator_credentials.is_some() {
                env.push(format!(
                    "CONCORDIUM_NODE_BAKER_CREDENTIALS_FILE={CONTAINER_BAKER_CREDS_PATH}"
                ))
            }
            env
        };

        let container_config = ContainerCreateBody {
            image: Some(config.image.clone()),
            user: container_user,
            // Support both the root-level path in the network-specific images
            // and the PATH-based command in the combined image.
            cmd: Some(vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "if command -v concordium-node >/dev/null 2>&1; then exec concordium-node; else exec /concordium-node; fi".to_string(),
            ]),
            env: Some(env),
            exposed_ports: Some(vec![port_key.clone()]),
            host_config: Some(HostConfig {
                binds: Some(vec![format!(
                    "{}:{CONTAINER_DATA_DIR}",
                    temp_dir.path().display()
                )]),
                network_mode: Some("bridge".to_string()),
                port_bindings: Some(port_bindings),
                // Keep explicit lifecycle control so startup failures can
                // include logs before the container is removed.
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
        let setup_result = async {
            docker
                .start_container(&container_id, None)
                .await
                .context("Failed to start container")?;

            let inspect = docker
                .inspect_container(&container_id, None)
                .await
                .context("Failed to inspect started container")?;
            let settings = inspect
                .network_settings
                .context("No container network settings")?;
            let address = grpc_address(settings)?;
            let endpoint = Endpoint::from_str(&format!("http://{address}"))
                .context("Invalid gRPC endpoint")?
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(10));
            Ok::<_, anyhow::Error>(endpoint)
        }
        .await;

        let endpoint = match setup_result {
            Ok(value) => value,
            Err(error) => {
                log_container_output(docker, &container_id).await;
                if let Err(cleanup_error) = remove_container(docker, &container_id).await {
                    return Err(
                        error.context(format!("container cleanup also failed: {cleanup_error:#}"))
                    );
                }
                return Err(error);
            }
        };

        tracing::info!(
            container_id = %&container_id[..12],
            image = %config.image,
            grpc_endpoint = %endpoint.uri(),
            "node container started"
        );

        Ok((container_id, endpoint))
    }

    /// Poll the node's gRPC API until it is reachable, then wait until at
    /// least one block has been produced past genesis.
    ///
    /// Each step has a hard 30-second timeout.
    pub async fn wait_until_ready(&mut self) -> Result<()> {
        self.wait_for_health().await?;
        self.wait_for_first_block().await?;
        Ok(())
    }

    /// Emit the tail of the node logs to aid diagnosis before cleanup.
    pub async fn log_output(&self, docker: &Docker) {
        log_container_output(docker, &self.container_id).await;
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

        let container_cleanup = remove_container(docker, &self.container_id).await;
        let path = self.temp_dir.path().to_path_buf();
        // Attempt file deletion even if Docker removal failed. Drop would hide
        // filesystem errors and could incorrectly report successful cleanup.
        let data_cleanup = self
            .temp_dir
            .close()
            .with_context(|| format!("Failed to delete node data directory {}", path.display()));
        match (container_cleanup, data_cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(data_error)) => {
                Err(error.context(format!("Data cleanup also failed: {data_error:#}")))
            }
        }
    }

    /// Stop without deleting the container or its chain database.
    pub async fn pause(&self, docker: &Docker) -> Result<()> {
        docker
            .stop_container(
                &self.container_id,
                Some(StopContainerOptionsBuilder::default().t(10).build()),
            )
            .await
            .context("Failed to pause node")
    }

    /// Replace a stopped container using the same genesis, credentials and DB.
    /// On failure, this fixture still owns the directory and can be cleaned up.
    pub async fn replace_image(&mut self, docker: &Docker, image: &str) -> Result<()> {
        remove_container(docker, &self.container_id).await?;
        self.config.image = image.to_string();
        let (id, endpoint) = Self::create_container(docker, &self.config, &self.temp_dir).await?;
        self.container_id = id;
        self.endpoint = endpoint;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn container_id(&self) -> &str {
        &self.container_id
    }

    /// Return the node's bridge address for P2P connections, not host gRPC access.
    pub async fn peer_address(&self, docker: &Docker) -> Result<std::net::SocketAddr> {
        let inspect = docker.inspect_container(&self.container_id, None).await?;
        let networks = inspect
            .network_settings
            .and_then(|s| s.networks)
            .context("No container network settings")?;
        let ip = networks
            .get("bridge")
            .and_then(|s| s.ip_address.as_deref())
            .context("No peer IP address")?
            .parse()?;
        Ok(std::net::SocketAddr::new(ip, 8888))
    }

    /// Read the last 200 log lines from the current container.
    pub async fn recent_logs(&self, docker: &Docker) -> Result<String> {
        let options = LogsOptionsBuilder::default()
            .stdout(true)
            .stderr(true)
            .tail("200")
            .build();
        let mut stream = docker.logs(&self.container_id, Some(options));
        let mut text = String::new();
        while let Some(output) = stream.next().await {
            text.push_str(&output?.to_string());
        }
        Ok(text)
    }

    /// Read Docker's process state. This does not check gRPC or consensus health.
    pub async fn process_running(&self, docker: &Docker) -> Result<bool> {
        let inspect = docker.inspect_container(&self.container_id, None).await?;
        inspect
            .state
            .and_then(|state| state.running)
            .context("No node process status")
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Wait until `get_node_info` succeeds. Block production is not required.
    pub async fn wait_for_health(&mut self) -> Result<()> {
        tracing::info!("waiting for node gRPC readiness...");
        tokio::time::timeout(READINESS_TIMEOUT, async {
            loop {
                let result = match self.connect().await {
                    Ok(mut client) => client.get_node_info().await.map(|_| ()),
                    Err(error) => {
                        tracing::debug!("connect: {error:#}");
                        tokio::time::sleep(POLL_INTERVAL).await;
                        continue;
                    }
                };
                match result {
                    Ok(()) => {
                        tracing::info!("node gRPC API is reachable");
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::debug!("readiness query: {e}");
                        tokio::time::sleep(POLL_INTERVAL).await;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            bail!(
                "Node gRPC API did not become reachable within {} seconds",
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

/// Read Docker's assigned host port. Never use a private container IP for gRPC.
fn grpc_address(settings: bollard::models::NetworkSettings) -> Result<std::net::SocketAddr> {
    let port_key = format!("{CONTAINER_GRPC_PORT}/tcp");
    let port = settings
        .ports
        .and_then(|mut ports| ports.remove(&port_key))
        .flatten()
        .and_then(|bindings| bindings.into_iter().find_map(|binding| binding.host_port))
        .context("Docker did not publish the node gRPC port")?
        .parse()
        .context("Docker returned an invalid node gRPC port")?;
    Ok(std::net::SocketAddr::from(([127, 0, 0, 1], port)))
}

/// Log the last 100 container log lines. Report read errors without stopping cleanup.
async fn log_container_output(docker: &Docker, container_id: &str) {
    let options = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .tail("100")
        .build();
    let mut logs = docker.logs(container_id, Some(options));
    while let Some(output) = logs.next().await {
        match output {
            Ok(output) => tracing::error!(container_id = %&container_id[..12], "node: {}", output),
            Err(error) => {
                tracing::warn!(container_id = %&container_id[..12], "could not read node logs: {error}");
                break;
            }
        }
    }
}

/// Force container removal. Treat an already absent container as success.
async fn remove_container(docker: &Docker, container_id: &str) -> Result<()> {
    let options = RemoveContainerOptionsBuilder::default().force(true).build();
    match docker.remove_container(container_id, Some(options)).await {
        Ok(()) => Ok(()),
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(()),
        Err(error) => Err(error).context("Failed to remove node container"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{EndpointSettings, NetworkSettings};

    #[test]
    fn grpc_endpoint_uses_loopback_published_port_not_private_ip() -> Result<()> {
        let settings = NetworkSettings {
            ports: Some(HashMap::from([(
                "20000/tcp".into(),
                Some(vec![PortBinding {
                    host_ip: Some("127.0.0.1".into()),
                    host_port: Some("32123".into()),
                }]),
            )])),
            networks: Some(HashMap::from([(
                "suite".into(),
                EndpointSettings {
                    ip_address: Some("172.30.0.2".into()),
                    ..Default::default()
                },
            )])),
            ..Default::default()
        };
        assert_eq!(grpc_address(settings)?.to_string(), "127.0.0.1:32123");
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires Docker"]
    async fn stop_reports_data_directory_deletion_failure() -> Result<()> {
        let docker = Docker::connect_with_local_defaults()?;
        let parent = tempfile::tempdir()?;
        let temp_dir = TempDir::new_in(parent.path())?;
        let path = temp_dir.path().to_path_buf();
        // A file in place of the directory makes removal fail on all users,
        // including root. The parent directory owns the test's cleanup.
        std::fs::remove_dir(&path)?;
        std::fs::write(&path, "not a directory")?;
        let node = NodeFixture {
            container_id: format!(
                "missing-e2e-{}",
                parent.path().file_name().unwrap().to_string_lossy()
            ),
            endpoint: Endpoint::from_static("http://127.0.0.1:1"),
            temp_dir,
            config: NodeConfig {
                image: String::new(),
                genesis_bytes: Vec::new(),
                validator_credentials: None,
            },
        };
        let other_dir = TempDir::new_in(parent.path())?;
        let other_path = other_dir.path().to_path_buf();
        let other = NodeFixture {
            container_id: format!("other-{}", node.container_id),
            endpoint: Endpoint::from_static("http://127.0.0.1:1"),
            temp_dir: other_dir,
            config: NodeConfig {
                image: String::new(),
                genesis_bytes: Vec::new(),
                validator_credentials: None,
            },
        };
        let error = crate::fixture::nodes::stop(vec![node, other], &docker)
            .await
            .expect_err("Directory deletion failure must fail cleanup");
        assert!(!other_path.exists(), "Later node cleanup must still run");
        assert!(format!("{error:#}").contains("Failed to delete node data directory"));
        assert!(path.is_file());
        Ok(())
    }

    #[test]
    fn missing_published_port_is_an_error() {
        assert!(grpc_address(NetworkSettings::default()).is_err());
    }

    #[test]
    fn invalid_published_port_is_an_error() {
        let settings = NetworkSettings {
            ports: Some(HashMap::from([(
                "20000/tcp".into(),
                Some(vec![PortBinding {
                    host_ip: Some("127.0.0.1".into()),
                    host_port: Some("invalid".into()),
                }]),
            )])),
            ..Default::default()
        };
        assert!(grpc_address(settings).is_err());
    }
}
