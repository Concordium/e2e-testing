//! Transaction operations shared by the end-to-end suites.

/// Native token module reference used by the private-chain fixtures.
pub(crate) const TOKEN_MODULE: &str =
    "5c5c2645db84a7026d78f2501740f60a8ccb8fae5c166dc2428077fd9a699a4a";

use crate::fixture::genesis::GenesisAccount;
use anyhow::Result;
use concordium_rust_sdk::{
    base::{
        protocol_level_tokens::{TokenId, TokenOperations},
        updates::{self, CreatePlt, UpdatePayload},
    },
    common::types::TransactionTime,
    genesis::output::GeneratedKeyPair,
    types::{
        BlockItemSummary, UpdateKeysIndex,
        hashes::BlockHash,
        transactions::{BlockItem, EncodedPayload, send},
    },
    v2::{BlockIdentifier, Client},
};
use std::collections::BTreeMap;

/// Set transaction expiry to five minutes after the current UTC time.
pub(crate) fn expiry() -> TransactionTime {
    TransactionTime::from_seconds((chrono::Utc::now().timestamp() + 300) as u64)
}

/// Submit an item and return its finalized outcome without requiring success.
pub(crate) async fn submit_receipt(
    client: &mut Client,
    item: BlockItem<EncodedPayload>,
    label: &str,
) -> Result<(BlockHash, BlockItemSummary)> {
    let hash = client.send_block_item(&item).await?;
    tracing::debug!(operation = label, transaction = %hash, "submitted transaction");
    let receipt = client.wait_until_finalized(&hash).await?;
    tracing::debug!(
        operation = label,
        transaction = %hash,
        block = %receipt.0,
        "transaction finalized"
    );
    Ok(receipt)
}

/// Create a token and return its finalized outcome.
///
/// Uses the fixture's two-of-three governance threshold.
/// Concurrent callers must serialize sequence lookup through finalization.
/// Do not retry automatically after an uncertain submission outcome.
pub(crate) async fn create_token(
    client: &mut Client,
    governance: &[GeneratedKeyPair],
    request: CreatePlt,
) -> Result<(BlockHash, BlockItemSummary)> {
    let sequence = client
        .get_next_update_sequence_numbers(BlockIdentifier::LastFinal)
        .await?
        .response
        .protocol_level_tokens;
    let keys: BTreeMap<_, _> = governance
        .iter()
        .take(2)
        .map(|key| {
            (
                UpdateKeysIndex {
                    index: key.index as u16,
                },
                key.key_pair.clone(),
            )
        })
        .collect();
    let expiry = expiry();
    let instruction = updates::update::update(
        &keys,
        sequence,
        TransactionTime::from_seconds(0),
        expiry,
        UpdatePayload::CreatePlt(request),
    );
    submit_receipt(
        client,
        BlockItem::UpdateInstruction(instruction),
        "create-token",
    )
    .await
}

/// Sign token operations with the sender's next nonce and return their outcome.
pub(crate) async fn update_token(
    client: &mut Client,
    sender: &GenesisAccount,
    id: &TokenId,
    operations: TokenOperations,
    label: &str,
) -> Result<(BlockHash, BlockItemSummary)> {
    let nonce = client
        .get_next_account_sequence_number(&sender.address)
        .await?
        .nonce;
    let expiry = expiry();
    let transaction = send::token_update_operations(
        &sender.account_keys,
        sender.address,
        nonce,
        expiry,
        id.clone(),
        operations,
    );
    submit_receipt(client, BlockItem::AccountTransaction(transaction), label).await
}
