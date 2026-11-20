//! Suite-specific command-line options.
use clap::{Args, Parser};
use concordium_rust_sdk::types::ProtocolVersion;

#[derive(Debug, Parser)]
#[command(
    name = "concordium-e2e",
    version,
    about = "Concordium node end-to-end suites"
)]
pub enum Cli {
    /// Run smoke checks against a fresh chain using the selected protocol.
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
    /// Protocol version for genesis and smoke-test selection.
    #[arg(long, value_parser = parse_smoke_protocol, value_name = "VERSION")]
    pub protocol: ProtocolVersion,
}

/// Restrict protocol selection to versions supported by smoke fixtures.
fn parse_smoke_protocol(value: &str) -> Result<ProtocolVersion, String> {
    match value {
        "10" => Ok(ProtocolVersion::P10),
        "11" => Ok(ProtocolVersion::P11),
        _ => Err("supported smoke protocols are 10 and 11".into()),
    }
}
