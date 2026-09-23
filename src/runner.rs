//! Test runner — orchestrates fixture setup, test execution, and teardown.
//!
//! Sequence:
//! 1. Generate a fresh genesis block via [`fixture::genesis`].
//! 2. Start a node container via [`fixture::node`].
//! 3. Wait for the node to become ready (healthy + first block).
//! 4. Run each registered test in order, collecting results.
//! 5. Stop the container and delete temp files — always, even on failure.
//! 6. Return results to `main` for summary and exit code.

use crate::fixture::{
    genesis::GenesisFixture,
    node::{NodeConfig, NodeFixture},
};
use anyhow::{Context, Result};
use bollard::Docker;

/// The result of a single test case.
#[derive(Debug)]
pub struct TestResult {
    /// Human-readable test name.
    pub name: String,
    /// `true` if the test passed.
    pub passed: bool,
    /// Optional failure message.
    pub message: Option<String>,
}

use std::{future::Future, pin::Pin};

pub type TestFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + 'a>>;

/// A test function: receives a reference to the running node fixture and
/// returns a result. The name is used in the summary output.
pub struct Test {
    pub name: &'static str,
    pub run: Box<dyn for<'a> Fn(&'a Fixtures) -> TestFuture<'a> + Send>,
}

/// Test fixtures available to tests
pub struct Fixtures {
    /// Genesis information, contains accounts, validators, identity providers and so on.
    pub(crate) genesis: GenesisFixture,
    /// The running validator node.
    pub(crate) validator_node: NodeFixture,
}

/// Run the full test suite against the given node image.
///
/// * `filter` — if `Some`, only tests whose names contain the substring
///   (case-insensitive) are executed.
///
/// Always stops the node container before returning, regardless of outcome.
pub async fn run(image: &str, grpc_port: u16, filter: Option<&str>) -> Result<Vec<TestResult>> {
    // ── Connect to Docker daemon ──────────────────────────────────────────
    let docker =
        Docker::connect_with_local_defaults().context("Failed to connect to Docker daemon")?;

    // ── Step 1: Generate genesis ──────────────────────────────────────────────
    tracing::info!("generating genesis block...");
    let genesis = GenesisFixture::single_validator()?;
    tracing::info!(
        validators = genesis.validators.len(),
        accounts = genesis.accounts.len(),
        "genesis block generated"
    );

    // ── Step 2: Start node container using validator credentials ──────────────
    let validator_credentials = {
        let credentials = genesis
            .validators
            .first()
            .expect("genesis always produces at least one validator")
            .lock()
            .unwrap();
        serde_json::to_vec_pretty(&*credentials)
            .context("Failed serializing validator credentials")?
    };
    tracing::info!(image, grpc_port, "starting node container...");
    let mut node = NodeFixture::start(
        &docker,
        NodeConfig {
            image: image.to_string(),
            grpc_port,
            genesis_bytes: concordium_rust_sdk::genesis::serialize_genesis(&genesis.genesis_data),
            validator_credentials: Some(validator_credentials),
        },
    )
    .await?;

    // ── Step 3: Wait for readiness (health + block) ───────────────────────────
    let readiness_result = node.wait_until_ready().await;
    if let Err(ref e) = readiness_result {
        tracing::error!("node did not become ready: {e}");
        let _ = node.stop(&docker).await;
        return Err(readiness_result.unwrap_err());
    }

    let fixtures = Fixtures {
        genesis,
        validator_node: node,
    };

    // ── Step 4: Collect and run tests ─────────────────────────────────────────
    let tests = crate::tests::all_tests();
    let mut results = Vec::new();

    for test in tests {
        if let Some(f) = filter {
            if !test.name.to_lowercase().contains(&f.to_lowercase()) {
                continue;
            }
        }

        tracing::info!(test = test.name, "running test");
        let outcome =
            match tokio::time::timeout(std::time::Duration::from_secs(30), (test.run)(&fixtures))
                .await
            {
                Ok(outcome) => outcome,
                Err(_) => Err(anyhow::anyhow!("{} timed out after 30 seconds", test.name)),
            };
        let result = match outcome {
            Ok(()) => {
                tracing::info!(test = test.name, "PASS");
                TestResult {
                    name: test.name.to_string(),
                    passed: true,
                    message: None,
                }
            }
            Err(e) => {
                tracing::error!(test = test.name, error = %e, "FAIL");
                TestResult {
                    name: test.name.to_string(),
                    passed: false,
                    message: Some(format!("{e:#}")),
                }
            }
        };
        results.push(result);
    }

    // ── Step 5: Teardown (always) ─────────────────────────────────────────────
    if let Err(e) = fixtures.validator_node.stop(&docker).await {
        tracing::warn!("error during node teardown: {e}");
    }

    if results.is_empty() {
        tracing::info!("no tests registered — nothing to run");
    }

    Ok(results)
}

/// Print a human-readable summary of the test run to stdout.
pub fn print_summary(results: &[TestResult]) {
    if results.is_empty() {
        println!("\nno tests registered — nothing to run.");
        return;
    }

    let total = results.len();
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = total - passed;

    println!("\n{}", "─".repeat(60));
    for r in results {
        let status = if r.passed { "PASS" } else { "FAIL" };
        print!("  [{status}] {}", r.name);
        if let Some(msg) = &r.message {
            print!("  — {msg}");
        }
        println!();
    }
    println!("{}", "─".repeat(60));
    println!("  {total} tests run — {passed} passed, {failed} failed");
    println!("{}\n", "─".repeat(60));
}
