//! Preparation, direct lifecycle dispatch, unconditional cleanup and final reporting.

use crate::fixture::{genesis::GenesisAccount, node::NodeFixture};
use anyhow::{Result, ensure};
use bollard::Docker;
use concordium_rust_sdk::{genesis::output::GeneratedKeyPair, id::types::AccountAddress};
use futures_util::future::try_join_all;
use std::{future::Future, pin::Pin};
use tokio_util::sync::CancellationToken;

/// Shared run inputs and read-only validator access for preparation and execution hooks.
pub(super) struct TestContext<'a> {
    /// The runner mutates validators through this borrow; tests receive only `&TestContext`.
    pub validators: &'a mut Vec<NodeFixture>,
    /// All tracked addresses are observable; private signing accounts stay in test state.
    pub accounts: &'a [AccountAddress],
    /// Genesis governance signing keys for chain updates. Do not log these keys.
    pub governance: &'a [GeneratedKeyPair],
    pub docker: &'a Docker,
    pub genesis: &'a [u8],
    pub old_image: &'a str,
    pub new_image: &'a str,
}

/// Cleanup needs only Docker, even when preparation or network startup failed.
pub(super) struct CleanupContext<'a> {
    pub docker: &'a Docker,
}

/// Test hook Ok response signalling whether it is ongoing or completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) enum TestProgress {
    /// Test case is still running.
    Pending,
    /// Test case completed successfully.
    Passed,
}

#[async_trait::async_trait(?Send)]
pub trait ProtocolUpgradeTest {
    /// The display name for the test.
    fn name(&self) -> String;

    /// The number of genesis accounts required by the test.
    /// This number of accounts are then provided to the `prepare_before_update` hook.
    fn required_accounts(&self) -> usize {
        0
    }

    /// Initialize accounts and optional test-owned nodes on the ready, pre-update network.
    async fn prepare_before_update(
        &mut self,
        _context: &TestContext<'_>,
        _accounts: Vec<GenesisAccount>,
    ) -> Result<()> {
        Ok(())
    }

    /// Intended for transactions and other chain operations before the update.
    async fn run_before_update(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }

    /// Inspect operation results and capture baselines before software/protocol updates.
    /// Observation is the intent, not an enforced restriction on transactions.
    async fn finish_before_update(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }

    /// Verify preservation, capture state, or set up the next round of operations.
    async fn prepare_after_update(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }

    /// Intended for transactions and other chain operations after the update.
    async fn run_after_update(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }

    /// Inspect final results and verify test-owned resources, using test-local evidence.
    async fn finish_after_update(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }

    /// Called for every started preparation, including partial failures, after controlled exit.
    /// Cleanup runs independently of acceptance status and execution cancellation.
    async fn cleanup(&mut self, _context: &CleanupContext<'_>) -> Result<()> {
        Ok(())
    }
}

/// Owns tests, prepares them sequentially, and dispatches hooks to running tests.
pub(super) struct TestRegistry {
    /// Tests in registration order, each with private state and one expected result.
    tests: Vec<Registered>,
    /// Cancellation is observed centrally between preparation/execution hooks.
    cancellation: CancellationToken,
}

/// One acceptance check's group, display name, and final outcome.
pub(super) struct TestResult {
    pub group: &'static str,
    pub name: String,
    pub result: Result<()>,
}

/// Registers checks under one display header in the registry's flat test list.
pub(super) struct TestGroup<'a> {
    registry: &'a mut TestRegistry,
    header: &'static str,
}

impl TestGroup<'_> {
    pub(super) fn register<A: ProtocolUpgradeTest + 'static>(&mut self, test_definition: A) {
        self.registry.tests.push(Registered {
            group: self.header,
            test: Box::new(test_definition),
            state: TestState::Unprepared,
        });
    }
}

/// Internal state for each registered test in the registry.
struct Registered {
    group: &'static str,
    /// The test definition.
    test: Box<dyn ProtocolUpgradeTest>,
    /// The run state of the registered test.
    state: TestState,
}

/// Current state of a registered test.
enum TestState {
    /// Initial state once registered.
    Unprepared,
    /// State after successful preparation.
    Running,
    /// State after test hook signals completion.
    Complete,
    /// State once a test hook results in error.
    Failed(anyhow::Error),
}

