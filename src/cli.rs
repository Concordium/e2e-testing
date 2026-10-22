//! Suite-specific CLI. The legacy invocation resolves to the same smoke suite.
use clap::{Args, Parser, Subcommand};

/// Command-line options, including the legacy smoke invocation.
#[derive(Debug, Parser)]
#[command(
    name = "concordium-e2e",
    version,
    about = "Concordium node end-to-end suites",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    // TODO: Remove the legacy root-level smoke alias in a future CLI revision.
    /// Legacy smoke-suite image (prefer `smoke --image IMAGE`).
    #[arg(long, value_name = "IMAGE")]
    image: Option<String>,
    /// `None` uses the legacy smoke argument path.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run all smoke checks against a fresh single-validator P10 chain.
    Smoke(SmokeArgs),
    /// Run the complete rolling software upgrade and P10 -> P11 lifecycle.
    #[command(name = "protocol-p10-p11")]
    ProtocolP10P11(ProtocolArgs),
}

/// Image choices for the P10-to-P11 test.
#[derive(Debug, Args)]
struct ProtocolArgs {
    /// Docker image for nodes before software upgrades.
    #[arg(long, value_name = "IMAGE")]
    old_image: String,
    /// Replacement image. It must support P10 and P11.
    #[arg(long, value_name = "IMAGE")]
    new_image: String,
}

/// The selected suite with its required settings.
pub enum Suite {
    /// Smoke checks with the node image reference.
    Smoke(String),
    /// The P10-to-P11 continuity test with its settings.
    Protocol(crate::suites::protocol_p10_p11::Config),
}

#[derive(Debug, Args)]
struct SmokeArgs {
    /// Docker image for the Concordium node under test.
    #[arg(long, env = "CONCORDIUM_NODE_IMAGE", value_name = "IMAGE")]
    image: String,
}

impl Cli {
    /// Select the explicit suite or resolve the legacy smoke image argument and environment fallback.
    pub fn suite(self) -> Result<Suite, clap::Error> {
        match (self.image, self.command) {
            (_, Some(Command::Smoke(args))) => Ok(Suite::Smoke(args.image)),
            (_, Some(Command::ProtocolP10P11(args))) => {
                Ok(Suite::Protocol(crate::suites::protocol_p10_p11::Config {
                    old_image: args.old_image,
                    new_image: args.new_image,
                }))
            }
            (Some(image), None) => Ok(Suite::Smoke(image)),
            (None, None) => std::env::var("CONCORDIUM_NODE_IMAGE")
                .map(Suite::Smoke)
                .map_err(|_| {
                    clap::Error::raw(
                        clap::error::ErrorKind::MissingRequiredArgument,
                        "choose a suite (smoke --image IMAGE) or supply legacy --image IMAGE",
                    )
                }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_explicit_smoke_resolve_identically() {
        for arguments in [
            vec!["e2e", "--image", "node:test"],
            vec!["e2e", "smoke", "--image", "node:test"],
        ] {
            let Suite::Smoke(image) = Cli::try_parse_from(arguments).unwrap().suite().unwrap()
            else {
                panic!("Expected smoke suite");
            };
            assert_eq!(image, "node:test");
        }
    }

    #[test]
    fn ambiguous_root_and_subcommand_arguments_are_rejected() {
        assert!(Cli::try_parse_from(["e2e", "--image", "old", "smoke", "--image", "new"]).is_err());
    }

    #[test]
    fn protocol_requires_two_images() {
        assert!(Cli::try_parse_from(["e2e", "protocol-p10-p11", "--old-image", "old"]).is_err());
        let Suite::Protocol(config) = Cli::try_parse_from([
            "e2e",
            "protocol-p10-p11",
            "--old-image",
            "old",
            "--new-image",
            "new",
        ])
        .unwrap()
        .suite()
        .unwrap() else {
            panic!("Expected protocol suite");
        };
        assert_eq!(config.old_image, "old");
        assert_eq!(config.new_image, "new");
        assert!(
            Cli::try_parse_from([
                "e2e",
                "protocol-p10-p11",
                "--old-image",
                "old",
                "--new-image",
                "new",
                "--output-dir",
                "/tmp/results",
            ])
            .is_err()
        );
    }

    #[test]
    fn filtering_is_rejected() {
        assert!(
            Cli::try_parse_from(["e2e", "--image", "node:test", "--filter", "health"]).is_err()
        );
        assert!(
            Cli::try_parse_from(["e2e", "smoke", "--image", "node:test", "--filter", "health"])
                .is_err()
        );
    }
}
