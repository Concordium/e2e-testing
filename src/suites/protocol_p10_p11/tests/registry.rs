//! Provisioning, direct lifecycle dispatch and final reporting for single-result tests.
mod registered;
use super::{
    super::{
        cancellation::Cancellation,
        context::TestContext,
        results::{Recorder, TestId},
    },
    accounts_fixture::{Account, TestAccounts},
    upgrade_test::{TestDefinition, UpgradeTest, Verification},
};
use anyhow::{Result, ensure};
use std::{future::Future, pin::Pin, time::Duration};

pub(in super::super) type RegistryWork<'a> = Pin<Box<dyn Future<Output = Result<()>> + 'a>>;
type ProgressWork<'a> =
    Pin<Box<dyn Future<Output = Result<super::upgrade_test::TestProgress>> + 'a>>;

pub(in super::super) struct TestRegistry {
    tests: Vec<registered::Registered>,
    recorder: Recorder,
    cancellation: Cancellation,
    started: bool,
    provisioned: bool,
    stopped: bool,
}
impl TestRegistry {
    pub(super) fn new(cancellation: Cancellation) -> Self {
        Self {
            tests: Vec::new(),
            recorder: Recorder::new(),
            cancellation,
            started: false,
            provisioned: false,
            stopped: false,
        }
    }
    pub(super) fn register<T: TestDefinition + 'static>(&mut self) -> Result<()> {
        self.register_in::<T>(String::new())
    }
    pub(super) fn register_in<T: TestDefinition + 'static>(
        &mut self,
        context: String,
    ) -> Result<()> {
        if self.started || self.provisioned {
            self.stopped = true;
            anyhow::bail!("Tests must be registered before provisioning");
        }
        let mut roles = std::collections::BTreeSet::new();
        if !T::ACCOUNTS.iter().all(|role| roles.insert(*role)) {
            self.stopped = true;
            anyhow::bail!("Duplicate test account role");
        }
        let id = self.recorder.register(T::NAME, context)?;
        self.tests.push(registered::Registered::new::<T>(id));
        Ok(())
    }
    pub fn account_count(&self) -> usize {
        self.tests.iter().map(|test| test.roles().len()).sum()
    }
    pub fn provision(&mut self, accounts: Vec<Account>) -> Result<()> {
        let result = (|| {
            ensure!(
                !self.started && !self.provisioned && !self.stopped,
                "Accounts can only be provisioned once before execution"
            );
            ensure!(
                accounts.len() == self.account_count(),
                "Wrong number of provisioned accounts"
            );
            let mut accounts = accounts.into_iter();
            for test in &mut self.tests {
                self.cancellation.check()?;
                let roles = test.roles().to_vec();
                test.provision(
                    TestAccounts::new(&roles, accounts.by_ref().take(roles.len()).collect())?,
                    &mut self.recorder,
                )?;
            }
            self.provisioned = true;
            Ok(())
        })();
        if result.is_err() {
            self.stopped = true;
        }
        result
    }
    fn notify<'a, F>(
        &'a mut self,
        method: &'static str,
        context: &'a TestContext<'_>,
        invoke: F,
    ) -> RegistryWork<'a>
    where
        F: for<'b, 'net> Fn(
                &'b mut (dyn UpgradeTest + 'static),
                &'b TestContext<'net>,
                &'b mut Verification,
            ) -> ProgressWork<'b>
            + 'a,
    {
        Box::pin(async move {
            ensure!(
                !self.stopped,
                "Test execution stopped after a previous failure"
            );
            self.started = true;
            let result = async {
                ensure!(self.provisioned, "Test accounts have not been provisioned");
                self.recorder.validate()?;
                self.cancellation.check()?;
                for test in &mut self.tests {
                    test.invoke(
                        method,
                        context,
                        &mut self.recorder,
                        &self.cancellation,
                        &invoke,
                    )
                    .await?;
                }
                self.cancellation.check()
            }
            .await;
            self.stopped = result.is_err();
            result
        })
    }
    pub fn validator_offline<'a>(
        &'a mut self,
        index: usize,
        context: &'a TestContext<'_>,
    ) -> RegistryWork<'a> {
        self.notify(
            "validator_offline",
            context,
            move |test, context, verification| test.validator_offline(index, context, verification),
        )
    }
    pub fn validator_rejoined<'a>(
        &'a mut self,
        index: usize,
        context: &'a TestContext<'_>,
    ) -> RegistryWork<'a> {
        self.notify(
            "validator_rejoined",
            context,
            move |test, context, verification| {
                test.validator_rejoined(index, context, verification)
            },
        )
    }
    pub fn mixed_version<'a>(&'a mut self, context: &'a TestContext<'_>) -> RegistryWork<'a> {
        self.notify("mixed_version", context, |test, context, verification| {
            test.mixed_version(context, verification)
        })
    }
    pub fn before_activation<'a>(&'a mut self, context: &'a TestContext<'_>) -> RegistryWork<'a> {
        self.notify(
            "before_activation",
            context,
            |test, context, verification| test.before_activation(context, verification),
        )
    }
    pub fn after_activation<'a>(&'a mut self, context: &'a TestContext<'_>) -> RegistryWork<'a> {
        self.notify(
            "after_activation",
            context,
            |test, context, verification| test.after_activation(context, verification),
        )
    }
    pub fn preservation_verified<'a>(
        &'a mut self,
        context: &'a TestContext<'_>,
    ) -> RegistryWork<'a> {
        self.notify(
            "preservation_verified",
            context,
            |test, context, verification| test.preservation_verified(context, verification),
        )
    }
    pub fn operations_complete<'a>(&'a mut self, context: &'a TestContext<'_>) -> RegistryWork<'a> {
        self.notify(
            "operations_complete",
            context,
            |test, context, verification| test.operations_complete(context, verification),
        )
    }
    pub fn before_observer_upgrade<'a>(
        &'a mut self,
        context: &'a TestContext<'_>,
    ) -> RegistryWork<'a> {
        self.notify(
            "before_observer_upgrade",
            context,
            |test, context, verification| test.before_observer_upgrade(context, verification),
        )
    }
    pub fn observer_recovered<'a>(&'a mut self, context: &'a TestContext<'_>) -> RegistryWork<'a> {
        self.notify(
            "observer_recovered",
            context,
            |test, context, verification| test.observer_recovered(context, verification),
        )
    }
    pub fn all_nodes_ready<'a>(&'a mut self, context: &'a TestContext<'_>) -> RegistryWork<'a> {
        self.notify("all_nodes_ready", context, |test, context, verification| {
            test.all_nodes_ready(context, verification)
        })
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
        if let Err(error) = self.cancellation.check() {
            errors.push(format!("termination requested at completion: {error:#}"));
        }
        if self.stopped {
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
    use super::super::upgrade_test::TestDefinition;
    use super::super::upgrade_test::{TestProgress, Verification};
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
                genesis: GenesisFixture::four_validator_p10()?,
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
    impl TestDefinition for Pending {
        const NAME: &'static str = "Default methods stay unprocessed";
        fn new(_: &mut TestAccounts) -> Result<Self> {
            Ok(Self)
        }
    }
    #[async_trait::async_trait(?Send)]
    impl UpgradeTest for Pending {}
    struct Passed;
    impl TestDefinition for Passed {
        const NAME: &'static str = "One successful criterion";
        fn new(_: &mut TestAccounts) -> Result<Self> {
            Ok(Self)
        }
    }
    #[async_trait::async_trait(?Send)]
    impl UpgradeTest for Passed {
        async fn mixed_version(
            &mut self,
            _: &TestContext<'_>,
            verification: &mut Verification,
        ) -> Result<TestProgress> {
            verification.verify(async { Ok(()) }).await
        }
    }
    struct Failed;
    impl TestDefinition for Failed {
        const NAME: &'static str = "Failed criterion";
        fn new(_: &mut TestAccounts) -> Result<Self> {
            Ok(Self)
        }
    }
    #[async_trait::async_trait(?Send)]
    impl UpgradeTest for Failed {
        async fn mixed_version(
            &mut self,
            _: &TestContext<'_>,
            verification: &mut Verification,
        ) -> Result<TestProgress> {
            verification
                .verify(async { Err(anyhow::anyhow!("RPC disconnected").context("read balances")) })
                .await
        }
    }
    struct SetupFailed;
    impl TestDefinition for SetupFailed {
        const NAME: &'static str = "Setup failure is not verification failure";
        fn new(_: &mut TestAccounts) -> Result<Self> {
            Ok(Self)
        }
    }
    #[async_trait::async_trait(?Send)]
    impl UpgradeTest for SetupFailed {
        async fn mixed_version(
            &mut self,
            _: &TestContext<'_>,
            _: &mut Verification,
        ) -> Result<TestProgress> {
            anyhow::bail!("fixture RPC failed")
        }
    }
    struct Stateful {
        value: usize,
    }
    impl TestDefinition for Stateful {
        const NAME: &'static str = "State survives direct method calls";
        fn new(_: &mut TestAccounts) -> Result<Self> {
            Ok(Self { value: 0 })
        }
    }
    #[async_trait::async_trait(?Send)]
    impl UpgradeTest for Stateful {
        async fn before_activation(
            &mut self,
            _: &TestContext<'_>,
            _: &mut Verification,
        ) -> Result<TestProgress> {
            self.value = 7;
            Ok(TestProgress::Pending)
        }
        async fn after_activation(
            &mut self,
            _: &TestContext<'_>,
            verification: &mut Verification,
        ) -> Result<TestProgress> {
            verification
                .verify(async {
                    ensure!(self.value == 7, "Lost state");
                    self.value = 8;
                    Ok(())
                })
                .await
        }
    }
    struct BarePass;
    impl TestDefinition for BarePass {
        const NAME: &'static str = "Bare pass is invalid";
        fn new(_: &mut TestAccounts) -> Result<Self> {
            Ok(Self)
        }
    }
    #[async_trait::async_trait(?Send)]
    impl UpgradeTest for BarePass {
        async fn after_activation(
            &mut self,
            _: &TestContext<'_>,
            _: &mut Verification,
        ) -> Result<TestProgress> {
            Ok(TestProgress::Passed)
        }
    }
    struct ActiveWait;
    impl TestDefinition for ActiveWait {
        const NAME: &'static str = "Active wait";
        fn new(_: &mut TestAccounts) -> Result<Self> {
            Ok(Self)
        }
    }
    #[async_trait::async_trait(?Send)]
    impl UpgradeTest for ActiveWait {
        async fn after_activation(
            &mut self,
            _: &TestContext<'_>,
            verification: &mut Verification,
        ) -> Result<TestProgress> {
            verification.verify(std::future::pending()).await
        }
    }
    struct SetupWait;
    impl TestDefinition for SetupWait {
        const NAME: &'static str = "Setup wait";
        fn new(_: &mut TestAccounts) -> Result<Self> {
            Ok(Self)
        }
    }
    #[async_trait::async_trait(?Send)]
    impl UpgradeTest for SetupWait {
        async fn before_activation(
            &mut self,
            _: &TestContext<'_>,
            _: &mut Verification,
        ) -> Result<TestProgress> {
            std::future::pending().await
        }
    }
    struct Timeout;
    impl TestDefinition for Timeout {
        const NAME: &'static str = "Timed verification";
        fn new(_: &mut TestAccounts) -> Result<Self> {
            Ok(Self)
        }
    }
    #[async_trait::async_trait(?Send)]
    impl UpgradeTest for Timeout {
        async fn after_activation(
            &mut self,
            _: &TestContext<'_>,
            verification: &mut Verification,
        ) -> Result<TestProgress> {
            verification
                .verify(wait::bounded(
                    "test deadline",
                    Duration::from_millis(1),
                    std::future::pending(),
                ))
                .await
        }
    }
    struct ConstructorFailure;
    impl TestDefinition for ConstructorFailure {
        const NAME: &'static str = "Constructor failure";
        const ACCOUNTS: &'static [&'static str] = &["sender"];
        fn new(_: &mut TestAccounts) -> Result<Self> {
            anyhow::bail!("cannot construct fixture")
        }
    }
    #[async_trait::async_trait(?Send)]
    impl UpgradeTest for ConstructorFailure {}
    struct RepeatedVerification;
    impl TestDefinition for RepeatedVerification {
        const NAME: &'static str = "Verification must not repeat";
        fn new(_: &mut TestAccounts) -> Result<Self> {
            Ok(Self)
        }
    }
    #[async_trait::async_trait(?Send)]
    impl UpgradeTest for RepeatedVerification {
        async fn after_activation(
            &mut self,
            _: &TestContext<'_>,
            verification: &mut Verification,
        ) -> Result<TestProgress> {
            verification.verify(async { Ok(()) }).await?;
            verification.verify(async { Ok(()) }).await
        }
    }
    struct SwallowedFailure;
    impl TestDefinition for SwallowedFailure {
        const NAME: &'static str = "Failure cannot be swallowed";
        fn new(_: &mut TestAccounts) -> Result<Self> {
            Ok(Self)
        }
    }
    #[async_trait::async_trait(?Send)]
    impl UpgradeTest for SwallowedFailure {
        async fn after_activation(
            &mut self,
            _: &TestContext<'_>,
            verification: &mut Verification,
        ) -> Result<TestProgress> {
            let _ = verification
                .verify(async { anyhow::bail!("original verification failure") })
                .await;
            Ok(TestProgress::Passed)
        }
    }

    #[tokio::test]
    async fn default_methods_do_not_pass_and_missing_results_fail_without_another_error()
    -> Result<()> {
        let network = Network::new()?;
        let mut registry = TestRegistry::new(Cancellation::new());
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
        let mut registry = TestRegistry::new(Cancellation::new());
        registry.register::<Passed>()?;
        registry.register::<Failed>()?;
        registry.register::<Pending>()?;
        registry.provision(Vec::new())?;
        assert!(registry.mixed_version(&network.context()).await.is_err());
        let output = registry.summary();
        assert!(output.contains("[PASS] One successful criterion"));
        assert!(output.contains("[FAIL] Failed criterion — read balances: RPC disconnected"));
        assert!(output.contains("[UNPROCESSED] Default methods stay unprocessed"));
        assert!(registry.after_activation(&network.context()).await.is_err());
        Ok(())
    }
    #[tokio::test]
    async fn setup_and_invalid_pass_never_fabricate_verification_success() -> Result<()> {
        let network = Network::new()?;
        let mut registry = TestRegistry::new(Cancellation::new());
        registry.register::<SetupFailed>()?;
        registry.provision(Vec::new())?;
        assert!(registry.mixed_version(&network.context()).await.is_err());
        let output = registry.summary();
        assert!(output.contains("[ERROR] Setup failure is not verification failure"));
        assert!(output.contains("verification unprocessed"));
        assert!(!output.contains("[FAIL] Setup failure"));
        let mut registry = TestRegistry::new(Cancellation::new());
        registry.register::<BarePass>()?;
        registry.provision(Vec::new())?;
        assert!(registry.after_activation(&network.context()).await.is_err());
        assert!(registry.summary().contains("verification unprocessed"));
        Ok(())
    }
    #[tokio::test]
    async fn state_is_retained_and_completed_tests_are_not_invoked_again() -> Result<()> {
        let network = Network::new()?;
        let mut registry = TestRegistry::new(Cancellation::new());
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
    async fn cancellation_is_attributed_to_active_verification_only() -> Result<()> {
        let network = Network::new()?;
        let cancellation = Cancellation::new();
        let mut registry = TestRegistry::new(cancellation.clone());
        registry.register::<ActiveWait>()?;
        registry.provision(Vec::new())?;
        let context = network.context();
        let signal = async {
            tokio::task::yield_now().await;
            cancellation.request();
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
    async fn cancellation_in_setup_and_timeouts_keep_their_classification() -> Result<()> {
        let network = Network::new()?;
        let context = network.context();
        let cancellation = Cancellation::new();
        let mut registry = TestRegistry::new(cancellation.clone());
        registry.register::<SetupWait>()?;
        registry.provision(Vec::new())?;
        let signal = async {
            tokio::task::yield_now().await;
            cancellation.request();
        };
        let (result, ()) = tokio::join!(registry.before_activation(&context), signal);
        assert!(result.is_err());
        assert!(registry.summary().contains("[ERROR] Setup wait"));
        assert!(registry.summary().contains("verification unprocessed"));
        let mut registry = TestRegistry::new(Cancellation::new());
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
        let mut registry = TestRegistry::new(Cancellation::new());
        registry.register::<ConstructorFailure>()?;
        let mut genesis = GenesisFixture::four_validator_p10_with_accounts(1)?;
        assert!(
            registry
                .provision(genesis.accounts.split_off(VALIDATOR_COUNT))
                .is_err()
        );
        assert!(registry.summary().contains("[ERROR] Constructor failure"));
        assert!(registry.summary().contains("verification unprocessed"));
        let mut registry = TestRegistry::new(Cancellation::new());
        registry.register::<Pending>()?;
        registry.provision(Vec::new())?;
        assert!(registry.register::<Passed>().is_err());
        assert!(registry.finish(Ok(()), Ok(()), Duration::ZERO).is_err());
        Ok(())
    }
    #[tokio::test]
    async fn repeated_verification_fails_without_replacing_the_first_result() -> Result<()> {
        let network = Network::new()?;
        let mut registry = TestRegistry::new(Cancellation::new());
        registry.register::<RepeatedVerification>()?;
        registry.provision(Vec::new())?;
        assert!(registry.after_activation(&network.context()).await.is_err());
        let output = registry.summary();
        assert!(output.contains("[ERROR] Verification must not repeat"));
        assert!(output.contains("Verification may execute only once"));
        assert!(output.contains("verification passed"));
        assert!(registry.finish(Ok(()), Ok(()), Duration::ZERO).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn retained_verification_errors_cannot_be_swallowed_by_a_method() -> Result<()> {
        let network = Network::new()?;
        let mut registry = TestRegistry::new(Cancellation::new());
        registry.register::<SwallowedFailure>()?;
        registry.provision(Vec::new())?;
        assert!(registry.after_activation(&network.context()).await.is_err());
        assert!(
            registry
                .summary()
                .contains("[FAIL] Failure cannot be swallowed — original verification failure")
        );
        Ok(())
    }
    #[tokio::test]
    async fn finish_accounts_for_cleanup_cancellation_and_outside_run_errors() -> Result<()> {
        let network = Network::new()?;
        let cancellation = Cancellation::new();
        let mut registry = TestRegistry::new(cancellation.clone());
        registry.register::<Passed>()?;
        registry.provision(Vec::new())?;
        registry.mixed_version(&network.context()).await?;
        cancellation.request();
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
    #[test]
    fn repeated_test_families_require_unique_contexts() -> Result<()> {
        let mut registry = TestRegistry::new(Cancellation::new());
        registry.register_in::<Pending>("validator 0".into())?;
        registry.register_in::<Pending>("validator 1".into())?;
        assert!(
            registry
                .register_in::<Pending>("validator 0".into())
                .is_err()
        );
        assert!(registry.finish(Ok(()), Ok(()), Duration::ZERO).is_err());
        Ok(())
    }
}
