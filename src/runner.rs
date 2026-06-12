//! Test runner — orchestrates fixture setup, test execution, and teardown.
//!
//! The runner:
//! 1. Generates a fresh genesis block via [`fixture::genesis`].
//! 2. Starts a node container via [`fixture::node`].
//! 3. Waits for the node to become ready.
//! 4. Runs each registered test in order, collecting results.
//! 5. Tears down the node (always, even on failure).
//! 6. Returns the collected results to `main` for exit-code and summary logic.

/// The result of a single test case.
#[derive(Debug)]
pub struct TestResult {
    /// Human-readable test name.
    pub name: String,
    /// `true` if the test passed.
    pub passed: bool,
    /// Optional failure message.
    pub message: Option<String>,
}

/// Run the full test suite against the given node image.
///
/// `filter` — if `Some`, only tests whose names contain the substring (case-
/// insensitive) are executed.
///
/// Returns a `Vec` of one [`TestResult`] per scheduled test.
pub async fn run(
    _image: &str,
    _grpc_port: u16,
    _filter: Option<&str>,
) -> anyhow::Result<Vec<TestResult>> {
    // TODO(task-06): wire up fixture creation, node startup, and test dispatch
    // once tasks 01, 02, 04, and 05 are complete.
    tracing::info!("no tests registered yet");
    Ok(Vec::new())
}

/// Print a human-readable summary of the test run to stdout.
pub fn print_summary(results: &[TestResult]) {
    if results.is_empty() {
        println!("\nno tests registered — nothing to run.");
        return;
    }

    let total = results.len();
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = total - passed;

    println!("\n{}", "─".repeat(60));
    for r in results {
        let status = if r.passed { "PASS" } else { "FAIL" };
        print!("  [{status}] {}", r.name);
        if let Some(msg) = &r.message {
            print!("  — {msg}");
        }
        println!();
    }
    println!("{}", "─".repeat(60));
    println!("  {total} tests run — {passed} passed, {failed} failed");
    println!("{}\n", "─".repeat(60));
}
