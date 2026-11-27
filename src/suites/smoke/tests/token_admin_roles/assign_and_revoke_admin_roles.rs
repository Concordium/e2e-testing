//! Admin-role assignment, revocation, and handover.

use super::super::super::{Test, TestContext};
use super::support::create_mint_burn_token;
use super::support::single_module_event;
use super::support::{execute_expecting_rejection, execute_expecting_success};
use crate::fixture::genesis::GenesisAccount;
use crate::operations;
use crate::registry::TestGroup;
use anyhow::ensure;
use anyhow::{Context, Result};
use concordium_rust_sdk::base::protocol_level_tokens::TokenAdminRole;
use concordium_rust_sdk::base::protocol_level_tokens::TokenAmount;
use concordium_rust_sdk::base::protocol_level_tokens::TokenId;
use concordium_rust_sdk::base::protocol_level_tokens::TokenModuleEvent;
use concordium_rust_sdk::base::protocol_level_tokens::TokenRoleAuthorizations;
use concordium_rust_sdk::base::protocol_level_tokens::operations as token_operations;
use concordium_rust_sdk::id::types::AccountAddress;
use concordium_rust_sdk::v2::Client;

pub(super) fn register(group: &mut TestGroup<'_, Test>) {
    group.register(
        "Mint role assignment and revocation apply across account aliases",
        Test {
            required_accounts: 2,
            run: Box::new(|context, accounts| {
                Box::pin(mint_role_assignment_and_revocation_apply_across_aliases(
                    context, accounts,
                ))
            }),
        },
    );
    group
        .register(
            "The update-admin-roles role supports alias identity, repeated assignments, and handover without a governance-account override",
            Test {
                required_accounts: 3,
                run: Box::new(|context, accounts| {
                    Box::pin(admin_role_handover_without_governance_account_override(
                        context, accounts,
                    ))
                }),
            },
        )
        .register(
            "Revoking selected admin roles preserves unrelated role assignments",
            Test {
                required_accounts: 2,
                run: Box::new(|context, accounts| {
                    Box::pin(assign_multiple_admin_roles_and_revoke_a_subset(context, accounts))
                }),
            },
        );
}

async fn mint_role_assignment_and_revocation_apply_across_aliases(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let [governance_account, mut mint_role_holder]: [GenesisAccount; 2] =
        accounts.try_into().ok().context("Expected two accounts")?;
    let canonical = mint_role_holder.address;
    let assignment_alias = canonical.get_alias(1).context("Invalid assignment alias")?;
    let sender_alias = canonical.get_alias(2).context("Invalid sender alias")?;
    let revoke_alias = canonical.get_alias(3).context("Invalid revoke alias")?;
    let mut client = context.validator_node.connect().await?;
    let (id, _) = create_mint_burn_token(
        &mut client,
        context,
        &governance_account,
        "E2E-RBAC-ALIASES",
    )
    .await?;
    let assigned = execute_expecting_success(
        &mut client,
        &governance_account,
        &id,
        token_operations::assign_admin_roles(assignment_alias, vec![TokenAdminRole::Mint]),
    )
    .await?;
    let authorizations = client
        .get_token_authorizations(assigned, id.clone())
        .await?;
    assert_role_accounts(
        "mint after alias assignment",
        &authorizations.mint,
        &[governance_account.address, canonical],
    )?;

    // Keep the signing keys; use an alias as the transaction sender.
    // Submit sequentially because aliases share the account nonce.
    mint_role_holder.address = sender_alias;
    let minted = execute_expecting_success(
        &mut client,
        &mint_role_holder,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(1, 0)),
    )
    .await?;
    ensure!(
        client
            .get_account_info(&canonical.into(), minted)
            .await?
            .response
            .token_amount(&id)
            == Some(TokenAmount::from_raw(1, 0)),
        "Alias sender did not mint to the canonical account"
    );
    let revoked = execute_expecting_success(
        &mut client,
        &governance_account,
        &id,
        token_operations::revoke_admin_roles(revoke_alias, vec![TokenAdminRole::Mint]),
    )
    .await?;
    let authorizations = client.get_token_authorizations(revoked, id.clone()).await?;
    assert_role_accounts(
        "mint after alias revocation",
        &authorizations.mint,
        &[governance_account.address],
    )?;
    execute_expecting_rejection(
        &mut client,
        &mint_role_holder,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(1, 0)),
    )
    .await?;
    mint_role_holder.address = canonical;
    execute_expecting_rejection(
        &mut client,
        &mint_role_holder,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(1, 0)),
    )
    .await?;
    Ok(())
}

