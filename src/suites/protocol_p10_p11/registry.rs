//! Provisioning, direct lifecycle dispatch and final reporting for single-result tests.
use crate::fixture::genesis::GenesisAccount;
mod registered;
mod runtime;
mod upgrade_test;
use super::{
    context::TestContext,
    results::{Recorder, TestId},
};
use anyhow::{Result, ensure};
use runtime::Checkpoint;
use std::{future::Future, pin::Pin, time::Duration};
use tokio_util::sync::CancellationToken;
pub(super) use upgrade_test::{ProtocolUpgradeTest, TestProgress};

pub(super) type RegistryWork<'a> = Pin<Box<dyn Future<Output = Result<()>> + 'a>>;

/// Registry lifecycle
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegistryState {
    /// Accept test definitions before account provisioning starts.
    Registering,
    /// Account provisioning has started.
    Provisioned,
    /// Lifecycle notifications have started.
    Running,
    /// A failure prevents further registration, provisioning, or notifications.
    Stopped,
}

impl RegistryState {
    /// Reject invalid transitions and retain failure as a terminal state.
    fn transition(&mut self, next: Self) -> Result<()> {
        let allowed = matches!(
            (*self, next),
            (Self::Registering, Self::Registering | Self::Provisioned)
                | (Self::Provisioned | Self::Running, Self::Running)
                | (_, Self::Stopped)
        );
        let previous = *self;
        *self = if allowed { next } else { Self::Stopped };
        ensure!(
            allowed,
            "Invalid registry transition: {previous:?} -> {next:?}"
        );
        Ok(())
    }
}

/// Owns registered acceptance tests, provisions their accounts, and dispatches hooks sequentially.
/// Stops dispatch after a failure and reports incomplete results as suite failures.
pub(super) struct TestRegistry {
    /// Tests in registration order, each with private state and one expected result.
    tests: Vec<registered::Registered>,
    /// Records verification outcomes and setup errors for the final summary.
    recorder: Recorder,
    /// Shared shutdown request checked during provisioning, dispatch, and completion.
    cancellation: CancellationToken,
    /// Controls which registry operations are permitted.
    state: RegistryState,
}

impl TestRegistry {
    pub(super) fn new(cancellation: CancellationToken) -> Self {
        Self {
            tests: Vec::new(),
            recorder: Recorder::new(),
            cancellation,
            state: RegistryState::Registering,
        }
    }

