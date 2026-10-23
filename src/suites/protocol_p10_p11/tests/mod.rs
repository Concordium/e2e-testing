//! One registered direct-trait test per acceptance criterion.
mod accounts_fixture;
mod balance_preservation;
mod ccd;
mod chain_agreement;
mod observer;
mod protocol;
pub(super) mod registry;
mod rolling;
mod staking;
mod token_accounts;
mod tokens;
mod upgrade_test;

use super::cancellation::Cancellation;
use anyhow::Result;
use registry::TestRegistry;

pub(super) fn register(cancellation: Cancellation) -> Result<TestRegistry> {
    let mut registry = register_common(cancellation)?;
    tokens::register(&mut registry)?;
    register_network_results(&mut registry)?;
    Ok(registry)
}
fn register_common(cancellation: Cancellation) -> Result<TestRegistry> {
    let mut registry = TestRegistry::new(cancellation);
    rolling::register(&mut registry)?;
    ccd::register(&mut registry)?;
    staking::register(&mut registry)?;
    Ok(registry)
}
fn register_network_results(registry: &mut TestRegistry) -> Result<()> {
    chain_agreement::register(registry)?;
    protocol::register(registry)?;
    observer::register(registry)
}
#[cfg(test)]
pub(super) fn without_tokens(cancellation: Cancellation) -> Result<TestRegistry> {
    let mut registry = register_common(cancellation)?;
    register_network_results(&mut registry)?;
    Ok(registry)
}
#[cfg(test)]
mod unit_tests {
    use super::super::VALIDATOR_COUNT;
    use super::*;
    use crate::fixture::genesis::GenesisFixture;
    #[test]
    fn all_criteria_are_registered_before_execution() -> Result<()> {
        let registry = register(Cancellation::new())?;
        assert_eq!(registry.account_count(), 53);
        let summary = registry.summary();
        assert_eq!(summary.matches("[UNPROCESSED]").count(), 53);
        for index in 0..VALIDATOR_COUNT {
            assert_eq!(summary.matches(&format!("[validator {index}]")).count(), 4);
        }
        assert!(!summary.contains("[INCOMPLETE]"));
        Ok(())
    }
    #[test]
    fn independent_states_take_the_complete_account_budget() -> Result<()> {
        let mut registry = register(Cancellation::new())?;
        let mut genesis = GenesisFixture::four_validator_p10_with_accounts(u32::try_from(
            registry.account_count(),
        )?)?;
        registry.provision(genesis.accounts.split_off(VALIDATOR_COUNT))?;
        assert_eq!(registry.summary().matches("[UNPROCESSED]").count(), 53);
        Ok(())
    }
    #[test]
    fn ccd_registration_needs_no_token_or_staking_fixture() -> Result<()> {
        let mut registry = TestRegistry::new(Cancellation::new());
        ccd::register(&mut registry)?;
        assert_eq!(registry.account_count(), 10);
        let summary = registry.summary();
        assert_eq!(summary.matches("[UNPROCESSED]").count(), 10);
        assert!(!summary.contains("token"));
        assert!(!summary.contains("staking"));
        Ok(())
    }
    #[test]
    fn token_free_inventory_is_independent() -> Result<()> {
        let registry = without_tokens(Cancellation::new())?;
        assert_eq!(registry.account_count(), 33);
        assert_eq!(registry.summary().matches("[UNPROCESSED]").count(), 42);
        Ok(())
    }
}
