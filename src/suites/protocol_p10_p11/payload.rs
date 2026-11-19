//! Revision-specific P10 -> P11 update parameters, not a generic proposal builder.
use anyhow::Result;
use concordium_rust_sdk::{
    base::updates::{AccessStructure, ProtocolUpdate},
    common::{self, Serial},
    smart_contracts::common::Duration,
    types::{UpdateKeysIndex, UpdateKeysThreshold},
};

/// Provisional P11 update identifier accepted by the compatible node image.
/// This value is not the final protocol specification hash.
pub const SPECIFICATION_HASH: &str =
    "000000000000000000000000000000000000000000000000000000000000000b";
/// Source revision whose P10.ProtocolP11 implementation defines this fixture.
pub const SOURCE_REVISION: &str = "dd095ef7b90ca3101b5e37c7e1c61224356e35de";
/// Token module reference accepted by the P10 token-creation update.
pub const TOKEN_MODULE: &str = "5c5c2645db84a7026d78f2501740f60a8ccb8fae5c166dc2428077fd9a699a4a";

/// Build the revision-specific update with 2-of-3 access and a one-day maximum lock duration.
pub fn protocol_update() -> Result<ProtocolUpdate> {
    let access = AccessStructure {
        authorized_keys: [0, 1, 2]
            .into_iter()
            .map(|index| UpdateKeysIndex { index })
            .collect(),
        threshold: UpdateKeysThreshold::try_from(2u16)?,
    };
    let mut auxiliary = common::to_bytes(&access);
    Duration::from_millis(86_400_000).serial(&mut auxiliary);
    Ok(ProtocolUpdate {
        message: "Private e2e P10 to P11 transition".into(),
        specification_url: "https://github.com/Concordium/concordium-update-proposals".into(),
        specification_hash: SPECIFICATION_HASH.parse()?,
        specification_auxiliary_data: auxiliary,
    })
}

/// Return the hexadecimal SHA-256 digest of the encoded protocol-update announcement.
pub fn payload_hash() -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = common::to_bytes(&protocol_update()?);
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auxiliary_data_matches_haskell_access_then_duration_encoding() -> Result<()> {
        // u16 count, three u16 indices, u16 threshold, u64 duration in milliseconds.
        assert_eq!(
            protocol_update()?.specification_auxiliary_data,
            vec![0, 3, 0, 0, 0, 1, 0, 2, 0, 2, 0, 0, 0, 0, 5, 38, 92, 0]
        );
        Ok(())
    }
}
