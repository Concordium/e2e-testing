//! Recover an observer from unsupported P11 activation while retaining its database.
use super::super::{
    REJOIN_TIMEOUT,
    lifecycle::{CleanupContext, ProtocolUpgradeTest, TestContext, TestProgress, UpgradeRegistry},
    state::FinalizedBlock,
    wait,
};
use super::tokens::support::token_equivalent;
use crate::fixture::{
    genesis::GenesisAccount,
    node::{NodeConfig, NodeFixture},
};
use anyhow::{Context, Result, ensure};
use concordium_rust_sdk::{
    endpoints::{QueryError, RPCError},
    id::types::AccountAddress,
    protocol_level_tokens::{TokenId, TokenInfo},
    types::AccountInfo,
    v2::{Client, Code},
};
use futures_util::TryStreamExt;
use std::collections::BTreeMap;

pub(super) fn register(registry: &mut UpgradeRegistry) {
    registry
        .group("Observer recovery after unsupported P11 activation")
        .register("An old-software observer stops at P11 activation, then catches up after a database-preserving software upgrade and matches validator chain, CCD, staking, and token state", Box::new(ObserverP11Recovery::default()));
}

/// Let the healthy observer initiate connections to validators.
async fn connect_to_validators(context: &TestContext<'_>, node: &NodeFixture) -> Result<()> {
    let mut client = node.connect().await?;
    for validator in context.validators.iter() {
        client
            .peer_connect(validator.peer_address(context.docker).await?)
            .await?;
    }
    Ok(())
}

fn is_unsupported_update(error: &RPCError) -> bool {
    matches!(
        error,
        RPCError::CallError(status)
            if status.code() == Code::FailedPrecondition
                && status.message()
                    == "The network is stopped due to unrecognized protocol update."
    )
}

#[derive(Default)]
struct ObserverP11Recovery {
    node: Option<NodeFixture>,
}

#[async_trait::async_trait(?Send)]
impl ProtocolUpgradeTest for ObserverP11Recovery {
    async fn prepare_before_update(
        &mut self,
        context: &TestContext<'_>,
        _accounts: Vec<GenesisAccount>,
    ) -> Result<()> {
        // Retain the fixture before readiness checks so partial preparation can clean up.
        self.node = Some(
            NodeFixture::start(
                context.docker,
                NodeConfig {
                    image: context.old_image.to_owned(),
                    genesis_bytes: context.genesis.to_vec(),
                    validator_credentials: None,
                },
            )
            .await?,
        );
        let node = self.node.as_mut().context("Observer not prepared")?;
        node.wait_for_health().await?;
        connect_to_validators(context, node).await?;
        node.wait_until_ready().await?;
        ensure!(
            node.connect()
                .await?
                .get_consensus_info()
                .await?
                .protocol_version
                .0
                == 10,
            "Observer did not start at P10"
        );
        Ok(())
    }

    async fn cleanup(&mut self, context: &CleanupContext<'_>) -> Result<()> {
        if let Some(node) = self.node.take() {
            node.stop(context.docker).await?;
        }
        Ok(())
    }