type HookWork<'a> = Pin<Box<dyn Future<Output = Result<TestProgress>> + 'a>>;

impl TestRegistry {
    pub(super) fn new(cancellation: CancellationToken) -> Self {
        Self {
            tests: Vec::new(),
            cancellation,
        }
    }

    pub(super) fn group(&mut self, header: &'static str) -> TestGroup<'_> {
        TestGroup {
            registry: self,
            header,
        }
    }

    pub fn account_count(&self) -> usize {
        self.tests
            .iter()
            .map(|test| test.test.required_accounts())
            .sum()
    }

    /// Run the prepare hooks distributing the requested accounts between each test.
    ///
    /// Fails immediately if any test results in error during preparation.
    /// Caller must ensure to run cleanup afterwards.
    pub async fn prepare_before_update(
        &mut self,
        context: &TestContext<'_>,
        accounts: Vec<GenesisAccount>,
    ) -> Result<()> {
        let mut accounts = accounts.into_iter();
        for registered in &mut self.tests {
            let accounts = accounts
                .by_ref()
                .take(registered.test.required_accounts())
                .collect();
            ensure!(!self.cancellation.is_cancelled(), "Termination requested");
            // Mark before awaiting so partial preparation is eligible for cleanup.
            registered.state = TestState::Running;
            if let Err(error) = registered
                .test
                .prepare_before_update(context, accounts)
                .await
            {
                let name = registered.test.name();
                registered.state =
                    TestState::Failed(anyhow::anyhow!("prepare_before_update: {error:#}"));
                return Err(error.context(format!("Prepare {name}")));
            }
        }
        Ok(())
    }

    /// Run cleanup hooks for every prepared test in reverse registration order.
    ///
    /// If any cleanup hook fails, the error is collected and remaining cleanup hooks are still run.
    /// The final result is Ok if no errors are produced otherwise an Err representing the
    /// collection of errors are returned.
    pub async fn cleanup(&mut self, context: &CleanupContext<'_>) -> Result<()> {
        let mut errors = Vec::new();
        for registered in self.tests.iter_mut().rev() {
            if matches!(registered.state, TestState::Unprepared) {
                continue;
            }
            if let Err(error) = registered.test.cleanup(context).await {
                errors.push(format!("{}: {error:#}", registered.test.name()));
            }
        }
        ensure!(
            errors.is_empty(),
            "Test cleanup failed: {}",
            errors.join("; ")
        );
        Ok(())
    }

    /// Trigger test hooks on every running test and update their run state based on the result.
    ///
    /// If any hook call fails, the error is collected and remaining hooks are still run.
    /// The final result is Ok if no errors are produced otherwise an Err representing the
    /// collection of errors are returned.
    pub async fn run_test_hook<'a>(
        &'a mut self,
        hook: &impl Fn(&'a mut dyn ProtocolUpgradeTest) -> HookWork<'a>,
    ) -> Result<()> {
        let result = try_join_all(self.tests.iter_mut().map(|registered| {
            let cancellation = self.cancellation.clone();
            async move {
                if !matches!(registered.state, TestState::Running) {
                    return Ok(None);
                }
                ensure!(!cancellation.is_cancelled(), "Termination requested");
                let name = registered.test.name();
                registered.state = match hook(registered.test.as_mut()).await {
                    Ok(TestProgress::Passed) => TestState::Complete,
                    Ok(TestProgress::Pending) => TestState::Running,
                    Err(err) => TestState::Failed(err),
                };
                if let TestState::Failed(err) = &registered.state {
                    Ok(Some(format!("{name}: {err}")))
                } else {
                    Ok(None)
                }
            }
        }))
        .await?;
        let errors: Vec<_> = result.into_iter().flatten().collect();
        ensure!(
            errors.is_empty(),
            "Test phase failed: {}",
            errors.join("; ")
        );
        Ok(())
    }

    /// Convert the registry of tests into the final test results.
    pub fn finish(self) -> Vec<TestResult> {
        self.tests
            .into_iter()
            .map(|registered| {
                let result = match registered.state {
                    TestState::Unprepared | TestState::Running => Err(anyhow::anyhow!(
                        "Incomplete test: {}",
                        registered.test.name()
                    )),
                    TestState::Failed(err) => Err(err),
                    TestState::Complete => Ok(()),
                };
                TestResult {
                    group: registered.group,
                    name: registered.test.name(),
                    result,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    //! Lifecycle regression tests. The Docker client is never used for requests.
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Copy, Default)]
    enum Outcome {
        #[default]
        Pending,
        Passed,
        Failed,
    }

    struct FakeTest {
        name: &'static str,
        events: Rc<RefCell<Vec<String>>>,
        prepare_fails: bool,
        run: Outcome,
        cleanup_fails: bool,
        cancel_on_run: Option<CancellationToken>,
    }

    impl FakeTest {
        fn new(name: &'static str, events: &Rc<RefCell<Vec<String>>>) -> Self {
            Self {
                name,
                events: events.clone(),
                prepare_fails: false,
                run: Outcome::Pending,
                cleanup_fails: false,
                cancel_on_run: None,
            }
        }

        fn record(&self, hook: &str) {
            self.events
                .borrow_mut()
                .push(format!("{}:{hook}", self.name));
        }
    }

    #[async_trait::async_trait(?Send)]
    impl ProtocolUpgradeTest for FakeTest {
        fn name(&self) -> String {
            self.name.into()
        }

        async fn prepare_before_update(
            &mut self,
            _context: &TestContext<'_>,
            _accounts: Vec<GenesisAccount>,
        ) -> Result<()> {
            self.record("prepare");
            ensure!(!self.prepare_fails, "preparation failed");
            Ok(())
        }

        async fn run_before_update(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
            self.record("run");
            if let Some(cancellation) = &self.cancel_on_run {
                cancellation.cancel();
            }
            match self.run {
                Outcome::Pending => Ok(TestProgress::Pending),
                Outcome::Passed => Ok(TestProgress::Passed),
                Outcome::Failed => anyhow::bail!("run failed"),
            }
        }

        async fn run_after_update(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
            self.record("after");
            Ok(TestProgress::Passed)
        }

        async fn cleanup(&mut self, _context: &CleanupContext<'_>) -> Result<()> {
            self.record("cleanup");
            ensure!(!self.cleanup_fails, "cleanup failed");
            Ok(())
        }
    }

    struct Fixtures {
        docker: Docker,
        validators: Vec<NodeFixture>,
    }

    impl Fixtures {
        fn new() -> Result<Self> {
            Ok(Self {
                docker: Docker::connect_with_local_defaults()?,
                validators: Vec::new(),
            })
        }

        fn context(&mut self) -> TestContext<'_> {
            TestContext {
                validators: &mut self.validators,
                accounts: &[],
                governance: &[],
                docker: &self.docker,
                genesis: &[],
                old_image: "old",
                new_image: "new",
            }
        }
    }

    #[tokio::test]
    async fn partial_preparation_stops_and_cleans_up_in_reverse_order() -> Result<()> {
        let mut fixtures = Fixtures::new()?;
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut registry = TestRegistry::new(CancellationToken::new());
        let first = FakeTest::new("first", &events);
        let mut second = FakeTest::new("second", &events);
        second.prepare_fails = true;
        second.cleanup_fails = true;
        let mut group = registry.group("fake");
        group.register(first);
        group.register(second);
        group.register(FakeTest::new("unstarted", &events));

        assert!(
            registry
                .prepare_before_update(&fixtures.context(), Vec::new())
                .await
                .is_err()
        );
        assert!(
            registry
                .cleanup(&CleanupContext {
                    docker: &fixtures.docker
                })
                .await
                .is_err()
        );
        assert_eq!(
            *events.borrow(),
            [
                "first:prepare",
                "second:prepare",
                "second:cleanup",
                "first:cleanup"
            ]
        );
        let results = registry.finish();
        assert!(results.iter().all(|test| test.result.is_err()));
        assert!(
            format!("{:#}", results[1].result.as_ref().unwrap_err())
                .contains("prepare_before_update")
        );
        Ok(())
    }

    #[tokio::test]
    async fn phase_failure_finishes_eligible_tests_but_prevents_advancement() -> Result<()> {
        let mut fixtures = Fixtures::new()?;
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut registry = TestRegistry::new(CancellationToken::new());
        let mut failing = FakeTest::new("failing", &events);
        failing.run = Outcome::Failed;
        let mut passing = FakeTest::new("passing", &events);
        passing.run = Outcome::Passed;
        let mut group = registry.group("fake");
        group.register(failing);
        group.register(passing);
        let context = fixtures.context();
        registry.prepare_before_update(&context, Vec::new()).await?;

        // Mirror the runner's `?` barrier between phases.
        let outcome: Result<()> = async {
            registry
                .run_test_hook(&|test| test.run_before_update(&context))
                .await?;
            registry
                .run_test_hook(&|test| test.run_after_update(&context))
                .await
        }
        .await;
        assert!(outcome.is_err());
        assert_eq!(
            *events.borrow(),
            [
                "failing:prepare",
                "passing:prepare",
                "failing:run",
                "passing:run"
            ]
        );
        let results = registry.finish();
        assert!(results[0].result.is_err());
        assert!(results[1].result.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn completed_tests_skip_later_hooks_but_still_receive_cleanup() -> Result<()> {
        let mut fixtures = Fixtures::new()?;
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut registry = TestRegistry::new(CancellationToken::new());
        let mut early = FakeTest::new("early", &events);
        early.run = Outcome::Passed;
        let mut group = registry.group("fake");
        group.register(early);
        group.register(FakeTest::new("ongoing", &events));
        let context = fixtures.context();
        registry.prepare_before_update(&context, Vec::new()).await?;
        registry
            .run_test_hook(&|test| test.run_before_update(&context))
            .await?;
        registry
            .run_test_hook(&|test| test.run_after_update(&context))
            .await?;
        registry
            .cleanup(&CleanupContext {
                docker: &fixtures.docker,
            })
            .await?;
        assert_eq!(
            *events.borrow(),
            [
                "early:prepare",
                "ongoing:prepare",
                "early:run",
                "ongoing:run",
                "ongoing:after",
                "ongoing:cleanup",
                "early:cleanup"
            ]
        );
        assert!(registry.finish().iter().all(|test| test.result.is_ok()));
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_between_hooks_stops_execution_but_not_cleanup() -> Result<()> {
        let mut fixtures = Fixtures::new()?;
        let events = Rc::new(RefCell::new(Vec::new()));
        let cancellation = CancellationToken::new();
        let mut registry = TestRegistry::new(cancellation.clone());
        let mut first = FakeTest::new("first", &events);
        first.cancel_on_run = Some(cancellation);
        first.run = Outcome::Passed;
        let mut group = registry.group("fake");
        group.register(first);
        group.register(FakeTest::new("second", &events));
        let context = fixtures.context();
        registry.prepare_before_update(&context, Vec::new()).await?;
        assert!(
            registry
                .run_test_hook(&|test| test.run_before_update(&context))
                .await
                .is_err()
        );
        registry
            .cleanup(&CleanupContext {
                docker: &fixtures.docker,
            })
            .await?;
        assert_eq!(
            *events.borrow(),
            [
                "first:prepare",
                "second:prepare",
                "first:run",
                "second:cleanup",
                "first:cleanup"
            ]
        );
        let results = registry.finish();
        assert!(results[0].result.is_ok());
        assert!(results[1].result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_before_preparation_does_not_start_or_clean_up_tests() -> Result<()> {
        let mut fixtures = Fixtures::new()?;
        let events = Rc::new(RefCell::new(Vec::new()));
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let mut registry = TestRegistry::new(cancellation);
        registry
            .group("fake")
            .register(FakeTest::new("unstarted", &events));
        assert!(
            registry
                .prepare_before_update(&fixtures.context(), Vec::new())
                .await
                .is_err()
        );
        registry
            .cleanup(&CleanupContext {
                docker: &fixtures.docker,
            })
            .await?;
        assert!(events.borrow().is_empty());
        assert!(registry.finish()[0].result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn final_hook_cancellation_does_not_fail_completed_execution() -> Result<()> {
        let mut fixtures = Fixtures::new()?;
        let events = Rc::new(RefCell::new(Vec::new()));
        let cancellation = CancellationToken::new();
        let mut registry = TestRegistry::new(cancellation.clone());
        let mut test = FakeTest::new("last", &events);
        test.cancel_on_run = Some(cancellation);
        test.run = Outcome::Passed;
        registry.group("fake").register(test);
        let context = fixtures.context();
        registry.prepare_before_update(&context, Vec::new()).await?;
        registry
            .run_test_hook(&|test| test.run_before_update(&context))
            .await?;
        assert!(registry.finish()[0].result.is_ok());
        Ok(())
    }
}
