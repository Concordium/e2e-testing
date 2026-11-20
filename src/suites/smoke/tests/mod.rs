//! Independent end-to-end tests run concurrently against the live private chain.

use super::Test;

mod accounts;
mod finalisation;
mod health;
mod transfers;

/// Return all registered tests in execution order.
pub fn all_tests() -> Vec<Test> {
    let mut tests = health::tests();
    tests.extend(accounts::tests());
    tests.extend(transfers::tests());
    tests.extend(finalisation::tests());
    tests
}
