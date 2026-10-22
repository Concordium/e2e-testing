//! Direct lifecycle methods. Each implementation represents one acceptance criterion.
use super::super::context::TestContext;
use crate::fixture::genesis::GenesisAccount;
use anyhow::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) enum TestProgress {
    Pending,
    Passed,
}
/// Registration supplies one expected result. Default methods leave it pending.
/// Return Passed only after checking the criterion; any hook error fails it.
pub(in super::super) trait ProtocolUpgradeTest: Sized {
    /// Unique display name, including any validator index or other instance details.
    fn name() -> String;

    /// Number of private genesis accounts to allocate before construction.
    fn required_accounts() -> usize {
        0
    }

    /// Initialize private test state from allocated accounts before the network starts.
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self>;

    /// Validator 0–3 is stopped at P10; its pre-stop finalized height is available.
    /// The other three validators remain available for finalization and transactions.
    async fn on_validator_offline(
        &mut self,
        _index: usize,
        _context: &TestContext<'_>,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// The validator uses new software and its existing database at P10.
    /// It recovered the saved rejoin target. Transactions may run through it.
    async fn on_validator_rejoined(
        &mut self,
        _index: usize,
        _context: &TestContext<'_>,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// Validator 0 uses new software at P10; validators 1–3 and observer 4 use old software.
    /// Tests may establish their private fixtures with account transactions.
    async fn on_mixed_version(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// All validators use new software at P10; the observer still uses old software.
    /// Capture baselines. Submit no account transactions until preservation is verified.
    async fn on_before_activation(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// Finalized update-submission and P11-era block references are available.
    /// Capture and verify preserved state without account transactions.
    async fn on_after_activation(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// All activation methods succeeded. Fresh P11 transactions may run.
    async fn on_preservation_verified(
        &mut self,
        _context: &TestContext<'_>,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// Fresh-operation methods succeeded; the finalized P11 recovery target is saved.
    /// Tests may save finalized evidence for later observer agreement checks.
    async fn on_operations_complete(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// Observer 4 still uses old software. Logs, process status and P11 target are available.
    async fn on_before_observer_upgrade(
        &mut self,
        _context: &TestContext<'_>,
    ) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// Observer 4 uses new software and its existing database, and recovered the P11 target.
    async fn on_observer_recovered(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
    /// All five nodes use new software at P11. Compare saved finalized evidence.
    /// A saved snapshot may be later than the recovery target and need a catch-up wait.
    async fn on_all_nodes_ready(&mut self, _context: &TestContext<'_>) -> Result<TestProgress> {
        Ok(TestProgress::Pending)
    }
}
