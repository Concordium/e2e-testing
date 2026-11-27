//! Operational role holders remain subject to token restrictions.

use super::super::super::{Test, TestContext};
use super::support::create_mint_burn_token;
use super::support::metadata;
use super::support::{execute_expecting_rejection, execute_expecting_success};
use crate::fixture::genesis::GenesisAccount;
use crate::registry::TestGroup;
use anyhow::ensure;
use anyhow::{Context, Result};
use concordium_rust_sdk::base::protocol_level_tokens::TokenAdminRole;
use concordium_rust_sdk::base::protocol_level_tokens::TokenAmount;
use concordium_rust_sdk::base::protocol_level_tokens::operations as token_operations;

pub(super) fn register(group: &mut TestGroup<'_, Test>) {
    group.register(
        "Delegated mint authority does not permit circulating supply overflow",
        Test {
            required_accounts: 2,
            run: Box::new(|context, accounts| {
                Box::pin(delegated_mint_rejects_supply_overflow(context, accounts))
            }),
        },
    );
    group.register(
        "Delegated mint and burn roles do not permit amounts with mismatched decimals",
        Test {
            required_accounts: 2,
            run: Box::new(|context, accounts| {
                Box::pin(delegated_mint_and_burn_reject_mismatched_decimals(
                    context, accounts,
                ))
            }),
        },
    );
    group.register(
        "The mint role credits the sender but does not authorize burning",
        Test {
            required_accounts: 2,
            run: Box::new(|context, accounts| {
                Box::pin(mint_role_does_not_authorize_burning(context, accounts))
            }),
        },
    );
    group.register(
        "The burn role reduces the sender balance but does not authorize minting",
        Test {
            required_accounts: 2,
            run: Box::new(|context, accounts| {
                Box::pin(burn_role_does_not_authorize_minting(context, accounts))
            }),
        },
    );
    group.register(
        "The pause role permits pausing and unpausing but not metadata updates",
        Test {
            required_accounts: 2,
            run: Box::new(|context, accounts| {
                Box::pin(pause_role_does_not_authorize_metadata_updates(
                    context, accounts,
                ))
            }),
        },
    );
    group.register(
        "The update-metadata role replaces the reference but does not authorize pausing",
        Test {
            required_accounts: 2,
            run: Box::new(|context, accounts| {
                Box::pin(update_metadata_role_does_not_authorize_pausing(
                    context, accounts,
                ))
            }),
        },
    );
    group.register(
        "Operational role holders remain subject to token restrictions",
        Test {
            required_accounts: 3,
            run: Box::new(|context, accounts| {
                Box::pin(
                    operational_role_holders_remain_subject_to_token_restrictions(
                        context, accounts,
                    ),
                )
            }),
        },
    );
}

async fn delegated_mint_rejects_supply_overflow(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let [governance_account, mint_role_holder] = accounts.as_slice() else {
        anyhow::bail!("Expected two accounts for the supply-overflow check");
    };
    let mut client = context.validator_node.connect().await?;
    let (id, _) = create_mint_burn_token(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-OVERFLOW",
    )
    .await?;
    execute_expecting_success(
        &mut client,
        governance_account,
        &id,
        token_operations::assign_admin_roles(mint_role_holder.address, vec![TokenAdminRole::Mint]),
    )
    .await?;
    execute_expecting_success(
        &mut client,
        mint_role_holder,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(1, 0)),
    )
    .await?;
    let funded = execute_expecting_success(
        &mut client,
        mint_role_holder,
        &id,
        token_operations::transfer_tokens(
            governance_account.address,
            TokenAmount::from_raw(1, 0),
        ),
    )
    .await?;
    ensure!(
        client
            .get_token_info(id.clone(), funded)
            .await?
            .response
            .token_state
            .total_supply
            == TokenAmount::from_raw(1, 0),
        "Supply-overflow fixture must have one circulating token"
    );
    let balance = client
        .get_account_info(&mint_role_holder.address.into(), funded)
        .await?
        .response
        .token_amount(&id);
    ensure!(
        balance.is_none() || balance == Some(TokenAmount::from_raw(0, 0)),
        "Supply-overflow fixture must leave the mint-role holder with no tokens"
    );
    execute_expecting_rejection(
        &mut client,
        mint_role_holder,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(u64::MAX, 0)),
    )
    .await
    .context("Delegated mint exceeding circulating supply capacity must be rejected")?;
    Ok(())
}

