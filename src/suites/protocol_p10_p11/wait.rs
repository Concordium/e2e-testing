//! Bounded, observation-based readiness and agreement checks.
use super::{POLL_INTERVAL, REJOIN_TIMEOUT};
use crate::fixture::node::NodeFixture;
use anyhow::{Context, Result};
use concordium_rust_sdk::types::hashes::BlockHash;
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

/// Retry a test-owned comparison. Record only the final result outside this poll.
pub async fn agreement<F, Work>(mut compare: F) -> Result<()>
where
    F: FnMut() -> Work,
    Work: Future<Output = Result<()>>,
{
    let mut last_error = String::new();
    let result = bounded("finalized state agreement", REJOIN_TIMEOUT, async {
        loop {
            match compare().await {
                Ok(()) => return Ok(()),
                Err(error) => last_error = format!("{error:#}"),
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await;
    result.with_context(|| format!("Last agreement observation: {last_error}"))
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
