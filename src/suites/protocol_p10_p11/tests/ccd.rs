//! Independent CCD transaction, balance/nonce-preservation and account-agreement criteria.
use super::super::registry::{ProtocolUpgradeTest, TestProgress, TestRegistry};
use super::super::{
    REJOIN_TIMEOUT,
    context::TestContext,
    operations,
    state::{FinalizedBlock, rewards_between},
    wait,
};
use crate::fixture::genesis::GenesisAccount;
use anyhow::{Context as _, Result, ensure};
use concordium_rust_sdk::{common::types::Amount, id::types::AccountAddress, types::Nonce};
use std::collections::BTreeMap;

pub(super) fn register(registry: &mut TestRegistry) -> Result<()> {
    registry.register::<OldSoftwareTransfer>()?;
    registry.register::<NewSoftwareP10Transfer>()?;
    registry.register::<BalancePreservation>()?;
    registry.register::<NoncePreservation>()?;
    registry.register::<P11Transfer>()?;
    registry.register::<MixedAgreement>()?;
    registry.register::<BeforeAgreement>()?;
    registry.register::<AfterAgreement>()?;
    registry.register::<OperationsAgreement>()?;
    registry.register::<AllNodesAgreement>()
}
struct TransferAccounts {
    sender: GenesisAccount,
    recipient: GenesisAccount,
}
impl TransferAccounts {
    fn new(accounts: Vec<GenesisAccount>) -> Result<Self> {
        let [sender, recipient]: [GenesisAccount; 2] =
            accounts.try_into().ok().context("Expected 2 accounts")?;
        Ok(Self { sender, recipient })
    }
}

struct OldSoftwareTransfer(TransferAccounts);
impl ProtocolUpgradeTest for OldSoftwareTransfer {
    fn name() -> String {
        "CCD transfers through old software finalize".into()
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(TransferAccounts::new(accounts)?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        operations::ccd(
            &mut context.client(1).await?,
            &self.0.sender,
            self.0.recipient.address,
        )
        .await?;
        Ok(TestProgress::Passed)
    }
}

struct NewSoftwareP10Transfer(TransferAccounts);
impl ProtocolUpgradeTest for NewSoftwareP10Transfer {
    fn name() -> String {
        "CCD transfers through new software finalize at P10".into()
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(TransferAccounts::new(accounts)?))
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        operations::ccd(
            &mut context.client(0).await?,
            &self.0.sender,
            self.0.recipient.address,
        )
        .await?;
        Ok(TestProgress::Passed)
    }
}

struct P11Transfer(TransferAccounts);
impl ProtocolUpgradeTest for P11Transfer {
    fn name() -> String {
        "A fresh CCD transfer finalizes at P11".into()
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self(TransferAccounts::new(accounts)?))
    }
    async fn on_preservation_verified(
        &mut self,
        context: &TestContext<'_>,
    ) -> Result<TestProgress> {
        let block = operations::ccd(
            &mut context.client(0).await?,
            &self.0.sender,
            self.0.recipient.address,
        )
        .await?;
        operations::require_p11(&mut context.client(0).await?, block).await?;
        Ok(TestProgress::Passed)
    }
}

struct Balances {
    block: FinalizedBlock,
    amounts: BTreeMap<AccountAddress, Amount>,
}
struct BalancePreservation {
    sender: GenesisAccount,
    recipient: GenesisAccount,
    before: Option<Balances>,
}
impl ProtocolUpgradeTest for BalancePreservation {
    fn name() -> String {
        "CCD balances survive activation after accounting for rewards".into()
    }
    fn required_accounts() -> usize {
        2
    }

    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        let [sender, recipient]: [GenesisAccount; 2] =
            accounts.try_into().ok().context("Expected 2 accounts")?;
        Ok(Self {
            sender,
            recipient,
            before: None,
        })
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        let recipient = self.recipient.address;
        operations::ccd(&mut context.client(1).await?, &self.sender, recipient).await?;
        Ok(TestProgress::Pending)
    }

    async fn on_before_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.before = Some(
            wait::bounded(
                "capture pre-update balances",
                REJOIN_TIMEOUT,
                capture_balances(context),
            )
            .await?,
        );
        Ok(TestProgress::Pending)
    }

    async fn on_after_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        wait::bounded("verify balance preservation", REJOIN_TIMEOUT, async {
            let before = self
                .before
                .as_ref()
                .context("No pre-update balance evidence")?;
            let after = capture_balances(context).await?;
            let rewards =
                rewards_between(&mut context.client(0).await?, &before.block, &after.block).await?;
            compare_balances(&before.amounts, &after.amounts, &rewards.balances)
        })
        .await?;
        Ok(TestProgress::Passed)
    }
}

