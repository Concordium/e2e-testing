//! Ordered P10 -> P11 network lifecycle with test-owned acceptance checks.
mod cancellation;
mod context;
mod operations;
mod payload;
mod results;
mod state;
mod tests;
mod wait;

use crate::fixture::{
    genesis::GenesisFixture,
    node::{NodeConfig, NodeFixture},
    nodes,
};
use anyhow::{Context, Result, ensure};
use bollard::Docker;
use cancellation::Cancellation;
use concordium_rust_sdk::{
    base::updates::UpdatePayload,
    common::types::TransactionTime,
    id::types::AccountAddress,
    v2::{BlockIdentifier, Client},
};
use context::{LifecycleReferences, TestContext};
use std::{
    future::Future,
    time::{Duration, Instant},
};
use tests::registry::{RegistryWork, TestRegistry};
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
/// The observer follows the validators in the node list.
const OBSERVER_INDEX: usize = VALIDATOR_COUNT;

pub struct Config {
    pub old_image: String,
    /// Replacement image. It must support P10 and P11.
    pub new_image: String,
}

/// The core owns containers and lifecycle references, but no feature fixtures.
struct Lifecycle {
    docker: Docker,
    tests: TestRegistry,
    config: Config,
    cancellation: Cancellation,
    genesis: Option<GenesisFixture>,
    /// Validators occupy indices 0 to 3. The observer occupies index 4.
    nodes: Vec<NodeFixture>,
    tracked_accounts: Vec<AccountAddress>,
    references: LifecycleReferences,
}

/// Stop after the first failure and print acceptance coverage after unconditional cleanup.
pub async fn run(config: Config) -> Result<()> {
    let cancellation = Cancellation::new();
    let tests = tests::register(cancellation.clone())?;
    run_registered(config, cancellation, tests).await
}

async fn run_registered(
    config: Config,
    cancellation: Cancellation,
    tests: TestRegistry,
) -> Result<()> {
    let docker = match Docker::connect_with_local_defaults() {
        Ok(docker) => docker,
        Err(error) => return tests.finish(Err(error.into()), Ok(()), Duration::ZERO),
    };
    let mut lifecycle = Lifecycle {
        docker,
        tests,
        config,
        cancellation: cancellation.clone(),
        genesis: None,
        nodes: Vec::new(),
        tracked_accounts: Vec::new(),
        references: LifecycleReferences::default(),
    };
    // Finish creation or replacement and register its container ID before observing shutdown.
    let signal_task = tokio::spawn(async move {
        super::shutdown_signal().await;
        cancellation.request();
    });
    tokio::task::yield_now().await;
    let started_run = Instant::now();
    let outcome = lifecycle.execute().await;
    let cleanup = nodes::stop(std::mem::take(&mut lifecycle.nodes), &lifecycle.docker).await;
    signal_task.abort();
    lifecycle
        .tests
        .finish(outcome, cleanup, started_run.elapsed())
}

/// Check cancellation between actions, never by dropping in-flight resource operations.
async fn action(
    name: &str,
    cancellation: &Cancellation,
    operation: impl Future<Output = Result<()>>,
) -> Result<()> {
    let started = Instant::now();
    tracing::info!(action = name, "starting protocol action");
    let outcome = async {
        cancellation.check()?;
        operation.await?;
        cancellation.check()
    }
    .await;
    match &outcome {
        Ok(()) => tracing::info!(
            action = name,
            elapsed_seconds = started.elapsed().as_secs_f64(),
            "PASS"
        ),
        Err(error) => {
            tracing::error!(action = name, elapsed_seconds = started.elapsed().as_secs_f64(), error = %format!("{error:#}"), "FAIL")
        }
    }
    outcome.with_context(|| name.to_string())
}

