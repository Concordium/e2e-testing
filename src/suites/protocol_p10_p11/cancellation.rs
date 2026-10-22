//! Retained cancellation for checks and resource-safe lifecycle boundaries.
use anyhow::{Result, ensure};
use std::future::Future;
use tokio::sync::watch;

/// A shutdown request shared by the signal handler and lifecycle operations.
#[derive(Clone)]
pub(super) struct Cancellation {
    /// Remains true after shutdown. New waiters must also observe the request.
    requested: watch::Sender<bool>,
}

impl Cancellation {
    pub(super) fn new() -> Self {
        Self {
            requested: watch::channel(false).0,
        }
    }

    /// Request shutdown without consuming the request at the first checkpoint.
    pub(super) fn request(&self) {
        self.requested.send_replace(true);
    }

    /// Check before and after work that must finish without cancellation.
    pub(super) fn check(&self) -> Result<()> {
        ensure!(!*self.requested.borrow(), "Termination signal received");
        Ok(())
    }

    /// Interrupt work that is safe to drop. Never wrap container creation or replacement.
    pub(super) async fn run<T>(&self, future: impl Future<Output = Result<T>>) -> Result<T> {
        self.check()?;
        let mut requested = self.requested.subscribe();
        tokio::select! {
            biased;
            _ = requested.wait_for(|requested| *requested) => anyhow::bail!("Termination signal received"),
            result = future => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cancellation_interrupts_an_active_wait() {
        let cancellation = Cancellation::new();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let operation = cancellation.run(async {
            started_tx.send(()).unwrap();
            std::future::pending::<Result<()>>().await
        });
        let shutdown = async {
            started_rx.await.unwrap();
            cancellation.request();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_millis(100), async {
            tokio::join!(operation, shutdown)
        })
        .await
        .expect("Shutdown must interrupt the active wait");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Termination signal")
        );
    }

    #[tokio::test]
    async fn cancellation_prevents_later_work_and_remains_set() {
        let cancellation = Cancellation::new();
        cancellation.request();
        let mut started = false;
        let result = cancellation
            .run(async {
                started = true;
                Ok(())
            })
            .await;
        assert!(result.is_err());
        assert!(!started);
        assert!(cancellation.check().is_err());
        assert!(cancellation.clone().check().is_err());
    }
}
