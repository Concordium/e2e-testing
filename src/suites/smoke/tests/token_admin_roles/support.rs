//! Shared scenario setup, submission, and finalized evidence.

use super::super::super::TestContext;
use crate::fixture::genesis::GenesisAccount;
use crate::operations;
use anyhow::ensure;
use anyhow::{Context, Result};
use concordium_rust_sdk::base::common::cbor::cbor_encode;
use concordium_rust_sdk::base::protocol_level_tokens::MetadataUrl;
use concordium_rust_sdk::base::protocol_level_tokens::TokenId;
use concordium_rust_sdk::base::protocol_level_tokens::TokenModuleEvent;
use concordium_rust_sdk::base::protocol_level_tokens::TokenModuleInitializationParameters;
use concordium_rust_sdk::base::protocol_level_tokens::TokenOperation;
use concordium_rust_sdk::base::updates::CreatePlt;
use concordium_rust_sdk::protocol_level_tokens::TokenEventDetails;
use concordium_rust_sdk::types::AccountTransactionEffects;
use concordium_rust_sdk::types::BlockItemSummary;
use concordium_rust_sdk::types::hashes::BlockHash;
use concordium_rust_sdk::v2::Client;

pub(super) fn metadata(url: &str) -> MetadataUrl {
    MetadataUrl {
        url: url.into(),
        checksum_sha_256: None,
        additional: Default::default(),
    }
}

/// Create a zero-decimal token with mint/burn enabled, zero supply, and no lists.
/// Return the token ID and finalized creation block.
pub(super) async fn create_mint_burn_token(
    client: &mut Client,
    context: &TestContext,
    governance_account: &GenesisAccount,
    name: &str,
) -> Result<(TokenId, BlockHash)> {
    create_configured(client, context, governance_account, name, false).await
}

/// Create a zero-decimal token with mint/burn and both compliance lists enabled.
/// The initial supply is zero.
/// Return the token ID and finalized creation block.
pub(super) async fn create_mint_burn_token_with_compliance_lists(
    client: &mut Client,
    context: &TestContext,
    governance_account: &GenesisAccount,
    name: &str,
) -> Result<(TokenId, BlockHash)> {
    create_configured(client, context, governance_account, name, true).await
}

/// Create a zero-decimal, mint/burn-enabled token with zero supply.
/// `lists` enables or disables both the allow list and deny list.
async fn create_configured(
    client: &mut Client,
    context: &TestContext,
    governance_account: &GenesisAccount,
    name: &str,
    lists: bool,
) -> Result<(TokenId, BlockHash)> {
    let id = TokenId::try_from(name.to_string())?;
    let parameters = TokenModuleInitializationParameters {
        name: Some(name.into()),
        metadata: Some(metadata("https://example.invalid/original")),
        governance_account: Some(governance_account.address.into()),
        allow_list: Some(lists),
        deny_list: Some(lists),
        initial_supply: None,
        mintable: Some(true),
        burnable: Some(true),
    };
    let (created, summary) = {
        let keys = context.governance.lock().await;
        operations::create_token(
            client,
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
        "Creation failed: {summary:?}"
    );
    Ok((id, created))
}

/// Require finalized success and check metadata events when applicable.
pub(super) async fn execute_expecting_success(
    client: &mut Client,
    sender: &GenesisAccount,
    id: &TokenId,
    operation: TokenOperation,
) -> Result<BlockHash> {
    let expected_metadata = match &operation {
        TokenOperation::UpdateMetadata(reference) => Some(reference.clone()),
        _ => None,
    };
    let (block, summary) = operations::update_token(
        client,
        sender,
        id,
        [operation].into_iter().collect(),
        "administration-scenario",
    )
    .await?;
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "Unexpected execution outcome: {summary:?}"
    );
    if let Some(reference) = expected_metadata {
        let TokenModuleEvent::UpdateMetadata(event) = single_module_event(&summary, id)? else {
            anyhow::bail!("Metadata update emitted the wrong event");
        };
        ensure!(
            event.metadata_url == reference,
            "Metadata event contains the wrong reference"
        );
    }
    Ok(block)
}