impl Lifecycle {
    async fn notify<F>(&mut self, notify: F) -> Result<()>
    where
        F: for<'a, 'net> FnOnce(&'a mut TestRegistry, &'a TestContext<'net>) -> RegistryWork<'a>,
    {
        let context = TestContext::new(
            &self.nodes,
            &self.docker,
            &self.tracked_accounts,
            &self.genesis.as_ref().context("No genesis")?.governance_keys,
            &self.references,
        );
        notify(&mut self.tests, &context).await
    }

    /// Establish each network condition before directly notifying the test lifecycle methods.
    async fn execute(&mut self) -> Result<()> {
        let cancellation = self.cancellation.clone();
        action(
            "inspect images",
            &cancellation,
            cancellation.run(self.preflight()),
        )
        .await?;
        action("start network", &cancellation, self.start()).await?;
        action(
            "upgrade validator 0",
            &cancellation,
            self.upgrade_validator(0),
        )
        .await?;
        action(
            "prepare mixed-version P10 state",
            &cancellation,
            self.notify(TestRegistry::mixed_version),
        )
        .await?;
        for validator in 1..VALIDATOR_COUNT {
            action(
                &format!("upgrade validator {validator}"),
                &cancellation,
                self.upgrade_validator(validator),
            )
            .await?;
        }
        action(
            "capture pre-update state",
            &cancellation,
            self.notify(TestRegistry::before_activation),
        )
        .await?;
        action(
            "activate P11",
            &cancellation,
            cancellation.run(self.activate()),
        )
        .await?;
        action(
            "verify preserved state",
            &cancellation,
            self.notify(TestRegistry::after_activation),
        )
        .await?;
        action("exercise P11", &cancellation, async {
            self.notify(TestRegistry::preservation_verified).await?;
            self.references.p11_recovery_target = Some(
                cancellation
                    .run(async {
                        Ok(self
                            .client(0)
                            .await?
                            .get_consensus_info()
                            .await?
                            .last_finalized_block)
                    })
                    .await?,
            );
            self.notify(TestRegistry::operations_complete).await
        })
        .await?;
        action(
            "check old observer",
            &cancellation,
            self.notify(TestRegistry::before_observer_upgrade),
        )
        .await?;
        action("recover observer", &cancellation, self.recover_observer()).await?;
        action(
            "verify all-node agreement",
            &cancellation,
            self.notify(TestRegistry::all_nodes_ready),
        )
        .await
    }

    async fn client(&self, index: usize) -> Result<Client> {
        self.nodes[index].connect().await
    }

    /// Resolve and pin image identities before any containers start.
    async fn preflight(&mut self) -> Result<()> {
        let old = self
            .docker
            .inspect_image(&self.config.old_image)
            .await
            .context("Inspect old image (pull it first)")?;
        let new = self
            .docker
            .inspect_image(&self.config.new_image)
            .await
            .context("Inspect new image (pull it first)")?;
        ensure!(
            old.id != new.id,
            "Old/new references resolve to the same image"
        );
        tracing::info!(old_image = %self.config.old_image, old_id = ?old.id,
            new_image = %self.config.new_image, new_id = ?new.id,
            update_payload_sha256 = %payload::payload_hash()?, "resolved test images and update payload");
        self.config.old_image = old.id.context("Old image has no identity")?;
        self.config.new_image = new.id.context("New image has no identity")?;
        Ok(())
    }

    /// Register each successful start before readiness checks or cancellation.
    async fn start(&mut self) -> Result<()> {
        let cancellation = self.cancellation.clone();
        cancellation.check()?;
        let mut genesis = GenesisFixture::four_validator_p10_with_accounts(u32::try_from(
            self.tests.account_count(),
        )?)?;
        self.tracked_accounts.clear();
        for account in &genesis.accounts {
            self.tracked_accounts.push(account.lock().await.address);
        }
        let private_accounts = genesis.accounts.split_off(VALIDATOR_COUNT);
        self.tests.provision(private_accounts)?;
        self.genesis = Some(genesis);
        let genesis = self.genesis.as_ref().context("No genesis")?;
        let bytes = concordium_rust_sdk::genesis::serialize_genesis(&genesis.genesis_data);
        for credential_mutex in &genesis.validators {
            cancellation.check()?;
            let credentials = serde_json::to_vec(&*credential_mutex.lock().unwrap())?;
            let node = NodeFixture::start(
                &self.docker,
                NodeConfig {
                    image: self.config.old_image.clone(),
                    genesis_bytes: bytes.clone(),
                    validator_credentials: Some(credentials),
                },
            )
            .await?;
            self.nodes.push(node);
            cancellation.check()?;
        }
        cancellation.check()?;
        let observer = NodeFixture::start(
            &self.docker,
            NodeConfig {
                image: self.config.old_image.clone(),
                genesis_bytes: bytes,
                validator_credentials: None,
            },
        )
        .await?;
        self.nodes.push(observer);
        cancellation.check()?;
        // Peering must precede readiness checks to establish a quorum.
        cancellation.run(async {
            for node in &mut self.nodes { node.wait_for_health().await?; }
            nodes::connect_peers(&self.nodes, &self.docker).await?;
            for node in &mut self.nodes { node.wait_until_ready().await?; }
            for index in 0..self.nodes.len() {
                let mut client = self.client(index).await?;
                let consensus = client.get_consensus_info().await?;
                ensure!(consensus.protocol_version.0 == 10, "Node {index} did not start at P10");
                tracing::info!(node = index, version = %client.get_node_info().await?.version, protocol = 10, "node ready");
            }
            Ok(())
        }).await
    }

    /// Establish offline and rejoined conditions without exposing container mutation to tests.
    async fn upgrade_validator(&mut self, index: usize) -> Result<()> {
        let cancellation = self.cancellation.clone();
        let survivor = if index == 0 { 1 } else { 0 };
        let height = cancellation
            .run(async {
                Ok(self
                    .client(survivor)
                    .await?
                    .get_consensus_info()
                    .await?
                    .last_finalized_block_height)
            })
            .await?;
        cancellation.check()?;
        self.nodes[index].pause(&self.docker).await?;
        self.references.pre_stop_height = Some((index, height));
        self.notify(|tests, context| tests.validator_offline(index, context))
            .await?;
        cancellation.check()?;
        // Do not cancel replacement: the fixture must register its new container ID.
        self.nodes[index]
            .replace_image(&self.docker, &self.config.new_image)
            .await?;
        cancellation.check()?;
        let target = cancellation.run(async {
            self.nodes[index].wait_for_health().await?;
            nodes::connect_peers(&self.nodes, &self.docker).await?;
            let target = self.client(survivor).await?.get_consensus_info().await?.last_finalized_block;
            wait_block(&self.nodes[index], target, 10).await?;
            let mut upgraded = self.nodes[index].connect().await?;
            tracing::info!(node = index, version = %upgraded.get_node_info().await?.version, block = %target, "node upgraded and rejoined");
            Ok(target)
        }).await?;
        self.references.rejoin_target = Some((index, target));
        self.notify(|tests, context| tests.validator_rejoined(index, context))
            .await
    }

    /// Submit the update and save finalized block references for separate acceptance checks.
    async fn activate(&mut self) -> Result<()> {
        let mut client = self.client(0).await?;
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
        tracing::info!(effective_time = effective.seconds, block = %block.block_hash, "scheduled protocol update");
        self.references.update_submission_block = Some(
            operations::governance(
                &mut client,
                &self.genesis.as_ref().context("No genesis")?.governance_keys,
                sequence,
                effective,
                UpdatePayload::Protocol(payload::protocol_update()?),
                "protocol-update",
            )
            .await?,
        );
        self.references.p11_activation_block = Some(bounded("P11 activation", ACTIVATION_TIMEOUT, async {
            loop {
                let block = client.get_block_info(BlockIdentifier::LastFinal).await?.response;
                if block.finalized && block.protocol_version.0 == 11 && block.era_block_height.height > 0 {
                    tracing::info!(block = %block.block_hash, height = block.block_height.height, "P11 activated");
                    return Ok(block.block_hash);
                }
                let logs = self.nodes[0].recent_logs(&self.docker).await?;
                ensure!(!logs.contains("unsupported protocol update"),
                    "New image does not support the bundled P11 update (source revision {}); last finalized protocol {}, height {}", payload::SOURCE_REVISION, block.protocol_version.0, block.block_height.height);
                tracing::debug!(protocol = block.protocol_version.0, height = block.block_height.height, "waiting for P11 activation");
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }).await?);
        Ok(())
    }

    /// Retain the observer database and establish recovery before notifying tests.
    async fn recover_observer(&mut self) -> Result<()> {
        let cancellation = self.cancellation.clone();
        let running = cancellation
            .run(self.nodes[OBSERVER_INDEX].process_running(&self.docker))
            .await?;
        if running {
            self.nodes[OBSERVER_INDEX].pause(&self.docker).await?;
        }
        cancellation.check()?;
        self.nodes[OBSERVER_INDEX]
            .replace_image(&self.docker, &self.config.new_image)
            .await?;
        cancellation.check()?;
        cancellation.run(async {
            self.nodes[OBSERVER_INDEX].wait_for_health().await?;
            nodes::connect_peers(&self.nodes, &self.docker).await?;
            wait_block(&self.nodes[OBSERVER_INDEX], self.references.p11_recovery_target.context("No P11 recovery target")?, 11).await?;
            let mut observer = self.nodes[OBSERVER_INDEX].connect().await?;
            tracing::info!(version = %observer.get_node_info().await?.version, protocol = observer.get_consensus_info().await?.protocol_version.0, "observer recovered");
            Ok(())
        }).await?;
        self.notify(TestRegistry::observer_recovered).await
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    #[tokio::test]
    async fn cancelled_upgrade_does_not_access_the_next_validator() -> Result<()> {
        let cancellation = Cancellation::new();
        cancellation.request();
        let mut lifecycle = Lifecycle {
            docker: Docker::connect_with_local_defaults()?,
            tests: tests::register(cancellation.clone())?,
            config: Config {
                old_image: "old".into(),
                new_image: "new".into(),
            },
            cancellation,
            genesis: None,
            nodes: Vec::new(),
            tracked_accounts: Vec::new(),
            references: LifecycleReferences::default(),
        };
        let error = lifecycle.upgrade_validator(1).await.unwrap_err();
        assert!(error.to_string().contains("Termination signal"));
        Ok(())
    }
    #[tokio::test]
    async fn action_does_not_poll_work_after_cancellation() {
        let cancellation = Cancellation::new();
        cancellation.request();
        let polled = std::cell::Cell::new(false);
        let result = action("start network", &cancellation, async {
            polled.set(true);
            Ok(())
        })
        .await;
        assert!(!polled.get());
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("start network"));
        assert!(error.contains("Termination signal"));
    }

    #[tokio::test]
    async fn action_finishes_resource_work_before_observing_cancellation() {
        let cancellation = Cancellation::new();
        let registered = std::cell::Cell::new(false);
        let result = action("replace validator 2", &cancellation, async {
            cancellation.request();
            tokio::task::yield_now().await;
            registered.set(true);
            Ok(())
        })
        .await;
        assert!(registered.get());
        assert!(format!("{:#}", result.unwrap_err()).contains("Termination signal"));
    }

    #[tokio::test]
    async fn action_failure_preserves_context_and_stops_the_sequence() {
        let cancellation = Cancellation::new();
        let later_action_ran = std::cell::Cell::new(false);
        let outcome = async {
            action("upgrade validator 2", &cancellation, async {
                Err(anyhow::anyhow!("RPC disconnected").context("catch-up failed"))
            })
            .await?;
            action("upgrade validator 3", &cancellation, async {
                later_action_ran.set(true);
                Ok(())
            })
            .await
        }
        .await;
        let error = format!("{:#}", outcome.unwrap_err());
        assert!(error.contains("upgrade validator 2: catch-up failed: RPC disconnected"));
        assert!(!later_action_ran.get());
    }

    #[tokio::test]
    #[ignore = "requires Docker and compatible old/new images"]
    async fn lifecycle_without_token_test_is_independent() -> Result<()> {
        let cancellation = Cancellation::new();
        let tests = tests::without_tokens(cancellation.clone())?;
        run_registered(
            Config {
                old_image: std::env::var("CONCORDIUM_OLD_IMAGE")?,
                new_image: std::env::var("CONCORDIUM_NEW_IMAGE")?,
            },
            cancellation,
            tests,
        )
        .await
    }

    #[tokio::test]
    #[ignore = "requires Docker and a new image supporting the bundled P11 update"]
    async fn full_protocol_lifecycle() -> Result<()> {
        run(Config {
            old_image: std::env::var("CONCORDIUM_OLD_IMAGE")?,
            new_image: std::env::var("CONCORDIUM_NEW_IMAGE")?,
        })
        .await
    }
}