async fn capture_balances(context: &TestContext<'_>) -> Result<Balances> {
    let mut client = context.client(0).await?;
    let block = FinalizedBlock::capture(&mut client).await?;
    let mut amounts = BTreeMap::new();
    for address in context.accounts() {
        let info = client
            .get_account_info(&(*address).into(), block.hash)
            .await?
            .response;
        amounts.insert(*address, info.account_amount);
    }
    Ok(Balances { block, amounts })
}
fn compare_balances(
    before: &BTreeMap<AccountAddress, Amount>,
    after: &BTreeMap<AccountAddress, Amount>,
    rewards: &BTreeMap<AccountAddress, Amount>,
) -> Result<()> {
    ensure!(
        before.keys().eq(after.keys()),
        "Tracked balance accounts changed"
    );
    for (address, amount) in before {
        let reward = rewards.get(address).copied().unwrap_or(Amount::zero());
        ensure!(
            after[address] == *amount + reward,
            "Unexpected CCD balance change for {address} (accounted reward: {reward})"
        );
    }
    Ok(())
}

struct Nonces {
    block: FinalizedBlock,
    values: BTreeMap<AccountAddress, Nonce>,
}
struct NoncePreservation {
    sender: GenesisAccount,
    recipient: GenesisAccount,
    before: Option<Nonces>,
}
impl ProtocolUpgradeTest for NoncePreservation {
    fn name() -> String {
        "Account nonces are preserved".into()
    }
    fn required_accounts() -> usize {
        2
    }
    fn setup(accounts: Vec<GenesisAccount>) -> Result<Self> {
        let [sender, recipient]: [GenesisAccount; 2] =
            accounts.try_into().ok().context("Expected 2 accounts")?;
        Ok(Self {
            sender,
            recipient,
            before: None,
        })
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        operations::ccd(
            &mut context.client(0).await?,
            &self.sender,
            self.recipient.address,
        )
        .await?;
        Ok(TestProgress::Pending)
    }
    async fn on_before_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.before = Some(
            wait::bounded(
                "capture account nonces",
                REJOIN_TIMEOUT,
                capture_nonces(context),
            )
            .await?,
        );
        Ok(TestProgress::Pending)
    }
    async fn on_after_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        wait::bounded("verify nonce preservation", REJOIN_TIMEOUT, async {
            let before = self.before.as_ref().context("No nonce baseline")?;
            let after = capture_nonces(context).await?;
            ensure!(
                after.block.height >= before.block.height,
                "Finalized height regressed"
            );
            ensure!(
                before.values == after.values,
                "Nonces changed across activation"
            );
            Ok(())
        })
        .await?;
        Ok(TestProgress::Passed)
    }
}
async fn capture_nonces(context: &TestContext<'_>) -> Result<Nonces> {
    let mut client = context.client(0).await?;
    let block = FinalizedBlock::capture(&mut client).await?;
    let mut values = BTreeMap::new();
    for address in context.accounts() {
        let info = client
            .get_account_info(&(*address).into(), block.hash)
            .await?
            .response;
        values.insert(*address, info.account_nonce);
    }
    Ok(Nonces { block, values })
}
#[derive(PartialEq, Debug)]
struct CcdAccount {
    amount: Amount,
    nonce: Nonce,
}
struct Snapshot {
    block: FinalizedBlock,
    accounts: BTreeMap<AccountAddress, CcdAccount>,
}
async fn capture(context: &TestContext<'_>) -> Result<Snapshot> {
    let mut client = context.client(0).await?;
    let block = FinalizedBlock::capture(&mut client).await?;
    let mut accounts = BTreeMap::new();
    for address in context.accounts() {
        let info = client
            .get_account_info(&(*address).into(), block.hash)
            .await?
            .response;
        accounts.insert(
            *address,
            CcdAccount {
                amount: info.account_amount,
                nonce: info.account_nonce,
            },
        );
    }
    Ok(Snapshot { block, accounts })
}
async fn compare(context: &TestContext<'_>, snapshot: &Snapshot, observer: bool) -> Result<()> {
    for node in context.node_indices(observer) {
        let mut client = context.client(node).await?;
        ensure!(
            client
                .get_block_info(snapshot.block.hash)
                .await?
                .response
                .finalized,
            "Node {node} has not finalized CCD evidence"
        );
        for (address, expected) in &snapshot.accounts {
            let info = client
                .get_account_info(&(*address).into(), snapshot.block.hash)
                .await?
                .response;
            ensure!(
                info.account_amount == expected.amount && info.account_nonce == expected.nonce,
                "Node {node} disagrees on CCD account {address}"
            );
        }
    }
    Ok(())
}