/// Require finalized rejection, charged energy, and unchanged relevant token state.
pub(super) async fn execute_expecting_rejection(
    client: &mut Client,
    sender: &GenesisAccount,
    id: &TokenId,
    operation: TokenOperation,
) -> Result<BlockHash> {
    let baseline = client
        .get_block_info(concordium_rust_sdk::v2::BlockIdentifier::LastFinal)
        .await?
        .response
        .block_hash;
    let before = client.get_token_info(id.clone(), baseline).await?.response;
    let sender_before = client
        .get_account_info(&sender.address.into(), baseline)
        .await?
        .response
        .token_amount(id);
    let authorizations_before = client
        .get_token_authorizations(baseline, id.clone())
        .await?;
    let target_account = match &operation {
        TokenOperation::Transfer(details) => Some(details.recipient.address),
        TokenOperation::AddAllowList(details)
        | TokenOperation::RemoveAllowList(details)
        | TokenOperation::AddDenyList(details)
        | TokenOperation::RemoveDenyList(details) => Some(details.target.address),
        _ => None,
    };
    let target_before = if let Some(target) = target_account {
        client
            .get_account_info(&target.into(), baseline)
            .await?
            .response
            .tokens
            .into_iter()
            .find(|token| token.token_id == *id)
            .map(|token| token.state)
    } else {
        None
    };
    let (block, summary) = operations::update_token(
        client,
        sender,
        id,
        [operation].into_iter().collect(),
        "administration-scenario",
    )
    .await?;
    ensure!(
        summary.is_success().as_known() == Some(&false),
        "Unexpected execution outcome: {summary:?}"
    );
    ensure!(
        summary.energy_cost.energy > 0,
        "Rejected execution consumed no energy"
    );
    let sender_after = client
        .get_account_info(&sender.address.into(), block)
        .await?
        .response
        .token_amount(id);
    ensure!(
        sender_before == sender_after,
        "Rejected operation changed sender token balance"
    );
    let after = client.get_token_info(id.clone(), block).await?.response;
    ensure!(
        before.token_state.total_supply == after.token_state.total_supply
            && before.token_state.module_state == after.token_state.module_state,
        "Rejected operation changed token state"
    );
    let authorizations_after = client.get_token_authorizations(block, id.clone()).await?;
    ensure!(
        cbor_encode(&authorizations_before) == cbor_encode(&authorizations_after),
        "Rejected operation changed token authorizations"
    );
    if let Some(target) = target_account {
        let target_after = client
            .get_account_info(&target.into(), block)
            .await?
            .response
            .tokens
            .into_iter()
            .find(|token| token.token_id == *id)
            .map(|token| token.state);
        ensure!(
            target_before == target_after,
            "Rejected operation changed token account state for {target}"
        );
    }
    Ok(block)
}

/// Batch successful setup only when no intermediate outcome is under test.
pub(super) async fn execute_successful_batch(
    client: &mut Client,
    sender: &GenesisAccount,
    id: &TokenId,
    operations: Vec<TokenOperation>,
) -> Result<()> {
    let (_, summary) = operations::update_token(
        client,
        sender,
        id,
        operations.into_iter().collect(),
        "scenario-setup",
    )
    .await?;
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "Scenario setup failed: {summary:?}"
    );
    Ok(())
}

/// Require one token-module event and decode its public payload.
pub(super) fn single_module_event(
    summary: &BlockItemSummary,
    id: &TokenId,
) -> Result<TokenModuleEvent> {
    let transaction = summary
        .details
        .as_known()
        .and_then(|details| details.as_account_transaction())
        .context("Expected an account transaction receipt")?;
    let effects = transaction
        .effects
        .as_known()
        .context("Unknown account transaction effects")?;
    let AccountTransactionEffects::TokenUpdate { events } = effects else {
        anyhow::bail!("Expected token-update effects");
    };
    ensure!(events.len() == 1, "Expected exactly one token event");
    let event = &events[0];
    ensure!(event.token_id == *id, "Event identifies another token");
    let TokenEventDetails::Module(event) = &event.event else {
        anyhow::bail!("Expected a token-module event");
    };
    event
        .decode_token_module_event()?
        .as_known()
        .cloned()
        .context("Unknown token-module event")
}
