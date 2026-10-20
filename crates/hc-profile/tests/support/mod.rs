//! Fixture plumbing shared by the profile tests.
//!
//! Two tiers:
//!
//! * **Committed**: scrubbed copies of four real profile ZIPs in `tests/fixtures/`, with every
//!   identifier replaced by a synthetic value of the same length and character class (see
//!   `tests/fixtures/scrub.py`). These always run.
//! * **Raw**: the real ZIPs, opt-in via `KESSEL_RAW_PROFILES`. When unset, the gated assertions
//!   announce the skip on stderr rather than passing quietly.

#![allow(
    clippy::redundant_pub_crate,
    reason = "contradicts rustc's unreachable_pub; pub(crate) is the correct visibility"
)]
#![allow(
    dead_code,
    reason = "each integration-test binary links this module separately and uses a subset"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "test code may panic, and a skip notice must reach stderr"
)]

use std::io::Write as _;
use std::path::{Path, PathBuf};

use hc_profile::Profile;

/// The env var that opts the suite into the real, unscrubbed profile ZIPs.
///
/// `KESSEL_RAW_PROFILES=1` looks in `<workspace>/secrets`; any other value is taken as the
/// directory to look in (useful from a git worktree, which has no `secrets/` of its own).
pub(crate) const RAW_PROFILES_ENV: &str = "KESSEL_RAW_PROFILES";

/// The four committed fixtures.
pub(crate) const FIXTURES: [Fixture; 4] = [
    Fixture {
        appliance: "hob",
        file: "hob-aes.zip",
        real_marker: "hob",
    },
    Fixture {
        appliance: "dishwasher",
        file: "dishwasher-tls.zip",
        real_marker: "dishwasher",
    },
    Fixture {
        appliance: "oven",
        file: "oven-tls.zip",
        real_marker: "oven",
    },
    Fixture {
        appliance: "fridge",
        file: "fridge-tls.zip",
        real_marker: "fridge",
    },
];

/// One appliance, in both tiers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fixture {
    /// Human name, used in assertion messages.
    pub(crate) appliance: &'static str,
    /// The committed, scrubbed ZIP under `tests/fixtures/`.
    pub(crate) file: &'static str,
    /// The substring that identifies this appliance's real ZIP filename.
    pub(crate) real_marker: &'static str,
}

/// `crates/hc-profile/tests/fixtures`.
#[must_use]
pub(crate) fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Raw bytes of a committed fixture.
#[must_use]
pub(crate) fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = fixture_dir().join(name);
    std::fs::read(&path).unwrap_or_else(|err| panic!("reading fixture {}: {err}", path.display()))
}

/// A committed fixture, parsed.
#[must_use]
pub(crate) fn fixture_profile(name: &str) -> Profile {
    Profile::from_zip_bytes(&fixture_bytes(name))
        .unwrap_or_else(|err| panic!("parsing fixture {name}: {err}"))
}

/// The directory holding the real ZIPs, if the gate is open.
#[must_use]
pub(crate) fn raw_profile_dir() -> Option<PathBuf> {
    let value = std::env::var(RAW_PROFILES_ENV).ok()?;
    if value.is_empty() {
        return None;
    }
    if value == "1" || value == "true" {
        // `<crate>/../..` is the workspace root.
        return Some(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("secrets"),
        );
    }
    Some(PathBuf::from(value))
}

/// The real ZIP for one appliance, if the gate is open and the file is there.
#[must_use]
pub(crate) fn raw_profile_path(fixture: &Fixture) -> Option<PathBuf> {
    let dir = raw_profile_dir()?;
    let entries = std::fs::read_dir(&dir).ok()?;
    entries
        .filter_map(core::result::Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension().is_some_and(|ext| ext == "zip")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(fixture.real_marker))
        })
}

/// Announces a skipped gated assertion on the process's real stderr.
///
/// Not `eprintln!`: libtest captures it, so the skip would be invisible without `--nocapture`.
/// Writing to the `stderr` handle bypasses that capture.
pub(crate) fn announce_skip(test: &str, reason: &str) {
    let mut err = std::io::stderr();
    let _ = writeln!(err, "SKIP {test}: {reason}");
    let _ = err.flush();
}

/// Runs `body` once per real profile, or announces a loud skip if the gate is shut.
///
/// Returns the number of real profiles actually visited, so a caller can assert on it when the
/// gate *is* open.
pub(crate) fn for_each_raw_profile(test: &str, mut body: impl FnMut(&Fixture, Profile)) -> usize {
    let Some(dir) = raw_profile_dir() else {
        announce_skip(
            test,
            &format!(
                "{RAW_PROFILES_ENV} is not set, so the real profile ZIPs were not exercised \
                 (the scrubbed fixtures still were)"
            ),
        );
        return 0;
    };

    let mut visited = 0;
    for fixture in &FIXTURES {
        let Some(path) = raw_profile_path(fixture) else {
            announce_skip(
                test,
                &format!(
                    "no real ZIP for the {} in {}",
                    fixture.appliance,
                    dir.display()
                ),
            );
            continue;
        };
        let profile = Profile::from_zip_path(&path)
            .unwrap_or_else(|err| panic!("parsing the real {} profile: {err}", fixture.appliance));
        body(fixture, profile);
        visited += 1;
    }
    visited
}

/// Rebuilds a ZIP from `(name, body)` pairs, to forge an odd archive.
#[must_use]
pub(crate) fn build_zip(members: &[(String, Vec<u8>)]) -> Vec<u8> {
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in members {
        writer.start_file(name.as_str(), options).unwrap();
        writer.write_all(body).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// Rebuilds a ZIP with its members **deflated** rather than stored.
///
/// For the zip-bomb test: a stored archive is as large as its contents, so a small archive that
/// inflates hugely needs compression.
#[must_use]
pub(crate) fn build_deflated_zip(members: &[(String, Vec<u8>)]) -> Vec<u8> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in members {
        writer.start_file(name.as_str(), options).unwrap();
        writer.write_all(body).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// Whether a ZIP member name carries this extension, case-insensitively.
#[must_use]
pub(crate) fn has_extension(name: &str, extension: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
}

/// Every member of a ZIP, as `(name, bytes)` in archive order.
#[must_use]
pub(crate) fn read_zip(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    use std::io::Read as _;

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    let mut members = Vec::with_capacity(names.len());
    for name in names {
        let mut member = archive.by_name(&name).unwrap();
        let mut body = Vec::new();
        member.read_to_end(&mut body).unwrap();
        drop(member);
        members.push((name, body));
    }
    members
}
