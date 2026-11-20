use super::super::Fixtures;
use anyhow::{Context, ensure};
use concordium_rust_sdk::{
    common::types::Amount,
    types::AccountStakingInfo,
    v2::{BlockIdentifier, Upward},
};
use std::time::Duration;

pub(super) fn tests() -> Vec<super::Test> {
    vec![super::Test {
        group: "Accounts",
        name: "Validator account exists",
        run: Box::new(|fixtures| Box::pin(validator_account_exists(fixtures))),
    }]
}

const TIMEOUT: Duration = Duration::from_secs(30);
/// Initial total validator balance, including its staked CCD.
const VALIDATOR_BALANCE: Amount = Amount::from_micro_ccd(3_500_000_000_000);

/// Check the validator's initial balance and active status.
/// Query genesis so transfers and rewards cannot change the observed state.
async fn validator_account_exists(fixtures: &Fixtures) -> anyhow::Result<()> {
    let address = fixtures.genesis.accounts[0].address;

    let mut client = fixtures.validator_node.connect().await?;
    let info = tokio::time::timeout(
        TIMEOUT,
        client.get_account_info(&address.into(), BlockIdentifier::AbsoluteHeight(0.into())),
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
