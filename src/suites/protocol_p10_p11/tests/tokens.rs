//! Token creation, minting, transfers, and preservation across the update.
pub(super) mod support;

use super::super::{
    REJOIN_TIMEOUT,
    lifecycle::{ProtocolUpgradeTest, TestContext, TestProgress, UpgradeRegistry},
    operations, wait,
};
use crate::fixture::genesis::GenesisAccount;
use anyhow::{Context as _, Result, ensure};
use concordium_rust_sdk::{
    id::types::AccountAddress,
    protocol_level_tokens::{TokenAccountState, TokenAmount},
};
use std::collections::BTreeMap;
use support::{Fixture, Snapshot, capture, token_equivalent};

pub(super) fn register(registry: &mut UpgradeRegistry) {
    registry.group("Tokens").register(
        "Token creation, minting, transfers, and preservation across P11",
        Box::new(TokenContinuity::default()),
    );
}

#[derive(Default)]
struct TokenContinuity {
    fixture: Option<Fixture>,
    before: Option<Snapshot>,
}

#[async_trait::async_trait(?Send)]
impl ProtocolUpgradeTest for TokenContinuity {
    fn required_accounts(&self) -> usize {
        2
    }

    async fn prepare_before_update(
        &mut self,
        _context: &TestContext<'_>,
        accounts: Vec<GenesisAccount>,
    ) -> Result<()> {
        self.fixture = Some(Fixture::new(accounts, "E2E-CONTINUITY")?);
        Ok(())
    }

    async fn run_before_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let fixture = self
            .fixture
            .as_ref()
            .context("Token fixture not prepared")?;
        let mut client = context.validators[0].connect().await?;
        operations::create_token(
            &mut client,
            context.governance,
            fixture.governor.address,
            &fixture.id,
        )
        .await?;
        operations::mint(&mut client, &fixture.governor, &fixture.id).await?;
        fixture.transfer(context, 1, false).await?;
        Ok(TestProgress::Pending)
    }

    async fn finish_before_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let fixture = self
            .fixture
            .as_ref()
            .context("Token fixture not prepared")?;
        let before = wait::bounded(
            "capture token baseline",
            REJOIN_TIMEOUT,
            capture(context, &fixture.id),
        )
        .await?;
        let governor_balance = balance(&before.accounts, &fixture.governor.address)?;
        let recipient_balance = before
            .accounts
            .get(&fixture.recipient.address)
            .and_then(Option::as_ref)
            .map(|state| state.balance);
        ensure!(
            governor_balance == TokenAmount::from_raw(1090, 0),
            "Governor preparation balance is incorrect: expected {:?}, got {:?}; \
             snapshot block {} at height {}; recipient balance {:?}; total supply {:?}",
            TokenAmount::from_raw(1090, 0),
            governor_balance,
            before.block.hash,
            before.block.height,
            recipient_balance,
            before.token.token_state.total_supply
        );
        ensure!(
            balance(&before.accounts, &fixture.recipient.address)? == TokenAmount::from_raw(10, 0),
            "Recipient preparation balance is incorrect"
        );
        self.before = Some(before);
        Ok(TestProgress::Pending)
    }

    async fn prepare_after_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let fixture = self
            .fixture
            .as_ref()
            .context("Token fixture not prepared")?;
        let before = self.before.as_ref().context("No token baseline")?;
        let after = wait::bounded(
            "capture preserved token state",
            REJOIN_TIMEOUT,
            capture(context, &fixture.id),
        )
        .await?;
        ensure!(
            after.block.height >= before.block.height,
            "Finalized height regressed"
        );
        ensure!(
            token_equivalent(&before.token, &after.token)?,
            "Token configuration or supply changed across activation"
        );
        ensure!(
            before.accounts == after.accounts,
            "Token account balances or module state changed across activation"
        );
        Ok(TestProgress::Pending)
    }

    async fn run_after_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.fixture
            .as_ref()
            .context("Token fixture not prepared")?
            .transfer(context, 0, true)
            .await?;
        Ok(TestProgress::Passed)
    }
}

fn balance(
    accounts: &BTreeMap<AccountAddress, Option<TokenAccountState>>,
    address: &AccountAddress,
) -> Result<TokenAmount> {
    Ok(accounts
        .get(address)
        .and_then(Option::as_ref)
        .context("Prepared account has no token state")?
        .balance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use concordium_rust_sdk::protocol_level_tokens::{AccountToken, TokenId};

    #[test]
    fn evidence_ignores_another_tests_token() -> Result<()> {
        let id = TokenId::try_from("E2E-ACCOUNTS".to_string())?;
        let token = |id: TokenId, amount| AccountToken {
            token_id: id,
            state: TokenAccountState {
                balance: TokenAmount::from_raw(amount, 0),
                module_state: None,
            },
        };
        let other = TokenId::try_from("E2E".to_string())?;
        let own = token(id.clone(), 10);
        let before = support::project(vec![token(other.clone(), 100), own.clone()], &id);
        let after = support::project(vec![token(other.clone(), 900), own.clone()], &id);
        assert_eq!(before, after);
        assert_eq!(before, Some(own.state));
        assert!(support::project(vec![token(other, 100)], &id).is_none());
        Ok(())
    }

    #[test]
    fn baseline_requires_real_holder_state() {
        assert!(balance(&BTreeMap::new(), &AccountAddress([1; 32])).is_err());
    }
}
