use crate::runner::Fixtures;
use anyhow::{ensure, Context};
use concordium_rust_sdk::{
    common::types::Amount,
    types::AccountStakingInfo,
    v2::{BlockIdentifier, Upward},
};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);
const VALIDATOR_BALANCE: Amount = Amount::from_micro_ccd(3_500_000_000_000);

/// Verify the validator's genesis state before any transaction is submitted.
///
/// This test assumes that it runs before the transfer test and before the first
/// payday. Only empty blocks can exist at this point. Therefore, transaction
/// fees and payday rewards cannot have changed the validator's genesis balance.
pub async fn validator_account_exists(fixtures: &Fixtures) -> anyhow::Result<()> {
    let address = fixtures.genesis.accounts[0].lock().await.address;

    let mut client = fixtures.validator_node.connect().await?;
    let info = tokio::time::timeout(
        TIMEOUT,
        client.get_account_info(&address.into(), BlockIdentifier::Best),
    )
    .await
    .context("accounts::validator_account_exists timed out after 30 seconds")??
    .response;

    ensure!(
        info.account_amount == VALIDATOR_BALANCE,
        "validator balance mismatch: expected {VALIDATOR_BALANCE}, got {}",
        info.account_amount
    );
    ensure!(
        matches!(
            info.account_stake,
            Some(Upward::Known(AccountStakingInfo::Baker {
                is_suspended: false,
                ..
            }))
        ),
        "genesis validator is not an active validator"
    );
    Ok(())
}
