//! Independent end-to-end tests run concurrently against the live private chain.

use super::Test;
use crate::registry::TestRegistry;

mod accounts;
mod finalisation;
mod health;
mod transfers;

/// Register checks in report order.
pub fn register() -> TestRegistry<Test> {
    let mut registry = TestRegistry::default();
    health::register(&mut registry);
    accounts::register(&mut registry);
    transfers::register(&mut registry);
    finalisation::register(&mut registry);
    registry
}
