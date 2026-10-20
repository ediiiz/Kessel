//! The profile store: what makes an onboarded device survive a restart.
//!
//! Two files per device, `<slug>.zip` and `<slug>.json`:
//!
//! * the ZIP verbatim, because `hc_profile::Credentials` is `#[non_exhaustive]` and cannot be
//!   constructed outside its crate. Re-parsing the original bytes keeps `Profile::from_zip_bytes` the
//!   single parse path, and everything derived (model, mapping, role, fingerprint) is recomputed;
//! * the JSON record, holding only what the export cannot know: the operator's host and port and the
//!   fingerprint that identifies the device.
//!
//! Both are written `0600`. The ZIP carries the `haId`, serial and MAC, so the directory is as
//! sensitive as `secrets/`, which is why the record carries a fingerprint and not the `haId`.
//!
//! Every test is filesystem-only, against a `tempfile::TempDir` and the scrubbed fixtures.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

mod common;

use hc_profile::Profile;
use server::store::{ProfileStore, StoredDevice};

/// A record for a device reached at `host`, with no port override.
fn record(profile: &Profile, host: Option<&str>) -> StoredDevice {
    StoredDevice {
        fingerprint: profile.fingerprint(),
        manual_host: host.map(str::to_owned),
        port: None,
    }
}

/// A saved device comes back with everything the daemon needs to rebuild it.
///
/// The assertion that matters is on `manual_host` and `port`: they are not in the ZIP, and losing
/// them would silently return the device to a derived host that usually does not resolve.
#[test]
fn a_saved_device_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(dir.path());
    let zip = common::hob_zip_bytes();
    let profile = Profile::from_zip_bytes(&zip).unwrap();

    let mut stored = record(&profile, Some("192.0.2.55"));
    stored.port = Some(8080);
    store.save("cooktop", &zip, &stored).unwrap();

    let loaded = store.load_all();
    assert_eq!(loaded.len(), 1, "exactly the one device that was saved");
    let device = loaded.first().unwrap();
    assert_eq!(device.slug, "cooktop");
    assert_eq!(
        device.record.manual_host.as_deref(),
        Some("192.0.2.55"),
        "the operator's host must survive the round trip — it is not recoverable from the ZIP"
    );
    assert_eq!(device.record.port, Some(8080), "and so must the port");
    assert_eq!(
        device.record.fingerprint,
        profile.fingerprint(),
        "the record's fingerprint must still identify the appliance in the ZIP beside it"
    );
    assert_eq!(
        device.profile.fingerprint(),
        profile.fingerprint(),
        "the re-parsed profile must be the same appliance"
    );
    assert_eq!(
        device.profile.host(),
        "192.0.2.55",
        "the stored host must be applied to the re-parsed profile, not merely carried beside it"
    );
}

/// Both files are owner-only. The ZIP holds the `haId`, serial, MAC and the appliance psk.
#[cfg(unix)]
#[test]
fn both_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(dir.path());
    let zip = common::hob_zip_bytes();
    let profile = Profile::from_zip_bytes(&zip).unwrap();
    store
        .save("cooktop", &zip, &record(&profile, None))
        .unwrap();

    for name in ["cooktop.zip", "cooktop.json"] {
        let mode = std::fs::metadata(dir.path().join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "{name} carries appliance key material and must be owner-only, like the API token's file"
        );
    }
}

/// Saving the same slug again replaces it: one device, one ZIP and one record.
#[test]
fn saving_the_same_slug_twice_replaces_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(dir.path());
    let zip = common::hob_zip_bytes();
    let profile = Profile::from_zip_bytes(&zip).unwrap();

    store
        .save("cooktop", &zip, &record(&profile, Some("10.0.0.1")))
        .unwrap();
    store
        .save("cooktop", &zip, &record(&profile, Some("10.0.0.2")))
        .unwrap();

    let loaded = store.load_all();
    assert_eq!(loaded.len(), 1, "one device, not two");
    assert_eq!(
        loaded.first().unwrap().record.manual_host.as_deref(),
        Some("10.0.0.2"),
        "the second save wins"
    );
    let files = std::fs::read_dir(dir.path()).unwrap().count();
    assert_eq!(files, 2, "one ZIP and one record, not four files");
}

