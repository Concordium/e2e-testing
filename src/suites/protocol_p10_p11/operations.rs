//! Small finalized operations used before, during and after upgrades.
use super::{TRANSACTION_TIMEOUT, payload};
use crate::fixture::genesis::LockedAccount;
use anyhow::{Context, Result, ensure};
use concordium_rust_sdk::{
    base::{
        common::cbor::cbor_encode,
        protocol_level_tokens::{
            CborHolderAccount, CoinInfo, MetadataUrl, TokenAmount, TokenId,
            TokenModuleInitializationParameters, operations,
        },
        updates::{self, CreatePlt, UpdatePayload},
    },
    common::types::{Amount, TransactionTime},
    genesis::output::GeneratedKeyPair,
    id::types::AccountAddress,
    types::{
        DelegationTarget, UpdateKeysIndex, UpdateSequenceNumber,
        hashes::BlockHash,
        transactions::{BlockItem, ConfigureDelegationPayload, EncodedPayload, send},
    },
    v2::{BlockIdentifier, Client},
};
use std::collections::BTreeMap;

/// Set the transaction expiry to five minutes after the current UTC time.
pub fn expiry() -> TransactionTime {
    TransactionTime::from_seconds((chrono::Utc::now().timestamp() + 300) as u64)
}

/// Submit an item and require a successful finalized result. Return its block hash.
pub async fn submit(
    client: &mut Client,
    item: BlockItem<EncodedPayload>,
    label: &str,
) -> Result<BlockHash> {
    Ok(submit_receipt(client, item, label).await?.0)
}

/// Record the transaction hash before waiting. Reject failed or unknown finalized results.
async fn submit_receipt(
    client: &mut Client,
    item: BlockItem<EncodedPayload>,
    label: &str,
) -> Result<(BlockHash, concordium_rust_sdk::types::BlockItemSummary)> {
    let hash = client.send_block_item(&item).await?;
    tracing::info!(operation = label, transaction = %hash, "submitted transaction");
    let (block, summary) =
        tokio::time::timeout(TRANSACTION_TIMEOUT, client.wait_until_finalized(&hash))
            .await
            .with_context(|| {
                format!(
                    "{label}: transaction {hash} not finalized within {} seconds",
                    TRANSACTION_TIMEOUT.as_secs()
                )
            })??;
    ensure!(
        summary.is_success().as_known() == Some(&true),
        "{label}: unsuccessful or unknown outcome: {summary:?}"
    );
    tracing::info!(operation = label, transaction = %hash, block = %block, "transaction finalized successfully");
    Ok((block, summary))
}

/// Transfer one CCD between regular accounts. Check balances, fees, rewards, and the sender nonce.
pub async fn ccd(
    client: &mut Client,
    sender_account: &LockedAccount,
    recipient: AccountAddress,
) -> Result<BlockHash> {
    let sender = sender_account.lock().await;
    let nonce = client
        .get_next_account_sequence_number(&sender.address)
        .await?
        .nonce;
    let amount = Amount::from_micro_ccd(1_000_000);
    let item = BlockItem::AccountTransaction(send::transfer(
        &sender.account_keys,
        sender.address,
        nonce,
        expiry(),
        recipient,
        amount,
    ));
    let (block, summary) = submit_receipt(client, item, "ccd-transfer").await?;
    let info = client.get_block_info(block).await?.response;
    let before = client
        .get_account_info(&recipient.into(), info.block_parent)
        .await?
        .response;
    let sender_before = client
        .get_account_info(&sender.address.into(), info.block_parent)
        .await?
        .response;
    let after = client
        .get_account_info(&recipient.into(), block)
        .await?
        .response;
    // Payday rewards can coincide with the transfer block.
    let rewards = super::state::rewards_in_block(client, block).await?;
    let reward = rewards.get(&recipient).copied().unwrap_or(Amount::zero());
    ensure!(
        after.account_amount == before.account_amount + amount + reward,
        "CCD recipient delta incorrect"
    );
    let sender_after = client
        .get_account_info(&sender.address.into(), block)
        .await?
        .response;
    ensure!(
        sender_after.account_nonce == nonce.next(),
        "CCD sender nonce did not advance"
    );
    let fee = summary
        .details
        .as_known()
        .and_then(|details| details.as_account_transaction())
        .context("CCD transfer has no account transaction receipt")?
        .cost;
    let sender_reward = rewards
        .get(&sender.address)
        .copied()
        .unwrap_or(Amount::zero());
    ensure!(
        sender_after.account_amount == sender_before.account_amount + sender_reward - amount - fee,
        "CCD sender balance delta does not match amount and fee"
    );
    Ok(block)
}

