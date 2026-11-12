//! Token scenario fixtures and evidence at exact finalized blocks.
use super::super::super::{operations, registry::TestContext, state::FinalizedBlock};
use crate::fixture::genesis::GenesisAccount;
use anyhow::{Context as _, Result};
use concordium_rust_sdk::{
    base::protocol_level_tokens::TokenId,
    id::types::AccountAddress,
    protocol_level_tokens::{AccountToken, TokenAccountState, TokenInfo},
    types::hashes::BlockHash,
    v2::Client,
};
use std::collections::BTreeMap;

pub(super) struct Fixture {
    pub(super) id: TokenId,
    pub(super) governor: GenesisAccount,
    pub(super) recipient: GenesisAccount,
}
impl Fixture {
    pub(super) fn new(accounts: Vec<GenesisAccount>, id: &str) -> Result<Self> {
        let [governor, recipient]: [GenesisAccount; 2] =
            accounts.try_into().ok().context("Expected two accounts")?;
        Ok(Self {
            id: TokenId::try_from(id.to_string())?,
            governor,
            recipient,
        })
    }
    pub(super) async fn transfer(
        &self,
        context: &TestContext<'_>,
        node: usize,
        p11: bool,
    ) -> Result<()> {
        let recipient = self.recipient.address;
        let mut client = context.validators[node].connect().await?;
        let block =
            operations::transfer_token(&mut client, &self.governor, recipient, &self.id).await?;
        if p11 {
            operations::require_p11(&mut client, block).await?;
        }
        Ok(())
    }
}

pub(super) struct Snapshot {
    pub(super) block: FinalizedBlock,
    pub(super) token: TokenInfo,
    pub(super) accounts: BTreeMap<AccountAddress, Option<TokenAccountState>>,
}
pub(super) async fn capture(context: &TestContext<'_>, id: &TokenId) -> Result<Snapshot> {
    let mut client = context.validators[0].connect().await?;
    let block = FinalizedBlock::capture(&mut client).await?;
    let token = client
        .get_token_info(id.clone(), block.hash)
        .await?
        .response;
    let accounts = collect_accounts(&mut client, context.accounts, id, block.hash).await?;
    Ok(Snapshot {
        block,
        token,
        accounts,
    })
}

/// Read each account's token state at the same block. Preserve absent token states.
pub(crate) async fn collect_accounts(
    client: &mut Client,
    addresses: &[AccountAddress],
    id: &TokenId,
    block: BlockHash,
) -> Result<BTreeMap<AccountAddress, Option<TokenAccountState>>> {
    let mut accounts = BTreeMap::new();
    for address in addresses {
        let info = client
            .get_account_info(&(*address).into(), block)
            .await?
            .response;
        accounts.insert(*address, project(info.tokens, id));
    }
    Ok(accounts)
}

pub(crate) fn project(tokens: Vec<AccountToken>, id: &TokenId) -> Option<TokenAccountState> {
    tokens
        .into_iter()
        .find(|token| &token.token_id == id)
        .map(|token| token.state)
}

/// Treat omitted coin information as explicit CCD information, without ignoring other fields.
pub(in super::super) fn token_equivalent(left: &TokenInfo, right: &TokenInfo) -> Result<bool> {
    let normalize = |token: &TokenInfo| -> Result<_> {
        let mut module = token.token_state.decode_module_state()?;
        if let Some(governor) = &mut module.governance_account {
            governor
                .coin_info
                .get_or_insert(concordium_rust_sdk::base::protocol_level_tokens::CoinInfo::CCD);
        }
        Ok(module)
    };
    Ok(left.token_id == right.token_id
        && left.token_state.token_module_ref == right.token_state.token_module_ref
        && left.token_state.decimals == right.token_state.decimals
        && left.token_state.total_supply == right.token_state.total_supply
        && normalize(left)? == normalize(right)?)
}

#[cfg(test)]
mod unit_tests {
    #[test]
    fn fixture_requires_exactly_two_accounts() -> Result<()> {
        assert!(Fixture::new(Vec::new(), "E2E").is_err());
        let mut genesis =
            crate::fixture::genesis::GenesisFixture::four_validator_p10_with_accounts(3)?;
        let mut accounts = genesis.accounts.split_off(4);
        let single = accounts.split_off(2);
        assert!(Fixture::new(single, "E2E").is_err());
        assert!(Fixture::new(accounts, "E2E").is_ok());
        Ok(())
    }

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
