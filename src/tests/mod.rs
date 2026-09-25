//! End-to-end tests run sequentially against the live private chain.

use crate::runner::Test;

mod accounts;
mod finalisation;
mod health;
mod transfers;

/// Return all registered tests in execution order.
pub fn all_tests() -> Vec<Test> {
    vec![
        Test {
            name: "health::node_is_reachable",
            run: Box::new(|fixtures| Box::pin(health::node_is_reachable(fixtures))),
        },
        Test {
            name: "health::node_produces_blocks",
            run: Box::new(|fixtures| Box::pin(health::node_produces_blocks(fixtures))),
        },
        Test {
            name: "accounts::validator_account_exists",
            run: Box::new(|fixtures| Box::pin(accounts::validator_account_exists(fixtures))),
        },
        Test {
            name: "transfers::ccd_transfer_is_finalised",
            run: Box::new(|fixtures| Box::pin(transfers::ccd_transfer_is_finalised(fixtures))),
        },
        Test {
            name: "finalisation::last_finalised_block_advances",
            run: Box::new(|fixtures| {
                Box::pin(finalisation::last_finalised_block_advances(fixtures))
            }),
        },
    ]
}