/// Require the operation's finalized block to use P11.
pub async fn require_p11(client: &mut Client, block: BlockHash) -> Result<()> {
    let info = client.get_block_info(block).await?.response;
    ensure!(
        info.finalized && info.protocol_version.0 == 11,
        "Post-update operation did not finalize at P11"
    );
    Ok(())
}

/// Sign with two genesis level-2 keys and submit an update.
/// Set its timeout before its effective time, unless the update is immediate.
pub async fn governance(
    client: &mut Client,
    governance_keys: &[GeneratedKeyPair],
    sequence: UpdateSequenceNumber,
    effective: TransactionTime,
    payload: UpdatePayload,
    label: &str,
) -> Result<BlockHash> {
    let keys: BTreeMap<_, _> = governance_keys
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
    // Non-immediate governance updates require timeout before effective time.
    let timeout = if effective.seconds == 0 {
        expiry()
    } else {
        TransactionTime::from_seconds(
            effective
                .seconds
                .checked_sub(1)
                .context("Invalid effective time")?,
        )
    };
    let instruction = updates::update::update(&keys, sequence, effective, timeout, payload);
    submit(client, BlockItem::UpdateInstruction(instruction), label).await
}

/// Create 1,000 units of the requested token for the test's governor account.
pub async fn create_token(
    client: &mut Client,
    governance_keys: &[GeneratedKeyPair],
    governor: AccountAddress,
    id: &TokenId,
) -> Result<()> {
    let parameters = TokenModuleInitializationParameters {
        name: Some("E2E continuity token".into()),
        metadata: Some(MetadataUrl {
            url: "https://example.invalid/e2e-token.json".into(),
            checksum_sha_256: None,
            additional: Default::default(),
        }),
        governance_account: Some(CborHolderAccount {
            address: governor,
            coin_info: Some(CoinInfo::CCD),
        }),
        allow_list: Some(false),
        deny_list: Some(false),
        initial_supply: Some(TokenAmount::from_raw(1000, 0)),
        mintable: Some(true),
        burnable: Some(true),
    };
    let sequence = client
        .get_next_update_sequence_numbers(BlockIdentifier::LastFinal)
        .await?
        .response
        .protocol_level_tokens;
    governance(
        client,
        governance_keys,
        sequence,
        TransactionTime::from_seconds(0),
        UpdatePayload::CreatePlt(CreatePlt {
            token_id: id.clone(),
            token_module: payload::TOKEN_MODULE.parse()?,
            decimals: 0,
            initialization_parameters: cbor_encode(&parameters).into(),
        }),
        "create-token",
    )
    .await?;
    Ok(())
}

/// Mint 100 units of the requested token and check the increase in supply.
pub async fn mint(client: &mut Client, sender_account: &LockedAccount, id: &TokenId) -> Result<()> {
    let sender = sender_account.lock().await;
    let nonce = client
        .get_next_account_sequence_number(&sender.address)
        .await?
        .nonce;
    let item = send::token_update_operations(
        &sender.account_keys,
        sender.address,
        nonce,
        expiry(),
        id.clone(),
        [operations::mint_tokens(TokenAmount::from_raw(100, 0))]
            .into_iter()
            .collect(),
    );
    let block = submit(client, BlockItem::AccountTransaction(item), "mint-token").await?;
    let parent = client.get_block_info(block).await?.response.block_parent;
    let before = client.get_token_info(id.clone(), parent).await?.response;
    let after = client.get_token_info(id.clone(), block).await?.response;
    ensure!(
        after.token_state.total_supply
            == TokenAmount::from_raw(before.token_state.total_supply.value() + 100, 0),
        "Mint supply delta incorrect"
    );
    Ok(())
}