async fn assign_multiple_admin_roles_and_revoke_a_subset(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let governance_account = &accounts[0];
    let role_holder = &accounts[1];
    let mut client = context.validator_node.connect().await?;
    let (id, created) = create_mint_burn_token(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-LIFECYCLE",
    )
    .await?;
    let role_holder_before = client
        .get_account_info(&role_holder.address.into(), created)
        .await?
        .response;
    ensure!(
        role_holder_before
            .tokens
            .iter()
            .all(|token| token.token_id != id),
        "Role recipient already holds the token"
    );

    let (assigned, summary) = operations::update_token(
        &mut client,
        governance_account,
        &id,
        [token_operations::assign_admin_roles(
            role_holder.address,
            vec![TokenAdminRole::Mint, TokenAdminRole::Burn],
        )]
        .into_iter()
        .collect(),
        "assign-mint-and-burn",
    )
    .await?;
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "Admin-role assignment did not succeed: {summary:?}"
    );
    let event = single_module_event(&summary, &id)?;
    let TokenModuleEvent::AssignAdminRoles(event) = event else {
        anyhow::bail!("Expected an assign-admin-roles event");
    };
    ensure!(
        event.account.address == role_holder.address
            && event.roles.len() == 2
            && event.roles.contains(&TokenAdminRole::Mint)
            && event.roles.contains(&TokenAdminRole::Burn),
        "Assign-admin-roles event has incorrect account or roles"
    );

    let (revoked, summary) = operations::update_token(
        &mut client,
        governance_account,
        &id,
        [token_operations::revoke_admin_roles(
            role_holder.address,
            vec![TokenAdminRole::Mint],
        )]
        .into_iter()
        .collect(),
        "revoke-mint",
    )
    .await?;
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "Admin-role revocation did not succeed: {summary:?}"
    );
    let event = single_module_event(&summary, &id)?;
    let TokenModuleEvent::RevokeAdminRoles(event) = event else {
        anyhow::bail!("Expected a revoke-admin-roles event");
    };
    ensure!(
        event.account.address == role_holder.address && event.roles == vec![TokenAdminRole::Mint],
        "Revoke-admin-roles event has incorrect account or roles"
    );

    let authorizations_before_assignment = client
        .get_token_authorizations(created, id.clone())
        .await?;
    assert_role_accounts(
        "mint before assignment",
        &authorizations_before_assignment.mint,
        &[governance_account.address],
    )?;
    assert_role_accounts(
        "burn before assignment",
        &authorizations_before_assignment.burn,
        &[governance_account.address],
    )?;

    let authorizations_after_assignment = client
        .get_token_authorizations(assigned, id.clone())
        .await?;
    assert_role_accounts(
        "update-admin-roles after assignment",
        &authorizations_after_assignment.update_admin_roles,
        &[governance_account.address],
    )?;
    assert_role_accounts(
        "mint after assignment",
        &authorizations_after_assignment.mint,
        &[governance_account.address, role_holder.address],
    )?;
    assert_role_accounts(
        "burn after assignment",
        &authorizations_after_assignment.burn,
        &[governance_account.address, role_holder.address],
    )?;
    assert_role_accounts(
        "pause after assignment",
        &authorizations_after_assignment.pause,
        &[governance_account.address],
    )?;
    assert_role_accounts(
        "update-metadata after assignment",
        &authorizations_after_assignment.update_metadata,
        &[governance_account.address],
    )?;
    ensure!(
        authorizations_after_assignment.update_allow_list.is_none()
            && authorizations_after_assignment.update_deny_list.is_none(),
        "Disabled list roles became supported after assignment"
    );

    let authorizations_after_revocation = client.get_token_authorizations(revoked, id).await?;
    assert_role_accounts(
        "update-admin-roles after revocation",
        &authorizations_after_revocation.update_admin_roles,
        &[governance_account.address],
    )?;
    assert_role_accounts(
        "mint after revocation",
        &authorizations_after_revocation.mint,
        &[governance_account.address],
    )?;
    assert_role_accounts(
        "burn after revocation",
        &authorizations_after_revocation.burn,
        &[governance_account.address, role_holder.address],
    )?;
    assert_role_accounts(
        "pause after revocation",
        &authorizations_after_revocation.pause,
        &[governance_account.address],
    )?;
    assert_role_accounts(
        "update-metadata after revocation",
        &authorizations_after_revocation.update_metadata,
        &[governance_account.address],
    )?;
    ensure!(
        authorizations_after_revocation.update_allow_list.is_none()
            && authorizations_after_revocation.update_deny_list.is_none(),
        "Disabled list roles became supported after revocation"
    );
    Ok(())
}

