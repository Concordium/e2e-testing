//! Preparation, direct lifecycle dispatch, unconditional cleanup and final reporting.

use crate::fixture::{genesis::GenesisAccount, node::NodeFixture};
use crate::registry::TestRegistry;
use crate::report::Outcome as ReportOutcome;
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

pub(super) type UpgradeRegistry = TestRegistry<Box<dyn ProtocolUpgradeTest>>;

type HookWork<'a> = Pin<Box<dyn Future<Output = Result<TestProgress>> + 'a>>;

pub(super) fn account_count(executor: &UpgradeRegistry) -> usize {
    executor
        .entries()
        .iter()
        .map(|entry| entry.definition.required_accounts())
        .sum()
}

/// Prepare tests sequentially, assigning exclusive accounts in registration order.
/// Stop on failure; callers must still clean up every started preparation.
pub(super) async fn prepare_before_update(
    executor: &mut UpgradeRegistry,
    context: &TestContext<'_>,
    accounts: Vec<GenesisAccount>,
    cancellation: &CancellationToken,
) -> Result<()> {
    let mut accounts = accounts.into_iter();
    for registered in executor.entries_mut() {
        let accounts = accounts
            .by_ref()
            .take(registered.definition.required_accounts())
            .collect();
        ensure!(!cancellation.is_cancelled(), "Termination requested");
        // Mark before awaiting so partial preparation is eligible for cleanup.
        registered.check.outcome = ReportOutcome::Incomplete;
        if let Err(error) = registered
            .definition
            .prepare_before_update(context, accounts)
            .await
        {
            let name = &registered.check.name;
            registered.check.outcome =
                ReportOutcome::Failed(format!("prepare_before_update: {error:#}"));
            return Err(error.context(format!("Prepare {name}")));
        }
    }
    Ok(())
}

/// Clean up every started preparation in reverse registration order.
/// Attempt all eligible cleanups and aggregate failures.
pub(super) async fn cleanup(
    executor: &mut UpgradeRegistry,
    context: &CleanupContext<'_>,
) -> Result<()> {
    let mut errors = Vec::new();
    for registered in executor.entries_mut().iter_mut().rev() {
        if matches!(registered.check.outcome, ReportOutcome::NotRun) {
            continue;
        }
        if let Err(error) = registered.definition.cleanup(context).await {
            errors.push(format!("{}: {error:#}", registered.check.name));
        }
    }
    ensure!(
        errors.is_empty(),
        "Test cleanup failed: {}",
        errors.join("; ")
    );
    Ok(())
}

