//! Token preparation functions and feature-local evidence. Every criterion owns its asset.
mod criteria;
use super::super::{
    REJOIN_TIMEOUT,
    context::TestContext,
    operations,
    state::{FinalizedBlock, account_at},
    wait,
};
use super::{
    accounts_fixture::{Account, TestAccounts},
    registry::TestRegistry,
};
use anyhow::{Context as _, Result, ensure};
use concordium_rust_sdk::{
    base::protocol_level_tokens::TokenId,
    id::types::AccountAddress,
    protocol_level_tokens::{AccountToken, TokenAccountState, TokenInfo},
};
use std::collections::BTreeMap;

pub(super) fn register(registry: &mut TestRegistry) -> Result<()> {
    criteria::register(registry)
}
struct Owner {
    id: TokenId,
    governor: Account,
}
impl Owner {
    fn new(accounts: &mut TestAccounts, id: &str) -> Result<Self> {
        Ok(Self {
            id: TokenId::try_from(id.to_string())?,
            governor: accounts.take("governor")?,
        })
    }
    async fn create(&self, context: &TestContext<'_>) -> Result<()> {
        let governor = self.governor.lock().await.address;
        operations::create_token(
            &mut context.client(0).await?,
            context.governance(),
            governor,
            &self.id,
        )
        .await
    }
    async fn mint(&self, context: &TestContext<'_>) -> Result<()> {
        operations::mint(&mut context.client(0).await?, &self.governor, &self.id).await
    }
}
struct Fixture {
    owner: Owner,
    recipient: Account,
}
impl Fixture {
    fn new(accounts: &mut TestAccounts, id: &str) -> Result<Self> {
        Ok(Self {
            owner: Owner::new(accounts, id)?,
            recipient: accounts.take("recipient")?,
        })
    }
    async fn prepare(&self, context: &TestContext<'_>) -> Result<()> {
        prepare_token(
            context,
            &self.owner.id,
            &self.owner.governor,
            &self.recipient,
        )
        .await
    }
    async fn transfer(&self, context: &TestContext<'_>, node: usize, p11: bool) -> Result<()> {
        let recipient = self.recipient.lock().await.address;
        let mut client = context.client(node).await?;
        let block = operations::transfer_token(
            &mut client,
            &self.owner.governor,
            recipient,
            &self.owner.id,
        )
        .await?;
        if p11 {
            operations::require_p11(&mut client, block).await?;
        }
        Ok(())
    }
}
/// Reuse operations, never another criterion's accounts or asset.
pub(super) async fn prepare_token(
    context: &TestContext<'_>,
    id: &TokenId,
    governor: &Account,
    recipient: &Account,
) -> Result<()> {
    let address = governor.lock().await.address;
    let recipient = recipient.lock().await.address;
    let mut new = context.client(0).await?;
    operations::create_token(&mut new, context.governance(), address, id).await?;
    operations::transfer_token(&mut context.client(1).await?, governor, recipient, id).await?;
    Ok(())
}
struct Configuration {
    block: FinalizedBlock,
    token: TokenInfo,
}
async fn configuration(context: &TestContext<'_>, id: &TokenId) -> Result<Configuration> {
    let mut client = context.client(0).await?;
    let block = FinalizedBlock::capture(&mut client).await?;
    let token = client
        .get_token_info(id.clone(), block.hash)
        .await?
        .response;
    Ok(Configuration { block, token })
}
struct Snapshot {
    configuration: Configuration,
    accounts: BTreeMap<AccountAddress, Option<TokenAccountState>>,
}
async fn capture(context: &TestContext<'_>, id: &TokenId) -> Result<Snapshot> {
    let configuration = configuration(context, id).await?;
    let mut client = context.client(0).await?;
    let mut accounts = BTreeMap::new();
    for address in context.accounts() {
        let info = account_at(&mut client, *address, configuration.block.hash).await?;
        accounts.insert(*address, project(info.tokens, id));
    }
    Ok(Snapshot {
        configuration,
        accounts,
    })
}
fn project(tokens: Vec<AccountToken>, id: &TokenId) -> Option<TokenAccountState> {
    tokens
        .into_iter()
        .find(|token| &token.token_id == id)
        .map(|token| token.state)
}
/// Treat omitted coin information as explicit CCD information, without ignoring other fields.
fn token_equivalent(left: &TokenInfo, right: &TokenInfo) -> Result<bool> {
    let normalize = |token: &TokenInfo| -> Result<_> {
        let mut module = token.token_state.decode_module_state()?;
        if let Some(governor) = &mut module.governance_account {
            governor.coin_info =
                Some(concordium_rust_sdk::base::protocol_level_tokens::CoinInfo::CCD);
        }
        Ok(module)
    };
    Ok(left.token_id == right.token_id
        && left.token_state.token_module_ref == right.token_state.token_module_ref
        && left.token_state.decimals == right.token_state.decimals
        && left.token_state.total_supply == right.token_state.total_supply
        && normalize(left)? == normalize(right)?)
}
async fn compare(context: &TestContext<'_>, snapshot: &Snapshot, observer: bool) -> Result<()> {
    let reference = &snapshot.configuration;
    for node in context.node_indices(observer) {
        let mut client = context.client(node).await?;
        ensure!(
            client
                .get_block_info(reference.block.hash)
                .await?
                .response
                .finalized,
            "Node {node} has not finalized token evidence"
        );
        let token = client
            .get_token_info(reference.token.token_id.clone(), reference.block.hash)
            .await?
            .response;
        ensure!(
            token_equivalent(&token, &reference.token)?,
            "Node {node} disagrees on token configuration"
        );
        for (address, expected) in &snapshot.accounts {
            let info = account_at(&mut client, *address, reference.block.hash).await?;
            ensure!(
                project(info.tokens, &reference.token.token_id).as_ref() == expected.as_ref(),
                "Node {node} disagrees on token account {address}"
            );
        }
    }
    Ok(())
}
async fn agreement(
    context: &TestContext<'_>,
    id: &TokenId,
    reference: Option<&Snapshot>,
    observer: bool,
) -> Result<()> {
    let current;
    let snapshot = match reference {
        Some(saved) => saved,
        None => {
            current = wait::bounded(
                "capture token agreement evidence",
                REJOIN_TIMEOUT,
                capture(context, id),
            )
            .await?;
            &current
        }
    };
    wait::agreement(|| compare(context, snapshot, observer)).await
}
#[cfg(test)]
mod unit_tests {
    use super::*;
    use concordium_rust_sdk::{
        base::{
            common::cbor::cbor_encode,
            protocol_level_tokens::{CborHolderAccount, CoinInfo, TokenAmount, TokenModuleState},
        },
        protocol_level_tokens::TokenState,
    };
    #[test]
    fn comparison_normalizes_coin_info_but_preserves_supply_and_governor() -> Result<()> {
        let mut module = TokenModuleState {
            name: Some("E2E".into()),
            metadata: None,
            governance_account: Some(CborHolderAccount {
                address: AccountAddress([1; 32]),
                coin_info: None,
            }),
            allow_list: Some(false),
            deny_list: Some(false),
            mintable: Some(true),
            burnable: Some(true),
            paused: Some(false),
        };
        let old = TokenInfo {
            token_id: TokenId::try_from("E2E-NORMALIZE".to_string())?,
            token_state: TokenState {
                token_module_ref: crate::suites::protocol_p10_p11::payload::TOKEN_MODULE.parse()?,
                decimals: 0,
                total_supply: TokenAmount::from_raw(1000, 0),
                module_state: cbor_encode(&module).into(),
            },
        };
        let mut new = old.clone();
        module.governance_account.as_mut().unwrap().coin_info = Some(CoinInfo::CCD);
        new.token_state.module_state = cbor_encode(&module).into();
        assert!(token_equivalent(&old, &new)?);
        new.token_state.total_supply = TokenAmount::from_raw(1001, 0);
        assert!(!token_equivalent(&old, &new)?);
        new.token_state.total_supply = old.token_state.total_supply;
        module.governance_account.as_mut().unwrap().address = AccountAddress([2; 32]);
        new.token_state.module_state = cbor_encode(&module).into();
        assert!(!token_equivalent(&old, &new)?);
        Ok(())
    }
}