    pub(super) fn register<T: ProtocolUpgradeTest + 'static>(&mut self) -> Result<()> {
        self.state.transition(RegistryState::Registering)?;
        let name = T::name();
        let id = match self.recorder.register(name.clone()) {
            Ok(id) => id,
            Err(error) => {
                self.state.transition(RegistryState::Stopped)?;
                return Err(error);
            }
        };
        self.tests.push(registered::Registered::new::<T>(id, name));
        Ok(())
    }

    pub fn account_count(&self) -> usize {
        self.tests.iter().map(|test| test.account_count()).sum()
    }

    pub fn provision(&mut self, accounts: Vec<GenesisAccount>) -> Result<()> {
        let result = (|| {
            self.state.transition(RegistryState::Provisioned)?;
            ensure!(
                accounts.len() == self.account_count(),
                "Wrong number of provisioned accounts"
            );
            let mut accounts = accounts.into_iter();
            for test in &mut self.tests {
                ensure!(
                    !self.cancellation.is_cancelled(),
                    "Termination signal received"
                );
                let count = test.account_count();
                test.provision(accounts.by_ref().take(count).collect(), &mut self.recorder)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.state.transition(RegistryState::Stopped)?;
        }
        result
    }

    fn notify<'a>(
        &'a mut self,
        checkpoint: Checkpoint,
        context: &'a TestContext<'_>,
    ) -> RegistryWork<'a> {
        Box::pin(async move {
            self.state.transition(RegistryState::Running)?;
            let result = async {
                self.recorder.validate()?;
                ensure!(
                    !self.cancellation.is_cancelled(),
                    "Termination signal received"
                );
                for test in &mut self.tests {
                    test.invoke(checkpoint, context, &mut self.recorder, &self.cancellation)
                        .await?;
                }
                ensure!(
                    !self.cancellation.is_cancelled(),
                    "Termination signal received"
                );
                Ok(())
            }
            .await;
            if result.is_err() {
                self.state.transition(RegistryState::Stopped)?;
            }
            result
        })
    }

    pub fn validator_offline<'a>(
        &'a mut self,
        index: usize,
        context: &'a TestContext<'_>,
    ) -> RegistryWork<'a> {
        self.notify(Checkpoint::ValidatorOffline(index), context)
    }

    pub fn validator_rejoined<'a>(
        &'a mut self,
        index: usize,
        context: &'a TestContext<'_>,
    ) -> RegistryWork<'a> {
        self.notify(Checkpoint::ValidatorRejoined(index), context)
    }

    pub fn mixed_version<'a>(&'a mut self, context: &'a TestContext<'_>) -> RegistryWork<'a> {
        self.notify(Checkpoint::MixedVersion, context)
    }

    pub fn before_activation<'a>(&'a mut self, context: &'a TestContext<'_>) -> RegistryWork<'a> {
        self.notify(Checkpoint::BeforeActivation, context)
    }

    pub fn after_activation<'a>(&'a mut self, context: &'a TestContext<'_>) -> RegistryWork<'a> {
        self.notify(Checkpoint::AfterActivation, context)
    }

    pub fn preservation_verified<'a>(
        &'a mut self,
        context: &'a TestContext<'_>,
    ) -> RegistryWork<'a> {
        self.notify(Checkpoint::PreservationVerified, context)
    }

    pub fn operations_complete<'a>(&'a mut self, context: &'a TestContext<'_>) -> RegistryWork<'a> {
        self.notify(Checkpoint::OperationsComplete, context)
    }

    pub fn before_observer_upgrade<'a>(
        &'a mut self,
        context: &'a TestContext<'_>,
    ) -> RegistryWork<'a> {
        self.notify(Checkpoint::BeforeObserverUpgrade, context)
    }

    pub fn observer_recovered<'a>(&'a mut self, context: &'a TestContext<'_>) -> RegistryWork<'a> {
        self.notify(Checkpoint::ObserverRecovered, context)
    }

    pub fn all_nodes_ready<'a>(&'a mut self, context: &'a TestContext<'_>) -> RegistryWork<'a> {
        self.notify(Checkpoint::AllNodesReady, context)
    }

    pub fn finish(self, run: Result<()>, cleanup: Result<()>, elapsed: Duration) -> Result<()> {
        let errors = self.final_errors(run, &cleanup);
        tracing::info!(
            passed = errors.is_empty(),
            elapsed_seconds = elapsed.as_secs_f64(),
            "protocol suite finished"
        );
        print!("{}", self.recorder.render(&cleanup, elapsed, &errors));
        ensure!(
            errors.is_empty(),
            "P10 -> P11 continuity suite failed:\n{}",
            errors.join("\n")
        );
        Ok(())
    }

    fn final_errors(&self, run: Result<()>, cleanup: &Result<()>) -> Vec<String> {
        let mut errors = Vec::new();
        if let Err(error) = run {
            errors.push(format!("lifecycle: {error:#}"));
        }
        if let Err(error) = cleanup {
            errors.push(format!("cleanup: {error:#}"));
        }
        if self.cancellation.is_cancelled() {
            errors.push("termination requested at completion: Termination signal received".into());
        }
        if self.state == RegistryState::Stopped {
            errors.push("Test execution stopped after a failure".into());
        }
        if !self.recorder.complete() {
            errors.push("Acceptance results are incomplete or invalid".into());
        }
        errors
    }

    #[cfg(test)]
    pub(super) fn summary(&self) -> String {
        self.recorder.render(&Ok(()), Duration::ZERO, &[])
    }
}
#[cfg(test)]
mod unit_tests {
    #[test]
    fn wrong_account_budget_stops_provisioning() -> anyhow::Result<()> {
        let mut registry = super::TestRegistry::new(super::CancellationToken::new());
        registry.register::<ConstructorFailure>()?;
        assert_eq!(registry.account_count(), 1);
        assert!(registry.provision(Vec::new()).is_err());
        assert_eq!(registry.state, super::RegistryState::Stopped);
        Ok(())
    }