/// Dispatch a phase to incomplete tests and aggregate hook failures.
pub(super) async fn run_test_hook<'a>(
    executor: &'a mut UpgradeRegistry,
    hook: &impl Fn(&'a mut dyn ProtocolUpgradeTest) -> HookWork<'a>,
    cancellation: &CancellationToken,
) -> Result<()> {
    let result = try_join_all(executor.entries_mut().iter_mut().map(|registered| {
        let cancellation = cancellation.clone();
        async move {
            if !matches!(registered.check.outcome, ReportOutcome::Incomplete) {
                return Ok(None);
            }
            ensure!(!cancellation.is_cancelled(), "Termination requested");
            let name = &registered.check.name;
            registered.check.outcome = match hook(registered.definition.as_mut()).await {
                Ok(TestProgress::Passed) => ReportOutcome::Passed,
                Ok(TestProgress::Pending) => ReportOutcome::Incomplete,
                Err(err) => ReportOutcome::Failed(format!("{err:#}")),
            };
            if let ReportOutcome::Failed(err) = &registered.check.outcome {
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
        let cancellation = CancellationToken::new();
        let mut registry = UpgradeRegistry::default();
        let first = FakeTest::new("first", &events);
        let mut second = FakeTest::new("second", &events);
        second.prepare_fails = true;
        second.cleanup_fails = true;
        let mut group = registry.group("fake");
        group.register("first", Box::new(first));
        group.register("second", Box::new(second));
        group.register("unstarted", Box::new(FakeTest::new("unstarted", &events)));

        assert!(
            prepare_before_update(
                &mut registry,
                &fixtures.context(),
                Vec::new(),
                &cancellation
            )
            .await
            .is_err()
        );
        assert!(
            cleanup(
                &mut registry,
                &CleanupContext {
                    docker: &fixtures.docker,
                },
            )
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
        let results = registry.into_checks();
        assert!(matches!(results[0].outcome, ReportOutcome::Incomplete));
        assert!(matches!(
            &results[1].outcome,
            ReportOutcome::Failed(message) if message.contains("prepare_before_update")
        ));
        assert!(matches!(results[2].outcome, ReportOutcome::NotRun));
        Ok(())
    }

    #[tokio::test]
    async fn phase_failure_finishes_eligible_tests_but_prevents_advancement() -> Result<()> {
        let mut fixtures = Fixtures::new()?;
        let events = Rc::new(RefCell::new(Vec::new()));
        let cancellation = CancellationToken::new();
        let mut registry = UpgradeRegistry::default();
        let mut failing = FakeTest::new("failing", &events);
        failing.run = Outcome::Failed;
        let mut passing = FakeTest::new("passing", &events);
        passing.run = Outcome::Passed;
        let mut group = registry.group("fake");
        group.register("failing", Box::new(failing));
        group.register("passing", Box::new(passing));
        let context = fixtures.context();
        prepare_before_update(&mut registry, &context, Vec::new(), &cancellation).await?;

        // Mirror the runner's `?` barrier between phases.
        let outcome: Result<()> = async {
            run_test_hook(
                &mut registry,
                &|test| test.run_before_update(&context),
                &cancellation,
            )
            .await?;
            run_test_hook(
                &mut registry,
                &|test| test.run_after_update(&context),
                &cancellation,
            )
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
        let results = registry.into_checks();
        assert!(matches!(results[0].outcome, ReportOutcome::Failed(_)));
        assert!(results[1].outcome.is_passed());
        Ok(())
    }

    #[tokio::test]
    async fn completed_tests_skip_later_hooks_but_still_receive_cleanup() -> Result<()> {
        let mut fixtures = Fixtures::new()?;
        let events = Rc::new(RefCell::new(Vec::new()));
        let cancellation = CancellationToken::new();
        let mut registry = UpgradeRegistry::default();
        let mut early = FakeTest::new("early", &events);
        early.run = Outcome::Passed;
        let mut group = registry.group("fake");
        group.register("early", Box::new(early));
        group.register("ongoing", Box::new(FakeTest::new("ongoing", &events)));
        let context = fixtures.context();
        prepare_before_update(&mut registry, &context, Vec::new(), &cancellation).await?;
        run_test_hook(
            &mut registry,
            &|test| test.run_before_update(&context),
            &cancellation,
        )
        .await?;
        run_test_hook(
            &mut registry,
            &|test| test.run_after_update(&context),
            &cancellation,
        )
        .await?;
        cleanup(
            &mut registry,
            &CleanupContext {
                docker: &fixtures.docker,
            },
        )
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
        assert!(
            registry
                .into_checks()
                .iter()
                .all(|test| test.outcome.is_passed())
        );
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_between_hooks_stops_execution_but_not_cleanup() -> Result<()> {
        let mut fixtures = Fixtures::new()?;
        let events = Rc::new(RefCell::new(Vec::new()));
        let cancellation = CancellationToken::new();
        let mut registry = UpgradeRegistry::default();
        let mut first = FakeTest::new("first", &events);
        first.cancel_on_run = Some(cancellation.clone());
        first.run = Outcome::Passed;
        let mut group = registry.group("fake");
        group.register("first", Box::new(first));
        group.register("second", Box::new(FakeTest::new("second", &events)));
        let context = fixtures.context();
        prepare_before_update(&mut registry, &context, Vec::new(), &cancellation).await?;
        assert!(
            run_test_hook(
                &mut registry,
                &|test| test.run_before_update(&context),
                &cancellation
            )
            .await
            .is_err()
        );
        cleanup(
            &mut registry,
            &CleanupContext {
                docker: &fixtures.docker,
            },
        )
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
        let results = registry.into_checks();
        assert!(results[0].outcome.is_passed());
        assert!(matches!(results[1].outcome, ReportOutcome::Incomplete));
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_before_preparation_does_not_start_or_clean_up_tests() -> Result<()> {
        let mut fixtures = Fixtures::new()?;
        let events = Rc::new(RefCell::new(Vec::new()));
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let mut registry = UpgradeRegistry::default();
        registry
            .group("fake")
            .register("unstarted", Box::new(FakeTest::new("unstarted", &events)));
        assert!(
            prepare_before_update(
                &mut registry,
                &fixtures.context(),
                Vec::new(),
                &cancellation
            )
            .await
            .is_err()
        );
        cleanup(
            &mut registry,
            &CleanupContext {
                docker: &fixtures.docker,
            },
        )
        .await?;
        assert!(events.borrow().is_empty());
        assert!(matches!(
            registry.into_checks()[0].outcome,
            ReportOutcome::NotRun
        ));
        Ok(())
    }

    #[tokio::test]
    async fn final_hook_cancellation_does_not_fail_completed_execution() -> Result<()> {
        let mut fixtures = Fixtures::new()?;
        let events = Rc::new(RefCell::new(Vec::new()));
        let cancellation = CancellationToken::new();
        let mut registry = UpgradeRegistry::default();
        let mut test = FakeTest::new("last", &events);
        test.cancel_on_run = Some(cancellation.clone());
        test.run = Outcome::Passed;
        registry.group("fake").register("last", Box::new(test));
        let context = fixtures.context();
        prepare_before_update(&mut registry, &context, Vec::new(), &cancellation).await?;
        run_test_hook(
            &mut registry,
            &|test| test.run_before_update(&context),
            &cancellation,
        )
        .await?;
        assert!(registry.into_checks()[0].outcome.is_passed());
        Ok(())
    }
}
