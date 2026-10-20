//! `Profile::fingerprint`: the stable, non-identifying key that decides whether a re-uploaded ZIP
//! is a new appliance or the same one again.
//!
//! The `haId` must not reach logs or stored records, so the store keys on a digest of it. This is
//! not a privacy mechanism: an AES `haId` is small enough to enumerate.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests may panic; library code may not"
)]

mod support;

use std::collections::BTreeSet;

use hc_profile::Profile;
use support::{FIXTURES, build_deflated_zip, fixture_bytes, fixture_profile, read_zip};

/// The digest is hex-encoded SHA-256: 64 lowercase hex characters, and nothing else.
#[test]
fn a_fingerprint_is_lowercase_hex_sha256() {
    let fingerprint = fixture_profile("hob-aes.zip").fingerprint();
    assert_eq!(
        fingerprint.len(),
        64,
        "SHA-256 is 32 bytes, so its hex form is 64 characters; got {fingerprint:?}"
    );
    assert!(
        fingerprint
            .chars()
            .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character)),
        "the digest must be lowercase hex so it is safe as a filename component; got {fingerprint:?}"
    );
}

/// Each of the four appliances gets its own fingerprint, so two appliances never upsert onto each
/// other.
#[test]
fn each_appliance_has_its_own_fingerprint() {
    let fingerprints: BTreeSet<String> = FIXTURES
        .iter()
        .map(|fixture| fixture_profile(fixture.file).fingerprint())
        .collect();
    assert_eq!(
        fingerprints.len(),
        FIXTURES.len(),
        "all four fixtures must fingerprint differently, or onboarding one would overwrite another"
    );
}

/// The fingerprint follows the appliance's identity, not the ZIP's bytes.
///
/// A re-exported profile differs byte-for-byte (compression, member order, `created`) but must
/// still match. Here every member is re-deflated and the member order reversed.
#[test]
fn a_reencoded_zip_for_one_appliance_fingerprints_the_same() {
    for fixture in &FIXTURES {
        let original = fixture_bytes(fixture.file);
        let mut members = read_zip(&original);
        members.reverse();
        let reencoded = build_deflated_zip(&members);

        assert_ne!(
            original, reencoded,
            "the {} re-encoding must actually differ in bytes, or this test proves nothing",
            fixture.appliance
        );

        let first = Profile::from_zip_bytes(&original).unwrap().fingerprint();
        let again = Profile::from_zip_bytes(&reencoded).unwrap().fingerprint();
        assert_eq!(
            first, again,
            "the {} must fingerprint identically from a re-encoded archive — this is what makes a \
             re-upload an update rather than a duplicate device",
            fixture.appliance
        );
    }
}

/// A manual host override is operator configuration, not identity: it must not move the fingerprint.
///
/// The store keys the record holding the host on the fingerprint, so it must not shift when the
/// host is edited.
#[test]
fn a_manual_host_does_not_change_the_fingerprint() {
    let plain = fixture_profile("oven-tls.zip");
    let overridden = fixture_profile("oven-tls.zip").with_manual_host("10.0.0.42");
    assert_eq!(
        plain.fingerprint(),
        overridden.fingerprint(),
        "the fingerprint keys the record that stores the host, so editing the host must not move it"
    );
}

/// The digest must not contain the identifier it was derived from.
///
/// The `haId` is read from the (scrubbed) manifest rather than hardcoded, so the test survives a
/// re-scrub.
#[test]
fn a_fingerprint_does_not_contain_the_ha_id() {
    for fixture in &FIXTURES {
        let ha_id = manifest_ha_id(fixture.file);
        let fingerprint = fixture_profile(fixture.file).fingerprint();
        assert!(
            !fingerprint.contains(&ha_id),
            "the {} fingerprint must not carry the haId it was derived from",
            fixture.appliance
        );
        assert!(
            !fingerprint.contains(&ha_id.to_ascii_lowercase()),
            "the {} fingerprint must not carry the lowercased haId either",
            fixture.appliance
        );
    }
}

/// The `haId` a fixture's manifest declares, read straight out of the archive.
fn manifest_ha_id(file: &str) -> String {
    let members = read_zip(&fixture_bytes(file));
    let (_, body) = members
        .iter()
        .find(|(name, _)| name.to_ascii_lowercase().ends_with(".json"))
        .expect("every fixture has a JSON manifest");
    let manifest: serde_json::Value = serde_json::from_slice(body).unwrap();
    // `.get` rather than indexing, which `clippy::indexing_slicing` rejects.
    manifest
        .get("haId")
        .and_then(serde_json::Value::as_str)
        .expect("the manifest declares a haId")
        .to_owned()
}