/// Transfer 10 units of the requested token. Check balances and unchanged supply.
pub async fn transfer_token(
    client: &mut Client,
    sender_account: &LockedAccount,
    recipient: AccountAddress,
    id: &TokenId,
) -> Result<BlockHash> {
    let sender = sender_account.lock().await;
    let nonce = client
        .get_next_account_sequence_number(&sender.address)
        .await?
        .nonce;
    let amount = TokenAmount::from_raw(10, 0);
    let item = send::token_update_operations(
        &sender.account_keys,
        sender.address,
        nonce,
        expiry(),
        id.clone(),
        [operations::transfer_tokens(recipient, amount)]
            .into_iter()
            .collect(),
    );
    let block = submit(
        client,
        BlockItem::AccountTransaction(item),
        "token-transfer",
    )
    .await?;
    let info = client.get_block_info(block).await?.response;
    let before = client
        .get_account_info(&recipient.into(), info.block_parent)
        .await?
        .response
        .token_amount(id);
    let after = client
        .get_account_info(&recipient.into(), block)
        .await?
        .response
        .token_amount(id);
    let before = before.unwrap_or(TokenAmount::from_raw(0, 0));
    ensure!(
        after == Some(TokenAmount::from_raw(before.value() + 10, 0)),
        "Token recipient delta incorrect"
    );
    let sender_before = client
        .get_account_info(&sender.address.into(), info.block_parent)
        .await?
        .response
        .token_amount(id)
        .context("Sender has no tokens")?;
    let sender_after = client
        .get_account_info(&sender.address.into(), block)
        .await?
        .response
        .token_amount(id)
        .context("Sender lost token entry")?;
    ensure!(
        sender_after
            == TokenAmount::from_raw(
                sender_before
                    .value()
                    .checked_sub(10)
                    .context("Insufficient test tokens")?,
                0
            ),
        "Token sender delta incorrect"
    );
    ensure!(
        client
            .get_token_info(id.clone(), info.block_parent)
            .await?
            .response
            .token_state
            .total_supply
            == client
                .get_token_info(id.clone(), block)
                .await?
                .response
                .token_state
                .total_supply,
        "Transfer changed token supply"
    );
    Ok(block)
}

/// Configure the test's delegator to delegate 1,000 CCD to passive delegation.
/// Disable automatic staking of rewards and check the finalized configuration.
pub async fn delegate(client: &mut Client, sender_account: &LockedAccount) -> Result<()> {
    let sender = sender_account.lock().await;
    let nonce = client
        .get_next_account_sequence_number(&sender.address)
        .await?
        .nonce;
    let item = send::configure_delegation(
        &sender.account_keys,
        sender.address,
        nonce,
        expiry(),
        ConfigureDelegationPayload {
            capital: Some(Amount::from_micro_ccd(1_000_000_000)),
            restake_earnings: Some(false),
            delegation_target: Some(DelegationTarget::Passive),
        },
    );
    let block = submit(
        client,
        BlockItem::AccountTransaction(item),
        "passive-delegation",
    )
    .await?;
    let info = client
        .get_account_info(&sender.address.into(), block)
        .await?
        .response;
    ensure!(
        matches!(info.account_stake.as_ref().and_then(|stake| stake.as_known()),
        Some(concordium_rust_sdk::types::AccountStakingInfo::Delegated { staked_amount, restake_earnings: false, delegation_target: DelegationTarget::Passive, .. })
        if *staked_amount == Amount::from_micro_ccd(1_000_000_000)),
        "Passive delegation configuration incorrect"
    );
    Ok(())
}
