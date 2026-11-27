//! Initial admin-role assignments for supported token features.

use super::super::super::Test;
use super::super::super::TestContext;
use crate::fixture::genesis::GenesisAccount;
use crate::operations;
use crate::registry::TestGroup;
use anyhow::Result;
use anyhow::ensure;
use concordium_rust_sdk::base::common::cbor::cbor_encode;
use concordium_rust_sdk::base::protocol_level_tokens::MetadataUrl;
use concordium_rust_sdk::base::protocol_level_tokens::TokenId;
use concordium_rust_sdk::base::protocol_level_tokens::TokenModuleInitializationParameters;
use concordium_rust_sdk::base::protocol_level_tokens::TokenRoleAuthorizations;
use concordium_rust_sdk::base::updates::CreatePlt;
use concordium_rust_sdk::id::types::AccountAddress;

#[derive(Clone, Copy)]
struct Features {
    mint: bool,
    burn: bool,
    allow_list: bool,
    deny_list: bool,
}

pub(super) fn register(group: &mut TestGroup<'_, Test>) {
    group
        .register(
            "Creation assigns the governance account only update-admin-roles, pause, and update-metadata roles when optional features are disabled",
            initial_role_assignment_test(
                "E2E-RBAC-INITIAL-OFF",
                Features {
                    mint: false,
                    burn: false,
                    allow_list: false,
                    deny_list: false,
                },
            ),
        )
        .register(
            "Creation assigns the governance account all seven admin roles when all features are enabled",
            initial_role_assignment_test(
                "E2E-RBAC-INITIAL-ALL",
                Features {
                    mint: true,
                    burn: true,
                    allow_list: true,
                    deny_list: true,
                },
            ),
        )
        .register(
            "Creation assigns the governance account mint and update-allow-list roles but no burn or update-deny-list roles",
            initial_role_assignment_test(
                "E2E-RBAC-INITIAL-MA",
                Features {
                    mint: true,
                    burn: false,
                    allow_list: true,
                    deny_list: false,
                },
            ),
        )
        .register(
            "Creation assigns the governance account burn and update-deny-list roles but no mint or update-allow-list roles",
            initial_role_assignment_test(
                "E2E-RBAC-INITIAL-BD",
                Features {
                    mint: false,
                    burn: true,
                    allow_list: false,
                    deny_list: true,
                },
            ),
        );
}

fn initial_role_assignment_test(id: &'static str, features: Features) -> Test {
    Test {
        required_accounts: 1,
        run: Box::new(move |context, accounts| {
            Box::pin(initial_roles_match_enabled_features(
                context, accounts, id, features,
            ))
        }),
    }
}

async fn initial_roles_match_enabled_features(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
    id: &str,
    features: Features,
) -> Result<()> {
    let governance_account = accounts[0].address;
    let id = TokenId::try_from(id.to_string())?;
    let parameters = TokenModuleInitializationParameters {
        name: Some("Initial admin-role assignments".into()),
        metadata: Some(MetadataUrl {
            url: "https://example.invalid/e2e-rbac-initial.json".into(),
            checksum_sha_256: None,
            additional: Default::default(),
        }),
        governance_account: Some(governance_account.into()),
        allow_list: Some(features.allow_list),
        deny_list: Some(features.deny_list),
        initial_supply: None,
        mintable: Some(features.mint),
        burnable: Some(features.burn),
    };
    let request = CreatePlt {
        token_id: id.clone(),
        token_module: operations::TOKEN_MODULE.parse()?,
        decimals: 0,
        initialization_parameters: cbor_encode(&parameters).into(),
    };
    let mut client = context.validator_node.connect().await?;
    let (block, summary) = {
        let governance_keys = context.governance.lock().await;
        operations::create_token(&mut client, &governance_keys, request).await?
    };
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "Token creation did not succeed: {summary:?}"
    );
    let authorizations = client.get_token_authorizations(block, id).await?;
    for (name, role) in [
        ("update-admin-roles", &authorizations.update_admin_roles),
        ("pause", &authorizations.pause),
        ("update-metadata", &authorizations.update_metadata),
    ] {
        let role = role
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("{name} role is unsupported"))?;
        ensure!(
            role.accounts.len() == 1 && role.accounts[0].address == governance_account,
            "{name} must be assigned only to the governance account"
        );
    }
    if features.mint {
        assert_role_assigned_only_to_governance_account(
            "mint",
            &authorizations.mint,
            governance_account,
        )?;
    } else {
        assert_role_unsupported("mint", &authorizations.mint)?;
    }

    if features.burn {
        assert_role_assigned_only_to_governance_account(
            "burn",
            &authorizations.burn,
            governance_account,
        )?;
    } else {
        assert_role_unsupported("burn", &authorizations.burn)?;
    }

    if features.allow_list {
        assert_role_assigned_only_to_governance_account(
            "update-allow-list",
            &authorizations.update_allow_list,
            governance_account,
        )?;
    } else {
        assert_role_unsupported("update-allow-list", &authorizations.update_allow_list)?;
    }

    if features.deny_list {
        assert_role_assigned_only_to_governance_account(
            "update-deny-list",
            &authorizations.update_deny_list,
            governance_account,
        )?;
    } else {
        assert_role_unsupported("update-deny-list", &authorizations.update_deny_list)?;
    }
    Ok(())
}

fn assert_role_assigned_only_to_governance_account(
    name: &str,
    role: &Option<TokenRoleAuthorizations>,
    governance_account: AccountAddress,
) -> Result<()> {
    let role = role
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("{name} role is unsupported"))?;
    ensure!(
        role.accounts.len() == 1 && role.accounts[0].address == governance_account,
        "{name} must be assigned only to the governance account"
    );
    Ok(())
}

fn assert_role_unsupported(name: &str, role: &Option<TokenRoleAuthorizations>) -> Result<()> {
    ensure!(role.is_none(), "{name} role must be unsupported");
    Ok(())
}