/// A record can be rewritten without re-supplying the ZIP — an operator editing the host or port.
#[test]
fn a_record_can_be_updated_without_the_zip() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(dir.path());
    let zip = common::hob_zip_bytes();
    let profile = Profile::from_zip_bytes(&zip).unwrap();
    store
        .save("cooktop", &zip, &record(&profile, Some("10.0.0.1")))
        .unwrap();

    let mut updated = record(&profile, Some("10.0.0.9"));
    updated.port = Some(8080);
    store.save_record("cooktop", &updated).unwrap();

    let loaded = store.load_all();
    assert_eq!(loaded.len(), 1);
    let device = loaded.first().unwrap();
    assert_eq!(device.record.manual_host.as_deref(), Some("10.0.0.9"));
    assert_eq!(device.record.port, Some(8080));
    assert_eq!(
        device.profile.fingerprint(),
        profile.fingerprint(),
        "the ZIP is untouched by a record-only update"
    );
}

/// Deleting removes both files, so a removed device does not come back on the next restart.
#[test]
fn deleting_removes_both_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(dir.path());
    let zip = common::hob_zip_bytes();
    let profile = Profile::from_zip_bytes(&zip).unwrap();
    store
        .save("cooktop", &zip, &record(&profile, None))
        .unwrap();

    store.delete("cooktop").unwrap();

    assert!(store.load_all().is_empty(), "the device is gone");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "and so are both of its files — a leftover ZIP would be resurrected on the next boot"
    );
}

/// A slug that would escape the directory is refused before anything is written.
///
/// The operator chooses the slug, so it is untrusted input that becomes a filename.
#[test]
fn a_slug_that_escapes_the_directory_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(dir.path());
    let zip = common::hob_zip_bytes();
    let profile = Profile::from_zip_bytes(&zip).unwrap();
    let stored = record(&profile, None);

    for slug in [
        "..",
        "../evil",
        "sub/dir",
        "sub\\dir",
        "/absolute",
        "",
        "with space",
        "with.dot",
    ] {
        assert!(
            store.save(slug, &zip, &stored).is_err(),
            "the slug {slug:?} must be refused: it reaches the filesystem as a filename"
        );
        assert!(
            store.save_record(slug, &stored).is_err(),
            "and refused on the record-only path too, which is a second way in"
        );
    }
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "a refused slug must write nothing at all, not a partial pair"
    );
}

/// One unreadable profile must not stop the daemon booting the others.
///
/// One corrupt ZIP must not leave the daemon serving no devices at all.
#[test]
fn a_corrupt_zip_is_skipped_and_the_rest_still_load() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(dir.path());
    let zip = common::hob_zip_bytes();
    let profile = Profile::from_zip_bytes(&zip).unwrap();
    store
        .save("cooktop", &zip, &record(&profile, None))
        .unwrap();

    let oven_zip = common::fixture_zip_bytes("oven-tls.zip");
    let oven = Profile::from_zip_bytes(&oven_zip).unwrap();
    store.save("oven", &oven_zip, &record(&oven, None)).unwrap();

    // Corrupt the oven's archive in place, leaving its record intact.
    std::fs::write(dir.path().join("oven.zip"), b"not a zip at all").unwrap();

    let loaded = store.load_all();
    assert_eq!(
        loaded.len(),
        1,
        "the readable device still loads; the corrupt one is skipped, not fatal"
    );
    assert_eq!(loaded.first().unwrap().slug, "cooktop");
}

/// A record with no ZIP beside it is not a device.
///
/// The pair is written ZIP-first, so a record with no ZIP means the profile is gone and there is
/// nothing to re-parse.
#[test]
fn an_orphaned_record_is_not_a_device() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(dir.path());
    std::fs::write(
        dir.path().join("ghost.json"),
        br#"{"fingerprint":"00","manual_host":null,"port":null}"#,
    )
    .unwrap();

    assert!(
        store.load_all().is_empty(),
        "a record with no profile beside it cannot be rebuilt into a device"
    );
}

/// An absent directory is the ordinary first-run state, not an error.
#[test]
fn a_missing_directory_loads_nothing_without_failing() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(dir.path().join("not-created-yet"));
    assert!(
        store.load_all().is_empty(),
        "a fresh install has no profile directory and must boot normally"
    );
}

/// Devices load in a deterministic order, so slug allocation after a restart is reproducible.
#[test]
fn devices_load_in_slug_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(dir.path());
    for (slug, fixture) in [
        ("oven", "oven-tls.zip"),
        ("cooktop", "hob-aes.zip"),
        ("fridge", "fridge-tls.zip"),
    ] {
        let zip = common::fixture_zip_bytes(fixture);
        let profile = Profile::from_zip_bytes(&zip).unwrap();
        store.save(slug, &zip, &record(&profile, None)).unwrap();
    }

    let slugs: Vec<String> = store
        .load_all()
        .into_iter()
        .map(|device| device.slug)
        .collect();
    assert_eq!(slugs, ["cooktop", "fridge", "oven"]);
}
