use anyhow::Result;
use clap::Parser;

mod cli;
mod fixture;
mod suites;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match cli::Cli::parse()
        .suite()
        .unwrap_or_else(|error| error.exit())
    {
        cli::Suite::Smoke(image) => {
            tracing::info!(image = %image, "starting smoke suite");
            let results = suites::smoke::run(&image).await?;
            suites::smoke::print_summary(&results);
            if results.iter().any(|r| !r.passed) {
                anyhow::bail!("Smoke suite failed");
            }
        }
        cli::Suite::Protocol(config) => suites::protocol_p10_p11::run(config).await?,
    }

    Ok(())
}
