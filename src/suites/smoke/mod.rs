//! Smoke suite — fixture setup, independent checks, and teardown.
//!
//! Sequence:
//! 1. Generate a fresh genesis block via [`crate::fixture::genesis`].
//! 2. Start a node container via [`crate::fixture::node`].
//! 3. Wait for the node to become ready (healthy + first block).
//! 4. Run registered tests concurrently, retaining results in registration order.
//! 5. Stop the container and delete temp files — always, even on failure.
//! 6. Return results to `main` for summary and exit code.

mod tests;

use super::shutdown_signal;
use crate::fixture::{
    genesis::GenesisFixture,
    node::{NodeConfig, NodeFixture},
};
use crate::report::{Check, Outcome, SuiteReport};
use anyhow::{Context, Result};
use bollard::Docker;
use concordium_rust_sdk::types::{ProtocolVersion, queries::ProtocolVersionInt};
use std::time::Instant;

use std::{future::Future, pin::Pin};

/// A test operation that can borrow the suite fixtures.
pub type TestFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + 'a>>;

/// One named smoke check, run concurrently with the other registered checks.
pub struct Test {
    /// Run this check against the suite's shared chain.
    pub run: Box<dyn for<'a> Fn(&'a Fixtures) -> TestFuture<'a> + Send>,
}

/// Genesis keys and the running node shared by smoke checks.
pub struct Fixtures {
    /// Genesis accounts and keys used to sign test transactions.
    pub(crate) genesis: GenesisFixture,
    /// The running validator node.
    pub(crate) validator_node: NodeFixture,
}

/// Run the full test suite against the given node image.
///
/// Always stops the node container before returning, regardless of outcome.
pub async fn run(image: &str, protocol: ProtocolVersion) -> Result<SuiteReport> {
    let started = Instant::now();
    let mut registry = tests::register();
    // Install signal handlers before creating any Docker resources. A signal
    // received during startup is retained until the container can be cleaned up.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(());
    });
    let mut shutdown = Box::pin(async move {
        let _ = shutdown_rx.await;
    });
    tokio::task::yield_now().await;

    let docker =
        Docker::connect_with_local_defaults().context("Failed to connect to Docker daemon")?;

    // ── Step 1: Generate genesis ──────────────────────────────────────────
    tracing::info!("generating genesis block...");
    let genesis = GenesisFixture::single_validator(protocol)?;
    tracing::debug!(
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
    tracing::info!(image, "starting node container...");
    let mut node = NodeFixture::start(
        &docker,
        NodeConfig {
            image: image.to_string(),
            genesis_bytes: concordium_rust_sdk::genesis::serialize_genesis(&genesis.genesis_data),
            validator_credentials: Some(validator_credentials),
        },
    )
    .await?;

    // ── Step 3: Wait for readiness (health + block) ───────────────────────────
    let readiness_result = tokio::select! {
        result = async {
            node.wait_until_ready().await?;
            let actual = node
                .connect()
                .await?
                .get_consensus_info()
                .await?
                .protocol_version;
            anyhow::ensure!(
                actual == ProtocolVersionInt::from_enum(protocol),
                "Expected {protocol}, node reports {actual}"
            );
            Ok::<_, anyhow::Error>(())
        } => result,
        _ = &mut shutdown => {
            Err(anyhow::anyhow!("Received termination signal during node startup"))
        }
    };
    if let Err(error) = readiness_result {
        tracing::error!("node did not become ready: {error}");
        node.log_output(&docker).await;
        return match node.stop(&docker).await {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(error.context(format!(
                "Smoke setup failed; cleanup also failed: {cleanup_error:#}"
            ))),
        };
    }

    let fixtures = Fixtures {
        genesis,
        validator_node: node,
    };

    // ── Step 4: Collect and run tests ─────────────────────────────────────────

    let execution =
        futures_util::future::join_all(registry.entries_mut().iter_mut().map(|entry| {
            let fixtures = &fixtures;
            async move {
                let check = &mut entry.check;
                tracing::info!(test = check.name, "running test");
                check.outcome = Outcome::Incomplete;
                check.outcome = match (entry.definition.run)(fixtures).await {
                    Ok(()) => {
                        tracing::info!(test = check.name, "PASS");
                        Outcome::Passed
                    }
                    Err(error) => {
                        tracing::error!(test = check.name, error = %error, "FAIL");
                        Outcome::Failed(format!("{error:#}"))
                    }
                };
            }
        }));
    let interrupted = tokio::select! {
        _ = execution => false,
        _ = &mut shutdown => true,
    };
    let mut checks = registry.into_checks();

    checks.push(Check {
        group: "Suite lifecycle".into(),
        name: "Execution".into(),
        outcome: if interrupted {
            Outcome::Failed("Received termination signal during test execution".into())
        } else {
            Outcome::Passed
        },
    });

    // ── Step 5: Teardown (always) ─────────────────────────────────────────────
    let cleanup = fixtures.validator_node.stop(&docker).await;
    checks.push(Check {
        group: "Suite lifecycle".into(),
        name: "Cleanup".into(),
        outcome: match cleanup {
            Ok(()) => Outcome::Passed,
            Err(error) => Outcome::Failed(format!("{error:#}")),
        },
    });
    Ok(SuiteReport {
        title: format!("Smoke — {protocol}"),
        elapsed: started.elapsed(),
        checks,
    })
}
