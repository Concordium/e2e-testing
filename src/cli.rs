//! Suite-specific command-line options.
use clap::{Args, Parser};

#[derive(Debug, Parser)]
#[command(
    name = "concordium-e2e",
    version,
    about = "Concordium node end-to-end suites"
)]
pub enum Cli {
    /// Run all smoke checks against a fresh single-validator P10 chain.
    Smoke(SmokeArgs),
    /// Run the complete rolling software upgrade and P10 -> P11 lifecycle.
    #[command(name = "protocol-p10-p11")]
    ProtocolP10P11(ProtocolArgs),
}

/// Image choices for the P10-to-P11 test.
#[derive(Debug, Args)]
pub struct ProtocolArgs {
    /// Docker image for nodes before software upgrades.
    #[arg(long, value_name = "IMAGE")]
    pub old_image: String,
    /// Replacement image. It must support P10 and P11.
    #[arg(long, value_name = "IMAGE")]
    pub new_image: String,
}

#[derive(Debug, Args)]
pub struct SmokeArgs {
    /// Docker image for the Concordium node under test.
    #[arg(long, env = "CONCORDIUM_NODE_IMAGE", value_name = "IMAGE")]
    pub image: String,
}
