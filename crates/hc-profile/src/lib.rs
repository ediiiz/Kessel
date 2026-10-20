//! Appliance profile parsing: the profile ZIP plus its `DeviceDescription` and `FeatureMapping`
//! XML, parsed into the entity / feature / program model.
//!
//! Pure parsing, no I/O beyond the reader it is handed. The output is [`Profile`]:
//!
//! - [`Profile::from_zip_reader`] finds the `*.json` manifest and reads the two XML member names
//!   from its fields.
//! - [`Credentials`] holds `key` (psk64) and, on the AES path, `iv` (iv64).
//! - [`DeviceIdentity::derived_host`] is `haId` on the AES path, else `"{brand}-{type}-{haId}"`;
//!   [`Profile::set_manual_host`] overrides it.
//! - [`EntityModel`] is the entity / feature / enum / program / option model.
//!
//! **Secrets are wrapped at the boundary.** `key`, `iv`, `haId`, `serialNumber` and `mac` become
//! [`hc_crypto::Secret`] inside the manifest parser, so no `{:?}` downstream can leak one. `haId`
//! is included because on the AES path it contains the MAC.
//!
//! **A malformed entity is data, not an error.** One bad element must never take down a device:
//! it lands in [`Profile::skipped`] and everything else parses.

#![forbid(unsafe_code)]
#![allow(
    clippy::redundant_pub_crate,
    reason = "contradicts rustc's unreachable_pub; pub(crate) is the correct visibility"
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic; library code may not"
    )
)]

mod description;
mod error;
mod feature_mapping;
mod manifest;
mod model;
mod profile;
mod xml;

pub use error::{ProfileError, Result};
pub use manifest::{ConnectionType, Credentials, DeviceIdentity};
pub use model::{
    Access, ContentType, DeviceMeta, Entity, EntityKind, EntityModel, EnumType, NumericRange,
    Program, ProgramOption, ProtocolType, SkipReason, SkippedEntity, ValueType,
};
pub use profile::{MAX_MEMBER_BYTES, Profile};

/// The crate's own name, as it appears in `[workspace.members]`.
pub const CRATE_NAME: &str = "hc-profile";

#[cfg(test)]
pub mod test_support {
    use std::io::{Cursor, Write as _};

    /// Builds a stored (uncompressed) ZIP in memory from `(name, body)` pairs.
    pub(crate) fn zip_with(members: &[(&str, &str)]) -> Vec<u8> {
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for &(name, body) in members {
            writer.start_file(name, options).unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::{Access, CRATE_NAME, ConnectionType, EntityKind, Profile, ProfileError};
    use crate::test_support::zip_with;

    fn manifest_zip(manifest: &serde_json::Value) -> Vec<u8> {
        zip_with(&[
            ("profile.json", manifest.to_string().as_str()),
            ("fm.xml", "<featureMappingFile/>"),
            ("dd.xml", "<device/>"),
        ])
    }

    #[test]
    fn crate_name_is_stable() {
        assert_eq!(
            CRATE_NAME, "hc-profile",
            "crate name is part of the workspace layout"
        );
    }

    #[test]
    fn access_words_from_the_real_profiles_all_parse() {
        // The complete set observed across the four real DeviceDescriptions.
        assert_eq!(Access::parse("none"), Access::None, "none");
        assert_eq!(Access::parse("read"), Access::Read, "read");
        assert_eq!(
            Access::parse("readStatic"),
            Access::ReadStatic,
            "readStatic"
        );
        assert_eq!(Access::parse("writeOnly"), Access::WriteOnly, "writeOnly");
        assert_eq!(Access::parse("readWrite"), Access::ReadWrite, "readWrite");
        assert!(
            matches!(Access::parse("sideways"), Access::Other(ref word) if word == "sideways"),
            "an unknown access word is preserved, not dropped"
        );
    }

    #[test]
    fn access_none_is_neither_readable_nor_writable() {
        assert!(!Access::None.is_readable(), "access=none cannot be read");
        assert!(!Access::None.is_writable(), "access=none cannot be written");
        assert!(Access::ReadStatic.is_readable(), "readStatic is readable");
        assert!(!Access::ReadStatic.is_writable(), "readStatic is read-only");
    }

    #[test]
    fn only_the_literal_aes_is_the_aes_path() {
        assert!(ConnectionType::parse("AES").is_aes(), "AES");
        assert!(!ConnectionType::parse("TLS").is_aes(), "TLS");
        assert!(
            !ConnectionType::parse("aes").is_aes(),
            "the transport choice compares the literal `AES`; another spelling is not the AES path"
        );
    }

    #[test]
    fn lists_are_containers_and_leaves_are_not() {
        assert!(
            EntityKind::SettingList.is_container(),
            "a settingList nests settings"
        );
        assert!(!EntityKind::Setting.is_container(), "a setting is a leaf");
    }

    #[test]
    fn aes_profile_without_an_iv_is_rejected() {
        // iv64 is mandatory on the AES path: without it the CBC chain cannot run.
        let zip = manifest_zip(&serde_json::json!({
            "haId": "BRAND-VIB-000000000000",
            "type": "Hob",
            "brand": "BRAND",
            "vib": "VIB",
            "connectionType": "AES",
            "key": "synthetic-not-a-real-key",
            "featureMappingFileName": "fm.xml",
            "deviceDescriptionFileName": "dd.xml"
        }));
        assert!(
            matches!(
                Profile::from_zip_bytes(&zip),
                Err(ProfileError::AesWithoutIv)
            ),
            "an AES manifest with no iv must be a hard error"
        );
    }

    #[test]
    fn manifest_error_names_the_offending_field() {
        let zip = manifest_zip(&serde_json::json!({
            "haId": "000000000000000001",
            "type": "Oven",
            "brand": "BRAND",
            "vib": "VIB",
            "connectionType": 7,
            "key": "synthetic-not-a-real-key",
            "featureMappingFileName": "fm.xml",
            "deviceDescriptionFileName": "dd.xml"
        }));
        match Profile::from_zip_bytes(&zip) {
            Err(ProfileError::Manifest { path, .. }) => {
                assert_eq!(
                    path, "connectionType",
                    "serde_path_to_error names the field"
                );
            }
            other => panic!("expected a Manifest error, got {other:?}"),
        }
    }

    #[test]
    fn a_manifest_that_declares_a_missing_member_fails_loudly() {
        let zip = manifest_zip(&serde_json::json!({
            "haId": "000000000000000001",
            "type": "Oven",
            "brand": "BRAND",
            "vib": "VIB",
            "connectionType": "TLS",
            "key": "synthetic-not-a-real-key",
            "featureMappingFileName": "fm.xml",
            "deviceDescriptionFileName": "absent.xml"
        }));
        match Profile::from_zip_bytes(&zip) {
            Err(ProfileError::DeclaredMemberMissing { field, declared }) => {
                assert_eq!(field, "deviceDescriptionFileName", "the field is named");
                assert_eq!(declared, "absent.xml", "the declared name is quoted back");
            }
            other => panic!("expected DeclaredMemberMissing, got {other:?}"),
        }
    }
}
