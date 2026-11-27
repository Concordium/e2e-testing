//! Independent update-allow-list and update-deny-list roles preserve transfer restrictions.

use super::super::super::{Test, TestContext};
use super::support::{
    create_mint_burn_token_with_compliance_lists, execute_expecting_rejection,
    execute_expecting_success, execute_successful_batch,
};
use crate::fixture::genesis::GenesisAccount;
use crate::registry::TestGroup;
use anyhow::{Context, Result};
use concordium_rust_sdk::{
    base::protocol_level_tokens::{
        TokenAdminRole, TokenAmount, TokenId, operations as token_operations,
    },
    v2::Client,
};

pub(super) fn register(group: &mut TestGroup<'_, Test>) {
    group.register(
        "Update-allow-list and update-deny-list roles are independent and preserve transfer restrictions",
        Test {
            required_accounts: 7,
            run: Box::new(|context, accounts| {
                Box::pin(
                    allow_list_and_deny_list_roles_are_independent_and_preserve_transfer_restrictions(
                        context, accounts,
                    ),
                )
            }),
        },
    );
}

async fn allow_list_and_deny_list_roles_are_independent_and_preserve_transfer_restrictions(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let [
        governance_account,
        update_allow_list_role_holder,
        update_deny_list_role_holder,
        allow_sender,
        allow_recipient,
        deny_sender,
        deny_recipient,
    ] = accounts.as_slice()
    else {
        anyhow::bail!("Expected seven accounts for independent list-role flows");
    };

    let mut client = context.validator_node.connect().await?;
    let (id, _) = create_mint_burn_token_with_compliance_lists(
        &mut client,
        context,
        governance_account,
        "E2E-RBAC-COMPLIANCE",
    )
    .await?;

    execute_successful_batch(
        &mut client,
        governance_account,
        &id,
        vec![
            token_operations::assign_admin_roles(
                update_allow_list_role_holder.address,
                vec![TokenAdminRole::UpdateAllowList],
            ),
            token_operations::assign_admin_roles(
                update_deny_list_role_holder.address,
                vec![TokenAdminRole::UpdateDenyList],
            ),
            token_operations::assign_admin_roles(allow_sender.address, vec![TokenAdminRole::Mint]),
            token_operations::assign_admin_roles(deny_sender.address, vec![TokenAdminRole::Mint]),
        ],
    )
    .await?;

    execute_successful_batch(
        &mut client,
        update_allow_list_role_holder,
        &id,
        vec![
            token_operations::add_token_allow_list(governance_account.address),
            token_operations::add_token_allow_list(allow_sender.address),
            token_operations::add_token_allow_list(deny_sender.address),
            token_operations::add_token_allow_list(deny_recipient.address),
        ],
    )
    .await?;

    execute_successful_batch(
        &mut client,
        governance_account,
        &id,
        vec![
            token_operations::mint_tokens(TokenAmount::from_raw(20, 0)),
            token_operations::transfer_tokens(allow_sender.address, TokenAmount::from_raw(10, 0)),
            token_operations::transfer_tokens(deny_sender.address, TokenAmount::from_raw(10, 0)),
        ],
    )
    .await?;

    let mut allow_client = client.clone();
    let mut deny_client = client.clone();

    // Both checks finish before either role holder starts its dependent flow.
    let (allow_rejection, deny_rejection) = tokio::join!(
        execute_expecting_rejection(
            &mut allow_client,
            update_allow_list_role_holder,
            &id,
            token_operations::add_token_deny_list(governance_account.address),
        ),
        execute_expecting_rejection(
            &mut deny_client,
            update_deny_list_role_holder,
            &id,
            token_operations::add_token_allow_list(update_deny_list_role_holder.address),
        ),
    );
    allow_rejection.context("The update-allow-list role must not authorize deny-list updates")?;
    deny_rejection.context("The update-deny-list role must not authorize allow-list updates")?;

    // Supply, configuration, and role assignments stay fixed during both flows.
    // Each flow has separate role holders, transfer senders, and recipients.
    let (allow_result, deny_result) = tokio::join!(
        allow_list_membership_controls_transfers(
            &mut allow_client,
            update_allow_list_role_holder,
            allow_sender,
            allow_recipient,
            &id,
        ),
        deny_list_membership_controls_transfers(
            &mut deny_client,
            update_deny_list_role_holder,
            deny_sender,
            deny_recipient,
            &id,
        ),
    );
    allow_result.context("Allow-list transfer restrictions failed")?;
    deny_result.context("Deny-list transfer restrictions failed")?;
    Ok(())
}

async fn allow_list_membership_controls_transfers(
    client: &mut Client,
    role_holder: &GenesisAccount,
    sender: &GenesisAccount,
    recipient: &GenesisAccount,
    id: &TokenId,
) -> Result<()> {
    execute_expecting_rejection(
        client,
        sender,
        id,
        token_operations::transfer_tokens(recipient.address, TokenAmount::from_raw(1, 0)),
    )
    .await?;
    execute_expecting_success(
        client,
        role_holder,
        id,
        token_operations::add_token_allow_list(recipient.address),
    )
    .await?;
    execute_expecting_success(
        client,
        sender,
        id,
        token_operations::transfer_tokens(recipient.address, TokenAmount::from_raw(1, 0)),
    )
    .await?;
    execute_expecting_success(
        client,
        role_holder,
        id,
        token_operations::remove_token_allow_list(recipient.address),
    )
    .await?;
    execute_expecting_rejection(
        client,
        sender,
        id,
        token_operations::transfer_tokens(recipient.address, TokenAmount::from_raw(1, 0)),
    )
    .await?;
    Ok(())
}

async fn deny_list_membership_controls_transfers(
    client: &mut Client,
    role_holder: &GenesisAccount,
    sender: &GenesisAccount,
    recipient: &GenesisAccount,
    id: &TokenId,
) -> Result<()> {
    execute_expecting_success(
        client,
        sender,
        id,
        token_operations::transfer_tokens(recipient.address, TokenAmount::from_raw(1, 0)),
    )
    .await?;
    execute_expecting_success(
        client,
        role_holder,
        id,
        token_operations::add_token_deny_list(recipient.address),
    )
    .await?;
    execute_expecting_rejection(
        client,
        sender,
        id,
        token_operations::transfer_tokens(recipient.address, TokenAmount::from_raw(1, 0)),
    )
    .await?;
    execute_expecting_success(
        client,
        role_holder,
        id,
        token_operations::remove_token_deny_list(recipient.address),
    )
    .await?;
    execute_expecting_success(
        client,
        sender,
        id,
        token_operations::transfer_tokens(recipient.address, TokenAmount::from_raw(1, 0)),
    )
    .await?;
    Ok(())
}
