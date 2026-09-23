use crate::runner::Fixtures;
use anyhow::{ensure, Context};
use concordium_rust_sdk::{
    common::types::{Amount, TransactionTime},
    types::transactions::{send, BlockItem},
    v2::BlockIdentifier,
};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);
const TRANSFER_AMOUNT: Amount = Amount::from_micro_ccd(1_000_000);

pub async fn ccd_transfer_is_finalised(fixtures: &Fixtures) -> anyhow::Result<()> {
    // Keep exclusive access from the live nonce read through finalisation and
    // post-state checks, so no other test can submit with this account.
    let sender = fixtures.genesis.accounts[1].lock().await;
    let recipient_address = fixtures.genesis.accounts[0].lock().await.address;

    let mut client = fixtures.validator_node.connect().await?;
    let recipient_before = client
        .get_account_info(&recipient_address.into(), BlockIdentifier::Best)
        .await?
        .response
        .account_amount;

    // The sequence number is deliberately fetched immediately before signing.
    let nonce = client
        .get_next_account_sequence_number(&sender.address)
        .await?
        .nonce;
    let expiry = TransactionTime::from_seconds((chrono::Utc::now().timestamp() + 300) as u64);
    let transaction = send::transfer(
        &sender.account_keys,
        sender.address,
        nonce,
        expiry,
        recipient_address,
        TRANSFER_AMOUNT,
    );
    let hash = client
        .send_block_item(&BlockItem::AccountTransaction(transaction))
        .await?;

    tokio::time::timeout(TIMEOUT, client.wait_until_finalized(&hash))
        .await
        .context("transfers::ccd_transfer_is_finalised timed out after 30 seconds")??;

    let recipient_after = client
        .get_account_info(&recipient_address.into(), BlockIdentifier::Best)
        .await?
        .response
        .account_amount;
    ensure!(
        recipient_after == recipient_before + TRANSFER_AMOUNT,
        "recipient balance mismatch: expected {}, got {}",
        recipient_before + TRANSFER_AMOUNT,
        recipient_after
    );

    let sender_after = client
        .get_account_info(&sender.address.into(), BlockIdentifier::Best)
        .await?
        .response;
    ensure!(
        sender_after.account_nonce == nonce.next(),
        "sender sequence number did not advance by one: before {nonce}, after {}",
        sender_after.account_nonce
    );
    Ok(())
}
