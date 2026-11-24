//! Ordered P10 -> P11 network lifecycle with test-owned acceptance checks.
mod lifecycle;
mod operations;
mod payload;
mod state;
mod tests;
mod wait;

use crate::fixture::{
    genesis::GenesisFixture,
    node::{NodeConfig, NodeFixture},
    nodes,
};
use crate::report::{Check, Outcome as ReportOutcome, SuiteReport};
use anyhow::{Context, Result, ensure};
use bollard::Docker;
use concordium_rust_sdk::{
    base::updates::UpdatePayload, common::types::TransactionTime, id::types::AccountAddress,
    v2::BlockIdentifier,
};
use lifecycle::{CleanupContext, TestContext, UpgradeRegistry};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use wait::{block as wait_block, bounded};

/// Maximum wait for a submitted item to reach finalization.
pub const TRANSACTION_TIMEOUT: Duration = Duration::from_secs(60);
/// Maximum wait for recovery, agreement, or preservation evidence.
const REJOIN_TIMEOUT: Duration = Duration::from_secs(90);
/// Maximum wait for finalized P11 progress after the update is finalized.
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(180);
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Genesis accounts and nodes place the four validators first, in the same order.
const VALIDATOR_COUNT: usize = 4;

pub struct Config {
    pub old_image: String,
    /// Replacement image supporting both P10 and P11.
    pub new_image: String,
}

/// Print acceptance coverage after unconditional cleanup on controlled exit.
/// Clean up test-owned resources before validators. Panics/hard stops are not covered.
pub async fn run(config: Config) -> Result<SuiteReport> {
    let cancellation = CancellationToken::new();
    let mut tests = tests::register();
    // Bound individual Docker requests without dropping whole resource-changing hooks.
    let docker = Docker::connect_with_local_defaults()?.with_timeout(Duration::from_secs(10));
    let signal_task = {
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            super::shutdown_signal().await;
            tracing::info!("Received request for termination");
            cancellation.cancel();
        })
    };
    let started = Instant::now();
    let mut validators = Vec::new();
    let outcome = execute(&docker, config, &mut tests, &mut validators, &cancellation).await;
    // Cleanup is outside the cancellation boundary and independent of test status.
    tracing::info!("Run test 'cleanup' hooks");
    let test_cleanup = lifecycle::cleanup(&mut tests, &CleanupContext { docker: &docker }).await;
    tracing::info!("Cleanup validators");
    let validator_cleanup = nodes::stop(validators, &docker).await;
    signal_task.abort();
    let results = tests.into_checks();
    let cleanup_errors: Vec<_> = [test_cleanup, validator_cleanup]
        .into_iter()
        .filter_map(Result::err)
        .map(|error| format!("{error:#}"))
        .collect();
    let cleanup = if cleanup_errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(cleanup_errors.join("; ")))
    };
    Ok(SuiteReport {
        title: "P10 → P11 continuity".into(),
        elapsed: started.elapsed(),
        checks: results
            .into_iter()
            .chain([
                Check {
                    group: "Suite lifecycle".into(),
                    name: "Execution".into(),
                    outcome: match outcome {
                        Ok(()) => ReportOutcome::Passed,
                        Err(error) => ReportOutcome::Failed(format!("{error:#}")),
                    },
                },
                Check {
                    group: "Suite lifecycle".into(),
                    name: "Cleanup".into(),
                    outcome: match cleanup {
                        Ok(()) => ReportOutcome::Passed,
                        Err(error) => ReportOutcome::Failed(format!("{error:#}")),
                    },
                },
            ])
            .collect(),
    })
}

