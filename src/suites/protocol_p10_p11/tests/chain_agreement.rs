//! Validator finalized state-hash criteria, independent of feature fixtures.
use super::super::registry::{ProtocolUpgradeTest, TestContext, TestProgress, TestRegistry};
use super::super::{state::FinalizedBlock, wait};
use anyhow::{Result, ensure};

pub(super) fn register(registry: &mut TestRegistry) {
    let mut registry = registry.group("Chain agreement");
    registry.register(Before);
    registry.register(After);
    registry.register(Operations);
}

async fn agreement(context: &TestContext<'_>) -> Result<()> {
    let reference = FinalizedBlock::capture(&mut context.validators[0].connect().await?).await?;
    for (node, validator) in context.validators.iter().enumerate() {
        let mut client = validator.connect().await?;
        wait::finalized(&mut client, reference.hash).await?;
        let block = client.get_block_info(reference.hash).await?.response;
        ensure!(
            block.block_state_hash == reference.state_hash,
            "Validator {node} disagrees on finalized state hash"
        );
    }
    Ok(())
}

struct Before;
#[async_trait::async_trait(?Send)]
impl ProtocolUpgradeTest for Before {
    fn name(&self) -> String {
        "Validators agree on the pre-update finalized block and state hash".into()
    }
    async fn finish_before_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context).await?;
        Ok(TestProgress::Passed)
    }
}

struct After;
#[async_trait::async_trait(?Send)]
impl ProtocolUpgradeTest for After {
    fn name(&self) -> String {
        "Validators agree on the preserved finalized block and state hash".into()
    }
    async fn prepare_after_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context).await?;
        Ok(TestProgress::Passed)
    }
}

struct Operations;
#[async_trait::async_trait(?Send)]
impl ProtocolUpgradeTest for Operations {
    fn name(&self) -> String {
        "Validators agree on the finalized P11 block and state hash".into()
    }
    async fn finish_after_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context).await?;
        Ok(TestProgress::Passed)
    }
}
