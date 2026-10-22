//! Independent delegation, staking-preservation and staking-agreement criteria.
use super::super::registry::{ProtocolUpgradeTest, TestProgress, TestRegistry};
use super::super::{
    REJOIN_TIMEOUT,
    context::TestContext,
    operations,
    state::{FinalizedBlock, rewards_between},
    wait,
};
use crate::fixture::genesis::GenesisAccount;
use anyhow::{Context as _, Result, ensure};
use concordium_rust_sdk::{
    common::types::Amount,
    id::types::AccountAddress,
    types::{AccountInfo, AccountStakingInfo},
};
use std::collections::BTreeMap;

/// Register independent delegation, preservation, and cross-node agreement criteria.
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
    delegator: GenesisAccount,
}
impl Fixture {
    /// Require one private account for this criterion's delegation setup.
    fn new(accounts: Vec<GenesisAccount>) -> Result<Self> {
        let [delegator]: [GenesisAccount; 1] =
            accounts.try_into().ok().context("Expected 1 account")?;
        Ok(Self { delegator })
    }
    /// Finalize passive delegation through old software while node versions are mixed.
    async fn prepare(&self, context: &TestContext<'_>) -> Result<()> {
        operations::delegate(&mut context.client(1).await?, &self.delegator).await
    }
}

struct Delegation(Fixture);
impl ProtocolUpgradeTest for Delegation {
    fn name() -> String {
        "Passive delegation finalizes with rewards not restaked".into()
    }
    fn required_accounts() -> usize {
        1
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Fixture::new(accounts)?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        Ok(TestProgress::Passed)
    }
}

struct Snapshot {
    block: FinalizedBlock,
    accounts: BTreeMap<AccountAddress, Option<AccountStakingInfo>>,
}
/// Preserve absent staking roles, but reject SDK variants this suite cannot interpret.
fn known_staking_info(info: &AccountInfo) -> Result<Option<AccountStakingInfo>> {
    info.account_stake
        .as_ref()
        .map(|stake| stake.as_known().context("Unknown staking state").cloned())
        .transpose()
}
/// Read all tracked staking roles and settings at one finalized block on validator 0.
async fn capture(context: &TestContext<'_>) -> Result<Snapshot> {
    let mut client = context.client(0).await?;
    let block = FinalizedBlock::capture(&mut client).await?;
    let mut accounts = BTreeMap::new();
    for address in context.accounts() {
        let info = client
            .get_account_info(&(*address).into(), block.hash)
            .await?
            .response;
        accounts.insert(*address, known_staking_info(&info)?);
    }
    Ok(Snapshot { block, accounts })
}
struct Preservation {
    fixture: Fixture,
    before: Option<Snapshot>,
}
impl ProtocolUpgradeTest for Preservation {
    fn name() -> String {
        "Staking configuration is preserved with only restakable reward deltas".into()
    }
    fn required_accounts() -> usize {
        1
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self {
            fixture: Fixture::new(accounts)?,
            before: None,
        })
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.fixture.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn on_before_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.before = Some(
            wait::bounded("capture staking baseline", REJOIN_TIMEOUT, capture(context)).await?,
        );
        Ok(TestProgress::Pending)
    }
    async fn on_after_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        wait::bounded("reward-aware staking preservation", REJOIN_TIMEOUT, async {
            let before = self.before.as_ref().context("No staking baseline")?;
            let after = capture(context).await?;
            let rewards =
                rewards_between(&mut context.client(0).await?, &before.block, &after.block).await?;
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
        })
        .await?;
        Ok(TestProgress::Passed)
    }
}

/// Require unchanged roles and settings, allowing only restaked reward increases in stake.
/// The supplied reward must exclude foundation credits.
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

/// Compare validators against saved staking evidence at its exact finalized block.
/// Include the observer only when requested.
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
            let info = client
                .get_account_info(&(*address).into(), snapshot.block.hash)
                .await?
                .response;
            ensure!(
                serde_json::to_value(known_staking_info(&info)?)?
                    == serde_json::to_value(expected)?,
                "Node {node} disagrees on staking account {address}"
            );
        }
    }
    Ok(())
}

/// Retry cross-node comparisons using saved evidence, or capture a baseline once if absent.
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
impl ProtocolUpgradeTest for MixedAgreement {
    fn name() -> String {
        "Mixed-version nodes agree on finalized staking configuration".into()
    }
    fn required_accounts() -> usize {
        1
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Fixture::new(accounts)?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        agreement(context, None, true).await?;
        Ok(TestProgress::Passed)
    }
}

struct BeforeAgreement(Fixture);
impl ProtocolUpgradeTest for BeforeAgreement {
    fn name() -> String {
        "All nodes agree on pre-update staking configuration".into()
    }
    fn required_accounts() -> usize {
        1
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Fixture::new(accounts)?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn on_before_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, None, true).await?;
        Ok(TestProgress::Passed)
    }
}

struct AfterAgreement(Fixture);
impl ProtocolUpgradeTest for AfterAgreement {
    fn name() -> String {
        "Validators agree on preserved staking configuration".into()
    }
    fn required_accounts() -> usize {
        1
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Fixture::new(accounts)?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn on_after_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, None, false).await?;
        Ok(TestProgress::Passed)
    }
}

struct OperationsAgreement(Fixture);
impl ProtocolUpgradeTest for OperationsAgreement {
    fn name() -> String {
        "Validators agree on finalized P11 staking configuration".into()
    }
    fn required_accounts() -> usize {
        1
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(Fixture::new(accounts)?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.0.prepare(context).await?;
        Ok(TestProgress::Pending)
    }
    async fn on_operations_complete(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, None, false).await?;
        Ok(TestProgress::Passed)
    }
}

struct AllNodesAgreement {
    fixture: Fixture,
    reference: Option<Snapshot>,
}
impl ProtocolUpgradeTest for AllNodesAgreement {
    fn name() -> String {
        "All five nodes agree on saved P11 staking configuration".into()
    }
    fn required_accounts() -> usize {
        1
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self {
            fixture: Fixture::new(accounts)?,
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
                "save staking recovery evidence",
                REJOIN_TIMEOUT,
                capture(context),
            )
            .await?,
        );
        Ok(TestProgress::Pending)
    }
    async fn on_all_nodes_ready(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(
            context,
            Some(
                self.reference
                    .as_ref()
                    .context("No staking recovery evidence")?,
            ),
            true,
        )
        .await?;
        Ok(TestProgress::Passed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use concordium_rust_sdk::types::DelegationTarget;
    /// Build passive delegation state with stake measured in micro-CCD.
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
