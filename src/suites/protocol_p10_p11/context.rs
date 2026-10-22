//! Borrowed protocol network access and retained lifecycle references for tests.
use super::OBSERVER_INDEX;
use crate::fixture::node::NodeFixture;
use anyhow::{Context, Result, ensure};
use bollard::Docker;
use concordium_rust_sdk::{
    genesis::output::GeneratedKeyPair,
    id::types::AccountAddress,
    types::{AbsoluteBlockHeight, hashes::BlockHash},
    v2::Client,
};

/// Precise references retained across network transitions, not proof that a check passed.
#[derive(Default)]
pub(super) struct LifecycleReferences {
    /// Validator index and finalized height sampled before stopping it.
    pub pre_stop_height: Option<(usize, AbsoluteBlockHeight)>,
    /// Validator index and finalized P10 block used for its rejoin wait.
    pub rejoin_target: Option<(usize, BlockHash)>,
    pub update_submission_block: Option<BlockHash>,
    pub p11_activation_block: Option<BlockHash>,
    /// Finalized block saved after fresh P11 operations, before observer replacement.
    pub p11_recovery_target: Option<BlockHash>,
}

/// Network access and saved lifecycle evidence shared by test hooks.
/// Validators precede the observer in node order; private signing accounts stay in each test.
/// Saved references become available only after the corresponding lifecycle action.
pub(super) struct TestContext<'net> {
    nodes: &'net [NodeFixture],
    docker: &'net Docker,
    accounts: &'net [AccountAddress],
    governance: &'net [GeneratedKeyPair],
    references: &'net LifecycleReferences,
}

impl<'net> TestContext<'net> {
    /// Borrow fixtures and saved references for a round of hook notifications.
    pub fn new(
        nodes: &'net [NodeFixture],
        docker: &'net Docker,
        accounts: &'net [AccountAddress],
        governance: &'net [GeneratedKeyPair],
        references: &'net LifecycleReferences,
    ) -> Self {
        Self {
            nodes,
            docker,
            accounts,
            governance,
            references,
        }
    }
    /// All tracked addresses are observable; private signing accounts stay in test state.
    pub fn accounts(&self) -> &[AccountAddress] {
        self.accounts
    }
    /// Genesis governance signing keys for chain updates. Do not log these keys.
    pub fn governance(&self) -> &[GeneratedKeyPair] {
        self.governance
    }
    /// Iterate validator indices, optionally including the observer at the end.
    pub fn node_indices(&self, include_observer: bool) -> std::ops::Range<usize> {
        0..if include_observer {
            self.nodes.len()
        } else {
            self.nodes.len().saturating_sub(1)
        }
    }
    /// Open a gRPC client to the indexed node; fail if the index is unknown or connection fails.
    pub async fn client(&self, index: usize) -> Result<Client> {
        self.nodes
            .get(index)
            .context("Unknown node")?
            .connect()
            .await
    }
    /// Finalized height sampled before stopping this validator; fail if absent or for another index.
    pub fn pre_stop_height(&self, index: usize) -> Result<AbsoluteBlockHeight> {
        let (target, height) = self
            .references
            .pre_stop_height
            .context("No pre-stop height")?;
        ensure!(target == index, "Pre-stop validator mismatch");
        Ok(height)
    }
    /// Saved finalized P10 catch-up target; fail if absent or assigned to another validator.
    pub fn rejoin_target(&self, index: usize) -> Result<BlockHash> {
        let (target, block) = self.references.rejoin_target.context("No rejoin target")?;
        ensure!(target == index, "Rejoin validator mismatch");
        Ok(block)
    }
    /// Finalized block containing the protocol update submission; fail if not yet saved.
    pub fn update_submission_block(&self) -> Result<BlockHash> {
        self.references
            .update_submission_block
            .context("No finalized update submission")
    }
    /// Saved finalized block showing P11-era progress; fail if activation has not been observed.
    pub fn p11_activation_block(&self) -> Result<BlockHash> {
        self.references
            .p11_activation_block
            .context("No finalized P11 activation block")
    }
    /// Finalized block saved after fresh P11 operations for observer recovery; fail if absent.
    pub fn p11_recovery_target(&self) -> Result<BlockHash> {
        self.references
            .p11_recovery_target
            .context("No P11 recovery target")
    }
    /// Read the last 200 log lines from the observer's current container.
    pub async fn observer_logs(&self) -> Result<String> {
        self.nodes[OBSERVER_INDEX].recent_logs(self.docker).await
    }
    /// Read Docker's process status for the observer, not its gRPC or consensus health.
    pub async fn observer_running(&self) -> Result<bool> {
        self.nodes[OBSERVER_INDEX]
            .process_running(self.docker)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_references_require_the_matching_validator() -> Result<()> {
        let docker = Docker::connect_with_local_defaults()?;
        let genesis = crate::fixture::genesis::GenesisFixture::four_validator_p10_with_accounts(0)?;
        let references = LifecycleReferences {
            pre_stop_height: Some((2, AbsoluteBlockHeight::from(7))),
            ..LifecycleReferences::default()
        };
        let context = TestContext::new(&[], &docker, &[], &genesis.governance_keys, &references);
        assert_eq!(context.pre_stop_height(2)?, AbsoluteBlockHeight::from(7));
        assert!(context.pre_stop_height(1).is_err());
        assert!(context.rejoin_target(2).is_err());
        Ok(())
    }
}
