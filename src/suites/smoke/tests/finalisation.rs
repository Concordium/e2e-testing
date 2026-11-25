use super::super::TestContext;
use anyhow::{Context, bail};
use std::time::Duration;

pub(super) fn register(registry: &mut crate::registry::TestRegistry<super::Test>) {
    registry.group("Finalization").register(
        "Last finalized block advances",
        super::Test {
            required_accounts: 0,
            run: Box::new(|fixtures, _accounts| Box::pin(last_finalised_block_advances(fixtures))),
        },
    );
}

const TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

async fn last_finalised_block_advances(fixtures: &TestContext) -> anyhow::Result<()> {
    let mut client = fixtures.validator_node.connect().await?;
    let initial = client
        .get_consensus_info()
        .await?
        .last_finalized_block_height;

    tokio::time::timeout(TIMEOUT, async {
        loop {
            let current = client.get_consensus_info().await?.last_finalized_block_height;
            if current > initial {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        bail!(
            "finalisation::last_finalised_block_advances timed out after 30 seconds; height remained at {}",
            initial.height
        )
    })
    .context("failed while polling finalised block height")
}
