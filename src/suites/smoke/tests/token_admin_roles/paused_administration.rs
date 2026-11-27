//! Delegated administration remains available while the token is paused.

use super::super::super::{Test, TestContext};
use super::support::{
    create_mint_burn_token_with_compliance_lists, execute_expecting_success,
    execute_successful_batch, metadata, single_module_event,
};
use crate::fixture::genesis::GenesisAccount;
use crate::operations;
use crate::registry::TestGroup;
use anyhow::{Context, Result, ensure};
use concordium_rust_sdk::base::protocol_level_tokens::{
    TokenAdminRole, TokenAmount, TokenId, TokenModuleAccountState, TokenModuleEvent,
    operations as token_operations,
};
use concordium_rust_sdk::{id::types::AccountAddress, types::hashes::BlockHash, v2::Client};

pub(super) fn register(group: &mut TestGroup<'_, Test>) {
    group.register(
        "Delegated role, metadata, and compliance-list administration succeeds while paused",
        Test {
            required_accounts: 9,
            run: Box::new(|context, accounts| {
                Box::pin(delegated_administration_remains_available_while_paused(
                    context, accounts,
                ))
            }),
        },
    );
}

async fn delegated_administration_remains_available_while_paused(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let [
        governance_account,
        update_metadata_role_holder,
        pause_role_holder,
        update_admin_roles_role_holder,
        target,
        update_allow_list_role_holder,
        update_deny_list_role_holder,
        allow_target,
        deny_target,
    ] = accounts.as_slice()
    else {
        anyhow::bail!("Expected nine accounts for paused administration");
    };
    let mut client = context.validator_node.connect().await?;
    let (id, _) = create_mint_burn_token_with_compliance_lists(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-PAUSED-ADMIN",
    )
    .await?;
    execute_successful_batch(
        &mut client,
        governance_account,
        &id,
        vec![
            token_operations::assign_admin_roles(
                update_metadata_role_holder.address,
                vec![TokenAdminRole::UpdateMetadata],
            ),
            token_operations::assign_admin_roles(
                pause_role_holder.address,
                vec![TokenAdminRole::Pause],
            ),
            token_operations::assign_admin_roles(
                update_admin_roles_role_holder.address,
                vec![TokenAdminRole::UpdateAdminRoles],
            ),
            token_operations::assign_admin_roles(
                update_allow_list_role_holder.address,
                vec![TokenAdminRole::UpdateAllowList],
            ),
            token_operations::assign_admin_roles(
                update_deny_list_role_holder.address,
                vec![TokenAdminRole::UpdateDenyList],
            ),
            // Fund targets so their token states remain present after list removal.
            token_operations::add_token_allow_list(governance_account.address),
            token_operations::add_token_allow_list(allow_target.address),
            token_operations::add_token_allow_list(deny_target.address),
            token_operations::mint_tokens(TokenAmount::from_raw(2, 0)),
            token_operations::transfer_tokens(allow_target.address, TokenAmount::from_raw(1, 0)),
            token_operations::transfer_tokens(deny_target.address, TokenAmount::from_raw(1, 0)),
            token_operations::remove_token_allow_list(allow_target.address),
        ],
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

    let mut metadata_client = client.clone();
    let mut roles_client = client.clone();
    let mut allow_client = client.clone();
    let mut deny_client = client.clone();
    let (metadata_result, roles_result, allow_result, deny_result) = tokio::join!(
        update_metadata_while_paused(&mut metadata_client, update_metadata_role_holder, &id),
        update_admin_roles_while_paused(
            &mut roles_client,
            update_admin_roles_role_holder,
            governance_account.address,
            target.address,
            &id,
        ),
        update_allow_list_while_paused(
            &mut allow_client,
            update_allow_list_role_holder,
            allow_target.address,
            &id,
            paused,
        ),
        update_deny_list_while_paused(
            &mut deny_client,
            update_deny_list_role_holder,
            deny_target.address,
            &id,
            paused,
        ),
    );
    metadata_result.context("Delegated metadata replacement while paused failed")?;
    roles_result.context("Delegated role updates while paused failed")?;
    allow_result.context("Delegated allow-list updates while paused failed")?;
    deny_result.context("Delegated deny-list updates while paused failed")?;

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
    Ok(())
}

async fn update_allow_list_while_paused(
    client: &mut Client,
    role_holder: &GenesisAccount,
    target: AccountAddress,
    id: &TokenId,
    baseline: BlockHash,
) -> Result<()> {
    ensure!(
        membership_while_paused(client, target, id, baseline)
            .await?
            .allow_list
            == Some(false),
        "Allow-list target is already a member"
    );
    let added = execute_expecting_success(
        client,
        role_holder,
        id,
        token_operations::add_token_allow_list(target),
    )
    .await?;
    ensure!(
        membership_while_paused(client, target, id, added)
            .await?
            .allow_list
            == Some(true),
        "Paused allow-list addition did not add the member"
    );
    let removed = execute_expecting_success(
        client,
        role_holder,
        id,
        token_operations::remove_token_allow_list(target),
    )
    .await?;
    ensure!(
        membership_while_paused(client, target, id, removed)
            .await?
            .allow_list
            == Some(false),
        "Paused allow-list removal did not remove the member"
    );
    Ok(())
}

async fn update_deny_list_while_paused(
    client: &mut Client,
    role_holder: &GenesisAccount,
    target: AccountAddress,
    id: &TokenId,
    baseline: BlockHash,
) -> Result<()> {
    ensure!(
        membership_while_paused(client, target, id, baseline)
            .await?
            .deny_list
            == Some(false),
        "Deny-list target is already a member"
    );
    let added = execute_expecting_success(
        client,
        role_holder,
        id,
        token_operations::add_token_deny_list(target),
    )
    .await?;
    ensure!(
        membership_while_paused(client, target, id, added)
            .await?
            .deny_list
            == Some(true),
        "Paused deny-list addition did not add the member"
    );
    let removed = execute_expecting_success(
        client,
        role_holder,
        id,
        token_operations::remove_token_deny_list(target),
    )
    .await?;
    ensure!(
        membership_while_paused(client, target, id, removed)
            .await?
            .deny_list
            == Some(false),
        "Paused deny-list removal did not remove the member"
    );
    Ok(())
}

/// Read membership and require paused token state at the same finalized block.
async fn membership_while_paused(
    client: &mut Client,
    target: AccountAddress,
    id: &TokenId,
    block: BlockHash,
) -> Result<TokenModuleAccountState> {
    ensure!(
        client
            .get_token_info(id.clone(), block)
            .await?
            .response
            .token_state
            .decode_module_state()?
            .paused
            == Some(true),
        "List administration did not retain paused state"
    );
    let account = client
        .get_account_info(&target.into(), block)
        .await?
        .response;
    account
        .tokens
        .iter()
        .find(|token| token.token_id == *id)
        .context("Funded list target has no token state")?
        .state
        .decode_module_state()
        .context("Cannot decode list target membership")
}

async fn update_metadata_while_paused(
    client: &mut Client,
    role_holder: &GenesisAccount,
    id: &TokenId,
) -> Result<()> {
    let replacement = metadata("https://example.invalid/paused-administration");
    let updated = execute_expecting_success(
        client,
        role_holder,
        id,
        token_operations::update_metadata(replacement.clone()),
    )
    .await?;
    let state = client
        .get_token_info(id.clone(), updated)
        .await?
        .response
        .token_state
        .decode_module_state()?;
    ensure!(
        state.metadata == Some(replacement) && state.paused == Some(true),
        "Metadata replacement did not retain the reference and paused state"
    );
    Ok(())
}

async fn update_admin_roles_while_paused(
    client: &mut Client,
    role_holder: &GenesisAccount,
    governance_account: AccountAddress,
    target: AccountAddress,
    id: &TokenId,
) -> Result<()> {
    let (assigned, summary) = operations::update_token(
        client,
        role_holder,
        id,
        [token_operations::assign_admin_roles(
            target,
            vec![TokenAdminRole::Mint],
        )]
        .into_iter()
        .collect(),
        "paused-assign-admin-roles",
    )
    .await?;
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "Paused admin-role assignment failed: {summary:?}"
    );
    let TokenModuleEvent::AssignAdminRoles(event) = single_module_event(&summary, id)? else {
        anyhow::bail!("Paused admin-role assignment emitted the wrong event");
    };
    ensure!(
        event.account.address == target && event.roles == vec![TokenAdminRole::Mint],
        "Paused admin-role assignment event has incorrect account or roles"
    );
    let authorizations = client
        .get_token_authorizations(assigned, id.clone())
        .await?;
    let mint = authorizations.mint.context("Mint role is unsupported")?;
    ensure!(
        mint.accounts.len() == 2
            && mint
                .accounts
                .iter()
                .any(|account| account.address == governance_account)
            && mint
                .accounts
                .iter()
                .any(|account| account.address == target),
        "Paused admin-role assignment did not assign the mint role to the target"
    );
    ensure!(
        client
            .get_token_info(id.clone(), assigned)
            .await?
            .response
            .token_state
            .decode_module_state()?
            .paused
            == Some(true),
        "Admin-role assignment did not retain paused state"
    );

    let (revoked, summary) = operations::update_token(
        client,
        role_holder,
        id,
        [token_operations::revoke_admin_roles(
            target,
            vec![TokenAdminRole::Mint],
        )]
        .into_iter()
        .collect(),
        "paused-revoke-admin-roles",
    )
    .await?;
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "Paused admin-role revocation failed: {summary:?}"
    );
    let TokenModuleEvent::RevokeAdminRoles(event) = single_module_event(&summary, id)? else {
        anyhow::bail!("Paused admin-role revocation emitted the wrong event");
    };
    ensure!(
        event.account.address == target && event.roles == vec![TokenAdminRole::Mint],
        "Paused admin-role revocation event has incorrect account or roles"
    );
    let authorizations = client.get_token_authorizations(revoked, id.clone()).await?;
    let mint = authorizations.mint.context("Mint role is unsupported")?;
    ensure!(
        mint.accounts.len() == 1 && mint.accounts[0].address == governance_account,
        "Paused admin-role revocation did not remove the target's mint role"
    );
    ensure!(
        client
            .get_token_info(id.clone(), revoked)
            .await?
            .response
            .token_state
            .decode_module_state()?
            .paused
            == Some(true),
        "Admin-role revocation did not retain paused state"
    );
    Ok(())
}