    async fn prepare_after_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        // The old observer may already reject peer_connect RPCs after activation.
        // Initiate connections from validators without using the observer's RPCs.
        let node = self.node.as_ref().context("Observer not prepared")?;
        let address = node.peer_address(context.docker).await?;
        for validator in context.validators.iter() {
            validator
                .connect()
                .await?
                .peer_connect(address)
                .await
                .context("Validator could not reconnect to the old observer")?;
        }
        wait::bounded(
            "observer reaches unsupported update",
            REJOIN_TIMEOUT,
            async {
                loop {
                    let logs = node.recent_logs(context.docker).await?;
                    if logs.contains("unsupported protocol update")
                        && logs.contains("has taken effect")
                    {
                        return Ok(());
                    }
                    tokio::time::sleep(super::super::POLL_INTERVAL).await;
                }
            },
        )
        .await?;
        Ok(TestProgress::Pending)
    }

    async fn finish_before_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let evidence = Evidence::capture(context).await?;
        evidence
            .verify_network(
                context,
                self.node.as_ref().context("Observer not prepared")?,
            )
            .await?;
        Ok(TestProgress::Pending)
    }

    async fn finish_after_update(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let evidence = Evidence::capture(context).await?;
        let node = self.node.as_mut().context("Observer not prepared")?;
        // The observer still uses old software: inspect its logs, process status,
        // and finalized state before replacing its container.
        let logs = node.recent_logs(context.docker).await?;
        ensure!(
            logs.contains("unsupported protocol update") && logs.contains("has taken effect"),
            "Observer did not report an unsupported update taking effect"
        );
        ensure!(
            node.process_running(context.docker).await?,
            "Old observer stopped after the unsupported protocol update"
        );
        let mut client = node
            .connect()
            .await
            .context("Old observer API unavailable")?;
        // A stopped old node may reject queries instead of serving its P10 state.
        match client.get_consensus_info().await {
            Ok(info) => ensure!(
                info.protocol_version.0 == 10,
                "Old observer unexpectedly follows P11"
            ),
            Err(QueryError::RPCError(ref error)) if is_unsupported_update(error) => (),
            Err(error) => return Err(error.into()),
        }
        match client.get_block_info(evidence.block.hash).await {
            Err(QueryError::RPCError(ref error)) if is_unsupported_update(error) => (),
            Err(error) if error.is_not_found() => (),
            Err(error) => return Err(error.into()),
            Ok(_) => anyhow::bail!("Old observer recognizes the P11 reference block"),
        }
        // Upgrade with the existing database, then recover the saved finalized P11 block.
        node.pause(context.docker).await?;
        node.replace_image(context.docker, context.new_image)
            .await?;
        node.wait_for_health().await?;
        connect_to_validators(context, node).await?;
        wait::block(node, evidence.block.hash, 11).await?;
        ensure!(
            node.connect()
                .await?
                .get_consensus_info()
                .await?
                .protocol_version
                .0
                == 11,
            "Observer did not recover P11"
        );
        // All five nodes now use new software at P11. Compare the exact saved
        // finalized evidence rather than independently sampled latest state.
        evidence.verify_network(context, node).await?;
        Ok(TestProgress::Passed)
    }
}

/// Independent evidence at one finalized block, covering every tracked account and token.
struct Evidence {
    block: FinalizedBlock,
    accounts: BTreeMap<AccountAddress, AccountInfo>,
    tokens: Vec<(TokenId, TokenInfo)>,
}

impl Evidence {
    async fn capture(context: &TestContext<'_>) -> Result<Self> {
        wait::bounded("capture observer evidence", REJOIN_TIMEOUT, async {
            let mut client = context.validators[0].connect().await?;
            let block = FinalizedBlock::capture(&mut client).await?;
            let mut accounts = BTreeMap::new();
            for address in context.accounts {
                accounts.insert(
                    *address,
                    client
                        .get_account_info(&(*address).into(), block.hash)
                        .await?
                        .response,
                );
            }
            // Discover assets from the chain, never from another test's private fixture.
            let ids: Vec<TokenId> = client
                .get_token_list(block.hash)
                .await?
                .response
                .try_collect()
                .await?;
            let mut tokens = Vec::new();
            for id in ids {
                let info = client
                    .get_token_info(id.clone(), block.hash)
                    .await?
                    .response;
                tokens.push((id, info));
            }
            Ok(Self {
                block,
                accounts,
                tokens,
            })
        })
        .await
    }

    async fn compare(&self, client: &mut Client) -> Result<()> {
        wait::finalized(client, self.block.hash).await?;
        let block = client.get_block_info(self.block.hash).await?.response;
        ensure!(
            block.finalized && block.block_state_hash == self.block.state_hash,
            "Observer scenario: finalized block/state hash disagreement"
        );
        for (address, expected) in &self.accounts {
            let actual = client
                .get_account_info(&(*address).into(), self.block.hash)
                .await?
                .response;
            ensure!(
                actual.account_amount == expected.account_amount
                    && actual.account_nonce == expected.account_nonce,
                "Observer scenario: CCD balance/nonce disagreement for {address}"
            );
            ensure!(
                actual.account_stake == expected.account_stake,
                "Observer scenario: staking disagreement for {address}"
            );
            // Includes absent token states, balances and module state for every asset.
            ensure!(
                actual.tokens == expected.tokens,
                "Observer scenario: token account disagreement for {address}"
            );
        }
        for (id, expected) in &self.tokens {
            let actual = client
                .get_token_info(id.clone(), self.block.hash)
                .await?
                .response;
            ensure!(
                token_equivalent(expected, &actual)?,
                "Observer scenario: token configuration disagreement for {id}"
            );
        }
        Ok(())
    }

    async fn verify_network(
        &self,
        context: &TestContext<'_>,
        observer: &NodeFixture,
    ) -> Result<()> {
        for validator in context.validators.iter() {
            self.compare(&mut validator.connect().await?).await?;
        }
        self.compare(&mut observer.connect().await?).await
    }
}
