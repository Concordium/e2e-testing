//! Token-account preservation with its own asset, signers and finalized baseline.
use super::super::registry::{ProtocolUpgradeTest, TestProgress};
use super::super::{REJOIN_TIMEOUT, context::TestContext, state::FinalizedBlock, wait::bounded};
use super::tokens::{collect_accounts, prepare_token};
use crate::fixture::genesis::GenesisAccount;
use anyhow::{Context as _, Result, ensure};
use concordium_rust_sdk::{
    base::protocol_level_tokens::{TokenAmount, TokenId},
    id::types::AccountAddress,
    protocol_level_tokens::TokenAccountState,
};
use std::collections::BTreeMap;

const TOKEN_ID: &str = "E2E-ACCOUNTS";
type TokenAccounts = BTreeMap<AccountAddress, Option<TokenAccountState>>;
struct Snapshot {
    block: FinalizedBlock,
    accounts: TokenAccounts,
}

pub(super) struct TokenPreservation {
    id: TokenId,
    governor: GenesisAccount,
    recipient: GenesisAccount,
    before: Option<Snapshot>,
}
impl ProtocolUpgradeTest for TokenPreservation {
    fn name() -> String {
        "Token account balances and module state survive activation".into()
    }
    fn required_accounts() -> usize {
        2
    }

    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        let [governor, recipient]: [GenesisAccount; 2] =
            accounts.try_into().ok().context("Expected 2 accounts")?;
        Ok(Self {
            id: TokenId::try_from(TOKEN_ID.to_string())?,
            governor,
            recipient,
            before: None,
        })
    }

    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        prepare_token(context, &self.id, &self.governor, &self.recipient).await?;
        Ok(TestProgress::Pending)
    }

    async fn on_before_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let before = bounded(
            "capture independent token accounts",
            REJOIN_TIMEOUT,
            capture(context, &self.id),
        )
        .await?;
        let governor = self.governor.address;
        let recipient = self.recipient.address;
        ensure!(
            balance(&before.accounts, &governor)? == TokenAmount::from_raw(990, 0),
            "Governor preparation balance is incorrect"
        );
        ensure!(
            balance(&before.accounts, &recipient)? == TokenAmount::from_raw(10, 0),
            "Recipient preparation balance is incorrect"
        );
        self.before = Some(before);
        Ok(TestProgress::Pending)
    }

    async fn on_after_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        bounded(
            "verify independent token preservation",
            REJOIN_TIMEOUT,
            async {
                let before = self
                    .before
                    .as_ref()
                    .context("No independent token baseline")?;
                let after = capture(context, &self.id).await?;
                ensure!(
                    after.block.height >= before.block.height,
                    "Finalized height regressed"
                );
                unchanged(&before.accounts, &after.accounts)
            },
        )
        .await?;
        Ok(TestProgress::Passed)
    }
}

async fn capture(context: &TestContext<'_>, id: &TokenId) -> Result<Snapshot> {
    let mut client = context.client(0).await?;
    let block = FinalizedBlock::capture(&mut client).await?;
    // An absent token must not produce a vacuously passing map of empty account states.
    client.get_token_info(id.clone(), block.hash).await?;
    let accounts = collect_accounts(&mut client, context.accounts(), id, block.hash).await?;
    Ok(Snapshot { block, accounts })
}

fn balance(accounts: &TokenAccounts, address: &AccountAddress) -> Result<TokenAmount> {
    Ok(accounts
        .get(address)
        .and_then(Option::as_ref)
        .context("Prepared account has no token state")?
        .balance)
}

fn unchanged(before: &TokenAccounts, after: &TokenAccounts) -> Result<()> {
    ensure!(
        before == after,
        "Token account balances or module state changed across activation"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tokens::project;
    use super::*;
    use concordium_rust_sdk::protocol_level_tokens::AccountToken;
    fn token(id: &str, amount: u64) -> Result<AccountToken> {
        Ok(AccountToken {
            token_id: TokenId::try_from(id.to_string())?,
            state: TokenAccountState {
                balance: TokenAmount::from_raw(amount, 0),
                module_state: None,
            },
        })
    }
    #[test]
    fn evidence_ignores_another_tests_token() -> Result<()> {
        let id = TokenId::try_from(TOKEN_ID.to_string())?;
        let own = token(TOKEN_ID, 10)?;
        let before = project(vec![token("E2E", 100)?, own.clone()], &id);
        let after = project(vec![token("E2E", 900)?, own.clone()], &id);
        assert_eq!(before, after);
        assert_eq!(before, Some(own.state));
        assert!(project(vec![token("E2E", 100)?], &id).is_none());
        Ok(())
    }
    #[test]
    fn changes_to_own_balance_module_state_and_tracked_accounts_fail() -> Result<()> {
        let address = AccountAddress([1; 32]);
        let before = BTreeMap::from([(address, Some(token(TOKEN_ID, 10)?.state))]);
        unchanged(&before, &before)?;
        let mut after = before.clone();
        after.get_mut(&address).unwrap().as_mut().unwrap().balance = TokenAmount::from_raw(9, 0);
        assert!(unchanged(&before, &after).is_err());
        let mut after = before.clone();
        after
            .get_mut(&address)
            .unwrap()
            .as_mut()
            .unwrap()
            .module_state = Some(vec![0xf6].into());
        assert!(unchanged(&before, &after).is_err());
        assert!(unchanged(&before, &BTreeMap::new()).is_err());
        Ok(())
    }
    #[test]
    fn baseline_requires_real_holder_state() {
        assert!(balance(&BTreeMap::new(), &AccountAddress([1; 32])).is_err());
    }
}
