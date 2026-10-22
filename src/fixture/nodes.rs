//! Shared operations on suite-owned nodes; no separate network fixture.
use super::node::NodeFixture;
use anyhow::Result;
use bollard::Docker;

/// Request connections between every pair of supplied nodes. Do not connect to other runs.
pub async fn connect_peers(nodes: &[NodeFixture], docker: &Docker) -> Result<()> {
    let mut addresses = Vec::new();
    for node in nodes {
        addresses.push(node.peer_address(docker).await?);
    }
    for (index, node) in nodes.iter().enumerate() {
        let mut client = node.connect().await?;
        for (peer, address) in addresses.iter().enumerate() {
            if peer != index {
                client.peer_connect(*address).await?;
            }
        }
    }
    Ok(())
}

/// Stop all supplied nodes. Collect errors without skipping later nodes.
pub async fn stop(nodes: Vec<NodeFixture>, docker: &Docker) -> Result<()> {
    let mut errors = Vec::new();
    for node in nodes {
        if let Err(error) = node.stop(docker).await {
            errors.push(format!("{error:#}"));
        }
    }
    anyhow::ensure!(
        errors.is_empty(),
        "Node cleanup failed: {}",
        errors.join("; ")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{genesis::GenesisFixture, node::NodeConfig};
    use anyhow::Context;
    use std::time::Duration;

    #[tokio::test]
    #[ignore = "requires Docker and a node image"]
    async fn failed_start_cleans_up_previously_started_nodes() -> Result<()> {
        let docker = Docker::connect_with_local_defaults()?;
        let image = std::env::var("CONCORDIUM_OLD_IMAGE")?;
        let genesis = GenesisFixture::single_validator_p10()?;
        let bytes = concordium_rust_sdk::genesis::serialize_genesis(&genesis.genesis_data);
        let mut nodes = Vec::new();
        let startup = async {
            for image in [image, "concordium-e2e-deliberately-missing:image".into()] {
                let node = NodeFixture::start(
                    &docker,
                    NodeConfig {
                        image,
                        genesis_bytes: bytes.clone(),
                        validator_credentials: None,
                    },
                )
                .await?;
                nodes.push(node);
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let ids: Vec<_> = nodes
            .iter()
            .map(|node| node.container_id().to_string())
            .collect();
        stop(nodes, &docker).await?;
        assert!(startup.is_err());
        assert_eq!(ids.len(), 1, "Expected one node to start before failure");
        for id in ids {
            let error = docker
                .inspect_container(&id, None)
                .await
                .expect_err("Node was not removed");
            assert!(matches!(
                error,
                bollard::errors::Error::DockerResponseServerError {
                    status_code: 404,
                    ..
                }
            ));
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires Docker and old/new node images"]
    async fn rolling_restart_preserves_database() -> Result<()> {
        let docker = Docker::connect_with_local_defaults()?;
        let old = std::env::var("CONCORDIUM_OLD_IMAGE")?;
        let new = std::env::var("CONCORDIUM_NEW_IMAGE")?;
        let genesis = GenesisFixture::four_validator_p10()?;
        let mut nodes = Vec::new();
        let result = async {
            let bytes = concordium_rust_sdk::genesis::serialize_genesis(&genesis.genesis_data);
            for index in 0..=genesis.validators.len() {
                let credentials = genesis
                    .validators
                    .get(index)
                    .map(|keys| serde_json::to_vec(&*keys.lock().unwrap()))
                    .transpose()?;
                let node = NodeFixture::start(
                    &docker,
                    NodeConfig {
                        image: old.clone(),
                        genesis_bytes: bytes.clone(),
                        validator_credentials: credentials,
                    },
                )
                .await?;
                nodes.push(node);
            }
            for node in &mut nodes {
                node.wait_for_health().await?;
            }
            connect_peers(&nodes, &docker).await?;
            for node in &mut nodes {
                node.wait_until_ready().await?;
            }
            let mut client = nodes[0].connect().await?;
            let before = client.get_consensus_info().await?;
            let mut surviving = nodes[1].connect().await?;
            let height = surviving
                .get_consensus_info()
                .await?
                .last_finalized_block_height;
            nodes[0].pause(&docker).await?;
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    if surviving
                        .get_consensus_info()
                        .await?
                        .last_finalized_block_height
                        > height
                    {
                        return Ok::<_, anyhow::Error>(());
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            })
            .await
            .context(
                "Finalization did not advance while validator was offline within 30 seconds",
            )??;
            nodes[0].replace_image(&docker, &new).await?;
            nodes[0].wait_for_health().await?;
            connect_peers(&nodes, &docker).await?;
            nodes[0].wait_until_ready().await?;
            let mut client = nodes[0].connect().await?;
            let after = client.get_consensus_info().await?;
            anyhow::ensure!(
                after.genesis_block == before.genesis_block,
                "Genesis changed"
            );
            client.get_block_info(before.best_block).await?;
            let target = surviving.get_consensus_info().await?.last_finalized_block;
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    match client.get_block_info(target).await {
                        Ok(info) if info.response.finalized => return Ok::<_, anyhow::Error>(()),
                        Err(error) if !error.is_not_found() => return Err(error.into()),
                        _ => (),
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            })
            .await
            .context("Upgraded validator did not recover finalized history within 30 seconds")??;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        stop(nodes, &docker).await?;
        result
    }
}
