//! One result per validator and continuity criterion, with private transfer accounts.
use super::super::registry::{ProtocolUpgradeTest, TestProgress, TestRegistry};
use super::super::{
    POLL_INTERVAL, REJOIN_TIMEOUT, context::TestContext, operations, wait::bounded,
};
use crate::fixture::genesis::GenesisAccount;
use anyhow::{Context as _, Result, ensure};

pub(super) fn register(registry: &mut TestRegistry) -> Result<()> {
    register_validator::<0>(registry)?;
    register_validator::<1>(registry)?;
    register_validator::<2>(registry)?;
    register_validator::<3>(registry)
}

/// Register the same four continuity criteria for validator index `I` (0–3).
/// Each test handles only hooks for `I` and includes the index in its name.
/// This helper keeps the criterion set consistent across all four validators.
fn register_validator<const I: usize>(registry: &mut TestRegistry) -> Result<()> {
    registry.register::<Finalization<I>>()?;
    registry.register::<Transfer<I, false>>()?;
    registry.register::<Rejoin<I>>()?;
    registry.register::<Transfer<I, true>>()
}

struct Finalization<const I: usize>;
impl<const I: usize> ProtocolUpgradeTest for Finalization<I> {
    fn name() -> String {
        format!("Finalization progresses with validator {I} offline")
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_validator_offline(
        &mut self,
        index: usize,
        context: &TestContext<'_>,
    ) -> Result<TestProgress> {
        if index != I {
            return Ok(TestProgress::Pending);
        }
        bounded(
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
        )
        .await?;
        Ok(TestProgress::Passed)
    }
}

struct Rejoin<const I: usize>;
impl<const I: usize> ProtocolUpgradeTest for Rejoin<I> {
    fn name() -> String {
        format!("Upgraded validator {I} recognizes the finalized P10 target")
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_validator_rejoined(
        &mut self,
        index: usize,
        context: &TestContext<'_>,
    ) -> Result<TestProgress> {
        if index != I {
            return Ok(TestProgress::Pending);
        }
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
        Ok(TestProgress::Passed)
    }
}

struct Transfer<const I: usize, const REJOINED: bool> {
    sender: GenesisAccount,
    recipient: GenesisAccount,
}
impl<const I: usize, const REJOINED: bool> ProtocolUpgradeTest for Transfer<I, REJOINED> {
    fn name() -> String {
        if REJOINED {
            format!("A CCD transfer through rejoined validator {I} finalizes")
        } else {
            format!("A CCD transfer finalizes with validator {I} offline")
        }
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        let [sender, recipient]: [GenesisAccount; 2] =
            accounts.try_into().ok().context("Expected 2 accounts")?;
        Ok(Self { sender, recipient })
    }
    async fn on_validator_offline(
        &mut self,
        index: usize,
        context: &TestContext<'_>,
    ) -> Result<TestProgress> {
        if REJOINED || index != I {
            return Ok(TestProgress::Pending);
        }
        let node = if I == 0 { 1 } else { 0 };
        operations::ccd(
            &mut context.client(node).await?,
            &self.sender,
            self.recipient.address,
        )
        .await?;
        Ok(TestProgress::Passed)
    }
    async fn on_validator_rejoined(
        &mut self,
        index: usize,
        context: &TestContext<'_>,
    ) -> Result<TestProgress> {
        if !REJOINED || index != I {
            return Ok(TestProgress::Pending);
        }
        operations::ccd(
            &mut context.client(I).await?,
            &self.sender,
            self.recipient.address,
        )
        .await?;
        Ok(TestProgress::Passed)
    }
}
