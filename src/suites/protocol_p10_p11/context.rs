//! Read-only protocol network access and retained lifecycle references for tests.
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

pub(super) struct TestContext<'net> {
    nodes: &'net [NodeFixture],
    docker: &'net Docker,
    accounts: &'net [AccountAddress],
    governance: &'net [GeneratedKeyPair],
    references: &'net LifecycleReferences,
}

impl<'net> TestContext<'net> {
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
    pub fn governance(&self) -> &[GeneratedKeyPair] {
        self.governance
    }
    pub fn node_indices(&self, include_observer: bool) -> std::ops::Range<usize> {
        0..if include_observer {
            self.nodes.len()
        } else {
            self.nodes.len().saturating_sub(1)
        }
    }
    pub async fn client(&self, index: usize) -> Result<Client> {
        self.nodes
            .get(index)
            .context("Unknown node")?
            .connect()
            .await
    }
    pub fn pre_stop_height(&self, index: usize) -> Result<AbsoluteBlockHeight> {
        let (target, height) = self
            .references
            .pre_stop_height
            .context("No pre-stop height")?;
        ensure!(target == index, "Pre-stop validator mismatch");
        Ok(height)
    }
    pub fn rejoin_target(&self, index: usize) -> Result<BlockHash> {
        let (target, block) = self.references.rejoin_target.context("No rejoin target")?;
        ensure!(target == index, "Rejoin validator mismatch");
        Ok(block)
    }
    pub fn update_submission_block(&self) -> Result<BlockHash> {
        self.references
            .update_submission_block
            .context("No finalized update submission")
    }
    pub fn p11_activation_block(&self) -> Result<BlockHash> {
        self.references
            .p11_activation_block
            .context("No finalized P11 activation block")
    }
    pub fn p11_recovery_target(&self) -> Result<BlockHash> {
        self.references
            .p11_recovery_target
            .context("No P11 recovery target")
    }
    pub async fn observer_logs(&self) -> Result<String> {
        self.nodes[OBSERVER_INDEX].recent_logs(self.docker).await
    }
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
        let genesis = crate::fixture::genesis::GenesisFixture::four_validator_p10()?;
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
