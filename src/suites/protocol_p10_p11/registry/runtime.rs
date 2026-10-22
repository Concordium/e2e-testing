//! Type-erased dispatch for native async test hooks. Test implementations do not box futures.
use super::super::context::TestContext;
use super::{ProtocolUpgradeTest, TestProgress};
use anyhow::Result;
use std::{future::Future, pin::Pin};

/// Selects a hook; lifecycle ordering remains the caller's responsibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Checkpoint {
    ValidatorOffline(usize),
    ValidatorRejoined(usize),
    MixedVersion,
    BeforeActivation,
    AfterActivation,
    PreservationVerified,
    OperationsComplete,
    BeforeObserverUpgrade,
    ObserverRecovered,
    AllNodesReady,
}

impl Checkpoint {
    pub fn name(self) -> &'static str {
        match self {
            Self::ValidatorOffline(_) => "validator_offline",
            Self::ValidatorRejoined(_) => "validator_rejoined",
            Self::MixedVersion => "mixed_version",
            Self::BeforeActivation => "before_activation",
            Self::AfterActivation => "after_activation",
            Self::PreservationVerified => "preservation_verified",
            Self::OperationsComplete => "operations_complete",
            Self::BeforeObserverUpgrade => "before_observer_upgrade",
            Self::ObserverRecovered => "observer_recovered",
            Self::AllNodesReady => "all_nodes_ready",
        }
    }
}

/// Futures borrow test state and context, and need not be Send.
type HookWork<'a> = Pin<Box<dyn Future<Output = Result<TestProgress>> + 'a>>;

/// The registry stores heterogeneous tests through this private interface.
pub(super) trait RuntimeTest {
    fn notify<'a, 'net>(
        &'a mut self,
        checkpoint: Checkpoint,
        context: &'a TestContext<'net>,
    ) -> HookWork<'a>;
}

impl<T: ProtocolUpgradeTest> RuntimeTest for T {
    fn notify<'a, 'net>(
        &'a mut self,
        checkpoint: Checkpoint,
        context: &'a TestContext<'net>,
    ) -> HookWork<'a> {
        Box::pin(async move {
            match checkpoint {
                Checkpoint::ValidatorOffline(index) => {
                    self.on_validator_offline(index, context).await
                }
                Checkpoint::ValidatorRejoined(index) => {
                    self.on_validator_rejoined(index, context).await
                }
                Checkpoint::MixedVersion => self.on_mixed_version(context).await,
                Checkpoint::BeforeActivation => self.on_before_activation(context).await,
                Checkpoint::AfterActivation => self.on_after_activation(context).await,
                Checkpoint::PreservationVerified => self.on_preservation_verified(context).await,
                Checkpoint::OperationsComplete => self.on_operations_complete(context).await,
                Checkpoint::BeforeObserverUpgrade => self.on_before_observer_upgrade(context).await,
                Checkpoint::ObserverRecovered => self.on_observer_recovered(context).await,
                Checkpoint::AllNodesReady => self.on_all_nodes_ready(context).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fixture::genesis::GenesisAccount, suites::protocol_p10_p11::context::LifecycleReferences,
    };
    use std::{cell::RefCell, rc::Rc};

    struct Probe {
        calls: Rc<RefCell<Vec<Checkpoint>>>,
    }
    impl Probe {
        async fn record(
            &mut self,
            checkpoint: Checkpoint,
            context: &TestContext<'_>,
        ) -> Result<TestProgress> {
            let calls = self.calls.clone();
            tokio::task::yield_now().await;
            anyhow::ensure!(context.accounts().is_empty(), "Unexpected probe accounts");
            calls.borrow_mut().push(checkpoint);
            Ok(TestProgress::Passed)
        }
    }
    impl ProtocolUpgradeTest for Probe {
        fn name() -> String {
            "Dispatch probe".into()
        }
        fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
            Ok(Self {
                calls: Rc::new(RefCell::new(Vec::new())),
            })
        }
        async fn on_validator_offline(
            &mut self,
            index: usize,
            context: &TestContext<'_>,
        ) -> Result<TestProgress> {
            self.record(Checkpoint::ValidatorOffline(index), context)
                .await
        }
        async fn on_validator_rejoined(
            &mut self,
            index: usize,
            context: &TestContext<'_>,
        ) -> Result<TestProgress> {
            self.record(Checkpoint::ValidatorRejoined(index), context)
                .await
        }
        async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
            self.record(Checkpoint::MixedVersion, context).await
        }
        async fn on_before_activation(
            &mut self,
            context: &TestContext<'_>,
        ) -> Result<TestProgress> {
            self.record(Checkpoint::BeforeActivation, context).await
        }
        async fn on_after_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
            self.record(Checkpoint::AfterActivation, context).await
        }
        async fn on_preservation_verified(
            &mut self,
            context: &TestContext<'_>,
        ) -> Result<TestProgress> {
            self.record(Checkpoint::PreservationVerified, context).await
        }
        async fn on_operations_complete(
            &mut self,
            context: &TestContext<'_>,
        ) -> Result<TestProgress> {
            self.record(Checkpoint::OperationsComplete, context).await
        }
        async fn on_before_observer_upgrade(
            &mut self,
            context: &TestContext<'_>,
        ) -> Result<TestProgress> {
            self.record(Checkpoint::BeforeObserverUpgrade, context)
                .await
        }
        async fn on_observer_recovered(
            &mut self,
            context: &TestContext<'_>,
        ) -> Result<TestProgress> {
            self.record(Checkpoint::ObserverRecovered, context).await
        }
        async fn on_all_nodes_ready(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
            self.record(Checkpoint::AllNodesReady, context).await
        }
    }

    #[tokio::test]
    async fn dispatch_preserves_hook_indices_context_and_non_send_state() -> Result<()> {
        let docker = bollard::Docker::connect_with_local_defaults()?;
        let references = LifecycleReferences::default();
        let context = TestContext::new(&[], &docker, &[], &[], &references);
        let probe = Probe::setup(Vec::new())?;
        let calls = probe.calls.clone();
        let mut test: Box<dyn RuntimeTest> = Box::new(probe);
        let checkpoints = [
            Checkpoint::ValidatorOffline(2),
            Checkpoint::ValidatorRejoined(3),
            Checkpoint::MixedVersion,
            Checkpoint::BeforeActivation,
            Checkpoint::AfterActivation,
            Checkpoint::PreservationVerified,
            Checkpoint::OperationsComplete,
            Checkpoint::BeforeObserverUpgrade,
            Checkpoint::ObserverRecovered,
            Checkpoint::AllNodesReady,
        ];
        for checkpoint in checkpoints {
            assert_eq!(
                test.notify(checkpoint, &context).await?,
                TestProgress::Passed
            );
        }
        assert_eq!(calls.borrow().as_slice(), checkpoints.as_slice());
        Ok(())
    }
}
