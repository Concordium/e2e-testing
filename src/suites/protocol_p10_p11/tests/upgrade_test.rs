//! Direct lifecycle methods. Each implementation represents one acceptance criterion.
use super::{super::context::TestContext, accounts_fixture::TestAccounts};
use anyhow::{Result, ensure};
use std::future::Future;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TestProgress {
    Pending,
    Passed,
}
pub(super) enum VerificationState {
    Pending,
    /// Verification started but did not finish. Cancellation leaves this state intact.
    Active,
    Passed,
    Failed(anyhow::Error),
}

/// Execute immediately. Cancellation and errors after entry belong to this verification.
/// The registry reads the retained state after the lifecycle method returns or is cancelled.
/// Pending means setup did not reach verification; Active means verification was interrupted.
pub(super) struct Verification {
    state: VerificationState,
}
impl Verification {
    pub fn new() -> Self {
        Self {
            state: VerificationState::Pending,
        }
    }
    pub fn into_state(self) -> VerificationState {
        self.state
    }
    pub async fn verify(&mut self, work: impl Future<Output = Result<()>>) -> Result<TestProgress> {
        ensure!(
            matches!(self.state, VerificationState::Pending),
            "Verification may execute only once"
        );
        self.state = VerificationState::Active;
        match work.await {
            Ok(()) => {
                self.state = VerificationState::Passed;
                Ok(TestProgress::Passed)
            }
            Err(error) => {
                let message = format!("{error:#}");
                self.state = VerificationState::Failed(error);
                anyhow::bail!("{message}");
            }
        }
    }
}

/// Metadata and construction used before runtime dispatch.
pub(super) trait TestDefinition: UpgradeTest + Sized {
    const NAME: &'static str;
    const ACCOUNTS: &'static [&'static str] = &[];
    fn new(accounts: &mut TestAccounts) -> Result<Self>;
}

/// Registration supplies one expected result. Default methods never satisfy it.
// async_trait adds a redundant must_use attribute to the generated future-returning methods.
#[allow(clippy::double_must_use)]
#[async_trait::async_trait(?Send)]
pub(super) trait UpgradeTest {
    /// Validator 0–3 is stopped at P10; its pre-stop finalized height is available.
    /// The other three validators remain available for finalization and transactions.
    async fn validator_offline(
        &mut self,
        _index: usize,
        _context: &TestContext<'_>,
        _verification: &mut Verification,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// The validator uses new software and its existing database at P10.
    /// It recovered the saved rejoin target. Transactions may run through it.
    async fn validator_rejoined(
        &mut self,
        _index: usize,
        _context: &TestContext<'_>,
        _verification: &mut Verification,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// Validator 0 uses new software at P10; validators 1–3 and observer 4 use old software.
    /// Tests may establish their private fixtures with account transactions.
    async fn mixed_version(
        &mut self,
        _context: &TestContext<'_>,
        _verification: &mut Verification,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// All validators use new software at P10; the observer still uses old software.
    /// Capture baselines. Submit no account transactions until preservation is verified.
    async fn before_activation(
        &mut self,
        _context: &TestContext<'_>,
        _verification: &mut Verification,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// Finalized update-submission and P11-era block references are available.
    /// Capture and verify preserved state without account transactions.
    async fn after_activation(
        &mut self,
        _context: &TestContext<'_>,
        _verification: &mut Verification,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// All activation methods succeeded. Fresh P11 transactions may run.
    async fn preservation_verified(
        &mut self,
        _context: &TestContext<'_>,
        _verification: &mut Verification,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// Fresh-operation methods succeeded; the finalized P11 recovery target is saved.
    /// Tests may save finalized evidence for later observer agreement checks.
    async fn operations_complete(
        &mut self,
        _context: &TestContext<'_>,
        _verification: &mut Verification,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// Observer 4 still uses old software. Logs, process status and P11 target are available.
    async fn before_observer_upgrade(
        &mut self,
        _context: &TestContext<'_>,
        _verification: &mut Verification,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// Observer 4 uses new software and its existing database, and recovered the P11 target.
    async fn observer_recovered(
        &mut self,
        _context: &TestContext<'_>,
        _verification: &mut Verification,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// All five nodes use new software at P11. Compare saved finalized evidence.
    /// A saved snapshot may be later than the recovery target and need a catch-up wait.
    async fn all_nodes_ready(
        &mut self,
        _context: &TestContext<'_>,
        _verification: &mut Verification,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn only_successful_verification_returns_passed_and_repeats_fail() -> Result<()> {
        let mut verification = Verification::new();
        assert_eq!(
            verification.verify(async { Ok(()) }).await?,
            TestProgress::Passed
        );
        assert!(verification.verify(async { Ok(()) }).await.is_err());
        Ok(())
    }
    #[tokio::test]
    async fn original_error_is_retained() {
        let mut verification = Verification::new();
        assert!(
            verification
                .verify(async { Err(anyhow::anyhow!("RPC error").context("read state")) })
                .await
                .is_err()
        );
        let VerificationState::Failed(error) = verification.into_state() else {
            panic!("expected failure")
        };
        assert_eq!(format!("{error:#}"), "read state: RPC error");
    }
}
