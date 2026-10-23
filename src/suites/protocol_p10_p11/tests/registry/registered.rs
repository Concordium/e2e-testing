//! Deferred construction and verification handling for one runtime test.
use super::super::upgrade_test::{TestDefinition, TestProgress, Verification, VerificationState};
use super::*;
use anyhow::Context as _;

/// Construct private test state only after genesis accounts have been allocated.
type Constructor = fn(&mut TestAccounts) -> Result<Box<dyn UpgradeTest>>;

pub(super) struct Registered {
    id: TestId,
    name: &'static str,
    roles: &'static [&'static str],
    construct: Constructor,
    test: Option<Box<dyn UpgradeTest>>,
    completed: bool,
}
impl Registered {
    pub fn new<T: TestDefinition + 'static>(id: TestId) -> Self {
        Self {
            id,
            name: T::NAME,
            roles: T::ACCOUNTS,
            construct: |accounts| Ok(Box::new(T::new(accounts)?)),
            test: None,
            completed: false,
        }
    }
    pub fn roles(&self) -> &[&'static str] {
        self.roles
    }
    pub fn provision(&mut self, mut accounts: TestAccounts, recorder: &mut Recorder) -> Result<()> {
        let result = (|| {
            let test = (self.construct)(&mut accounts)?;
            ensure!(
                accounts.all_taken(),
                "Test did not take all declared accounts"
            );
            Ok(test)
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
    pub async fn invoke<F>(
        &mut self,
        method: &'static str,
        context: &TestContext<'_>,
        recorder: &mut Recorder,
        cancellation: &Cancellation,
        invoke: F,
    ) -> Result<()>
    where
        F: for<'b, 'net> FnOnce(
            &'b mut (dyn UpgradeTest + 'static),
            &'b TestContext<'net>,
            &'b mut Verification,
        ) -> ProgressWork<'b>,
    {
        if self.completed {
            return Ok(());
        }
        cancellation.check()?;
        let test = self
            .test
            .as_mut()
            .context("Test state has not been provisioned")?;
        let mut verification = Verification::new();
        let result = cancellation
            .run(invoke(test.as_mut(), context, &mut verification))
            .await;
        match verification.into_state() {
            VerificationState::Failed(error) => recorder.record(self.id, Err(error)),
            VerificationState::Active => recorder.record(
                self.id,
                Err(result
                    .err()
                    .unwrap_or_else(|| anyhow::anyhow!("Verification did not finish"))),
            ),
            VerificationState::Passed => {
                recorder.record(self.id, Ok(()))?;
                self.completed = true;
                match result {
                    Ok(TestProgress::Passed) => Ok(()),
                    other => {
                        let error = other.err().unwrap_or_else(|| {
                            anyhow::anyhow!("Verification passed but its method returned Pending")
                        });
                        let message = format!("{} after verification: {error:#}", self.name);
                        recorder.setup_failed(self.id, error.context(method))?;
                        anyhow::bail!("{message}");
                    }
                }
            }
            VerificationState::Pending => {
                let error = match result {
                    Ok(TestProgress::Pending) => return Ok(()),
                    Ok(TestProgress::Passed) => {
                        anyhow::anyhow!("Passed requires the verification execution boundary")
                    }
                    Err(error) => error,
                };
                let error = error.context(method);
                let message = format!("{} setup: {error:#}", self.name);
                recorder.setup_failed(self.id, error)?;
                anyhow::bail!("{message}");
            }
        }
    }
}