async fn delegated_mint_and_burn_reject_mismatched_decimals(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let [governance_account, role_holder] = accounts.as_slice() else {
        anyhow::bail!("Expected two accounts for mismatched-decimal checks");
    };
    let mut client = context.validator_node.connect().await?;
    let (id, _) = create_mint_burn_token(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-DECIMALS",
    )
    .await?;
    execute_expecting_success(
        &mut client,
        governance_account,
        &id,
        token_operations::assign_admin_roles(
            role_holder.address,
            vec![TokenAdminRole::Mint, TokenAdminRole::Burn],
        ),
    )
    .await?;
    execute_expecting_success(
        &mut client,
        role_holder,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(5, 0)),
    )
    .await?;
    execute_expecting_rejection(
        &mut client,
        role_holder,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(1, 1)),
    )
    .await
    .context("Delegated mint with mismatched decimals must be rejected")?;
    execute_expecting_rejection(
        &mut client,
        role_holder,
        &id,
        token_operations::burn_tokens(TokenAmount::from_raw(1, 1)),
    )
    .await
    .context("Delegated burn with mismatched decimals must be rejected")?;
    Ok(())
}

async fn mint_role_does_not_authorize_burning(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let [governance_account, mint_role_holder] = accounts.as_slice() else {
        anyhow::bail!("Expected two accounts for the mint-role boundary");
    };
    let mut client = context.validator_node.connect().await?;
    let (id, _) = create_mint_burn_token(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-MINT-ONLY",
    )
    .await?;
    execute_expecting_success(
        &mut client,
        governance_account,
        &id,
        token_operations::assign_admin_roles(mint_role_holder.address, vec![TokenAdminRole::Mint]),
    )
    .await?;
    let minted = execute_expecting_success(
        &mut client,
        mint_role_holder,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(5, 0)),
    )
    .await?;
    ensure!(
        client
            .get_account_info(&mint_role_holder.address.into(), minted)
            .await?
            .response
            .token_amount(&id)
            == Some(TokenAmount::from_raw(5, 0)),
        "Delegated mint did not credit the sender"
    );
    ensure!(
        client
            .get_token_info(id.clone(), minted)
            .await?
            .response
            .token_state
            .total_supply
            == TokenAmount::from_raw(5, 0),
        "Delegated mint did not increase supply"
    );
    execute_expecting_rejection(
        &mut client,
        mint_role_holder,
        &id,
        token_operations::burn_tokens(TokenAmount::from_raw(1, 0)),
    )
    .await?;
    Ok(())
}

async fn burn_role_does_not_authorize_minting(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let [governance_account, burn_role_holder] = accounts.as_slice() else {
        anyhow::bail!("Expected two accounts for the burn-role boundary");
    };
    let mut client = context.validator_node.connect().await?;
    let (id, _) = create_mint_burn_token(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-BURN-ONLY",
    )
    .await?;
    execute_expecting_success(
        &mut client,
        governance_account,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(5, 0)),
    )
    .await?;
    execute_expecting_success(
        &mut client,
        governance_account,
        &id,
        token_operations::transfer_tokens(burn_role_holder.address, TokenAmount::from_raw(5, 0)),
    )
    .await?;
    execute_expecting_success(
        &mut client,
        governance_account,
        &id,
        token_operations::assign_admin_roles(burn_role_holder.address, vec![TokenAdminRole::Burn]),
    )
    .await?;
    let burned = execute_expecting_success(
        &mut client,
        burn_role_holder,
        &id,
        token_operations::burn_tokens(TokenAmount::from_raw(2, 0)),
    )
    .await?;
    ensure!(
        client
            .get_account_info(&burn_role_holder.address.into(), burned)
            .await?
            .response
            .token_amount(&id)
            == Some(TokenAmount::from_raw(3, 0)),
        "Delegated burn did not debit the sender"
    );
    ensure!(
        client
            .get_token_info(id.clone(), burned)
            .await?
            .response
            .token_state
            .total_supply
            == TokenAmount::from_raw(3, 0),
        "Delegated burn did not decrease supply"
    );
    execute_expecting_rejection(
        &mut client,
        burn_role_holder,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(1, 0)),
    )
    .await?;
    Ok(())
}

