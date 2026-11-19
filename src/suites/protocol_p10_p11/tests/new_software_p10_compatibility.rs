//! New software follows pre-update operations produced by old-software validators.
use super::super::{
    registry::{CleanupContext, ProtocolUpgradeTest, TestContext, TestProgress, TestRegistry},
    state::FinalizedBlock,
    wait,
};
use crate::fixture::{
    genesis::GenesisAccount,
    node::{NodeConfig, NodeFixture},
};
use anyhow::{Context, Result, ensure};

pub(super) fn register(registry: &mut TestRegistry) {
    registry
        .group("New software compatibility with P10")
        .register(NewSoftwareP10Compatibility::default());
}

#[derive(Default)]
struct NewSoftwareP10Compatibility {
    node: Option<NodeFixture>,
}

#[async_trait::async_trait(?Send)]
impl ProtocolUpgradeTest for NewSoftwareP10Compatibility {
    fn name(&self) -> String {
        "New software catches up with operations from old-software validators before the update"
            .into()
    }

    async fn prepare_before_update(
        &mut self,
        context: &TestContext<'_>,
        _accounts: Vec<GenesisAccount>,
    ) -> Result<()> {
        // Retain ownership before readiness checks so partial preparation can clean up.
        self.node = Some(
            NodeFixture::start(
                context.docker,
                NodeConfig {
                    image: context.new_image.to_owned(),
                    genesis_bytes: context.genesis.to_vec(),
                    validator_credentials: None,
                },
            )
            .await?,
        );
        let node = self
            .node
            .as_mut()
            .context("Mixed-software node not prepared")?;
        if let Err(error) = node.wait_for_health().await {
            // Preserve startup diagnostics before cleanup removes the container.
            let running = node.process_running(context.docker).await;
            let logs = match node.recent_logs(context.docker).await {
                Ok(logs) => logs,
                Err(log_error) => format!("Could not read container logs: {log_error:#}"),
            };
            return Err(error.context(format!(
                "New-image node failed to start (image {}, process running: {running:?}). Container logs:\n{logs}",
                context.new_image,
            )));
        }
        let mut client = node.connect().await?;
        for validator in context.validators.iter() {
            client
                .peer_connect(validator.peer_address(context.docker).await?)
                .await?;
        }
        node.wait_until_ready().await?;
        Ok(())
    }

    async fn finish_before_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let reference =
            FinalizedBlock::capture(&mut context.validators[0].connect().await?).await?;
        let node = self
            .node
            .as_ref()
            .context("Mixed-software node not prepared")?;
        wait::block(node, reference.hash, 10).await?;
        let block = node
            .connect()
            .await?
            .get_block_info(reference.hash)
            .await?
            .response;
        ensure!(
            block.finalized && block.block_state_hash == reference.state_hash,
            "New software disagrees with old-software validators after pre-update operations"
        );
        Ok(TestProgress::Passed)
    }

    async fn cleanup(&mut self, context: &CleanupContext<'_>) -> Result<()> {
        if let Some(node) = self.node.take() {
            node.stop(context.docker).await?;
        }
        Ok(())
    }
}
