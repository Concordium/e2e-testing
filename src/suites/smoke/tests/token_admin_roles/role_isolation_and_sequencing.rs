//! Token-local admin roles and immediate use of newly assigned roles.

use super::super::super::{Test, TestContext};
use super::support::create_mint_burn_token;
use super::support::metadata;
use super::support::{execute_expecting_rejection, execute_expecting_success};
use crate::fixture::genesis::GenesisAccount;
use crate::operations;
use crate::registry::TestGroup;
use anyhow::ensure;
use anyhow::{Context, Result};
use concordium_rust_sdk::base::protocol_level_tokens::TokenAdminRole;
use concordium_rust_sdk::base::protocol_level_tokens::TokenAmount;
use concordium_rust_sdk::base::protocol_level_tokens::TokenId;
use concordium_rust_sdk::base::protocol_level_tokens::operations as token_operations;

pub(super) fn register(group: &mut TestGroup<'_, Test>) {
    group.register(
        "Admin roles are token-local and assignments take effect within the transaction",
        Test {
            required_accounts: 2,
            run: Box::new(|context, accounts| {
                Box::pin(
                    roles_are_token_local_and_assignments_take_effect_within_the_transaction(
                        context, accounts,
                    ),
                )
            }),
        },
    );
}

async fn roles_are_token_local_and_assignments_take_effect_within_the_transaction(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let governance_account = &accounts[0];
    let role_holder = &accounts[1];
    let mut client = context.validator_node.connect().await?;
    let (id, _) = create_mint_burn_token(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-SEQUENCE",
    )
    .await?;
    let (other, _) = create_mint_burn_token(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-ISOLATED",
    )
    .await?;
    execute_expecting_success(
        &mut client,
        governance_account,
        &id,
        token_operations::assign_admin_roles(
            role_holder.address,
            vec![TokenAdminRole::UpdateAdminRoles],
        ),
    )
    .await?;
    execute_expecting_rejection(
        &mut client,
        role_holder,
        &other,
        token_operations::assign_admin_roles(role_holder.address, vec![TokenAdminRole::Mint]),
    )
    .await?;
    let (block, summary) = operations::update_token(
        &mut client,
        role_holder,
        &id,
        [
            token_operations::assign_admin_roles(role_holder.address, vec![TokenAdminRole::Mint]),
            token_operations::mint_tokens(TokenAmount::from_raw(7, 0)),
        ]
        .into_iter()
        .collect(),
        "assign-then-mint",
    )
    .await?;
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "Newly assigned mint role was not immediately usable: {summary:?}"
    );
    ensure!(
        client
            .get_account_info(&role_holder.address.into(), block)
            .await?
            .response
            .token_amount(&id)
            == Some(TokenAmount::from_raw(7, 0)),
        "Sequenced mint did not credit sender"
    );
    execute_expecting_rejection(
        &mut client,
        role_holder,
        &other,
        token_operations::mint_tokens(TokenAmount::from_raw(1, 0)),
    )
    .await?;
    let before = block;
    let first = metadata("https://example.invalid/first");
    let last = metadata("https://example.invalid/last");
    let (block, summary) = operations::update_token(
        &mut client,
        governance_account,
        &id,
        [
            token_operations::update_metadata(first),
            token_operations::update_metadata(last.clone()),
        ]
        .into_iter()
        .collect(),
        "replace-metadata-twice",
    )
    .await?;
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "Metadata sequence failed"
    );
    ensure!(
        client
            .get_token_info(id.clone(), block)
            .await?
            .response
            .token_state
            .decode_module_state()?
            .metadata
            == Some(last),
        "Final metadata reference was not retained"
    );
    ensure!(
        client
            .get_token_info(id.clone(), before)
            .await?
            .response
            .token_state
            .decode_module_state()?
            .metadata
            == Some(metadata("https://example.invalid/original")),
        "Historical metadata changed"
    );
    let missing = TokenId::try_from("E2E-RBAC-NOT-CREATED".to_string())?;
    let error = client
        .get_token_authorizations(block, missing)
        .await
        .expect_err("Missing-token authorization query succeeded");
    ensure!(
        error.is_not_found(),
        "Missing-token authorization query failed without establishing token absence: {error}"
    );
    let block = execute_expecting_success(
        &mut client,
        governance_account,
        &id,
        token_operations::revoke_admin_roles(
            governance_account.address,
            vec![TokenAdminRole::Pause],
        ),
    )
    .await?;
    let authorizations = client.get_token_authorizations(block, id).await?;
    ensure!(
        authorizations
            .pause
            .context("Supported pause role disappeared")?
            .accounts
            .is_empty(),
        "Revoked pause role is not empty"
    );
    ensure!(
        authorizations.update_allow_list.is_none(),
        "Unsupported list role was not distinct from empty pause role"
    );
    Ok(())
}
