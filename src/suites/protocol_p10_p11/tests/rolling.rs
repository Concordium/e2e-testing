//! One result per validator and continuity criterion, with private transfer accounts.
use super::super::{
    POLL_INTERVAL, REJOIN_TIMEOUT, context::TestContext, operations, wait::bounded,
};
use super::upgrade_test::TestDefinition;
use super::{
    accounts_fixture::{Account, TestAccounts},
    registry::TestRegistry,
    upgrade_test::{TestProgress, UpgradeTest, Verification},
};
use anyhow::{Result, ensure};

pub(super) fn register(registry: &mut TestRegistry) -> Result<()> {
    register_validator::<0>(registry)?;
    register_validator::<1>(registry)?;
    register_validator::<2>(registry)?;
    register_validator::<3>(registry)
}
fn register_validator<const I: usize>(registry: &mut TestRegistry) -> Result<()> {
    let target = format!("validator {I}");
    registry.register_in::<Finalization<I>>(target.clone())?;
    registry.register_in::<Transfer<I, false>>(target.clone())?;
    registry.register_in::<Rejoin<I>>(target.clone())?;
    registry.register_in::<Transfer<I, true>>(target)
}
struct Finalization<const I: usize>;
impl<const I: usize> TestDefinition for Finalization<I> {
    const NAME: &'static str = "Finalization progresses with this validator offline";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self)
    }
}
#[async_trait::async_trait(?Send)]
impl<const I: usize> UpgradeTest for Finalization<I> {
    async fn validator_offline(
        &mut self,
        index: usize,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        if index != I {
            return Ok(TestProgress::Pending);
        }
        verification
            .verify(bounded(
                "finalization during validator downtime",
                REJOIN_TIMEOUT,
                async {
                    let height = context.pre_stop_height(I)?;
                    let mut client = context.client(if I == 0 { 1 } else { 0 }).await?;
                    loop {
                        let observed = client.get_consensus_info().await?;
                        if observed.last_finalized_block_height > height {
                            return Ok(());
                        }
                        tracing::debug!(
                            height = observed.last_finalized_block_height.height,
                            "waiting for offline continuity"
                        );
                        tokio::time::sleep(POLL_INTERVAL).await;
                    }
                },
            ))
            .await
    }
}
struct Rejoin<const I: usize>;
impl<const I: usize> TestDefinition for Rejoin<I> {
    const NAME: &'static str = "The upgraded validator recognizes the finalized P10 target";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self)
    }
}
#[async_trait::async_trait(?Send)]
impl<const I: usize> UpgradeTest for Rejoin<I> {
    async fn validator_rejoined(
        &mut self,
        index: usize,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        if index != I {
            return Ok(TestProgress::Pending);
        }
        verification
            .verify(async {
                let block = context
                    .client(I)
                    .await?
                    .get_block_info(context.rejoin_target(I)?)
                    .await?
                    .response;
                ensure!(
                    block.finalized && block.protocol_version.0 == 10,
                    "Validator did not recover the P10 target"
                );
                Ok(())
            })
            .await
    }
}
struct Transfer<const I: usize, const REJOINED: bool> {
    sender: Account,
    recipient: Account,
}
impl<const I: usize, const REJOINED: bool> TestDefinition for Transfer<I, REJOINED> {
    const NAME: &'static str = if REJOINED {
        "A CCD transfer through the rejoined validator finalizes"
    } else {
        "A CCD transfer finalizes with this validator offline"
    };
    const ACCOUNTS: &'static [&'static str] = &["sender", "recipient"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self {
            sender: accounts.take("sender")?,
            recipient: accounts.take("recipient")?,
        })
    }
}
#[async_trait::async_trait(?Send)]
impl<const I: usize, const REJOINED: bool> UpgradeTest for Transfer<I, REJOINED> {
    async fn validator_offline(
        &mut self,
        index: usize,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        if REJOINED || index != I {
            return Ok(TestProgress::Pending);
        }
        verification
            .verify(self.transfer(context, if I == 0 { 1 } else { 0 }))
            .await
    }
    async fn validator_rejoined(
        &mut self,
        index: usize,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        if !REJOINED || index != I {
            return Ok(TestProgress::Pending);
        }
        verification.verify(self.transfer(context, I)).await
    }
}
impl<const I: usize, const REJOINED: bool> Transfer<I, REJOINED> {
    async fn transfer(&self, context: &TestContext<'_>, node: usize) -> Result<()> {
        let recipient = self.recipient.lock().await.address;
        operations::ccd(&mut context.client(node).await?, &self.sender, recipient).await?;
        Ok(())
    }
}
