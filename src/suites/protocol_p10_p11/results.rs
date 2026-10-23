//! One registered criterion is one in-memory result. No lifecycle or feature knowledge.
use anyhow::{Result, ensure};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TestId {
    owner: u64,
    index: usize,
}
enum Outcome {
    Passed,
    Failed(anyhow::Error),
}
struct Entry {
    name: &'static str,
    context: String,
    outcome: Option<Outcome>,
    setup_errors: Vec<anyhow::Error>,
}
pub(super) struct Recorder {
    owner: u64,
    entries: Vec<Entry>,
    violations: Vec<String>,
}
impl Recorder {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self {
            owner: NEXT.fetch_add(1, Ordering::Relaxed),
            entries: Vec::new(),
            violations: Vec::new(),
        }
    }
    pub fn register(&mut self, name: &'static str, context: String) -> Result<TestId> {
        if self
            .entries
            .iter()
            .any(|entry| entry.name == name && entry.context == context)
        {
            return self.invalid(format!("Duplicate test: {name} [{context}]"));
        }
        let id = TestId {
            owner: self.owner,
            index: self.entries.len(),
        };
        self.entries.push(Entry {
            name,
            context,
            outcome: None,
            setup_errors: Vec::new(),
        });
        Ok(id)
    }
    fn invalid<T>(&mut self, message: String) -> Result<T> {
        println!("[ERROR] {message}");
        self.violations.push(message.clone());
        anyhow::bail!("{message}");
    }
    fn validate_id(&mut self, id: TestId) -> Result<()> {
        if id.owner != self.owner || id.index >= self.entries.len() {
            return self.invalid("Unknown or foreign test handle".into());
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.violations.is_empty(),
            "Invalid test results: {}",
            self.violations.join("; ")
        );
        Ok(())
    }
    pub fn record(&mut self, id: TestId, result: Result<()>) -> Result<()> {
        self.validate_id(id)?;
        if self.entries[id.index].outcome.is_some() {
            return self.invalid("Duplicate test result; earlier result retained".into());
        }
        let entry = &mut self.entries[id.index];
        let failure = result
            .as_ref()
            .err()
            .map(|error| format!("{}: {error:#}", entry.name));
        entry.outcome = Some(match result {
            Ok(()) => Outcome::Passed,
            Err(error) => Outcome::Failed(error),
        });
        println!("{}", Self::line(entry));
        if let Some(message) = failure {
            anyhow::bail!("{message}");
        }
        Ok(())
    }
    /// Setup errors do not invent a failed verification or replace an earlier result.
    pub fn setup_failed(&mut self, id: TestId, error: anyhow::Error) -> Result<()> {
        self.validate_id(id)?;
        let entry = &mut self.entries[id.index];
        entry.setup_errors.push(error);
        println!("{}", Self::line(entry));
        Ok(())
    }
    fn line(entry: &Entry) -> String {
        let name = if entry.context.is_empty() {
            entry.name.to_string()
        } else {
            format!("{} [{}]", entry.name, entry.context)
        };
        if !entry.setup_errors.is_empty() {
            let verification = match &entry.outcome {
                Some(Outcome::Passed) => "verification passed".to_string(),
                Some(Outcome::Failed(error)) => format!("verification failed: {error:#}"),
                None => "verification unprocessed".to_string(),
            };
            let errors = entry
                .setup_errors
                .iter()
                .map(|error| format!("{error:#}"))
                .collect::<Vec<_>>()
                .join("; ");
            return format!("[ERROR] {name} — setup: {errors} ({verification})");
        }
        match &entry.outcome {
            Some(Outcome::Passed) => format!("[PASS] {name}"),
            Some(Outcome::Failed(error)) => format!("[FAIL] {name} — {error:#}"),
            None => format!("[UNPROCESSED] {name}"),
        }
    }
    pub fn complete(&self) -> bool {
        !self.entries.is_empty()
            && self.violations.is_empty()
            && self.entries.iter().all(|entry| {
                entry.setup_errors.is_empty() && matches!(entry.outcome, Some(Outcome::Passed))
            })
    }
    pub fn render(&self, cleanup: &Result<()>, elapsed: Duration, errors: &[String]) -> String {
        let mut output = String::from("\nP10 -> P11 acceptance results\n");
        for entry in &self.entries {
            output.push_str(&format!("  {}\n", Self::line(entry)));
        }
        for error in self.violations.iter().chain(errors) {
            output.push_str(&format!("  Run error: {error}\n"));
        }
        match cleanup {
            Ok(()) => output.push_str("  [PASS] cleanup\n"),
            Err(error) => output.push_str(&format!("  [FAIL] cleanup — {error:#}\n")),
        }
        let status = if self.complete() && cleanup.is_ok() && errors.is_empty() {
            "PASS"
        } else {
            "FAIL"
        };
        output.push_str(&format!(
            "  Result: {status} ({:.1}s)\n",
            elapsed.as_secs_f64()
        ));
        output
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_failure_and_missing_results_remain_explicit() -> Result<()> {
        let mut recorder = Recorder::new();
        let first = recorder.register("First", String::new())?;
        let second = recorder.register("Second", String::new())?;
        recorder.register("Third", String::new())?;
        recorder.record(first, Ok(()))?;
        assert!(
            recorder
                .record(
                    second,
                    Err(anyhow::anyhow!("RPC failed").context("read state"))
                )
                .is_err()
        );
        let output = recorder.render(&Ok(()), Duration::ZERO, &[]);
        assert!(output.contains("[PASS] First"));
        assert!(output.contains("[FAIL] Second — read state: RPC failed"));
        assert!(output.contains("[UNPROCESSED] Third"));
        assert!(output.contains("Result: FAIL"));
        Ok(())
    }
    #[test]
    fn duplicate_results_and_registrations_never_overwrite() -> Result<()> {
        let mut recorder = Recorder::new();
        let id = recorder.register("Transfer", "validator 2".into())?;
        recorder.record(id, Ok(()))?;
        assert!(recorder.complete());
        assert!(
            recorder
                .record(id, Err(anyhow::anyhow!("replacement")))
                .is_err()
        );
        assert!(!recorder.complete());
        assert!(
            recorder
                .render(&Ok(()), Duration::ZERO, &[])
                .contains("[PASS] Transfer")
        );
        assert!(recorder.register("Transfer", "validator 2".into()).is_err());
        recorder.register("Transfer", "validator 3".into())?;
        Ok(())
    }
    #[test]
    fn setup_and_run_errors_do_not_fabricate_failed_verification() -> Result<()> {
        let mut recorder = Recorder::new();
        let id = recorder.register("Prepared test", String::new())?;
        recorder.setup_failed(id, anyhow::anyhow!("snapshot unavailable"))?;
        let output = recorder.render(
            &Err(anyhow::anyhow!("TempDir close failed")),
            Duration::ZERO,
            &["termination during cleanup".into()],
        );
        assert!(output.contains("[ERROR] Prepared test"));
        assert!(output.contains("verification unprocessed"));
        assert!(!output.contains("[FAIL] Prepared test"));
        assert!(output.contains("TempDir close failed"));
        assert!(output.contains("termination during cleanup"));
        Ok(())
    }
    #[test]
    fn missing_results_fail_without_other_errors_and_foreign_handles_are_rejected() -> Result<()> {
        let mut first = Recorder::new();
        let id = first.register("Unreached", String::new())?;
        assert!(!first.complete());
        assert!(
            first
                .render(&Ok(()), Duration::ZERO, &[])
                .contains("Result: FAIL")
        );
        let mut second = Recorder::new();
        assert!(second.record(id, Ok(())).is_err());
        Ok(())
    }
}