/// Compare all assigned accounts without requiring query ordering.
fn assert_role_accounts(
    label: &str,
    role: &Option<TokenRoleAuthorizations>,
    expected: &[AccountAddress],
) -> Result<()> {
    let role = role
        .as_ref()
        .with_context(|| format!("{label}: role is unsupported"))?;
    let mut actual: Vec<_> = role
        .accounts
        .iter()
        .map(|account| account.address)
        .collect();
    let mut expected = expected.to_vec();
    actual.sort();
    expected.sort();
    ensure!(
        actual == expected,
        "{label}: expected {expected:?}, got {actual:?}"
    );
    Ok(())
}

/// Require assignment of the update-admin-roles role, its event, and nonce advancement.
async fn assign_update_admin_roles_and_check_event(
    client: &mut Client,
    sender: &GenesisAccount,
    id: &TokenId,
    target: AccountAddress,
) -> Result<()> {
    let nonce = client
        .get_next_account_sequence_number(&sender.address)
        .await?
        .nonce;
    let (_, summary) = operations::update_token(
        client,
        sender,
        id,
        [token_operations::assign_admin_roles(
            target,
            vec![TokenAdminRole::UpdateAdminRoles],
        )]
        .into_iter()
        .collect(),
        "assign-update-admin-roles",
    )
    .await?;
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "Admin-role assignment failed: {summary:?}"
    );
    let TokenModuleEvent::AssignAdminRoles(event) = single_module_event(&summary, id)? else {
        anyhow::bail!("Missing assign-admin-roles event");
    };
    ensure!(
        event.account.address == target && event.roles == vec![TokenAdminRole::UpdateAdminRoles],
        "Assign-admin-roles event has incorrect account or roles"
    );
    ensure!(
        client
            .get_account_info(
                &sender.address.into(),
                concordium_rust_sdk::v2::BlockIdentifier::LastFinal,
            )
            .await?
            .response
            .account_nonce
            == nonce.next(),
        "Admin-role assignment nonce did not advance"
    );
    Ok(())
}

/// Require successful revocation of an absent burn role and its matching event.
async fn revoke_absent_burn_role_and_check_event(
    client: &mut Client,
    sender: &GenesisAccount,
    id: &TokenId,
    target: AccountAddress,
) -> Result<()> {
    let before = client
        .get_token_authorizations(
            concordium_rust_sdk::v2::BlockIdentifier::LastFinal,
            id.clone(),
        )
        .await?
        .burn
        .context("Burn role is unsupported")?;
    ensure!(
        before.accounts.iter().all(|account| account.address != target),
        "Absent-role revocation target already holds the burn role"
    );
    let (block, summary) = operations::update_token(
        client,
        sender,
        id,
        [token_operations::revoke_admin_roles(
            target,
            vec![TokenAdminRole::Burn],
        )]
        .into_iter()
        .collect(),
        "revoke-absent",
    )
    .await?;
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "Absent-role revocation failed"
    );
    let TokenModuleEvent::RevokeAdminRoles(event) = single_module_event(&summary, id)? else {
        anyhow::bail!("Absent-role revocation emitted no event");
    };
    ensure!(
        event.account.address == target && event.roles == vec![TokenAdminRole::Burn],
        "Incorrect absent-role revocation event"
    );
    let after = client.get_token_authorizations(block, id.clone()).await?;
    let expected: Vec<_> = before
        .accounts
        .iter()
        .map(|account| account.address)
        .collect();
    assert_role_accounts(
        "burn after absent-role revocation",
        &after.burn,
        &expected,
    )?;
    Ok(())
}

