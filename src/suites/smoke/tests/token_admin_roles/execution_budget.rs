//! Reject admin operations with exhausted energy or malformed input.

use super::super::super::{Test, TestContext};
use super::support::create_mint_burn_token;
use super::support::metadata;
use crate::fixture::genesis::GenesisAccount;
use crate::operations;
use crate::registry::TestGroup;
use anyhow::Context;
use anyhow::Result;
use anyhow::ensure;
use concordium_rust_sdk::base::common::cbor::cbor_encode;
use concordium_rust_sdk::base::protocol_level_tokens::TokenAdminRole;
use concordium_rust_sdk::base::protocol_level_tokens::TokenId;
use concordium_rust_sdk::base::protocol_level_tokens::TokenOperation;
use concordium_rust_sdk::base::protocol_level_tokens::TokenOperations;
use concordium_rust_sdk::base::protocol_level_tokens::TokenOperationsPayload;
use concordium_rust_sdk::base::protocol_level_tokens::operations as token_operations;
use concordium_rust_sdk::types::BlockItemSummary;
use concordium_rust_sdk::types::Energy;
use concordium_rust_sdk::types::transactions::BlockItem;
use concordium_rust_sdk::types::transactions::Payload;
use concordium_rust_sdk::types::transactions::construct;
use concordium_rust_sdk::v2::BlockIdentifier;
use concordium_rust_sdk::v2::Client;

pub(super) fn register(group: &mut TestGroup<'_, Test>) {
    group.register(
        "Admin operations with exhausted energy or malformed input are rejected",
        Test {
            required_accounts: 2,
            run: Box::new(|context, accounts| {
                Box::pin(
                    reject_admin_operations_that_exhaust_energy_or_contain_malformed_input(
                        context, accounts,
                    ),
                )
            }),
        },
    );
}

async fn reject_admin_operations_that_exhaust_energy_or_contain_malformed_input(
    context: &TestContext,
    accounts: Vec<GenesisAccount>,
) -> Result<()> {
    let governance_account = &accounts[0];
    let recipient = &accounts[1];
    let mut client = context.validator_node.connect().await?;
    let (id, _) =
        create_mint_burn_token(&mut client, context, governance_account, "E2E-RBAC-BUDGET").await?;
    reject_operation_with_exhausted_energy(
        &mut client,
        governance_account,
        &id,
        token_operations::assign_admin_roles(recipient.address, vec![TokenAdminRole::Mint]),
    )
    .await
    .context("Admin-role assignment with exhausted execution energy must be rejected")?;

    reject_operation_with_exhausted_energy(
        &mut client,
        governance_account,
        &id,
        token_operations::revoke_admin_roles(
            governance_account.address,
            vec![TokenAdminRole::Mint],
        ),
    )
    .await
    .context("Admin-role revocation with exhausted execution energy must be rejected")?;

    reject_operation_with_exhausted_energy(
        &mut client,
        governance_account,
        &id,
        token_operations::update_metadata(metadata("https://example.invalid/budget")),
    )
    .await
    .context("Metadata update with exhausted execution energy must be rejected")?;

    // A break marker cannot be a valid top-level operations value.
    rejected_payload(&mut client, governance_account, &id, vec![0xff], 10_000)
        .await
        .context("Malformed admin-operation payload must be rejected")?;
    Ok(())
}

/// Require a valid operation to exhaust its execution allowance and be rejected.
async fn reject_operation_with_exhausted_energy(
    client: &mut Client,
    sender: &GenesisAccount,
    id: &TokenId,
    operation: TokenOperation,
) -> Result<()> {
    let operations: TokenOperations = [operation].into_iter().collect();
    let (budget, summary) =
        rejected_payload(client, sender, id, cbor_encode(&operations), 0).await?;
    ensure!(
        summary.energy_cost == budget,
        "Exhausted execution did not consume its budget"
    );
    Ok(())
}

/// Require charged rejection and unchanged token state; return budget and receipt.
/// `execution_energy` is added to the transaction's base energy allowance.
async fn rejected_payload(
    client: &mut Client,
    sender: &GenesisAccount,
    id: &TokenId,
    bytes: Vec<u8>,
    execution_energy: u64,
) -> Result<(Energy, BlockItemSummary)> {
    let baseline = client
        .get_block_info(BlockIdentifier::LastFinal)
        .await?
        .response
        .block_hash;
    let before = client
        .get_token_info(id.clone(), baseline)
        .await?
        .response;
    let authorizations_before = client
        .get_token_authorizations(baseline, id.clone())
        .await?;
    let nonce = client
        .get_next_account_sequence_number(&sender.address)
        .await?
        .nonce;
    let transaction = construct::make_transaction(
        sender.address,
        nonce,
        operations::expiry(),
        construct::GivenEnergy::Add {
            num_sigs: 1,
            energy: Energy {
                energy: execution_energy,
            },
        },
        Payload::TokenUpdate {
            payload: TokenOperationsPayload {
                token_id: id.clone(),
                operations: bytes.into(),
            },
        },
    )
    .sign(&sender.account_keys);
    let budget = transaction.header.energy_amount;
    let (block, summary) = operations::submit_receipt(
        client,
        BlockItem::AccountTransaction(transaction),
        "rejected-payload",
    )
    .await?;
    ensure!(
        summary.is_success().as_known() == Some(&false),
        "Expected finalized rejected execution: {summary:?}"
    );
    ensure!(
        summary.energy_cost.energy > 0 && summary.energy_cost <= budget,
        "Incorrect charged energy"
    );
    let after = client.get_token_info(id.clone(), block).await?.response;
    ensure!(
        before.token_state.total_supply == after.token_state.total_supply
            && before.token_state.module_state == after.token_state.module_state,
        "Rejected standalone payload changed state"
    );
    ensure!(
        cbor_encode(&authorizations_before)
            == cbor_encode(&client.get_token_authorizations(block, id.clone()).await?),
        "Rejected payload changed token authorizations"
    );
    Ok((budget, summary))
}
