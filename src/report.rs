//! Shared suite results, independent of test execution.

use std::{fmt, time::Duration};

/// A check's status. Preparation semantics apply to registered tests.
#[derive(Debug)]
pub enum Outcome {
    /// Preparation started and the check completed successfully.
    Passed,
    /// Preparation started and preparation or execution failed.
    Failed(String),
    /// Preparation has never started; test-owned cleanup must not run.
    NotRun,
    /// Preparation started, but the check has not completed.
    Incomplete,
}

impl Outcome {
    pub fn is_passed(&self) -> bool {
        matches!(self, Self::Passed)
    }
}

/// One test or lifecycle result in display order.
#[derive(Debug)]
pub struct Check {
    pub group: String,
    pub name: String,
    pub outcome: Outcome,
}

#[derive(Debug)]
pub struct SuiteReport {
    pub title: String,
    pub elapsed: Duration,
    pub checks: Vec<Check>,
}

impl SuiteReport {
    pub fn is_passed(&self) -> bool {
        self.checks.iter().all(|check| check.outcome.is_passed())
    }
}

impl fmt::Display for SuiteReport {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(output, "{}", self.title)?;
        let mut previous_group = None;
        let mut passed = 0usize;
        let mut failed = 0usize;
        let mut not_run = 0usize;
        let mut incomplete = 0usize;

        for check in &self.checks {
            if previous_group != Some(check.group.as_str()) {
                writeln!(output, "  {}", check.group)?;
                previous_group = Some(check.group.as_str());
            }
            write_outcome(output, &check.name, &check.outcome)?;
            match &check.outcome {
                Outcome::Passed => passed += 1,
                Outcome::Failed(_) => failed += 1,
                Outcome::NotRun => not_run += 1,
                Outcome::Incomplete => incomplete += 1,
            }
        }

        write!(output, "  Checks: {passed} passed, {failed} failed")?;
        if not_run > 0 {
            write!(output, ", {not_run} not run")?;
        }
        if incomplete > 0 {
            write!(output, ", {incomplete} incomplete")?;
        }
        writeln!(output)?;
        writeln!(
            output,
            "  Result: {} ({:.1}s)",
            if self.is_passed() { "PASS" } else { "FAIL" },
            self.elapsed.as_secs_f64()
        )
    }
}

fn write_outcome(output: &mut impl fmt::Write, name: &str, outcome: &Outcome) -> fmt::Result {
    match outcome {
        Outcome::Passed => writeln!(output, "    [PASS] {name}"),
        Outcome::Failed(message) => writeln!(output, "    [FAIL] {name}: {message}"),
        Outcome::NotRun => writeln!(output, "    [NOT RUN] {name}"),
        Outcome::Incomplete => writeln!(output, "    [INCOMPLETE] {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_groups_and_counts_all_outcomes() {
        let report = SuiteReport {
            title: "Smoke — P11".into(),
            elapsed: Duration::from_millis(1250),
            checks: vec![
                Check {
                    group: "Health".into(),
                    name: "Reachable".into(),
                    outcome: Outcome::Passed,
                },
                Check {
                    group: "Health".into(),
                    name: "Produces blocks".into(),
                    outcome: Outcome::Failed("timed out".into()),
                },
                Check {
                    group: "Token roles".into(),
                    name: "Initial roles".into(),
                    outcome: Outcome::NotRun,
                },
                Check {
                    group: "Token roles".into(),
                    name: "Handover".into(),
                    outcome: Outcome::Incomplete,
                },
                Check {
                    group: "Suite lifecycle".into(),
                    name: "Cleanup".into(),
                    outcome: Outcome::Passed,
                },
            ],
        };
        assert_eq!(
            report.to_string(),
            concat!(
                "Smoke — P11\n",
                "  Health\n",
                "    [PASS] Reachable\n",
                "    [FAIL] Produces blocks: timed out\n",
                "  Token roles\n",
                "    [NOT RUN] Initial roles\n",
                "    [INCOMPLETE] Handover\n",
                "  Suite lifecycle\n",
                "    [PASS] Cleanup\n",
                "  Checks: 2 passed, 1 failed, 1 not run, 1 incomplete\n",
                "  Result: FAIL (1.2s)\n",
            )
        );
    }

    #[test]
    fn cleanup_failure_fails_an_otherwise_successful_report() {
        let report = SuiteReport {
            title: "Smoke".into(),
            elapsed: Duration::ZERO,
            checks: vec![
                Check {
                    group: "Health".into(),
                    name: "Reachable".into(),
                    outcome: Outcome::Passed,
                },
                Check {
                    group: "Suite lifecycle".into(),
                    name: "Cleanup".into(),
                    outcome: Outcome::Failed("container removal failed".into()),
                },
            ],
        };
        assert_eq!(
            report.to_string(),
            concat!(
                "Smoke\n",
                "  Health\n",
                "    [PASS] Reachable\n",
                "  Suite lifecycle\n",
                "    [FAIL] Cleanup: container removal failed\n",
                "  Checks: 1 passed, 1 failed\n",
                "  Result: FAIL (0.0s)\n",
            )
        );
    }
}