    #[test]
    fn registry_transitions_reject_invalid_operations_and_stop_permanently() {
        use super::RegistryState::*;
        for initial in [Registering, Provisioned, Running, Stopped] {
            for next in [Registering, Provisioned, Running, Stopped] {
                let expected = match initial {
                    Registering => matches!(next, Registering | Provisioned | Stopped),
                    Provisioned | Running => matches!(next, Running | Stopped),
                    Stopped => next == Stopped,
                };
                let mut state = initial;
                assert_eq!(state.transition(next).is_ok(), expected);
                assert_eq!(state, if expected { next } else { Stopped });
            }
        }
    }

    #[test]
    fn registration_failure_prevents_further_registration_and_provisioning() -> anyhow::Result<()> {
        let mut registry = super::TestRegistry::new(super::CancellationToken::new());
        registry.register::<Pending>()?;
        assert!(registry.register::<Pending>().is_err());
        assert!(registry.register::<Passed>().is_err());
        assert!(registry.provision(Vec::new()).is_err());
        assert_eq!(registry.state, super::RegistryState::Stopped);
        Ok(())
    }

    use super::*;
    use crate::{
        fixture::genesis::GenesisFixture,
        suites::protocol_p10_p11::{VALIDATOR_COUNT, context::LifecycleReferences, wait},
    };
    use bollard::Docker;

