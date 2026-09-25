use anyhow::Result;
use clap::Parser;

mod fixture;
mod runner;
mod tests;

/// Automated end-to-end test suite for the Concordium node.
///
/// Spins up a fresh private single-validator chain, runs the test suite
/// against it, then tears the chain down — regardless of outcome.
#[derive(Debug, Parser)]
#[command(name = "concordium-e2e", version, about, long_about = None)]
struct Args {
    /// Docker image for the Concordium node under test
    /// (e.g. `concordium-node:7.0.4`).
    ///
    /// May also be supplied via the CONCORDIUM_NODE_IMAGE environment variable.
    #[arg(long, env = "CONCORDIUM_NODE_IMAGE", value_name = "IMAGE")]
    image: String,

    /// Run only tests whose names contain FILTER (case-insensitive substring match).
    #[arg(long, value_name = "FILTER")]
    filter: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    tracing::info!(image = %args.image, "starting e2e suite");

    let results = runner::run(&args.image, args.filter.as_deref()).await?;

    runner::print_summary(&results);

    if results.iter().any(|r| !r.passed) {
        std::process::exit(1);
    }

    Ok(())
}
