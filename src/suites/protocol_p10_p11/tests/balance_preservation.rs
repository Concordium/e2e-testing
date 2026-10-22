//! One independent CCD balance-preservation criterion with reward-aware evidence.
use super::super::{
    REJOIN_TIMEOUT,
    context::TestContext,
    operations,
    state::{FinalizedBlock, account_at, rewards_between},
    wait::bounded,
};
use super::upgrade_test::TestDefinition;
use super::{
    accounts_fixture::{Account, TestAccounts},
    upgrade_test::{TestProgress, UpgradeTest, Verification},
};
use anyhow::{Context as _, Result, ensure};
use concordium_rust_sdk::{common::types::Amount, id::types::AccountAddress};
use std::collections::BTreeMap;

struct Balances {
    block: FinalizedBlock,
    amounts: BTreeMap<AccountAddress, Amount>,
}

pub(super) struct BalancePreservation {
    sender: Account,
    recipient: Account,
    before: Option<Balances>,
}
impl TestDefinition for BalancePreservation {
    const NAME: &'static str = "CCD balances survive activation after accounting for rewards";
    const ACCOUNTS: &'static [&'static str] = &["sender", "recipient"];

    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self {
            sender: accounts.take("sender")?,
            recipient: accounts.take("recipient")?,
            before: None,
        })
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for BalancePreservation {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
        let recipient = self.recipient.lock().await.address;
        operations::ccd(&mut context.client(1).await?, &self.sender, recipient).await?;
        Ok(TestProgress::Pending)
    }

    async fn before_activation(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
        self.before = Some(
            bounded(
                "capture pre-update balances",
                REJOIN_TIMEOUT,
                capture(context),
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
            .verify(bounded(
                "verify balance preservation",
                REJOIN_TIMEOUT,
                async {
                    let before = self
                        .before
                        .as_ref()
                        .context("No pre-update balance evidence")?;
                    let after = capture(context).await?;
                    let rewards =
                        rewards_between(&mut context.client(0).await?, &before.block, &after.block)
                            .await?;
                    compare_balances(&before.amounts, &after.amounts, &rewards.balances)
                },
            ))
            .await
    }
}

async fn capture(context: &TestContext<'_>) -> Result<Balances> {
    let mut client = context.client(0).await?;
    let block = FinalizedBlock::capture(&mut client).await?;
    let mut amounts = BTreeMap::new();
    for address in context.accounts() {
        amounts.insert(
            *address,
            account_at(&mut client, *address, block.hash)
                .await?
                .account_amount,
        );
    }
    Ok(Balances { block, amounts })
}
fn compare_balances(
    before: &BTreeMap<AccountAddress, Amount>,
    after: &BTreeMap<AccountAddress, Amount>,
    rewards: &BTreeMap<AccountAddress, Amount>,
) -> Result<()> {
    ensure!(
        before.keys().eq(after.keys()),
        "Tracked balance accounts changed"
    );
    for (address, amount) in before {
        let reward = rewards.get(address).copied().unwrap_or(Amount::zero());
        ensure!(
            after[address] == *amount + reward,
            "Unexpected CCD balance change for {address} (accounted reward: {reward})"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rewards_are_allowed_but_fees_or_unexplained_deltas_are_not() -> Result<()> {
        let address = AccountAddress([1; 32]);
        let amount = Amount::from_micro_ccd;
        let before = BTreeMap::from([(address, amount(100))]);
        let after = BTreeMap::from([(address, amount(110))]);
        let rewards = BTreeMap::from([(address, amount(10))]);
        compare_balances(&before, &after, &rewards)?;
        assert!(compare_balances(&before, &after, &BTreeMap::new()).is_err());
        assert!(compare_balances(&before, &BTreeMap::new(), &rewards).is_err());
        assert!(
            compare_balances(&before, &BTreeMap::from([(address, amount(109))]), &rewards).is_err()
        );
        Ok(())
    }
}
