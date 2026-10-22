//! Deferred construction and verification handling for one runtime test.
use super::runtime::{Checkpoint, RuntimeTest};
use super::*;
use anyhow::Context as _;

/// Construct private test state only after genesis accounts have been allocated.
type Constructor = fn(Vec<GenesisAccount>) -> Result<Box<dyn RuntimeTest>>;

/// Retains one criterion across hooks and records its first returned pass or failure.
pub(super) struct Registered {
    id: TestId,
    name: String,
    /// Accounts reserved exclusively for this criterion during provisioning.
    account_count: usize,
    construct: Constructor,
    /// Constructed during provisioning, then retained across lifecycle hooks.
    test: Option<Box<dyn RuntimeTest>>,
    /// A returned pass prevents later hooks from running for this criterion.
    completed: bool,
}
impl Registered {
    pub fn new<T: ProtocolUpgradeTest + 'static>(id: TestId, name: String) -> Self {
        Self {
            id,
            name,
            account_count: T::required_accounts(),
            construct: |accounts| Ok(Box::new(T::setup(accounts)?)),
            test: None,
            completed: false,
        }
    }

    pub fn account_count(&self) -> usize {
        self.account_count
    }

    pub fn provision(
        &mut self,
        accounts: Vec<GenesisAccount>,
        recorder: &mut Recorder,
    ) -> Result<()> {
        let result = (|| {
            ensure!(
                accounts.len() == self.account_count,
                "Wrong number of test accounts"
            );
            (self.construct)(accounts)
        })();
        match result {
            Ok(test) => {
                self.test = Some(test);
                Ok(())
            }
            Err(error) => {
                let error = error.context("initialize test state");
                let message = format!("{} setup: {error:#}", self.name);
                recorder.setup_failed(self.id, error)?;
                anyhow::bail!("{message}");
            }
        }
    }

    pub async fn invoke(
        &mut self,
        checkpoint: Checkpoint,
        context: &TestContext<'_>,
        recorder: &mut Recorder,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        if self.completed {
            return Ok(());
        }
        ensure!(!cancellation.is_cancelled(), "Termination signal received");
        let test = self
            .test
            .as_mut()
            .context("Test state has not been provisioned")?;
        match cancellation
            .run_until_cancelled(test.notify(checkpoint, context))
            .await
            .context("Termination signal received")
            .flatten()
        {
            Ok(TestProgress::Pending) => Ok(()),
            Ok(TestProgress::Passed) => {
                recorder.record(self.id, Ok(()))?;
                self.completed = true;
                Ok(())
            }
            Err(error) => recorder.record(self.id, Err(error.context(checkpoint.name()))),
        }
    }
}
