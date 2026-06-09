//! Test cases.
//!
//! Each submodule corresponds to one use-case from the PRD.  Tests are
//! registered with the runner in the order they should execute; later tests
//! may rely on state established by earlier ones (e.g. `block_production` must
//! pass before `account_state` is meaningful).
//!
//! # Adding a test
//!
//! 1. Create a new submodule (e.g. `pub mod my_test;`).
//! 2. Implement an `async fn run(ctx: &TestContext) -> TestResult` in it.
//! 3. Register it in `runner::run` in the ordered list.

// TODO(task-07): add submodules as tests are implemented.
//
// pub mod health_check;
// pub mod block_production;
// pub mod account_state;
// pub mod ccd_transfer;
// pub mod finalization;
