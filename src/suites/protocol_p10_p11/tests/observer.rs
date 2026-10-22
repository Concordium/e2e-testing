//! Independent observer compatibility and database-preserving recovery criteria.
use super::super::registry::{ProtocolUpgradeTest, TestProgress, TestRegistry};
use super::super::{OBSERVER_INDEX, context::TestContext};
use crate::fixture::genesis::GenesisAccount;
use anyhow::{Context, Result, ensure};

pub(super) fn register(registry: &mut TestRegistry) -> Result<()> {
    registry.register::<Rejection>()?;
    registry.register::<Recovery>()
}

struct Rejection;
impl ProtocolUpgradeTest for Rejection {
    fn name() -> String {
        "Old software reports the unsupported update and cannot follow P11".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_before_observer_upgrade(
        &mut self,
        context: &TestContext<'_>,
    ) -> Result<TestProgress> {
        let logs = context.observer_logs().await?;
        ensure!(
            logs.contains("unsupported protocol update") && logs.contains("has taken effect"),
            "Observer did not report an unsupported update taking effect"
        );
        let running = context.observer_running().await?;
        tracing::info!(running, "old observer reported unsupported update");
        // An exited process cannot follow P11. A crash is not required.
        if !running {
            return Ok(TestProgress::Passed);
        }
        let mut client = context.client(OBSERVER_INDEX).await.context(
            "Running old observer API unavailable; cannot establish its finalized state",
        )?;
        let consensus = client.get_consensus_info().await?;
        ensure!(
            consensus.protocol_version.0 == 10,
            "Old observer unexpectedly follows P11"
        );
        match client.get_block_info(context.p11_recovery_target()?).await {
            Err(error) if error.is_not_found() => (),
            Err(error) => return Err(error.into()),
            Ok(_) => anyhow::bail!("Old observer recognizes the P11 reference block"),
        }
        tracing::info!(protocol = consensus.protocol_version.0, block = %consensus.last_finalized_block, "old observer cannot follow P11");
        Ok(TestProgress::Passed)
    }
}

struct Recovery;
impl ProtocolUpgradeTest for Recovery {
    fn name() -> String {
        "The upgraded observer recovers the saved P11 block with its existing database".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_observer_recovered(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let mut client = context.client(OBSERVER_INDEX).await?;
        let block = client
            .get_block_info(context.p11_recovery_target()?)
            .await?
            .response;
        let consensus = client.get_consensus_info().await?;
        ensure!(
            block.finalized && block.protocol_version.0 == 11 && consensus.protocol_version.0 == 11,
            "Observer did not recover the finalized P11 reference"
        );
        Ok(TestProgress::Passed)
    }
}
