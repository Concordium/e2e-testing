//! CCD transfers and reward-aware balance/nonce preservation.
use super::super::lifecycle::{ProtocolUpgradeTest, TestContext, TestProgress, UpgradeRegistry};
use super::super::{
    REJOIN_TIMEOUT, operations,
    state::{FinalizedBlock, rewards_between},
    wait,
};
use crate::fixture::genesis::GenesisAccount;
use anyhow::{Context as _, Result, ensure};
use concordium_rust_sdk::{common::types::Amount, id::types::AccountAddress, types::Nonce};
use std::collections::BTreeMap;

pub(super) fn register(registry: &mut UpgradeRegistry) {
    registry
        .group("CCD transfers")
        .register(
            "CCD transfers finalize before the update",
            Box::new(PreUpdateTransfer::default()),
        )
        .register(
            "CCD balances and nonces survive activation, accounting for rewards",
            Box::new(AccountPreservation::default()),
        )
        .register(
            "A fresh CCD transfer finalizes at P11",
            Box::new(P11Transfer::default()),
        );
}

struct TransferAccounts {
    sender: GenesisAccount,
    recipient: GenesisAccount,
}
impl TransferAccounts {
    fn new(accounts: Vec<GenesisAccount>) -> Result<Self> {
        let [sender, recipient]: [GenesisAccount; 2] =
            accounts.try_into().ok().context("Expected 2 accounts")?;
        Ok(Self { sender, recipient })
    }
}

#[derive(Default)]
struct PreUpdateTransfer(Option<TransferAccounts>);
#[async_trait::async_trait(?Send)]
impl ProtocolUpgradeTest for PreUpdateTransfer {
    fn required_accounts(&self) -> usize {
        2
    }
    async fn prepare_before_update(
        &mut self,
        _context: &TestContext<'_>,
        accounts: Vec<GenesisAccount>,
    ) -> Result<()> {
        self.0 = Some(TransferAccounts::new(accounts)?);
        Ok(())
    }
    async fn run_before_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let state = self.0.as_ref().context("Invalid setup")?;
        operations::ccd(
            &mut context.validators[0].connect().await?,
            &state.sender,
            state.recipient.address,
        )
        .await?;
        Ok(TestProgress::Passed)
    }
}

#[derive(Default)]
struct P11Transfer(Option<TransferAccounts>);
#[async_trait::async_trait(?Send)]
impl ProtocolUpgradeTest for P11Transfer {
    fn required_accounts(&self) -> usize {
        2
    }
    async fn prepare_before_update(
        &mut self,
        _context: &TestContext<'_>,
        accounts: Vec<GenesisAccount>,
    ) -> Result<()> {
        self.0 = Some(TransferAccounts::new(accounts)?);
        Ok(())
    }
    async fn run_after_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let state = self.0.as_ref().context("Invalid setup")?;
        let block = operations::ccd(
            &mut context.validators[0].connect().await?,
            &state.sender,
            state.recipient.address,
        )
        .await?;
        operations::require_p11(&mut context.validators[0].connect().await?, block).await?;
        Ok(TestProgress::Passed)
    }
}

#[derive(Default)]
struct AccountPreservation {
    before: Option<Snapshot>,
}
#[async_trait::async_trait(?Send)]
impl ProtocolUpgradeTest for AccountPreservation {
    async fn finish_before_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let before =
            wait::bounded("capture CCD baseline", REJOIN_TIMEOUT, capture(context)).await?;
        self.before = Some(before);
        Ok(TestProgress::Pending)
    }
    async fn prepare_after_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        wait::bounded("verify CCD preservation", REJOIN_TIMEOUT, async {
            let before = self.before.as_ref().context("No CCD baseline")?;
            let after = capture(context).await?;
            ensure!(
                after.block.height >= before.block.height,
                "Finalized height regressed"
            );
            let rewards = rewards_between(
                &mut context.validators[0].connect().await?,
                &before.block,
                &after.block,
            )
            .await?;
            compare_balances(&before.accounts, &after.accounts, &rewards.balances)?;
            for (address, expected) in &before.accounts {
                ensure!(
                    after.accounts[address].nonce == expected.nonce,
                    "Nonce changed across activation for {address}"
                );
            }
            Ok(())
        })
        .await?;
        Ok(TestProgress::Passed)
    }
}

fn compare_balances(
    before: &BTreeMap<AccountAddress, CcdAccount>,
    after: &BTreeMap<AccountAddress, CcdAccount>,
    rewards: &BTreeMap<AccountAddress, Amount>,
) -> Result<()> {
    ensure!(
        before.keys().eq(after.keys()),
        "Tracked balance accounts changed"
    );
    for (address, account) in before {
        let reward = rewards.get(address).copied().unwrap_or(Amount::zero());
        ensure!(
            after[address].amount == account.amount + reward,
            "Unexpected CCD balance change for {address} (accounted reward: {reward})"
        );
    }
    Ok(())
}

#[derive(PartialEq, Debug)]
struct CcdAccount {
    amount: Amount,
    nonce: Nonce,
}
struct Snapshot {
    block: FinalizedBlock,
    accounts: BTreeMap<AccountAddress, CcdAccount>,
}
async fn capture(context: &TestContext<'_>) -> Result<Snapshot> {
    let mut client = context.validators[0].connect().await?;
    let block = FinalizedBlock::capture(&mut client).await?;
    let mut accounts = BTreeMap::new();
    for address in context.accounts {
        let info = client
            .get_account_info(&(*address).into(), block.hash)
            .await?
            .response;
        accounts.insert(
            *address,
            CcdAccount {
                amount: info.account_amount,
                nonce: info.account_nonce,
            },
        );
    }
    Ok(Snapshot { block, accounts })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rewards_are_allowed_but_fees_or_unexplained_deltas_are_not() -> Result<()> {
        let address = AccountAddress([1; 32]);
        let account = |amount| CcdAccount {
            amount: Amount::from_micro_ccd(amount),
            nonce: Nonce { nonce: 1 },
        };
        let before = BTreeMap::from([(address, account(100))]);
        let after = BTreeMap::from([(address, account(110))]);
        let rewards = BTreeMap::from([(address, Amount::from_micro_ccd(10))]);
        compare_balances(&before, &after, &rewards)?;
        assert!(compare_balances(&before, &after, &BTreeMap::new()).is_err());
        assert!(compare_balances(&before, &BTreeMap::new(), &rewards).is_err());
        assert!(
            compare_balances(
                &before,
                &BTreeMap::from([(address, account(109))]),
                &rewards
            )
            .is_err()
        );
        Ok(())
    }
}
