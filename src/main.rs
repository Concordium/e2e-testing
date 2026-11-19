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
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,concordium_e2e=info")),
        )
        .init();

    match cli::Cli::parse() {
        cli::Cli::Smoke(args) => {
            tracing::info!(image = %args.image, "starting smoke suite");
            let results = suites::smoke::run(&args.image).await?;
            suites::smoke::print_summary(&results);
            if results.iter().any(|r| !r.passed) {
                anyhow::bail!("Smoke suite failed");
            }
        }
        cli::Cli::ProtocolP10P11(args) => {
            suites::protocol_p10_p11::run(suites::protocol_p10_p11::Config {
                old_image: args.old_image,
                new_image: args.new_image,
            })
            .await?;
        }
    }

    Ok(())
}
