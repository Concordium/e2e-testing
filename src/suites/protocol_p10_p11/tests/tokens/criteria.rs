//! Each token criterion creates and owns a distinct token fixture.
use super::super::upgrade_test::TestDefinition;
use super::super::{
    token_accounts::TokenPreservation,
    upgrade_test::{TestProgress, UpgradeTest, Verification},
};
use super::*;

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
impl TestDefinition for Creation {
    const NAME: &'static str = "A token can be created through new software at P10";
    const ACCOUNTS: &'static [&'static str] = &["governor"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Owner::new(accounts, "E2E-CREATE")?))
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for Creation {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification.verify(self.0.create(context)).await
    }
}
struct Minting(Owner);
impl TestDefinition for Minting {
    const NAME: &'static str = "Minting increases the token supply";
    const ACCOUNTS: &'static [&'static str] = &["governor"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Owner::new(accounts, "E2E-MINT")?))
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for Minting {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        self.0.create(context).await?;
        verification.verify(self.0.mint(context)).await
    }
}
struct MixedTransfer(Fixture);
impl TestDefinition for MixedTransfer {
    const NAME: &'static str =
        "A token transfer through old software preserves supply and updates balances";
    const ACCOUNTS: &'static [&'static str] = &["governor", "recipient"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Fixture::new(accounts, "E2E-TRANSFER")?))
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for MixedTransfer {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        self.0.owner.create(context).await?;
        verification
            .verify(self.0.transfer(context, 1, false))
            .await
    }
}
struct FreshTransfer(Fixture);
impl TestDefinition for FreshTransfer {
    const NAME: &'static str = "A fresh token transfer finalizes at P11";
    const ACCOUNTS: &'static [&'static str] = &["governor", "recipient"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Fixture::new(accounts, "E2E-P11")?))
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for FreshTransfer {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn preservation_verified(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification.verify(self.0.transfer(context, 0, true)).await
    }
}
struct ConfigurationPreservation {
    fixture: Fixture,
    before: Option<Configuration>,
}
impl TestDefinition for ConfigurationPreservation {
    const NAME: &'static str = "Token configuration and supply survive activation";
    const ACCOUNTS: &'static [&'static str] = &["governor", "recipient"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self {
            fixture: Fixture::new(accounts, "E2E-CONFIG")?,
            before: None,
        })
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for ConfigurationPreservation {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
        self.fixture.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn before_activation(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
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
    async fn after_activation(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification
            .verify(wait::bounded(
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
            ))
            .await
    }
}
struct MixedAgreement(Fixture);
impl TestDefinition for MixedAgreement {
    const NAME: &'static str =
        "Mixed-version nodes agree on finalized token configuration and balances";
    const ACCOUNTS: &'static [&'static str] = &["governor", "recipient"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Fixture::new(accounts, "E2E-AGREE-MIXED")?))
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for MixedAgreement {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        verification
            .verify(agreement(context, &self.0.owner.id, None, true))
            .await
    }
}
struct BeforeAgreement(Fixture);
impl TestDefinition for BeforeAgreement {
    const NAME: &'static str = "All nodes agree on pre-update token configuration and balances";
    const ACCOUNTS: &'static [&'static str] = &["governor", "recipient"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Fixture::new(accounts, "E2E-AGREE-BEFORE")?))
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for BeforeAgreement {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn before_activation(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification
            .verify(agreement(context, &self.0.owner.id, None, true))
            .await
    }
}
struct AfterAgreement(Fixture);
impl TestDefinition for AfterAgreement {
    const NAME: &'static str = "Validators agree on preserved token configuration and balances";
    const ACCOUNTS: &'static [&'static str] = &["governor", "recipient"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Fixture::new(accounts, "E2E-AGREE-AFTER")?))
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for AfterAgreement {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn after_activation(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification
            .verify(agreement(context, &self.0.owner.id, None, false))
            .await
    }
}
struct OperationsAgreement(Fixture);
impl TestDefinition for OperationsAgreement {
    const NAME: &'static str = "Validators agree on finalized P11 token configuration and balances";
    const ACCOUNTS: &'static [&'static str] = &["governor", "recipient"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Fixture::new(accounts, "E2E-AGREE-OPS")?))
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for OperationsAgreement {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn operations_complete(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification
            .verify(agreement(context, &self.0.owner.id, None, false))
            .await
    }
}
struct AllNodesAgreement {
    fixture: Fixture,
    reference: Option<Snapshot>,
}
impl TestDefinition for AllNodesAgreement {
    const NAME: &'static str = "All five nodes agree on saved P11 token configuration and balances";
    const ACCOUNTS: &'static [&'static str] = &["governor", "recipient"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self {
            fixture: Fixture::new(accounts, "E2E-AGREE-ALL")?,
            reference: None,
        })
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for AllNodesAgreement {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
        self.fixture.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn operations_complete(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
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
    async fn all_nodes_ready(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification
            .verify(async {
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
                .await
            })
            .await
    }
}
