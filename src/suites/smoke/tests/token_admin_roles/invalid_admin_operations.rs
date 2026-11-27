//! Reject invalid role updates and unauthorized or unsupported metadata updates.

use super::super::super::{Test, TestContext};
use super::support::{
    create_mint_burn_token, execute_expecting_rejection, execute_successful_batch, metadata,
};
use crate::fixture::genesis::GenesisAccount;
use crate::registry::TestGroup;
use anyhow::{Context, Result, ensure};
use concordium_rust_sdk::{
    base::{
        common::cbor::value::Value,
        protocol_level_tokens::{TokenAdminRole, TokenId, operations as token_operations},
    },
    id::types::AccountAddress,
    v2::{BlockIdentifier, Client},
};
use futures_util::FutureExt;

pub(super) fn register(group: &mut TestGroup<'_, Test>) {
    group.register(
        "Invalid role updates and unauthorized or unsupported metadata updates are rejected",
        Test {
            required_accounts: 11,
            run: Box::new(|context, accounts| {
                Box::pin(
                    reject_invalid_role_updates_and_unauthorized_or_unsupported_metadata_updates(
                        context, accounts,
                    ),
                )
            }),
        },
    );
}

async fn reject_invalid_role_updates_and_unauthorized_or_unsupported_metadata_updates(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let [
        governance_account,
        missing_assignment,
        missing_revoke,
        allow_assignment,
        allow_revoke,
        deny_assignment,
        deny_revoke,
        unauthorized_pause,
        unauthorized_unpause,
        unauthorized_metadata,
        unsupported_metadata,
    ] = accounts.as_slice()
    else {
        anyhow::bail!("Expected eleven accounts for invalid admin-operation checks");
    };

    let mut client = context.validator_node.connect().await?;
    let (id, _) =
        create_mint_burn_token(&mut client, context, governance_account, "E2E-RBAC-INVALID")
            .await?;
    let missing = AccountAddress([255; 32]);
    let error = client
        .get_account_info(&missing.into(), BlockIdentifier::LastFinal)
        .await
        .expect_err("Missing-account fixture unexpectedly exists");
    ensure!(
        error.is_not_found(),
        "Missing-account query failed without establishing account absence: {error}"
    );

    let roles = vec![
        TokenAdminRole::UpdateAdminRoles,
        TokenAdminRole::UpdateMetadata,
    ];
    execute_successful_batch(
        &mut client,
        governance_account,
        &id,
        vec![
            token_operations::assign_admin_roles(missing_assignment.address, roles.clone()),
            token_operations::assign_admin_roles(missing_revoke.address, roles.clone()),
            token_operations::assign_admin_roles(allow_assignment.address, roles.clone()),
            token_operations::assign_admin_roles(allow_revoke.address, roles.clone()),
            token_operations::assign_admin_roles(deny_assignment.address, roles.clone()),
            token_operations::assign_admin_roles(deny_revoke.address, roles.clone()),
            token_operations::assign_admin_roles(unsupported_metadata.address, roles),
        ],
    )
    .await?;

    // Each check owns its diagnostic and signer. Wait for every submitted check.
    let results = futures_util::future::join_all([
        reject_assignment_to_missing_account(client.clone(), missing_assignment, &id, missing)
            .boxed_local(),
        reject_revoke_from_missing_account(client.clone(), missing_revoke, &id, missing)
            .boxed_local(),
        reject_assignment_of_disabled_update_allow_list_role(
            client.clone(),
            allow_assignment,
            &id,
            governance_account.address,
        )
        .boxed_local(),
        reject_revocation_of_disabled_update_allow_list_role(
            client.clone(),
            allow_revoke,
            &id,
            governance_account.address,
        )
        .boxed_local(),
        reject_assignment_of_disabled_update_deny_list_role(
            client.clone(),
            deny_assignment,
            &id,
            governance_account.address,
        )
        .boxed_local(),
        reject_revocation_of_disabled_update_deny_list_role(
            client.clone(),
            deny_revoke,
            &id,
            governance_account.address,
        )
        .boxed_local(),
        reject_pause_without_pause_role(client.clone(), unauthorized_pause, &id).boxed_local(),
        reject_unpause_without_pause_role(client.clone(), unauthorized_unpause, &id).boxed_local(),
        reject_metadata_without_update_metadata_role(client.clone(), unauthorized_metadata, &id)
            .boxed_local(),
        reject_metadata_with_unsupported_fields(client.clone(), unsupported_metadata, &id)
            .boxed_local(),
    ])
    .await;

    let mut failures = Vec::new();
    for result in results {
        if let Err(error) = result {
            failures.push(format!("{error:#}"));
        }
    }
    ensure!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

async fn reject_assignment_to_missing_account(
    mut client: Client,
    sender: &GenesisAccount,
    id: &TokenId,
    missing: AccountAddress,
) -> Result<()> {
    execute_expecting_rejection(
        &mut client,
        sender,
        id,
        token_operations::assign_admin_roles(missing, vec![TokenAdminRole::Mint]),
    )
    .await
    .context("Assignment to a missing account must be rejected")?;
    Ok(())
}

async fn reject_revoke_from_missing_account(
    mut client: Client,
    sender: &GenesisAccount,
    id: &TokenId,
    missing: AccountAddress,
) -> Result<()> {
    execute_expecting_rejection(
        &mut client,
        sender,
        id,
        token_operations::revoke_admin_roles(missing, vec![TokenAdminRole::Mint]),
    )
    .await
    .context("Revocation from a missing account must be rejected")?;
    Ok(())
}

async fn reject_assignment_of_disabled_update_allow_list_role(
    mut client: Client,
    sender: &GenesisAccount,
    id: &TokenId,
    target: AccountAddress,
) -> Result<()> {
    execute_expecting_rejection(
        &mut client,
        sender,
        id,
        token_operations::assign_admin_roles(target, vec![TokenAdminRole::UpdateAllowList]),
    )
    .await
    .context("Assignment of a disabled update-allow-list role must be rejected")?;
    Ok(())
}

async fn reject_revocation_of_disabled_update_allow_list_role(
    mut client: Client,
    sender: &GenesisAccount,
    id: &TokenId,
    target: AccountAddress,
) -> Result<()> {
    execute_expecting_rejection(
        &mut client,
        sender,
        id,
        token_operations::revoke_admin_roles(target, vec![TokenAdminRole::UpdateAllowList]),
    )
    .await
    .context("Revocation of a disabled update-allow-list role must be rejected")?;
    Ok(())
}

async fn reject_assignment_of_disabled_update_deny_list_role(
    mut client: Client,
    sender: &GenesisAccount,
    id: &TokenId,
    target: AccountAddress,
) -> Result<()> {
    execute_expecting_rejection(
        &mut client,
        sender,
        id,
        token_operations::assign_admin_roles(target, vec![TokenAdminRole::UpdateDenyList]),
    )
    .await
    .context("Assignment of a disabled update-deny-list role must be rejected")?;
    Ok(())
}

async fn reject_revocation_of_disabled_update_deny_list_role(
    mut client: Client,
    sender: &GenesisAccount,
    id: &TokenId,
    target: AccountAddress,
) -> Result<()> {
    execute_expecting_rejection(
        &mut client,
        sender,
        id,
        token_operations::revoke_admin_roles(target, vec![TokenAdminRole::UpdateDenyList]),
    )
    .await
    .context("Revocation of a disabled update-deny-list role must be rejected")?;
    Ok(())
}

async fn reject_pause_without_pause_role(
    mut client: Client,
    sender: &GenesisAccount,
    id: &TokenId,
) -> Result<()> {
    execute_expecting_rejection(&mut client, sender, id, token_operations::pause())
        .await
        .context("Pause without the pause role must be rejected")?;
    Ok(())
}

async fn reject_unpause_without_pause_role(
    mut client: Client,
    sender: &GenesisAccount,
    id: &TokenId,
) -> Result<()> {
    execute_expecting_rejection(&mut client, sender, id, token_operations::unpause())
        .await
        .context("Unpause without the pause role must be rejected")?;
    Ok(())
}

async fn reject_metadata_without_update_metadata_role(
    mut client: Client,
    sender: &GenesisAccount,
    id: &TokenId,
) -> Result<()> {
    execute_expecting_rejection(
        &mut client,
        sender,
        id,
        token_operations::update_metadata(metadata("https://example.invalid/unauthorized")),
    )
    .await
    .context("Metadata update without the update-metadata role must be rejected")?;
    Ok(())
}

async fn reject_metadata_with_unsupported_fields(
    mut client: Client,
    sender: &GenesisAccount,
    id: &TokenId,
) -> Result<()> {
    let mut reference = metadata("https://example.invalid/extra");
    reference
        .additional
        .insert("name".into(), Value::Text("unsupported".into()));
    execute_expecting_rejection(
        &mut client,
        sender,
        id,
        token_operations::update_metadata(reference),
    )
    .await
    .context("Metadata update with unsupported fields must be rejected")?;
    Ok(())
}
