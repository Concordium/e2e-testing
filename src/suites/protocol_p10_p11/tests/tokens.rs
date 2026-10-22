//! Token acceptance criteria. Each criterion owns a distinct token fixture.
mod support;

use super::super::{
    REJOIN_TIMEOUT,
    context::TestContext,
    registry::{ProtocolUpgradeTest, TestProgress, TestRegistry},
    wait,
};
use super::token_accounts::TokenPreservation;
use crate::fixture::genesis::GenesisAccount;
use anyhow::{Context as _, Result, ensure};
#[cfg(test)]
pub(super) use support::project;
use support::{
    Configuration, Fixture, Owner, Snapshot, agreement, capture, configuration, token_equivalent,
};
pub(super) use support::{collect_accounts, prepare_token};

pub(super) fn register(registry: &mut TestRegistry) -> Result<()> {
    registry.register::<Creation>()?;
    registry.register::<Minting>()?;
    registry.register::<MixedTransfer>()?;
    registry.register::<ConfigurationPreservation>()?;
    registry.register::<TokenPreservation>()?;
    registry.register::<FreshTransfer>()?;
    registry.register::<MixedAgreement>()?;
    registry.register::<BeforeAgreement>()?;
    registry.register::<AfterAgreement>()?;
    registry.register::<OperationsAgreement>()?;
    registry.register::<AllNodesAgreement>()
}

struct Creation(Owner);
impl ProtocolUpgradeTest for Creation {
    fn name() -> String {
        "A token can be created through new software at P10".into()
    }
    fn required_accounts() -> usize {
        1
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Owner::new(accounts, "E2E-CREATE")?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.create(context).await?;
        Ok(TestProgress::Passed)
    }
}

struct Minting(Owner);
impl ProtocolUpgradeTest for Minting {
    fn name() -> String {
        "Minting increases the token supply".into()
    }
    fn required_accounts() -> usize {
        1
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Owner::new(accounts, "E2E-MINT")?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.create(context).await?;
        self.0.mint(context).await?;
        Ok(TestProgress::Passed)
    }
}

struct MixedTransfer(Fixture);
impl ProtocolUpgradeTest for MixedTransfer {
    fn name() -> String {
        "A token transfer through old software preserves supply and updates balances".into()
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Fixture::new(accounts, "E2E-TRANSFER")?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.owner.create(context).await?;
        self.0.transfer(context, 1, false).await?;
        Ok(TestProgress::Passed)
    }
}

struct FreshTransfer(Fixture);
impl ProtocolUpgradeTest for FreshTransfer {
    fn name() -> String {
        "A fresh token transfer finalizes at P11".into()
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Fixture::new(accounts, "E2E-P11")?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn on_preservation_verified(
        &mut self,
        context: &TestContext<'_>,
    ) -> Result<TestProgress> {
        self.0.transfer(context, 0, true).await?;
        Ok(TestProgress::Passed)
    }
}

struct ConfigurationPreservation {
    fixture: Fixture,
    before: Option<Configuration>,
}
impl ProtocolUpgradeTest for ConfigurationPreservation {
    fn name() -> String {
        "Token configuration and supply survive activation".into()
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self {
            fixture: Fixture::new(accounts, "E2E-CONFIG")?,
            before: None,
        })
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.fixture.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn on_before_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.before = Some(
            wait::bounded(
                "capture token configuration baseline",
                REJOIN_TIMEOUT,
                configuration(context, &self.fixture.owner.id),
            )
            .await?,
        );
        Ok(TestProgress::Pending)
    }
    async fn on_after_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        wait::bounded(
            "verify token configuration preservation",
            REJOIN_TIMEOUT,
            async {
                let before = self
                    .before
                    .as_ref()
                    .context("No token configuration baseline")?;
                let after = configuration(context, &self.fixture.owner.id).await?;
                ensure!(
                    after.block.height >= before.block.height,
                    "Finalized height regressed"
                );
                ensure!(
                    token_equivalent(&before.token, &after.token)?,
                    "Token configuration or supply changed across activation"
                );
                Ok(())
            },
        )
        .await?;
        Ok(TestProgress::Passed)
    }
}

struct MixedAgreement(Fixture);
impl ProtocolUpgradeTest for MixedAgreement {
    fn name() -> String {
        "Mixed-version nodes agree on finalized token configuration and balances".into()
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Fixture::new(accounts, "E2E-AGREE-MIXED")?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        agreement(context, &self.0.owner.id, None, true).await?;
        Ok(TestProgress::Passed)
    }
}

struct BeforeAgreement(Fixture);
impl ProtocolUpgradeTest for BeforeAgreement {
    fn name() -> String {
        "All nodes agree on pre-update token configuration and balances".into()
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Fixture::new(accounts, "E2E-AGREE-BEFORE")?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn on_before_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, &self.0.owner.id, None, true).await?;
        Ok(TestProgress::Passed)
    }
}

struct AfterAgreement(Fixture);
impl ProtocolUpgradeTest for AfterAgreement {
    fn name() -> String {
        "Validators agree on preserved token configuration and balances".into()
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Fixture::new(accounts, "E2E-AGREE-AFTER")?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn on_after_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, &self.0.owner.id, None, false).await?;
        Ok(TestProgress::Passed)
    }
}

struct OperationsAgreement(Fixture);
impl ProtocolUpgradeTest for OperationsAgreement {
    fn name() -> String {
        "Validators agree on finalized P11 token configuration and balances".into()
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Fixture::new(accounts, "E2E-AGREE-OPS")?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn on_operations_complete(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, &self.0.owner.id, None, false).await?;
        Ok(TestProgress::Passed)
    }
}

struct AllNodesAgreement {
    fixture: Fixture,
    reference: Option<Snapshot>,
}
impl ProtocolUpgradeTest for AllNodesAgreement {
    fn name() -> String {
        "All five nodes agree on saved P11 token configuration and balances".into()
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self {
            fixture: Fixture::new(accounts, "E2E-AGREE-ALL")?,
            reference: None,
        })
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.fixture.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn on_operations_complete(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.reference = Some(
            wait::bounded(
                "save token recovery evidence",
                REJOIN_TIMEOUT,
                capture(context, &self.fixture.owner.id),
            )
            .await?,
        );
        Ok(TestProgress::Pending)
    }
    async fn on_all_nodes_ready(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(
            context,
            &self.fixture.owner.id,
            Some(
                self.reference
                    .as_ref()
                    .context("No token recovery evidence")?,
            ),
            true,
        )
        .await?;
        Ok(TestProgress::Passed)
    }
}
