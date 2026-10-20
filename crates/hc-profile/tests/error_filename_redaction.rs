//! A `ProfileError` must not render an identifier-bearing member filename.
//!
//! XML members are named `<serial>_DeviceDescription.xml` or `<haId>_DeviceDescription.xml`. These
//! tests drive the four filename-carrying variants through the ZIP pipeline with a synthetic
//! sentinel as the prefix, and assert neither `{}` nor `{:?}` renders it while the suffix survives.
//! Each error is produced by forging a ZIP, since `ProfileError` is `#[non_exhaustive]`.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic; library code may not"
    )
)]

mod support;

use hc_profile::{Profile, ProfileError};
use serde_json::json;
use support::{build_deflated_zip, build_zip, fixture_bytes, has_extension, read_zip};

/// A synthetic stand-in for the `<serial>`/`<haId>` prefix. Carries no `_`, so the only `_` in
/// the forged filename is the structural separator.
const SENTINEL: &str = "SNTL7SERIAL0246813579";

/// A member filename shaped like the real fleet's: `<identifier>_DeviceDescription.xml`.
fn sentinel_description_name() -> String {
    format!("{SENTINEL}_DeviceDescription.xml")
}

/// The hob fixture's members, with the manifest's `deviceDescriptionFileName` repointed at
/// `declared`.
fn hob_members_declaring(declared: &str) -> Vec<(String, Vec<u8>)> {
    let mut members = read_zip(&fixture_bytes("hob-aes.zip"));
    for (name, body) in &mut members {
        if has_extension(name, "json") {
            let mut manifest: serde_json::Value = serde_json::from_slice(body).unwrap();
            manifest
                .as_object_mut()
                .expect("the manifest is a JSON object")
                .insert(
                    "deviceDescriptionFileName".to_owned(),
                    json!(declared.to_owned()),
                );
            *body = manifest.to_string().into_bytes();
        }
    }
    members
}

/// `DeclaredMemberMissing`: the manifest declares the sentinel name, the archive lacks it.
fn declared_missing_error() -> ProfileError {
    let members = hob_members_declaring(&sentinel_description_name());
    Profile::from_zip_bytes(&build_zip(&members)).expect_err("the declared member is absent")
}

/// `NotUtf8`: the sentinel-named member is present but not valid UTF-8.
fn not_utf8_error() -> ProfileError {
    let declared = sentinel_description_name();
    let mut members = hob_members_declaring(&declared);
    members.push((declared, vec![0xFF, 0xFE, 0xFD, 0xFC]));
    Profile::from_zip_bytes(&build_zip(&members)).expect_err("the member is not UTF-8")
}

/// `MemberTooLarge`: the sentinel-named member inflates past the cap.
fn too_large_error() -> ProfileError {
    let cap = usize::try_from(hc_profile::MAX_MEMBER_BYTES).expect("the cap fits a usize");
    let declared = sentinel_description_name();
    let mut members = hob_members_declaring(&declared);
    members.push((declared, vec![b' '; cap + 1]));
    Profile::from_zip_bytes(&build_deflated_zip(&members)).expect_err("the member is too large")
}

/// `Xml`: the sentinel-named member is valid UTF-8 but malformed XML.
fn xml_error() -> ProfileError {
    let declared = sentinel_description_name();
    let mut members = hob_members_declaring(&declared);
    members.push((
        declared,
        b"<deviceDescription><status uid=\"0001\" ".to_vec(),
    ));
    Profile::from_zip_bytes(&build_zip(&members)).expect_err("the member is malformed XML")
}

/// Neither `Display` nor `Debug` may render the sentinel; both must keep the actionable shape.
fn assert_redacts(err: &ProfileError) {
    let display = format!("{err}");
    let debug = format!("{err:?}");

    assert!(
        !display.contains(SENTINEL),
        "Display leaked the identifier prefix: {display}"
    );
    assert!(
        !debug.contains(SENTINEL),
        "Debug leaked the identifier prefix: {debug}"
    );
    // The redaction marker and the non-identifier suffix both survive.
    assert!(
        display.contains("<redacted>_DeviceDescription.xml"),
        "Display must keep the actionable, redacted shape: {display}"
    );
    assert!(
        debug.contains("<redacted>"),
        "Debug must show the redaction marker: {debug}"
    );
    assert!(
        debug.contains("_DeviceDescription.xml"),
        "Debug must keep the actionable suffix: {debug}"
    );
}

#[test]
fn profile_error_renders_no_identifier_bearing_filename() {
    // The `declared`-carrying variant …
    let missing = declared_missing_error();
    assert!(
        matches!(missing, ProfileError::DeclaredMemberMissing { .. }),
        "expected DeclaredMemberMissing, got {missing:?}"
    );
    // Display still names the manifest field.
    assert!(
        format!("{missing}").contains("deviceDescriptionFileName"),
        "the offending manifest field must survive redaction: {missing}"
    );
    assert_redacts(&missing);

    // … and the three `member`-carrying variants.
    let not_utf8 = not_utf8_error();
    assert!(
        matches!(not_utf8, ProfileError::NotUtf8 { .. }),
        "expected NotUtf8, got {not_utf8:?}"
    );
    assert_redacts(&not_utf8);

    let too_large = too_large_error();
    assert!(
        matches!(too_large, ProfileError::MemberTooLarge { .. }),
        "expected MemberTooLarge, got {too_large:?}"
    );
    assert_redacts(&too_large);

    let xml = xml_error();
    assert!(
        matches!(xml, ProfileError::Xml { .. }),
        "expected Xml, got {xml:?}"
    );
    assert_redacts(&xml);
}
