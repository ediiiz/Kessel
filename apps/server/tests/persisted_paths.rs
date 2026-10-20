//! Every path the daemon must remember across a restart lands in `/data`.
//!
//! The API token, the stored configuration and each onboarded profile default to relative paths
//! (`secrets/…`), so each needs an explicit `ENV` in the `Dockerfile`, re-asserted as a default in
//! the entrypoint script. Nothing inside `apps/server` can enforce packaging, and a wrong path only
//! shows after a restart, so this is a text assertion over those two files. It reads files and
//! starts nothing.

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

/// The three variables, the `/data` path each must carry, and what is lost when it does not.
///
/// A profile written outside `/data` means the operator re-onboards every appliance after every
/// add-on update.
const PERSISTED: [(&str, &str, &str); 3] = [
    (
        "KESSEL_TOKEN_FILE",
        "/data/kessel.token",
        "the API token regenerates on every restart and reaches the operator as an opaque 401",
    ),
    (
        "KESSEL_CONFIG_FILE",
        "/data/kessel-config.json",
        "the operator's broker settings vanish on restart with no error anywhere",
    ),
    (
        "KESSEL_PROFILE_DIR",
        "/data/profiles",
        "every onboarded appliance is lost on restart",
    ),
];

/// The image sets all three, at their `/data` paths.
#[test]
fn the_dockerfile_redirects_every_persisted_path_into_data() {
    let dockerfile = std::fs::read_to_string(common::workspace_root().join("Dockerfile"))
        .expect("the Dockerfile is readable");

    for (variable, path, consequence) in PERSISTED {
        let assignment = format!("{variable}={path}");
        assert!(
            dockerfile.contains(&assignment),
            "the Dockerfile must contain `{assignment}`, or {consequence}"
        );
    }
}

/// the entrypoint defaults all three to the same paths, so the guarantee survives a trimmed environment.
///
/// The values must **agree** with the `Dockerfile`'s: in the shipped image the `ENV` always wins, because
/// `:=` assigns only when a variable is unset or empty. A the entrypoint default that disagreed would look like
/// it configured something and do nothing at all.
#[test]
fn entrypoint_defaults_agree_with_the_dockerfile() {
    let entrypoint = std::fs::read_to_string(
        common::workspace_root()
            .join("docker")
            .join("entrypoint.sh"),
    )
    .expect("docker/entrypoint.sh is readable");

    for (variable, path, consequence) in PERSISTED {
        let default = format!(": \"${{{variable}:={path}}}\"");
        assert!(
            entrypoint.contains(&default),
            "docker/entrypoint.sh must default {variable} to {path} (as `{default}`) so a trimmed environment \
             still persists it, or {consequence}"
        );
        assert!(
            entrypoint.lines().any(|line| line.starts_with("export ")
                && line.split_whitespace().any(|word| word == variable)),
            "docker/entrypoint.sh must export {variable}, or the default is invisible to the daemon"
        );
    }
}

/// Each default in `apps/server` is **relative**, which is what makes the redirection necessary.
///
/// If one of these became absolute, the `ENV` above would be redundant and this test would be
/// asserting a rule whose reason is gone.
#[test]
fn the_in_crate_defaults_are_relative_paths() {
    for default in [
        server::auth::DEFAULT_TOKEN_FILE,
        server::config::DEFAULT_CONFIG_FILE,
        server::store::DEFAULT_PROFILE_DIR,
    ] {
        assert!(
            !std::path::Path::new(default).is_absolute(),
            "{default} is relative, so it resolves against the working directory — which is why the \
             image must override it. An absolute default here would mean this whole test is obsolete"
        );
    }
}