    struct Network {
        docker: Docker,
        genesis: GenesisFixture,
        references: LifecycleReferences,
    }
    impl Network {
        fn new() -> Result<Self> {
            Ok(Self {
                docker: Docker::connect_with_local_defaults()?,
                genesis: GenesisFixture::four_validator_p10_with_accounts(0)?,
                references: LifecycleReferences::default(),
            })
        }
        fn context(&self) -> TestContext<'_> {
            TestContext::new(
                &[],
                &self.docker,
                &[],
                &self.genesis.governance_keys,
                &self.references,
            )
        }
    }
    struct Pending;
    impl ProtocolUpgradeTest for Pending {
        fn name() -> String {
            "Default methods stay unprocessed".into()
        }
        fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
            Ok(Self)
        }
    }
    struct Passed;
    impl ProtocolUpgradeTest for Passed {
        fn name() -> String {
            "One successful criterion".into()
        }
        fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
            Ok(Self)
        }
        async fn on_mixed_version(&mut self, _: &TestContext<'_>) -> Result<TestProgress> {
            Ok(TestProgress::Passed)
        }
    }
    struct Failed;
    impl ProtocolUpgradeTest for Failed {
        fn name() -> String {
            "Failed criterion".into()
        }
        fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
            Ok(Self)
        }
        async fn on_mixed_version(&mut self, _: &TestContext<'_>) -> Result<TestProgress> {
            Err(anyhow::anyhow!("RPC disconnected").context("read balances"))
        }
    }
    struct SetupFailed;
    impl ProtocolUpgradeTest for SetupFailed {
        fn name() -> String {
            "Preparation failure".into()
        }
        fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
            Ok(Self)
        }
        async fn on_mixed_version(&mut self, _: &TestContext<'_>) -> Result<TestProgress> {
            anyhow::bail!("fixture RPC failed")
        }
    }
    struct Stateful {
        value: usize,
    }
    impl ProtocolUpgradeTest for Stateful {
        fn name() -> String {
            "State survives direct method calls".into()
        }
        fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
            Ok(Self { value: 0 })
        }
        async fn on_before_activation(&mut self, _: &TestContext<'_>) -> Result<TestProgress> {
            self.value = 7;
            Ok(TestProgress::Pending)
        }
        async fn on_after_activation(&mut self, _: &TestContext<'_>) -> Result<TestProgress> {
            ensure!(self.value == 7, "Lost state");
            self.value = 8;
            Ok(TestProgress::Passed)
        }
    }
    struct BarePass;
    impl ProtocolUpgradeTest for BarePass {
        fn name() -> String {
            "Hook returns a pass".into()
        }
        fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
            Ok(Self)
        }
        async fn on_after_activation(&mut self, _: &TestContext<'_>) -> Result<TestProgress> {
            Ok(TestProgress::Passed)
        }
    }
    struct ActiveWait;
    impl ProtocolUpgradeTest for ActiveWait {
        fn name() -> String {
            "Active wait".into()
        }
        fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
            Ok(Self)
        }
        async fn on_after_activation(&mut self, _: &TestContext<'_>) -> Result<TestProgress> {
            std::future::pending().await
        }
    }
    struct SetupWait;
    impl ProtocolUpgradeTest for SetupWait {
        fn name() -> String {
            "Setup wait".into()
        }
        fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
            Ok(Self)
        }
        async fn on_before_activation(&mut self, _: &TestContext<'_>) -> Result<TestProgress> {
            std::future::pending().await
        }
    }
    struct Timeout;
    impl ProtocolUpgradeTest for Timeout {
        fn name() -> String {
            "Timed verification".into()
        }
        fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
            Ok(Self)
        }
        async fn on_after_activation(&mut self, _: &TestContext<'_>) -> Result<TestProgress> {
            wait::bounded(
                "test deadline",
                Duration::from_millis(1),
                std::future::pending::<Result<()>>(),
            )
            .await?;
            Ok(TestProgress::Passed)
        }
    }
    struct ConstructorFailure;
    impl ProtocolUpgradeTest for ConstructorFailure {
        fn name() -> String {
            "Constructor failure".into()
        }
        fn required_accounts() -> usize {
            1
        }
        fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
            anyhow::bail!("cannot construct fixture")
        }
    }
    #[tokio::test]
    async fn default_methods_do_not_pass_and_missing_results_fail_without_another_error()
    -> Result<()> {
        let network = Network::new()?;
        let mut registry = TestRegistry::new(CancellationToken::new());
        registry.register::<Pending>()?;
        registry.provision(Vec::new())?;
        registry.before_activation(&network.context()).await?;
        registry.after_activation(&network.context()).await?;
        assert!(
            registry
                .summary()
                .contains("[UNPROCESSED] Default methods stay unprocessed")
        );
        assert!(registry.finish(Ok(()), Ok(()), Duration::ZERO).is_err());
        Ok(())
    }
    #[tokio::test]
    async fn results_are_immediate_and_partial_failure_stops_execution() -> Result<()> {
        let network = Network::new()?;
        let mut registry = TestRegistry::new(CancellationToken::new());
        registry.register::<Passed>()?;
        registry.register::<Failed>()?;
        registry.register::<Pending>()?;
        registry.provision(Vec::new())?;
        assert!(registry.mixed_version(&network.context()).await.is_err());
        let output = registry.summary();
        assert!(output.contains("[PASS] One successful criterion"));
        assert!(
            output.contains(
                "[FAIL] Failed criterion — mixed_version: read balances: RPC disconnected"
            )
        );
        assert!(output.contains("[UNPROCESSED] Default methods stay unprocessed"));
        assert!(registry.after_activation(&network.context()).await.is_err());
        Ok(())
    }
    #[tokio::test]
    async fn preparation_errors_fail_the_hook_and_returned_passes_are_accepted() -> Result<()> {
        let network = Network::new()?;
        let mut registry = TestRegistry::new(CancellationToken::new());
        registry.register::<SetupFailed>()?;
        registry.provision(Vec::new())?;
        assert!(registry.mixed_version(&network.context()).await.is_err());
        let output = registry.summary();
        assert!(output.contains("[FAIL] Preparation failure — mixed_version: fixture RPC failed"));
        assert!(!output.contains("[ERROR]"));
        let mut registry = TestRegistry::new(CancellationToken::new());
        registry.register::<BarePass>()?;
        registry.provision(Vec::new())?;
        registry.after_activation(&network.context()).await?;
        assert!(registry.summary().contains("[PASS] Hook returns a pass"));
        registry.finish(Ok(()), Ok(()), Duration::ZERO)?;
        Ok(())
    }
    #[tokio::test]
    async fn state_is_retained_and_completed_tests_are_not_invoked_again() -> Result<()> {
        let network = Network::new()?;
        let mut registry = TestRegistry::new(CancellationToken::new());
        registry.register::<Stateful>()?;
        registry.provision(Vec::new())?;
        registry.before_activation(&network.context()).await?;
        registry.after_activation(&network.context()).await?;
        registry.after_activation(&network.context()).await?;
        assert_eq!(
            registry
                .summary()
                .matches("[PASS] State survives direct method calls")
                .count(),
            1
        );
        registry.finish(Ok(()), Ok(()), Duration::ZERO)
    }
    #[tokio::test]
    async fn cancellation_fails_the_active_hook_but_leaves_unstarted_tests_unprocessed()
    -> Result<()> {
        let network = Network::new()?;
        let cancellation = CancellationToken::new();
        let mut registry = TestRegistry::new(cancellation.clone());
        registry.register::<ActiveWait>()?;
        registry.provision(Vec::new())?;
        let context = network.context();
        let signal = async {
            tokio::task::yield_now().await;
            cancellation.cancel();
        };
        let (result, ()) = tokio::join!(registry.after_activation(&context), signal);
        assert!(result.is_err());
        assert!(registry.summary().contains("[FAIL] Active wait"));
        assert!(registry.summary().contains("Termination signal"));
        let mut registry = TestRegistry::new(cancellation);
        registry.register::<Pending>()?;
        assert!(registry.provision(Vec::new()).is_err());
        assert!(registry.summary().contains("[UNPROCESSED] Default methods"));
        Ok(())
    }
    #[tokio::test]
    async fn cancellation_during_preparation_and_timeouts_fail_the_hook() -> Result<()> {
        let network = Network::new()?;
        let context = network.context();
        let cancellation = CancellationToken::new();
        let mut registry = TestRegistry::new(cancellation.clone());
        registry.register::<SetupWait>()?;
        registry.provision(Vec::new())?;
        let signal = async {
            tokio::task::yield_now().await;
            cancellation.cancel();
        };
        let (result, ()) = tokio::join!(registry.before_activation(&context), signal);
        assert!(result.is_err());
        assert!(
            registry
                .summary()
                .contains("[FAIL] Setup wait — before_activation: Termination signal")
        );
        let mut registry = TestRegistry::new(CancellationToken::new());
        registry.register::<Timeout>()?;
        registry.provision(Vec::new())?;
        assert!(registry.after_activation(&context).await.is_err());
        assert!(registry.summary().contains("[FAIL] Timed verification"));
        assert!(registry.summary().contains("test deadline exceeded"));
        Ok(())
    }
    #[test]
    fn constructor_errors_are_setup_errors_and_registration_closes_after_provisioning() -> Result<()>
    {
        let mut registry = TestRegistry::new(CancellationToken::new());
        registry.register::<ConstructorFailure>()?;
        let mut genesis = GenesisFixture::four_validator_p10_with_accounts(1)?;
        assert!(
            registry
                .provision(genesis.accounts.split_off(VALIDATOR_COUNT))
                .is_err()
        );
        assert!(registry.summary().contains("[ERROR] Constructor failure"));
        assert!(registry.summary().contains("verification unprocessed"));
        let mut registry = TestRegistry::new(CancellationToken::new());
        registry.register::<Pending>()?;
        registry.provision(Vec::new())?;
        assert!(registry.register::<Passed>().is_err());
        assert!(registry.finish(Ok(()), Ok(()), Duration::ZERO).is_err());
        Ok(())
    }
    #[tokio::test]
    async fn finish_accounts_for_cleanup_cancellation_and_outside_run_errors() -> Result<()> {
        let network = Network::new()?;
        let cancellation = CancellationToken::new();
        let mut registry = TestRegistry::new(cancellation.clone());
        registry.register::<Passed>()?;
        registry.provision(Vec::new())?;
        registry.mixed_version(&network.context()).await?;
        cancellation.cancel();
        let cleanup = Err(anyhow::anyhow!("TempDir close failed"));
        let errors = registry.final_errors(Err(anyhow::anyhow!("outside test")), &cleanup);
        let output = registry.recorder.render(&cleanup, Duration::ZERO, &errors);
        assert!(output.contains("[PASS] One successful criterion"));
        assert!(output.contains("TempDir close failed"));
        assert!(output.contains("termination requested at completion"));
        assert!(output.contains("outside test"));
        assert!(registry.finish(Ok(()), cleanup, Duration::ZERO).is_err());
        Ok(())
    }
    struct Repeated<const I: usize>;
    impl<const I: usize> ProtocolUpgradeTest for Repeated<I> {
        fn name() -> String {
            format!("Criterion for validator {I}")
        }
        fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
            Ok(Self)
        }
    }
    #[test]
    fn repeated_test_families_require_unique_names() -> Result<()> {
        let mut registry = TestRegistry::new(CancellationToken::new());
        registry.register::<Repeated<0>>()?;
        registry.register::<Repeated<1>>()?;
        assert!(registry.register::<Repeated<0>>().is_err());
        assert!(registry.finish(Ok(()), Ok(()), Duration::ZERO).is_err());
        Ok(())
    }
}
