//! Stateful continuity scenarios and independent network checks.
mod ccd;
mod chain_agreement;
mod new_software_p10_compatibility;
mod observer_p11_recovery;
mod staking;
mod tokens;

use super::registry::TestRegistry;
use tokio_util::sync::CancellationToken;

pub(super) fn register(cancellation: CancellationToken) -> TestRegistry {
    let mut registry = TestRegistry::new(cancellation);
    new_software_p10_compatibility::register(&mut registry);
    ccd::register(&mut registry);
    staking::register(&mut registry);
    tokens::register(&mut registry);
    chain_agreement::register(&mut registry);
    observer_p11_recovery::register(&mut registry);
    registry
}
