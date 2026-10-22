//! Shared finalized-block references and reward evidence. No feature snapshots or assertions.
use anyhow::{Context, Result, ensure};
use concordium_rust_sdk::{
    common::types::Amount,
    id::types::AccountAddress,
    types::{
        AbsoluteBlockHeight, AccountInfo, SpecialTransactionOutcome,
        hashes::{BlockHash, StateHash},
    },
    v2::Client,
};
use futures_util::TryStreamExt;
use std::collections::BTreeMap;

/// Immutable reference for queries that must use the same finalized block.
pub(super) struct FinalizedBlock {
    pub hash: BlockHash,
    pub height: AbsoluteBlockHeight,
    pub state_hash: StateHash,
}
impl FinalizedBlock {
    pub async fn capture(client: &mut Client) -> Result<Self> {
        let hash = client.get_consensus_info().await?.last_finalized_block;
        let info = client.get_block_info(hash).await?.response;
        ensure!(info.finalized, "Reference block is not finalized");
        tracing::info!(block = %hash, height = info.block_height.height, state_hash = %info.block_state_hash, "captured finalized block reference");
        Ok(Self {
            hash,
            height: info.block_height,
            state_hash: info.block_state_hash,
        })
    }
}

/// Query one account at a fixed block and validate the returned address.
pub(super) async fn account_at(
    client: &mut Client,
    address: AccountAddress,
    block: BlockHash,
) -> Result<AccountInfo> {
    let info = client
        .get_account_info(&address.into(), block)
        .await?
        .response;
    ensure!(
        info.account_address == address,
        "RPC returned a different account for {address}"
    );
    Ok(info)
}

#[derive(Default)]
pub(super) struct RewardCredits {
    /// Includes foundation payments as well as validator and delegation rewards.
    pub balances: BTreeMap<AccountAddress, Amount>,
    /// Excludes foundation payments, which cannot increase restaked stake.
    pub restakable: BTreeMap<AccountAddress, Amount>,
}
impl RewardCredits {
    fn add(&mut self, event: &SpecialTransactionOutcome) {
        let (address, amount, restakable) = match event {
            SpecialTransactionOutcome::PaydayAccountReward {
                account,
                transaction_fees,
                baker_reward,
                finalization_reward,
            } => (
                *account,
                *transaction_fees + *baker_reward + *finalization_reward,
                true,
            ),
            SpecialTransactionOutcome::PaydayFoundationReward {
                foundation_account,
                development_charge,
            } => (*foundation_account, *development_charge, false),
            SpecialTransactionOutcome::Mint {
                foundation_account,
                mint_platform_development_charge,
                ..
            } => (
                *foundation_account,
                *mint_platform_development_charge,
                false,
            ),
            _ => return, // Accrued rewards are not yet account credits.
        };
        *self.balances.entry(address).or_insert(Amount::zero()) += amount;
        if restakable {
            *self.restakable.entry(address).or_insert(Amount::zero()) += amount;
        }
    }
    fn merge(&mut self, other: Self) {
        for (address, amount) in other.balances {
            *self.balances.entry(address).or_insert(Amount::zero()) += amount;
        }
        for (address, amount) in other.restakable {
            *self.restakable.entry(address).or_insert(Amount::zero()) += amount;
        }
    }
}
async fn reward_credits_in_block(client: &mut Client, block: BlockHash) -> Result<RewardCredits> {
    let mut events = client.get_block_special_events(block).await?.response;
    let mut rewards = RewardCredits::default();
    while let Some(event) = events.try_next().await? {
        rewards.add(
            event
                .as_known()
                .context("Unknown special event; cannot account for rewards")?,
        );
    }
    Ok(rewards)
}
pub async fn rewards_in_block(
    client: &mut Client,
    block: BlockHash,
) -> Result<BTreeMap<AccountAddress, Amount>> {
    Ok(reward_credits_in_block(client, block).await?.balances)
}
pub(super) async fn rewards_between(
    client: &mut Client,
    before: &FinalizedBlock,
    after: &FinalizedBlock,
) -> Result<RewardCredits> {
    ensure!(after.height >= before.height, "Finalized height regressed");
    let mut rewards = RewardCredits::default();
    for height in before.height.height + 1..=after.height.height {
        let blocks = client
            .get_blocks_at_height(&AbsoluteBlockHeight::from(height).into())
            .await?;
        ensure!(
            blocks.len() == 1,
            "Expected one finalized block at height {height}"
        );
        let block = blocks[0];
        ensure!(
            client.get_block_info(block).await?.response.finalized,
            "Reward evidence block is not finalized"
        );
        rewards.merge(reward_credits_in_block(client, block).await?);
    }
    Ok(rewards)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn foundation_credits_are_not_restaked_as_validator_rewards() {
        let address = AccountAddress([1; 32]);
        let amount = Amount::from_micro_ccd;
        let mut rewards = RewardCredits::default();
        rewards.add(&SpecialTransactionOutcome::Mint {
            foundation_account: address,
            mint_baking_reward: Amount::zero(),
            mint_finalization_reward: Amount::zero(),
            mint_platform_development_charge: amount(20),
        });
        rewards.add(&SpecialTransactionOutcome::PaydayFoundationReward {
            foundation_account: address,
            development_charge: amount(5),
        });
        rewards.add(&SpecialTransactionOutcome::PaydayAccountReward {
            account: address,
            transaction_fees: amount(1),
            baker_reward: amount(2),
            finalization_reward: amount(3),
        });
        assert_eq!(rewards.balances[&address], amount(31));
        assert_eq!(rewards.restakable[&address], amount(6));
    }
}
