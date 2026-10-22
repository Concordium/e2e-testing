//! Inspect finalized transition references rather than infer coverage from orchestration.
use super::super::context::TestContext;
use super::super::registry::{ProtocolUpgradeTest, TestProgress, TestRegistry};
use crate::fixture::genesis::GenesisAccount;
use anyhow::{Result, ensure};

pub(super) fn register(registry: &mut TestRegistry) -> Result<()> {
    registry.register::<Submission>()?;
    registry.register::<Activation>()
}

struct Submission;
impl ProtocolUpgradeTest for Submission {
    fn name() -> String {
        "The protocol update has a finalized submission block".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_after_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let block = context
            .client(0)
            .await?
            .get_block_info(context.update_submission_block()?)
            .await?
            .response;
        ensure!(block.finalized, "Update submission block is not finalized");
        Ok(TestProgress::Passed)
    }
}

struct Activation;
impl ProtocolUpgradeTest for Activation {
    fn name() -> String {
        "A finalized block makes progress in the P11 era".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_after_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let block = context
            .client(0)
            .await?
            .get_block_info(context.p11_activation_block()?)
            .await?
            .response;
        ensure!(
            block.finalized && block.protocol_version.0 == 11 && block.era_block_height.height > 0,
            "No finalized P11 era progress"
        );
        Ok(TestProgress::Passed)
    }
}
