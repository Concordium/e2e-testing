//! Inspect finalized transition references rather than infer coverage from orchestration.
use super::super::context::TestContext;
use super::upgrade_test::TestDefinition;
use super::{
    accounts_fixture::TestAccounts,
    registry::TestRegistry,
    upgrade_test::{TestProgress, UpgradeTest, Verification},
};
use anyhow::{Result, ensure};
pub(super) fn register(registry: &mut TestRegistry) -> Result<()> {
    registry.register::<Submission>()?;
    registry.register::<Activation>()
}
struct Submission;
impl TestDefinition for Submission {
    const NAME: &'static str = "The protocol update has a finalized submission block";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self)
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for Submission {
    async fn after_activation(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification
            .verify(async {
                let block = context
                    .client(0)
                    .await?
                    .get_block_info(context.update_submission_block()?)
                    .await?
                    .response;
                ensure!(block.finalized, "Update submission block is not finalized");
                Ok(())
            })
            .await
    }
}
struct Activation;
impl TestDefinition for Activation {
    const NAME: &'static str = "A finalized block makes progress in the P11 era";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self)
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for Activation {
    async fn after_activation(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification
            .verify(async {
                let block = context
                    .client(0)
                    .await?
                    .get_block_info(context.p11_activation_block()?)
                    .await?
                    .response;
                ensure!(
                    block.finalized
                        && block.protocol_version.0 == 11
                        && block.era_block_height.height > 0,
                    "No finalized P11 era progress"
                );
                Ok(())
            })
            .await
    }
}
