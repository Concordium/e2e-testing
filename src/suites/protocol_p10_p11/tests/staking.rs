//! Passive delegation and reward-aware staking preservation across the update.
use super::super::lifecycle::{ProtocolUpgradeTest, TestContext, TestProgress, UpgradeRegistry};
use super::super::{
    REJOIN_TIMEOUT, operations,
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

pub(super) fn register(registry: &mut UpgradeRegistry) {
    registry.group("Staking").register(
        "Passive delegation and reward-aware staking preservation across P11",
        Box::new(StakingContinuity::default()),
    );
}

#[derive(Default)]
struct StakingContinuity {
    delegator: Option<GenesisAccount>,
    before: Option<Snapshot>,
}

#[async_trait::async_trait(?Send)]
impl ProtocolUpgradeTest for StakingContinuity {
    fn required_accounts(&self) -> usize {
        1
    }
    async fn prepare_before_update(
        &mut self,
        _context: &TestContext<'_>,
        accounts: Vec<GenesisAccount>,
    ) -> Result<()> {
        let [delegator]: [GenesisAccount; 1] =
            accounts.try_into().ok().context("Expected 1 account")?;
        self.delegator = Some(delegator);
        Ok(())
    }
    async fn run_before_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        operations::delegate(
            &mut context.validators[1].connect().await?,
            self.delegator.as_ref().context("Delegator not prepared")?,
        )
        .await?;
        Ok(TestProgress::Pending)
    }
    async fn finish_before_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let before =
            wait::bounded("capture staking baseline", REJOIN_TIMEOUT, capture(context)).await?;
        self.before = Some(before);
        Ok(TestProgress::Pending)
    }
    async fn prepare_after_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        wait::bounded("reward-aware staking preservation", REJOIN_TIMEOUT, async {
            let before = self.before.as_ref().context("No staking baseline")?;
            let after = capture(context).await?;
            let rewards = rewards_between(
                &mut context.validators[0].connect().await?,
                &before.block,
                &after.block,
            )
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
                staking_preserved(old.as_ref(), after.accounts[address].as_ref(), reward)
                    .with_context(|| format!("Staking preservation for {address}"))?;
            }
            Ok(())
        })
        .await?;
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
    let mut client = context.validators[0].connect().await?;
    let block = FinalizedBlock::capture(&mut client).await?;
    let mut accounts = BTreeMap::new();
    for address in context.accounts {
        let info = client
            .get_account_info(&(*address).into(), block.hash)
            .await?
            .response;
        accounts.insert(*address, known_staking_info(&info)?);
    }
    Ok(Snapshot { block, accounts })
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
    let expected_stake = old.staked_amount() + if restake { reward } else { Amount::zero() };
    let mut expected = old.clone();
    match &mut expected {
        AccountStakingInfo::Baker { staked_amount, .. }
        | AccountStakingInfo::Delegated { staked_amount, .. } => {
            *staked_amount = expected_stake;
        }
    }
    ensure!(
        *new == expected,
        "Staking configuration or stake changed across activation"
    );
    Ok(())
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
        assert!(staking_preserved(None, Some(&old), Amount::zero()).is_err());
        assert!(staking_preserved(None, None, Amount::zero()).is_ok());
        let mut changed_target = old.clone();
        if let AccountStakingInfo::Delegated {
            delegation_target, ..
        } = &mut changed_target
        {
            *delegation_target = DelegationTarget::Baker {
                baker_id: concordium_rust_sdk::types::AccountIndex::from(0).into(),
            };
        }
        assert!(staking_preserved(Some(&old), Some(&changed_target), Amount::zero()).is_err());
    }
}