async fn admin_role_handover_without_governance_account_override(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let governance_account = &accounts[0];
    let successor = &accounts[1];
    let outsider = &accounts[2];
    let mut client = context.validator_node.connect().await?;
    let (id, _) = create_mint_burn_token(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-HANDOVER",
    )
    .await?;
    let update_admin_roles = TokenAdminRole::UpdateAdminRoles;
    execute_expecting_rejection(
        &mut client,
        governance_account,
        &id,
        token_operations::revoke_admin_roles(governance_account.address, vec![update_admin_roles]),
    )
    .await?;
    execute_expecting_rejection(
        &mut client,
        outsider,
        &id,
        token_operations::assign_admin_roles(outsider.address, vec![TokenAdminRole::Mint]),
    )
    .await?;
    execute_expecting_rejection(
        &mut client,
        outsider,
        &id,
        token_operations::revoke_admin_roles(
            governance_account.address,
            vec![TokenAdminRole::Mint],
        ),
    )
    .await?;
    let mut alias = successor.address;
    alias.0[31] ^= 1;
    assign_update_admin_roles_and_check_event(&mut client, governance_account, &id, alias)
        .await
        .context("Initial update-admin-roles assignment failed")?;
    assign_update_admin_roles_and_check_event(&mut client, governance_account, &id, alias)
        .await
        .context("Repeated update-admin-roles assignment failed")?;
    let authorizations = client
        .get_token_authorizations(
            concordium_rust_sdk::v2::BlockIdentifier::LastFinal,
            id.clone(),
        )
        .await?;
    let role_authorizations = authorizations
        .update_admin_roles
        .context("Update-admin-roles role is unsupported")?;
    ensure!(
        role_authorizations.accounts.len() == 2
            && role_authorizations
                .accounts
                .iter()
                .any(|account| account.address == successor.address),
        "Token authorizations after alias assignment omit the canonical successor"
    );
    execute_expecting_rejection(
        &mut client,
        successor,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(1, 0)),
    )
    .await?;
    execute_expecting_rejection(
        &mut client,
        successor,
        &id,
        token_operations::revoke_admin_roles(alias, vec![update_admin_roles]),
    )
    .await?;
    execute_expecting_success(
        &mut client,
        successor,
        &id,
        token_operations::assign_admin_roles(successor.address, vec![TokenAdminRole::Mint]),
    )
    .await?;
    execute_expecting_success(
        &mut client,
        successor,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(1, 0)),
    )
    .await?;
    execute_expecting_success(
        &mut client,
        successor,
        &id,
        token_operations::revoke_admin_roles(
            governance_account.address,
            vec![update_admin_roles, TokenAdminRole::Mint],
        ),
    )
    .await?;
    execute_expecting_rejection(
        &mut client,
        governance_account,
        &id,
        token_operations::mint_tokens(TokenAmount::from_raw(1, 0)),
    )
    .await?;
    execute_expecting_rejection(
        &mut client,
        governance_account,
        &id,
        token_operations::assign_admin_roles(governance_account.address, vec![update_admin_roles]),
    )
    .await?;
    revoke_absent_burn_role_and_check_event(&mut client, successor, &id, outsider.address)
        .await
        .context("Initial revocation of an absent burn role failed")?;
    revoke_absent_burn_role_and_check_event(&mut client, successor, &id, outsider.address)
        .await
        .context("Repeated revocation of an absent burn role failed")?;
    Ok(())
}
