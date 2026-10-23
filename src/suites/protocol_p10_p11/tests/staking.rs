//! Independent delegation, staking-preservation and staking-agreement criteria.
use super::super::{
    REJOIN_TIMEOUT,
    context::TestContext,
    operations,
    state::{FinalizedBlock, account_at, rewards_between},
    wait,
};
use super::upgrade_test::TestDefinition;
use super::{
    accounts_fixture::{Account, TestAccounts},
    registry::TestRegistry,
    upgrade_test::{TestProgress, UpgradeTest, Verification},
};
use anyhow::{Context as _, Result, ensure};
use concordium_rust_sdk::{
    common::types::Amount,
    id::types::AccountAddress,
    types::{AccountInfo, AccountStakingInfo},
};
use std::collections::BTreeMap;

pub(super) fn register(registry: &mut TestRegistry) -> Result<()> {
    registry.register::<Delegation>()?;
    registry.register::<Preservation>()?;
    registry.register::<MixedAgreement>()?;
    registry.register::<BeforeAgreement>()?;
    registry.register::<AfterAgreement>()?;
    registry.register::<OperationsAgreement>()?;
    registry.register::<AllNodesAgreement>()
}
struct Fixture {
    delegator: Account,
}
impl Fixture {
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self {
            delegator: accounts.take("delegator")?,
        })
    }
    async fn prepare(&self, context: &TestContext<'_>) -> Result<()> {
        operations::delegate(&mut context.client(1).await?, &self.delegator).await
    }
}
struct Delegation(Fixture);
impl TestDefinition for Delegation {
    const NAME: &'static str = "Passive delegation finalizes with rewards not restaked";
    const ACCOUNTS: &'static [&'static str] = &["delegator"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Fixture::new(accounts)?))
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for Delegation {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification.verify(self.0.prepare(context)).await
    }
}
struct Snapshot {
    block: FinalizedBlock,
    accounts: BTreeMap<AccountAddress, Option<AccountStakingInfo>>,
}
fn project(info: &AccountInfo) -> Result<Option<AccountStakingInfo>> {
    info.account_stake
        .as_ref()
        .map(|stake| stake.as_known().context("Unknown staking state").cloned())
        .transpose()
}
async fn capture(context: &TestContext<'_>) -> Result<Snapshot> {
    let mut client = context.client(0).await?;
    let block = FinalizedBlock::capture(&mut client).await?;
    let mut accounts = BTreeMap::new();
    for address in context.accounts() {
        accounts.insert(
            *address,
            project(&account_at(&mut client, *address, block.hash).await?)?,
        );
    }
    Ok(Snapshot { block, accounts })
}
struct Preservation {
    fixture: Fixture,
    before: Option<Snapshot>,
}
impl TestDefinition for Preservation {
    const NAME: &'static str =
        "Staking configuration is preserved with only restakable reward deltas";
    const ACCOUNTS: &'static [&'static str] = &["delegator"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self {
            fixture: Fixture::new(accounts)?,
            before: None,
        })
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for Preservation {
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
            wait::bounded("capture staking baseline", REJOIN_TIMEOUT, capture(context)).await?,
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
                "reward-aware staking preservation",
                REJOIN_TIMEOUT,
                async {
                    let before = self.before.as_ref().context("No staking baseline")?;
                    let after = capture(context).await?;
                    let rewards =
                        rewards_between(&mut context.client(0).await?, &before.block, &after.block)
                            .await?;
                    ensure!(
                        before.accounts.keys().eq(after.accounts.keys()),
                        "Tracked staking accounts changed"
                    );
                    for (address, old) in &before.accounts {
                        let reward = rewards
                            .restakable
                            .get(address)
                            .copied()
                            .unwrap_or(Amount::zero());
                        staking_preserved(old.as_ref(), after.accounts[address].as_ref(), reward)?;
                    }
                    Ok(())
                },
            ))
            .await
    }
}
/// Permit stake increases only from restaked rewards, never foundation credits.
fn staking_preserved(
    old: Option<&AccountStakingInfo>,
    new: Option<&AccountStakingInfo>,
    reward: Amount,
) -> Result<()> {
    let (Some(old), Some(new)) = (old, new) else {
        ensure!(old.is_none() && new.is_none(), "Staking role changed");
        return Ok(());
    };
    let restake = match old {
        AccountStakingInfo::Baker {
            restake_earnings, ..
        }
        | AccountStakingInfo::Delegated {
            restake_earnings, ..
        } => *restake_earnings,
    };
    let expected = old.staked_amount() + if restake { reward } else { Amount::zero() };
    ensure!(new.staked_amount() == expected, "Unexpected stake delta");
    let config = |stake: &AccountStakingInfo| -> Result<serde_json::Value> {
        let mut value = serde_json::to_value(stake)?;
        value
            .as_object_mut()
            .context("Staking configuration is not an object")?
            .remove("stakedAmount");
        Ok(value)
    };
    ensure!(
        config(new)? == config(old)?,
        "Staking configuration changed across activation"
    );
    Ok(())
}
async fn compare(context: &TestContext<'_>, snapshot: &Snapshot, observer: bool) -> Result<()> {
    for node in context.node_indices(observer) {
        let mut client = context.client(node).await?;
        ensure!(
            client
                .get_block_info(snapshot.block.hash)
                .await?
                .response
                .finalized,
            "Node {node} has not finalized staking evidence"
        );
        for (address, expected) in &snapshot.accounts {
            let info = account_at(&mut client, *address, snapshot.block.hash).await?;
            ensure!(
                serde_json::to_value(project(&info)?)? == serde_json::to_value(expected)?,
                "Node {node} disagrees on staking account {address}"
            );
        }
    }
    Ok(())
}
async fn agreement(
    context: &TestContext<'_>,
    reference: Option<&Snapshot>,
    observer: bool,
) -> Result<()> {
    let current;
    let snapshot = match reference {
        Some(saved) => saved,
        None => {
            current = wait::bounded(
                "capture staking agreement evidence",
                REJOIN_TIMEOUT,
                capture(context),
            )
            .await?;
            &current
        }
    };
    wait::agreement(|| compare(context, snapshot, observer)).await
}
struct MixedAgreement(Fixture);
impl TestDefinition for MixedAgreement {
    const NAME: &'static str = "Mixed-version nodes agree on finalized staking configuration";
    const ACCOUNTS: &'static [&'static str] = &["delegator"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Fixture::new(accounts)?))
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
        verification.verify(agreement(context, None, true)).await
    }
}
struct BeforeAgreement(Fixture);
impl TestDefinition for BeforeAgreement {
    const NAME: &'static str = "All nodes agree on pre-update staking configuration";
    const ACCOUNTS: &'static [&'static str] = &["delegator"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Fixture::new(accounts)?))
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
        verification.verify(agreement(context, None, true)).await
    }
}
struct AfterAgreement(Fixture);
impl TestDefinition for AfterAgreement {
    const NAME: &'static str = "Validators agree on preserved staking configuration";
    const ACCOUNTS: &'static [&'static str] = &["delegator"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Fixture::new(accounts)?))
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
        verification.verify(agreement(context, None, false)).await
    }
}
struct OperationsAgreement(Fixture);
impl TestDefinition for OperationsAgreement {
    const NAME: &'static str = "Validators agree on finalized P11 staking configuration";
    const ACCOUNTS: &'static [&'static str] = &["delegator"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self(Fixture::new(accounts)?))
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
        verification.verify(agreement(context, None, false)).await
    }
}
struct AllNodesAgreement {
    fixture: Fixture,
    reference: Option<Snapshot>,
}
impl TestDefinition for AllNodesAgreement {
    const NAME: &'static str = "All five nodes agree on saved P11 staking configuration";
    const ACCOUNTS: &'static [&'static str] = &["delegator"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self {
            fixture: Fixture::new(accounts)?,
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
                "save staking recovery evidence",
                REJOIN_TIMEOUT,
                capture(context),
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
                    Some(
                        self.reference
                            .as_ref()
                            .context("No staking recovery evidence")?,
                    ),
                    true,
                )
                .await
            })
            .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use concordium_rust_sdk::types::DelegationTarget;
    fn delegated(amount: u64, restake: bool) -> AccountStakingInfo {
        AccountStakingInfo::Delegated {
            staked_amount: Amount::from_micro_ccd(amount),
            restake_earnings: restake,
            delegation_target: DelegationTarget::Passive,
            pending_change: None,
        }
    }
    #[test]
    fn restaked_rewards_are_allowed_but_unexplained_deltas_fail() {
        let old = delegated(100, true);
        let paid = delegated(110, true);
        assert!(staking_preserved(Some(&old), Some(&paid), Amount::from_micro_ccd(10)).is_ok());
        assert!(staking_preserved(Some(&old), Some(&paid), Amount::zero()).is_err());
    }
    #[test]
    fn passive_configuration_and_role_are_preserved() {
        let old = delegated(100, false);
        assert!(staking_preserved(Some(&old), Some(&old), Amount::from_micro_ccd(10)).is_ok());
        assert!(
            staking_preserved(Some(&old), Some(&delegated(100, true)), Amount::zero()).is_err()
        );
        assert!(staking_preserved(Some(&old), None, Amount::zero()).is_err());
    }
}
