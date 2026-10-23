//! Private signing accounts assigned to one test before containers start.
use crate::fixture::genesis::LockedAccount;
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;

pub(super) type Account = LockedAccount;

pub(super) struct TestAccounts {
    accounts: BTreeMap<&'static str, Account>,
}
impl TestAccounts {
    pub fn new(roles: &[&'static str], accounts: Vec<Account>) -> Result<Self> {
        ensure!(
            roles.len() == accounts.len(),
            "Account allocation size mismatch"
        );
        let mut assigned = BTreeMap::new();
        for (role, account) in roles.iter().copied().zip(accounts) {
            ensure!(
                assigned.insert(role, account).is_none(),
                "Duplicate account role: {role}"
            );
        }
        Ok(Self { accounts: assigned })
    }
    pub fn take(&mut self, role: &str) -> Result<Account> {
        self.accounts
            .remove(role)
            .with_context(|| format!("Unknown or already taken test account role: {role}"))
    }
    pub fn all_taken(&self) -> bool {
        self.accounts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::VALIDATOR_COUNT;
    use super::*;
    use crate::fixture::genesis::GenesisFixture;
    #[test]
    fn role_allocations_are_exclusive_and_single_use() -> Result<()> {
        let mut genesis = GenesisFixture::four_validator_p10_with_accounts(4)?;
        let mut first = genesis.accounts.split_off(VALIDATOR_COUNT);
        let second = first.split_off(2);
        let mut first = TestAccounts::new(&["sender", "recipient"], first)?;
        let mut second = TestAccounts::new(&["governor", "recipient"], second)?;
        assert!(first.take("governor").is_err());
        assert!(second.take("sender").is_err());
        let sender = first.take("sender")?;
        let recipient = first.take("recipient")?;
        let governor = second.take("governor")?;
        let other_recipient = second.take("recipient")?;
        let addresses = [
            sender.try_lock().unwrap().address,
            recipient.try_lock().unwrap().address,
            governor.try_lock().unwrap().address,
            other_recipient.try_lock().unwrap().address,
        ];
        assert_eq!(
            addresses
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            4
        );
        assert!(first.take("sender").is_err());
        assert!(first.all_taken() && second.all_taken());
        Ok(())
    }
    #[test]
    fn incorrect_or_duplicate_role_allocations_are_rejected() -> Result<()> {
        assert!(TestAccounts::new(&["sender"], Vec::new()).is_err());
        let mut genesis = GenesisFixture::four_validator_p10_with_accounts(2)?;
        assert!(
            TestAccounts::new(
                &["sender", "sender"],
                genesis.accounts.split_off(VALIDATOR_COUNT)
            )
            .is_err()
        );
        Ok(())
    }
}
