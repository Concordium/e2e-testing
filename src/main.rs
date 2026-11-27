use anyhow::Result;
use clap::Parser;

mod cli;
mod fixture;
mod operations;
mod registry;
mod report;
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
            let report = suites::smoke::run(&args.image, args.protocol).await?;
            print!("{report}");
            if !report.is_passed() {
                anyhow::bail!("Smoke suite failed");
            }
        }
        cli::Cli::ProtocolP10P11(args) => {
            let report = suites::protocol_p10_p11::run(suites::protocol_p10_p11::Config {
                old_image: args.old_image,
                new_image: args.new_image,
            })
            .await?;
            print!("{report}");
            if !report.is_passed() {
                anyhow::bail!("P10 -> P11 suite failed");
            }
        }
    }

    Ok(())
}
