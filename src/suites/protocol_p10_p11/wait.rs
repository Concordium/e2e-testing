//! Bounded, observation-based readiness and agreement checks.
use super::{POLL_INTERVAL, REJOIN_TIMEOUT};
use crate::fixture::node::NodeFixture;
use anyhow::{Context, Result};
use concordium_rust_sdk::{types::hashes::BlockHash, v2::Client};
use std::{future::Future, time::Duration};

/// Apply a deadline to an operation. Include its label and time limit in timeout errors.
pub async fn bounded<T>(
    label: &str,
    deadline: Duration,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(deadline, future)
        .await
        .with_context(|| {
            format!(
                "{label} exceeded {} seconds; inspect the preceding progress output",
                deadline.as_secs()
            )
        })?
}

/// Wait until the node uses the required protocol and reports the target block as finalized.
pub async fn block(node: &NodeFixture, target: BlockHash, protocol: u64) -> Result<()> {
    let mut client = node.connect().await?;
    let mut last = String::new();
    let result = bounded("node rejoin/catch-up", REJOIN_TIMEOUT, async {
        loop {
            let consensus = client.get_consensus_info().await?;
            last = format!(
                "protocol {}, finalized height {}",
                consensus.protocol_version.0, consensus.last_finalized_block_height.height
            );
            if consensus.protocol_version.0 == protocol {
                match client.get_block_info(target).await {
                    Ok(info) if info.response.finalized => return Ok(()),
                    Err(error) if !error.is_not_found() => return Err(error.into()),
                    _ => (),
                }
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await;
    result.with_context(|| format!("Last catch-up observation: {last}"))
}

/// Wait for a saved block to finalize. Propagate errors other than a missing block.
/// Compare immutable state only after this wait, without retrying mismatches.
pub async fn finalized(client: &mut Client, target: BlockHash) -> Result<()> {
    bounded("reference block finalization", REJOIN_TIMEOUT, async {
        loop {
            match client.get_block_info(target).await {
                Ok(info) if info.response.finalized => return Ok(()),
                Err(error) if !error.is_not_found() => return Err(error.into()),
                _ => (),
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await
    .with_context(|| format!("Waiting for finalized block {target}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn deadline_reports_operation_and_duration() {
        let result: Result<()> =
            bounded("activation", Duration::ZERO, std::future::pending()).await;
        assert!(format!("{:#}", result.unwrap_err()).contains("activation exceeded 0 seconds"));
    }
}
