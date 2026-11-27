//! Disabled mint and burn features reject operations and role updates.

use super::super::super::{Test, TestContext};
use super::support::{execute_expecting_rejection, execute_successful_batch, metadata};
use crate::fixture::genesis::GenesisAccount;
use crate::operations;
use crate::registry::TestGroup;
use anyhow::{Context, Result, ensure};
use concordium_rust_sdk::base::{
    common::cbor::cbor_encode,
    protocol_level_tokens::{
        TokenAdminRole, TokenAmount, TokenId, TokenModuleInitializationParameters,
        operations as token_operations,
    },
    updates::CreatePlt,
};

pub(super) fn register(group: &mut TestGroup<'_, Test>) {
    group.register(
        "Disabled mint and burn features reject operations and admin-role assignments or revocations",
        Test {
            required_accounts: 6,
            run: Box::new(|context, accounts| {
                Box::pin(disabled_mint_and_burn_reject_operations_and_role_updates(
                    context, accounts,
                ))
            }),
        },
    );
}

async fn disabled_mint_and_burn_reject_operations_and_role_updates(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let [
        governance_account,
        mint_assignment_sender,
        mint_revoke,
        burn_assignment_sender,
        burn_revoke,
        burn_sender,
    ] = accounts.as_slice()
    else {
        anyhow::bail!("Expected six accounts for concurrent disabled-feature checks");
    };
    let id = TokenId::try_from("E2E-RBAC-DISABLED".to_string())?;
    let parameters = TokenModuleInitializationParameters {
        name: Some("Disabled mint and burn".into()),
        metadata: Some(metadata("https://example.invalid/disabled")),
        governance_account: Some(governance_account.address.into()),
        allow_list: Some(false),
        deny_list: Some(false),
        initial_supply: Some(TokenAmount::from_raw(5, 0)),
        mintable: Some(false),
        burnable: Some(false),
    };
    let mut client = context.validator_node.connect().await?;
    let (created, summary) = {
        let keys = context.governance.lock().await;
        operations::create_token(
            &mut client,
            &keys,
            CreatePlt {
                token_id: id.clone(),
                token_module: operations::TOKEN_MODULE.parse()?,
                decimals: 0,
                initialization_parameters: cbor_encode(&parameters).into(),
            },
        )
        .await?
    };
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "Disabled-feature token creation failed: {summary:?}"
    );
    ensure!(
        client
            .get_account_info(&governance_account.address.into(), created)
            .await?
            .response
            .token_amount(&id)
            == Some(TokenAmount::from_raw(5, 0)),
        "Disabled-feature fixture lacks initial funds"
    );

    execute_successful_batch(
        &mut client,
        governance_account,
        &id,
        vec![
            token_operations::assign_admin_roles(
                mint_assignment_sender.address,
                vec![TokenAdminRole::UpdateAdminRoles],
            ),
            token_operations::assign_admin_roles(
                mint_revoke.address,
                vec![TokenAdminRole::UpdateAdminRoles],
            ),
            token_operations::assign_admin_roles(
                burn_assignment_sender.address,
                vec![TokenAdminRole::UpdateAdminRoles],
            ),
            token_operations::assign_admin_roles(
                burn_revoke.address,
                vec![TokenAdminRole::UpdateAdminRoles],
            ),
            token_operations::transfer_tokens(burn_sender.address, TokenAmount::from_raw(2, 0)),
        ],
    )
    .await?;

    let baseline = client
        .get_block_info(concordium_rust_sdk::v2::BlockIdentifier::LastFinal)
        .await?
        .response
        .block_hash;
    ensure!(
        client
            .get_account_info(&burn_sender.address.into(), baseline)
            .await?
            .response
            .token_amount(&id)
            == Some(TokenAmount::from_raw(2, 0)),
        "Burn fixture lacks sufficient funds"
    );

    let checks = [
        (
            mint_assignment_sender,
            token_operations::assign_admin_roles(
                governance_account.address,
                vec![TokenAdminRole::Mint],
            ),
            "Assigning a disabled mint role must be rejected",
        ),
        (
            mint_revoke,
            token_operations::revoke_admin_roles(
                governance_account.address,
                vec![TokenAdminRole::Mint],
            ),
            "Revoking a disabled mint role must be rejected",
        ),
        (
            burn_assignment_sender,
            token_operations::assign_admin_roles(
                governance_account.address,
                vec![TokenAdminRole::Burn],
            ),
            "Assigning a disabled burn role must be rejected",
        ),
        (
            burn_revoke,
            token_operations::revoke_admin_roles(
                governance_account.address,
                vec![TokenAdminRole::Burn],
            ),
            "Revoking a disabled burn role must be rejected",
        ),
        (
            governance_account,
            token_operations::mint_tokens(TokenAmount::from_raw(1, 0)),
            "Minting with mint disabled must be rejected",
        ),
        (
            burn_sender,
            token_operations::burn_tokens(TokenAmount::from_raw(1, 0)),
            "Burning with burn disabled must be rejected",
        ),
    ];
    let results =
        futures_util::future::join_all(checks.into_iter().map(|(sender, operation, label)| {
            let mut client = client.clone();
            let id = &id;
            async move {
                execute_expecting_rejection(&mut client, sender, id, operation)
                    .await
                    .context(label)
            }
        }))
        .await;
    let failures: Vec<_> = results
        .into_iter()
        .filter_map(Result::err)
        .map(|error| format!("{error:#}"))
        .collect();
    ensure!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}