async fn execute(
    docker: &Docker,
    config: Config,
    tests: &mut UpgradeRegistry,
    validators: &mut Vec<NodeFixture>,
    cancellation: &CancellationToken,
) -> Result<()> {
    // --- preflight: Resolve and pin image identities before any containers start. ---
    // Check before awaited work, never by dropping an in-flight resource operation.
    ensure!(!cancellation.is_cancelled(), "Termination requested");
    let old_image = docker
        .inspect_image(&config.old_image)
        .await
        .context("Inspect old image (pull it first)")?
        .id
        .context("Old image has no identity")?;
    let new_image = docker
        .inspect_image(&config.new_image)
        .await
        .context("Inspect new image (pull it first)")?
        .id
        .context("New image has no identity")?;
    ensure!(
        old_image != new_image,
        "Old/new references resolve to the same image"
    );
    tracing::debug!(old_image = %old_image, new_image = %new_image,
        update_payload_sha256 = %payload::payload_hash()?, "Resolved test images and payload");
    // --- start: Retain each successful start before readiness checks or cancellation. ---
    tracing::info!("Generating genesis");
    let mut genesis = GenesisFixture::four_validator_p10_with_accounts(u32::try_from(
        lifecycle::account_count(tests),
    )?)?;
    let accounts: Vec<AccountAddress> = genesis
        .accounts
        .iter()
        .map(|account| account.address)
        .collect();
    let test_accounts = genesis.accounts.split_off(VALIDATOR_COUNT);
    let bytes = concordium_rust_sdk::genesis::serialize_genesis(&genesis.genesis_data);
    let context = TestContext {
        validators,
        accounts: &accounts,
        governance: &genesis.governance_keys,
        docker,
        genesis: &bytes,
        old_image: &old_image,
        new_image: &new_image,
    };
    tracing::info!("Startup validators");
    for credentials in &genesis.validators {
        ensure!(!cancellation.is_cancelled(), "Termination requested");
        let validator_credentials = serde_json::to_vec(&*credentials.lock().unwrap())?;
        let node = NodeFixture::start(
            docker,
            NodeConfig {
                image: old_image.clone(),
                genesis_bytes: bytes.clone(),
                validator_credentials: Some(validator_credentials),
            },
        )
        .await?;
        // Retain ownership before readiness or cancellation checks.
        context.validators.push(node);
    }
    for node in context.validators.iter_mut() {
        ensure!(!cancellation.is_cancelled(), "Termination requested");
        node.wait_for_health().await?;
    }
    // Peering must precede block-production readiness checks to establish a quorum.
    ensure!(!cancellation.is_cancelled(), "Termination requested");
    nodes::connect_peers(context.validators, docker).await?;
    for node in context.validators.iter_mut() {
        ensure!(!cancellation.is_cancelled(), "Termination requested");
        node.wait_until_ready().await?;
    }
    for node in context.validators.iter() {
        ensure!(
            node.connect()
                .await?
                .get_consensus_info()
                .await?
                .protocol_version
                .0
                == 10,
            "Validator did not start at P10"
        );
    }
    // Prepare tests against a ready P10 network before upgrading any validators.
    tracing::info!("Run test 'prepare_before_update' hooks");
    lifecycle::prepare_before_update(tests, &context, test_accounts, cancellation).await?;
    tracing::info!("Run test 'run_before_update' hooks");
    lifecycle::run_test_hook(
        tests,
        &|test| test.run_before_update(&context),
        cancellation,
    )
    .await?;
    tracing::info!("Run test 'finish_before_update' hooks");
    lifecycle::run_test_hook(
        tests,
        &|test| test.finish_before_update(&context),
        cancellation,
    )
    .await?;

    // --- rolling upgrade: Exercise continuity with each validator offline and rejoined. ---
    for validator in 0..VALIDATOR_COUNT {
        // Observe continuity through a different validator while this one is stopped.
        let mut client = context.validators[if validator == 1 { 0 } else { 1 }]
            .connect()
            .await?;
        context.validators[validator].pause(docker).await?;
        let height = client
            .get_consensus_info()
            .await?
            .last_finalized_block_height;
        bounded(
            "finalization during validator downtime",
            REJOIN_TIMEOUT,
            async {
                loop {
                    let observed = client.get_consensus_info().await?;
                    if observed.last_finalized_block_height > height {
                        return Ok(());
                    }
                    tracing::debug!(
                        validator,
                        height = observed.last_finalized_block_height.height,
                        "waiting for offline continuity"
                    );
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            },
        )
        .await?;
        ensure!(!cancellation.is_cancelled(), "Termination requested");
        context.validators[validator]
            .replace_image(docker, &new_image)
            .await?;
        context.validators[validator].wait_for_health().await?;
        nodes::connect_peers(context.validators, docker).await?;
        ensure!(!cancellation.is_cancelled(), "Termination requested");
        let target = client.get_consensus_info().await?.last_finalized_block;
        wait_block(&context.validators[validator], target, 10).await?;
        tracing::debug!(validator, block = %target, "Upgraded validator recovered finalized pre-update target");
    }
    // --- activate P11: Submit the governance update and wait for finalized era progress. ---
    let mut client = context.validators[0].connect().await?;
    let block = client
        .get_block_info(BlockIdentifier::LastFinal)
        .await?
        .response;
    let effective =
        TransactionTime::from_seconds(u64::try_from(block.block_slot_time.timestamp() + 30)?);
    let sequence = client
        .get_next_update_sequence_numbers(BlockIdentifier::LastFinal)
        .await?
        .response
        .protocol;
    ensure!(!cancellation.is_cancelled(), "Termination requested");
    tracing::info!("Submitting P11 protocol update");
    operations::governance(
        &mut client,
        &genesis.governance_keys,
        sequence,
        effective,
        UpdatePayload::Protocol(payload::protocol_update()?),
        "protocol-update",
    )
    .await?;
    tracing::info!("Protocol update submission finalized, waiting for P11 progress");
    ensure!(!cancellation.is_cancelled(), "Termination requested");
    bounded("P11 activation", ACTIVATION_TIMEOUT, async {
        loop {
            let block = client
                .get_block_info(BlockIdentifier::LastFinal)
                .await?
                .response;
            if block.finalized
                && block.protocol_version.0 == 11
                && block.era_block_height.height > 0
            {
                return Ok(());
            }
            ensure!(
                !context.validators[0]
                    .recent_logs(docker)
                    .await?
                    .contains("unsupported protocol update"),
                "New image does not support bundled P11 update ({})",
                payload::SOURCE_REVISION
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await?;
    // --- prepare after update: Verify preservation and prepare the next operations. ---
    tracing::info!("Run test 'prepare_after_update' hooks");
    lifecycle::run_test_hook(
        tests,
        &|test| test.prepare_after_update(&context),
        cancellation,
    )
    .await?;
    // --- run after update: Intended for fresh transactions after preservation checks. ---
    tracing::info!("Run test 'run_after_update' hooks");
    lifecycle::run_test_hook(tests, &|test| test.run_after_update(&context), cancellation).await?;
    // Every post-update operation finishes before final test-local verification.
    tracing::info!("Run test 'finish_after_update' hooks");
    lifecycle::run_test_hook(
        tests,
        &|test| test.finish_after_update(&context),
        cancellation,
    )
    .await?;
    Ok(())
}
