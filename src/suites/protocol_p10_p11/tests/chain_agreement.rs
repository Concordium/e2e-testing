//! Finalized state-hash criteria do not depend on any feature's fixture.
use super::super::registry::{ProtocolUpgradeTest, TestProgress, TestRegistry};
use super::super::{context::TestContext, state::FinalizedBlock, wait};
use crate::fixture::genesis::GenesisAccount;
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
impl ProtocolUpgradeTest for Mixed {
    fn name() -> String {
        "Mixed-version nodes agree on the finalized block and state hash".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, None, true).await?;
        Ok(TestProgress::Passed)
    }
}

struct Before;
impl ProtocolUpgradeTest for Before {
    fn name() -> String {
        "All nodes agree on the pre-update finalized block and state hash".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_before_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, None, true).await?;
        Ok(TestProgress::Passed)
    }
}

struct After;
impl ProtocolUpgradeTest for After {
    fn name() -> String {
        "Validators agree on the preserved finalized block and state hash".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_after_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, None, false).await?;
        Ok(TestProgress::Passed)
    }
}

struct Operations;
impl ProtocolUpgradeTest for Operations {
    fn name() -> String {
        "Validators agree on the finalized P11 block and state hash".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_operations_complete(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, None, false).await?;
        Ok(TestProgress::Passed)
    }
}

struct AllNodes {
    reference: Option<FinalizedBlock>,
}
impl ProtocolUpgradeTest for AllNodes {
    fn name() -> String {
        "All five nodes agree on the saved finalized P11 block and state hash".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self { reference: None })
    }
    async fn on_operations_complete(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.reference = Some(FinalizedBlock::capture(&mut context.client(0).await?).await?);
        Ok(TestProgress::Pending)
    }
    async fn on_all_nodes_ready(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(
            context,
            Some(self.reference.as_ref().context("No P11 chain reference")?),
            true,
        )
        .await?;
        Ok(TestProgress::Passed)
    }
}
