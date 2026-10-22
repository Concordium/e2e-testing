//! Finalized state-hash criteria do not depend on any feature's fixture.
use super::super::{context::TestContext, state::FinalizedBlock, wait};
use super::upgrade_test::TestDefinition;
use super::{
    accounts_fixture::TestAccounts,
    registry::TestRegistry,
    upgrade_test::{TestProgress, UpgradeTest, Verification},
};
use anyhow::{Context as _, Result, ensure};
pub(super) fn register(registry: &mut TestRegistry) -> Result<()> {
    registry.register::<Mixed>()?;
    registry.register::<Before>()?;
    registry.register::<After>()?;
    registry.register::<Operations>()?;
    registry.register::<AllNodes>()
}
async fn compare(
    context: &TestContext<'_>,
    reference: &FinalizedBlock,
    observer: bool,
) -> Result<()> {
    for node in context.node_indices(observer) {
        let block = context
            .client(node)
            .await?
            .get_block_info(reference.hash)
            .await?
            .response;
        ensure!(
            block.finalized,
            "Node {node} has not finalized reference block {}",
            reference.hash
        );
        ensure!(
            block.block_state_hash == reference.state_hash,
            "Node {node} disagrees on finalized state hash"
        );
    }
    Ok(())
}
async fn agreement(
    context: &TestContext<'_>,
    reference: Option<&FinalizedBlock>,
    observer: bool,
) -> Result<()> {
    let current;
    let reference = match reference {
        Some(saved) => saved,
        None => {
            current = FinalizedBlock::capture(&mut context.client(0).await?).await?;
            &current
        }
    };
    wait::agreement(|| compare(context, reference, observer)).await
}
struct Mixed;
impl TestDefinition for Mixed {
    const NAME: &'static str = "Mixed-version nodes agree on the finalized block and state hash";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self)
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for Mixed {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification.verify(agreement(context, None, true)).await
    }
}
struct Before;
impl TestDefinition for Before {
    const NAME: &'static str = "All nodes agree on the pre-update finalized block and state hash";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self)
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for Before {
    async fn before_activation(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification.verify(agreement(context, None, true)).await
    }
}
struct After;
impl TestDefinition for After {
    const NAME: &'static str = "Validators agree on the preserved finalized block and state hash";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self)
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for After {
    async fn after_activation(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification.verify(agreement(context, None, false)).await
    }
}
struct Operations;
impl TestDefinition for Operations {
    const NAME: &'static str = "Validators agree on the finalized P11 block and state hash";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self)
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for Operations {
    async fn operations_complete(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification.verify(agreement(context, None, false)).await
    }
}
struct AllNodes {
    reference: Option<FinalizedBlock>,
}
impl TestDefinition for AllNodes {
    const NAME: &'static str =
        "All five nodes agree on the saved finalized P11 block and state hash";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self { reference: None })
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for AllNodes {
    async fn operations_complete(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
        self.reference = Some(FinalizedBlock::capture(&mut context.client(0).await?).await?);
        Ok(TestProgress::Pending)
    }
    async fn all_nodes_ready(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification
            .verify(async {
                agreement(
                    context,
                    Some(self.reference.as_ref().context("No P11 chain reference")?),
                    true,
                )
                .await
            })
            .await
    }
}