async fn pause_role_does_not_authorize_metadata_updates(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let [governance_account, pause_role_holder] = accounts.as_slice() else {
        anyhow::bail!("Expected two accounts for the pause-role boundary");
    };
    let mut client = context.validator_node.connect().await?;
    let (id, _) = create_mint_burn_token(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-PAUSE-ONLY",
    )
    .await?;
    execute_expecting_success(
        &mut client,
        governance_account,
        &id,
        token_operations::assign_admin_roles(
            pause_role_holder.address,
            vec![TokenAdminRole::Pause],
        ),
    )
    .await?;
    let paused = execute_expecting_success(
        &mut client,
        pause_role_holder,
        &id,
        token_operations::pause(),
    )
    .await?;
    ensure!(
        client
            .get_token_info(id.clone(), paused)
            .await?
            .response
            .token_state
            .decode_module_state()?
            .paused
            == Some(true),
        "Delegated pause did not pause the token"
    );
    let unpaused = execute_expecting_success(
        &mut client,
        pause_role_holder,
        &id,
        token_operations::unpause(),
    )
    .await?;
    ensure!(
        client
            .get_token_info(id.clone(), unpaused)
            .await?
            .response
            .token_state
            .decode_module_state()?
            .paused
            == Some(false),
        "Delegated unpause did not unpause the token"
    );
    execute_expecting_rejection(
        &mut client,
        pause_role_holder,
        &id,
        token_operations::update_metadata(metadata("https://example.invalid/pause-only")),
    )
    .await?;
    Ok(())
}

async fn update_metadata_role_does_not_authorize_pausing(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let [governance_account, update_metadata_role_holder] = accounts.as_slice() else {
        anyhow::bail!("Expected two accounts for the update-metadata-role boundary");
    };
    let mut client = context.validator_node.connect().await?;
    let (id, _) = create_mint_burn_token(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-METADATA-ONLY",
    )
    .await?;
    execute_expecting_success(
        &mut client,
        governance_account,
        &id,
        token_operations::assign_admin_roles(
            update_metadata_role_holder.address,
            vec![TokenAdminRole::UpdateMetadata],
        ),
    )
    .await?;
    let replacement = metadata("https://example.invalid/metadata-only");
    let updated = execute_expecting_success(
        &mut client,
        update_metadata_role_holder,
        &id,
        token_operations::update_metadata(replacement.clone()),
    )
    .await?;
    ensure!(
        client
            .get_token_info(id.clone(), updated)
            .await?
            .response
            .token_state
            .decode_module_state()?
            .metadata
            == Some(replacement),
        "Delegated metadata update did not replace the reference"
    );
    execute_expecting_rejection(
        &mut client,
        update_metadata_role_holder,
        &id,
        token_operations::pause(),
    )
    .await?;
    Ok(())
}

