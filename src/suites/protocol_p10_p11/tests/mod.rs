//! One registered direct-trait test per acceptance criterion.
mod ccd;
mod chain_agreement;
mod observer;
mod protocol;
mod rolling;
mod staking;
mod token_accounts;
mod tokens;

use super::registry::TestRegistry;
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub(super) fn register(cancellation: CancellationToken) -> Result<TestRegistry> {
    let mut registry = TestRegistry::new(cancellation);
    rolling::register(&mut registry)?;
    ccd::register(&mut registry)?;
    staking::register(&mut registry)?;
    tokens::register(&mut registry)?;
    chain_agreement::register(&mut registry)?;
    protocol::register(&mut registry)?;
    observer::register(&mut registry)?;
    Ok(registry)
}

#[cfg(test)]
mod unit_tests {
    use super::super::VALIDATOR_COUNT;
    use super::*;
    use crate::fixture::genesis::GenesisFixture;
    #[test]
    fn all_criteria_are_registered_before_execution() -> Result<()> {
        let registry = register(CancellationToken::new())?;
        assert_eq!(registry.account_count(), 53);
        let summary = registry.summary();
        assert_eq!(summary.matches("[UNPROCESSED]").count(), 53);
        for index in 0..VALIDATOR_COUNT {
            assert_eq!(summary.matches(&format!("validator {index}")).count(), 4);
        }
        assert!(!summary.contains("[INCOMPLETE]"));
        Ok(())
    }
    #[test]
    fn independent_states_take_the_complete_account_budget() -> Result<()> {
        let mut registry = register(CancellationToken::new())?;
        let mut genesis = GenesisFixture::four_validator_p10_with_accounts(u32::try_from(
            registry.account_count(),
        )?)?;
        registry.provision(genesis.accounts.split_off(VALIDATOR_COUNT))?;
        assert_eq!(registry.summary().matches("[UNPROCESSED]").count(), 53);
        Ok(())
    }
    #[test]
    fn ccd_registration_needs_no_token_or_staking_fixture() -> Result<()> {
        let mut registry = TestRegistry::new(CancellationToken::new());
        ccd::register(&mut registry)?;
        assert_eq!(registry.account_count(), 10);
        let summary = registry.summary();
        assert_eq!(summary.matches("[UNPROCESSED]").count(), 10);
        assert!(!summary.contains("token"));
        assert!(!summary.contains("staking"));
        Ok(())
    }
}
