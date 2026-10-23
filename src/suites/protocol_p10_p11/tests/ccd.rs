//! Independent CCD transaction, nonce-preservation and account-agreement criteria.
use super::super::{
    REJOIN_TIMEOUT,
    context::TestContext,
    operations,
    state::{FinalizedBlock, account_at},
    wait,
};
use super::upgrade_test::TestDefinition;
use super::{
    accounts_fixture::{Account, TestAccounts},
    balance_preservation::BalancePreservation,
    registry::TestRegistry,
    upgrade_test::{TestProgress, UpgradeTest, Verification},
};
use anyhow::{Context as _, Result, ensure};
use concordium_rust_sdk::{common::types::Amount, id::types::AccountAddress, types::Nonce};
use std::collections::BTreeMap;

pub(super) fn register(registry: &mut TestRegistry) -> Result<()> {
    registry.register::<Transfer<1, false>>()?;
    registry.register::<Transfer<0, false>>()?;
    registry.register::<BalancePreservation>()?;
    registry.register::<NoncePreservation>()?;
    registry.register::<Transfer<0, true>>()?;
    registry.register::<MixedAgreement>()?;
    registry.register::<BeforeAgreement>()?;
    registry.register::<AfterAgreement>()?;
    registry.register::<OperationsAgreement>()?;
    registry.register::<AllNodesAgreement>()
}
struct Transfer<const NODE: usize, const P11: bool> {
    sender: Account,
    recipient: Account,
}
impl<const NODE: usize, const P11: bool> TestDefinition for Transfer<NODE, P11> {
    const NAME: &'static str = if P11 {
        "A fresh CCD transfer finalizes at P11"
    } else if NODE == 1 {
        "CCD transfers through old software finalize"
    } else {
        "CCD transfers through new software finalize at P10"
    };
    const ACCOUNTS: &'static [&'static str] = &["sender", "recipient"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self {
            sender: accounts.take("sender")?,
            recipient: accounts.take("recipient")?,
        })
    }
}
#[async_trait::async_trait(?Send)]
impl<const NODE: usize, const P11: bool> UpgradeTest for Transfer<NODE, P11> {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        if P11 {
            return Ok(TestProgress::Pending);
        }
        verification
            .verify(transfer(
                &self.sender,
                &self.recipient,
                context,
                NODE,
                false,
            ))
            .await
    }
    async fn preservation_verified(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        if !P11 {
            return Ok(TestProgress::Pending);
        }
        verification
            .verify(transfer(&self.sender, &self.recipient, context, NODE, true))
            .await
    }
}
async fn transfer(
    sender: &Account,
    recipient: &Account,
    context: &TestContext<'_>,
    node: usize,
    p11: bool,
) -> Result<()> {
    let recipient = recipient.lock().await.address;
    let mut client = context.client(node).await?;
    let block = operations::ccd(&mut client, sender, recipient).await?;
    if p11 {
        operations::require_p11(&mut client, block).await?;
    }
    Ok(())
}
struct Nonces {
    block: FinalizedBlock,
    values: BTreeMap<AccountAddress, Nonce>,
}
struct NoncePreservation {
    sender: Account,
    recipient: Account,
    before: Option<Nonces>,
}
impl TestDefinition for NoncePreservation {
    const NAME: &'static str = "Account nonces are preserved";
    const ACCOUNTS: &'static [&'static str] = &["sender", "recipient"];
    fn new(accounts: &mut TestAccounts) -> Result<Self> {
        Ok(Self {
            sender: accounts.take("sender")?,
            recipient: accounts.take("recipient")?,
            before: None,
        })
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for NoncePreservation {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
        transfer(&self.sender, &self.recipient, context, 0, false).await?;
        Ok(TestProgress::Pending)
    }
    async fn before_activation(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
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
    async fn after_activation(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification
            .verify(wait::bounded(
                "verify nonce preservation",
                REJOIN_TIMEOUT,
                async {
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
                },
            ))
            .await
    }
}
async fn capture_nonces(context: &TestContext<'_>) -> Result<Nonces> {
    let mut client = context.client(0).await?;
    let block = FinalizedBlock::capture(&mut client).await?;
    let mut values = BTreeMap::new();
    for address in context.accounts() {
        values.insert(
            *address,
            account_at(&mut client, *address, block.hash)
                .await?
                .account_nonce,
        );
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
        let info = account_at(&mut client, *address, block.hash).await?;
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
            let info = account_at(&mut client, *address, snapshot.block.hash).await?;
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
impl TestDefinition for MixedAgreement {
    const NAME: &'static str = "Mixed-version nodes agree on finalized CCD balances and nonces";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self)
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for MixedAgreement {
    async fn mixed_version(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification.verify(agreement(context, None, true)).await
    }
}
struct BeforeAgreement;
impl TestDefinition for BeforeAgreement {
    const NAME: &'static str = "All nodes agree on pre-update CCD balances and nonces";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self)
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for BeforeAgreement {
    async fn before_activation(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification.verify(agreement(context, None, true)).await
    }
}
struct AfterAgreement;
impl TestDefinition for AfterAgreement {
    const NAME: &'static str = "Validators agree on preserved CCD balances and nonces";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self)
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for AfterAgreement {
    async fn after_activation(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification.verify(agreement(context, None, false)).await
    }
}
struct OperationsAgreement;
impl TestDefinition for OperationsAgreement {
    const NAME: &'static str = "Validators agree on finalized P11 CCD balances and nonces";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self)
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for OperationsAgreement {
    async fn operations_complete(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification.verify(agreement(context, None, false)).await
    }
}
struct AllNodesAgreement {
    reference: Option<Snapshot>,
}
impl TestDefinition for AllNodesAgreement {
    const NAME: &'static str = "All five nodes agree on saved P11 CCD balances and nonces";
    fn new(_: &mut TestAccounts) -> Result<Self> {
        Ok(Self { reference: None })
    }
}
#[async_trait::async_trait(?Send)]
impl UpgradeTest for AllNodesAgreement {
    async fn operations_complete(
        &mut self,
        context: &TestContext<'_>,
        _: &mut Verification,
    ) -> Result<TestProgress> {
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
    async fn all_nodes_ready(
        &mut self,
        context: &TestContext<'_>,
        verification: &mut Verification,
    ) -> Result<TestProgress> {
        verification
            .verify(async {
                agreement(
                    context,
                    Some(
                        self.reference
                            .as_ref()
                            .context("No CCD recovery evidence")?,
                    ),
                    true,
                )
                .await
            })
            .await
    }
}