async fn operational_role_holders_remain_subject_to_token_restrictions(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let governance_account = &accounts[0];
    let role_holder = &accounts[1];
    let token_holder = &accounts[2];
    let mut client = context.validator_node.connect().await?;
    let (id, _) = create_mint_burn_token(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-DELEGATED",
    )
    .await?;
    execute_expecting_success(
        &mut client,
        governance_account,
        &id,
        token_operations::assign_admin_roles(
            role_holder.address,
            vec![
                TokenAdminRole::Mint,
                TokenAdminRole::Burn,
                TokenAdminRole::Pause,
                TokenAdminRole::UpdateMetadata,
            ],
        ),
    )
    .await?;
    execute_expecting_success(
        &mut client,
        role_holder,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(20, 0)),
    )
    .await?;
    execute_expecting_success(
        &mut client,
        role_holder,
        &id,
        token_operations::transfer_tokens(token_holder.address, TokenAmount::from_raw(5, 0)),
    )
    .await?;
    let burned = execute_expecting_success(
        &mut client,
        role_holder,
        &id,
        token_operations::burn_tokens(TokenAmount::from_raw(3, 0)),
    )
    .await?;
    let token = client.get_token_info(id.clone(), burned).await?.response;
    ensure!(
        token.token_state.total_supply == TokenAmount::from_raw(17, 0),
        "Incorrect circulating supply after mint/burn"
    );
    for (account, balance) in [(role_holder, 12), (token_holder, 5)] {
        let info = client
            .get_account_info(&account.address.into(), burned)
            .await?
            .response;
        ensure!(
            info.token_amount(&id) == Some(TokenAmount::from_raw(balance, 0)),
            "Mint/burn changed wrong account balance"
        );
    }
    let mut other_client = client.clone();
    let (insufficient_balance, unauthorized_burn) = tokio::join!(
        execute_expecting_rejection(
            &mut client,
            role_holder,
            &id,
            token_operations::burn_tokens(TokenAmount::from_raw(13, 0)),
        ),
        execute_expecting_rejection(
            &mut other_client,
            token_holder,
            &id,
            token_operations::burn_tokens(TokenAmount::from_raw(1, 0)),
        ),
    );
    insufficient_balance.context("Burn exceeding the sender balance must be rejected")?;
    unauthorized_burn.context("Burn without the burn role must be rejected")?;
    execute_expecting_success(&mut client, role_holder, &id, token_operations::pause()).await?;
    let (paused_mint, paused_transfer) = tokio::join!(
        execute_expecting_rejection(
            &mut client,
            role_holder,
            &id,
            token_operations::mint_tokens(TokenAmount::from_raw(1, 0)),
        ),
        execute_expecting_rejection(
            &mut other_client,
            token_holder,
            &id,
            token_operations::transfer_tokens(role_holder.address, TokenAmount::from_raw(1, 0)),
        ),
    );
    paused_mint.context("Mint while paused must be rejected")?;
    paused_transfer.context("Transfer while paused must be rejected")?;

    execute_expecting_rejection(
        &mut client,
        role_holder,
        &id,
        token_operations::burn_tokens(TokenAmount::from_raw(1, 0)),
    )
    .await?;
    execute_expecting_success(
        &mut client,
        governance_account,
        &id,
        token_operations::assign_admin_roles(
            token_holder.address,
            vec![TokenAdminRole::UpdateMetadata],
        ),
    )
    .await?;
    let replacement = metadata("https://example.invalid/replacement");
    let updated = execute_expecting_success(
        &mut client,
        token_holder,
        &id,
        token_operations::update_metadata(replacement.clone()),
    )
    .await?;
    ensure!(
        client
            .get_token_info(id.clone(), updated)
            .await?
            .response
            .token_state
            .decode_module_state()?
            .metadata
            == Some(replacement),
        "Paused metadata replacement not stored"
    );
    let (unpaused, revoked) = tokio::join!(
        execute_expecting_success(&mut client, role_holder, &id, token_operations::unpause(),),
        execute_expecting_success(
            &mut other_client,
            governance_account,
            &id,
            token_operations::revoke_admin_roles(role_holder.address, vec![TokenAdminRole::Mint]),
        ),
    );
    unpaused.context("Delegated unpause failed")?;
    revoked.context("Mint-role revocation failed")?;
    execute_expecting_rejection(
        &mut client,
        role_holder,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(1, 0)),
    )
    .await?;
    Ok(())
}