async fn agreement(
    context: &TestContext<'_>,
    reference: Option<&Snapshot>,
    observer: bool,
) -> Result<()> {
    let current;
    let snapshot = match reference {
        Some(saved) => saved,
        None => {
            current = wait::bounded(
                "capture CCD agreement evidence",
                REJOIN_TIMEOUT,
                capture(context),
            )
            .await?;
            &current
        }
    };
    wait::agreement(|| compare(context, snapshot, observer)).await
}

struct MixedAgreement;
impl ProtocolUpgradeTest for MixedAgreement {
    fn name() -> String {
        "Mixed-version nodes agree on finalized CCD balances and nonces".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_mixed_version(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, None, true).await?;
        Ok(TestProgress::Passed)
    }
}

struct BeforeAgreement;
impl ProtocolUpgradeTest for BeforeAgreement {
    fn name() -> String {
        "All nodes agree on pre-update CCD balances and nonces".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_before_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, None, true).await?;
        Ok(TestProgress::Passed)
    }
}

struct AfterAgreement;
impl ProtocolUpgradeTest for AfterAgreement {
    fn name() -> String {
        "Validators agree on preserved CCD balances and nonces".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_after_activation(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, None, false).await?;
        Ok(TestProgress::Passed)
    }
}

struct OperationsAgreement;
impl ProtocolUpgradeTest for OperationsAgreement {
    fn name() -> String {
        "Validators agree on finalized P11 CCD balances and nonces".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self)
    }
    async fn on_operations_complete(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(context, None, false).await?;
        Ok(TestProgress::Passed)
    }
}

struct AllNodesAgreement {
    reference: Option<Snapshot>,
}
impl ProtocolUpgradeTest for AllNodesAgreement {
    fn name() -> String {
        "All five nodes agree on saved P11 CCD balances and nonces".into()
    }
    fn setup(_: Vec<GenesisAccount>) -> Result<Self> {
        Ok(Self { reference: None })
    }
    async fn on_operations_complete(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        self.reference = Some(
            wait::bounded(
                "save CCD recovery evidence",
                REJOIN_TIMEOUT,
                capture(context),
            )
            .await?,
        );
        Ok(TestProgress::Pending)
    }
    async fn on_all_nodes_ready(&mut self, context: &TestContext<'_>) -> Result<TestProgress> {
        agreement(
            context,
            Some(
                self.reference
                    .as_ref()
                    .context("No CCD recovery evidence")?,
            ),
            true,
        )
        .await?;
        Ok(TestProgress::Passed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rewards_are_allowed_but_fees_or_unexplained_deltas_are_not() -> Result<()> {
        let address = AccountAddress([1; 32]);
        let amount = Amount::from_micro_ccd;
        let before = BTreeMap::from([(address, amount(100))]);
        let after = BTreeMap::from([(address, amount(110))]);
        let rewards = BTreeMap::from([(address, amount(10))]);
        compare_balances(&before, &after, &rewards)?;
        assert!(compare_balances(&before, &after, &BTreeMap::new()).is_err());
        assert!(compare_balances(&before, &BTreeMap::new(), &rewards).is_err());
        assert!(
            compare_balances(&before, &BTreeMap::from([(address, amount(109))]), &rewards).is_err()
        );
        Ok(())
    }
}
