use super::super::Fixtures;
use anyhow::{Context, ensure};
use std::time::Duration;

pub(super) fn tests() -> Vec<super::Test> {
    vec![
        super::Test {
            group: "Health",
            name: "Node is reachable",
            run: Box::new(|fixtures| Box::pin(node_is_reachable(fixtures))),
        },
        super::Test {
            group: "Health",
            name: "Node produces blocks",
            run: Box::new(|fixtures| Box::pin(node_produces_blocks(fixtures))),
        },
    ]
}

const TIMEOUT: Duration = Duration::from_secs(30);

async fn node_is_reachable(fixtures: &Fixtures) -> anyhow::Result<()> {
    let mut client = fixtures.validator_node.connect().await?;
    tokio::time::timeout(TIMEOUT, client.get_node_info())
        .await
        .context("health::node_is_reachable timed out after 30 seconds")??;
    Ok(())
}

async fn node_produces_blocks(fixtures: &Fixtures) -> anyhow::Result<()> {
    let mut client = fixtures.validator_node.connect().await?;
    let info = tokio::time::timeout(TIMEOUT, client.get_consensus_info())
        .await
        .context("health::node_produces_blocks timed out after 30 seconds")??;
    ensure!(
        info.best_block_height.height >= 1,
        "expected best block height >= 1, got {}",
        info.best_block_height.height
    );
    Ok(())
}
