//! Tracked-file leak sweep: no real appliance identifier appears in any tracked file. Any hit is
//! a failure.
//!
//! Needles are the real identifiers from `secrets/`, loaded at runtime when `KESSEL_RAW_PROFILES`
//! points at them; otherwise the sweep announces a skip on stderr. The grep itself is proven by
//! [`a_planted_needle_is_reported_as_a_violation`], which needs no secrets. Violations are reported
//! by file and line only, never by value.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use hc_profile::Profile;

/// A value that must not appear in a tracked file. `value` is never rendered by the sweep.
#[derive(Debug, Clone)]
struct Needle {
    value: String,
}

/// One occurrence of a needle in a tracked file, located to a 1-indexed line.
#[derive(Debug, Clone)]
struct Hit {
    file: String,
    line: usize,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Pushes `value` as a needle if it is long enough to be a genuine identifier (a short token would
/// cause spurious substring hits).
fn push_needle(needles: &mut Vec<Needle>, value: &str) {
    if value.len() >= 6 {
        needles.push(Needle {
            value: value.to_owned(),
        });
    }
}

/// Announces a skipped env-gated subset on the process's real stderr, bypassing libtest's capture
/// so a passing test's skip is still visible.
fn announce(message: &str) {
    use std::io::Write as _;
    let mut err = std::io::stderr();
    let _ = writeln!(err, "{message}");
    let _ = err.flush();
}

/// Resolves the `secrets/` directory from `KESSEL_RAW_PROFILES`: `1`/`true` → `<workspace>/secrets`;
/// any other non-empty value → that directory.
///
/// Unset and empty are the same case, and both are announced as a skip.
fn raw_profiles_dir() -> Option<PathBuf> {
    let value = std::env::var("KESSEL_RAW_PROFILES")
        .ok()
        .unwrap_or_default();
    if value.is_empty() {
        announce(
            "SKIP no_real_identifier_in_tracked_files (secrets subset): KESSEL_RAW_PROFILES is not \
             set, so secrets/ real identifiers were not swept for",
        );
        return None;
    }
    if value == "1" || value == "true" {
        Some(workspace_root().join("secrets"))
    } else {
        Some(PathBuf::from(value))
    }
}

/// The real appliance identifiers from `secrets/` (haId / serial / mac / psk / iv), if the gate is
/// open. Returns `None` with a loud skip otherwise.
fn secrets_needles() -> Option<Vec<Needle>> {
    let dir = raw_profiles_dir()?;
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut needles = Vec::new();
    let mut found = 0usize;
    for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        if path.extension().is_none_or(|ext| ext != "zip") {
            continue;
        }
        let Ok(profile) = Profile::from_zip_path(&path) else {
            continue;
        };
        let identity = profile.identity();
        push_needle(&mut needles, identity.ha_id.expose_secret());
        if let Some(serial) = identity.serial_number.as_ref() {
            push_needle(&mut needles, serial.expose_secret());
        }
        if let Some(mac) = identity.mac.as_ref() {
            push_needle(&mut needles, mac.expose_secret());
        }
        let creds = profile.credentials();
        push_needle(&mut needles, creds.psk64.expose_secret());
        if let Some(iv) = creds.iv64.as_ref() {
            push_needle(&mut needles, iv.expose_secret());
        }
        found += 1;
    }
    if found == 0 {
        announce(&format!(
            "SKIP no_real_identifier_in_tracked_files (secrets subset): no readable profile ZIPs \
             in {}",
            dir.display()
        ));
        return None;
    }
    Some(needles)
}

/// The relative paths of every tracked file, via `git ls-files`.
fn tracked_files() -> Vec<String> {
    let output = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(workspace_root())
        .output()
        .expect("`git ls-files` runs");
    assert!(output.status.success(), "`git ls-files` exited non-zero");
    output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|slice| !slice.is_empty())
        .map(|slice| String::from_utf8_lossy(slice).into_owned())
        .collect()
}

/// Every needle occurrence across the tracked tree, located to `file:line`.
fn scan_tracked_tree(needles: &[Needle]) -> Vec<Hit> {
    let mut hits = Vec::new();
    for rel in tracked_files() {
        let Ok(bytes) = std::fs::read(workspace_root().join(&rel)) else {
            continue;
        };
        let content = String::from_utf8_lossy(&bytes);
        for (index, line) in content.lines().enumerate() {
            if needles.iter().any(|needle| line.contains(&needle.value)) {
                hits.push(Hit {
                    file: rel.clone(),
                    line: index + 1,
                });
            }
        }
    }
    hits
}

/// With `KESSEL_RAW_PROFILES` unset, the sweep announces its skip.
///
/// The announcement goes to the real stderr ([`announce`]), so this re-runs this test binary as a
/// child with the variable removed and asserts over the child's stderr.
#[test]
fn an_unset_raw_profiles_gate_announces_its_skip() {
    let exe = std::env::current_exe().expect("this test binary's own path");
    let output = Command::new(exe)
        .args(["--exact", "no_real_identifier_in_tracked_files"])
        .env_remove("KESSEL_RAW_PROFILES")
        .output()
        .expect("re-running this test binary as a child");
    assert!(
        output.status.success(),
        "the child run of the sweep itself must pass; it exited {:?}",
        output.status.code()
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "SKIP no_real_identifier_in_tracked_files (secrets subset): KESSEL_RAW_PROFILES is not \
             set"
        ),
        "with KESSEL_RAW_PROFILES unset the sweep must ANNOUNCE that it skipped the secrets/ \
         subset (a skip must never be silent), but no such line reached stderr. Child stderr \
         was: {stderr:?}"
    );
}

/// A synthetic sentinel, not an identifier, present in this tracked file so the negative control
/// below has something to find.
const NEGATIVE_CONTROL_SENTINEL: &str = "kessel-planted-needle-sentinel";

/// Negative control: the sweep catches a value planted in a tracked file and reports it as a
/// violation, using the same `scan_tracked_tree` as the real sweep. This is the sweep's
/// non-vacuity proof, since a clean tree yields zero hits.
#[test]
fn a_planted_needle_is_reported_as_a_violation() {
    let needles = vec![Needle {
        value: NEGATIVE_CONTROL_SENTINEL.to_owned(),
    }];

    let hits = scan_tracked_tree(&needles);
    assert!(
        !hits.is_empty(),
        "the sweep found no occurrence of the planted sentinel, which is present in this tracked \
         file — the grep over the tracked tree is not working"
    );
    assert!(
        hits.iter()
            .any(|hit| hit.file.ends_with("tracked_file_leak_sweep.rs")),
        "the planted sentinel was found, but not in this file, where it demonstrably is: {:?}",
        hits.iter().map(|hit| &hit.file).collect::<Vec<_>>()
    );
}

#[test]
fn no_real_identifier_in_tracked_files() {
    // With the gate unset this sweeps nothing, and `raw_profiles_dir` announces the skip.
    let Some(needles) = secrets_needles() else {
        return;
    };
    assert!(
        !needles.is_empty(),
        "KESSEL_RAW_PROFILES resolved to a directory that yielded no identifiers. An empty needle \
         set makes this gate pass by sweeping for nothing, which is the one outcome it must never \
         reach quietly — point the variable at the real profiles or unset it and take the announced \
         skip"
    );

    // Every hit is a violation, reported by file:line only, never the value.
    let violations: Vec<String> = scan_tracked_tree(&needles)
        .iter()
        .map(|hit| format!("{}:{}", hit.file, hit.line))
        .collect();
    assert!(
        violations.is_empty(),
        "a real appliance identifier appears in {} tracked location(s): {violations:?}",
        violations.len()
    );
}
