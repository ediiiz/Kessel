//! Stages the SPA for `rust-embed`.
//!
//! The embed's folder must exist at compile time, but `apps/web/build/` is generated and absent on
//! a fresh clone, and CI builds Rust before the SPA. So this script stages into `$OUT_DIR/spa/`:
//! the real build when present, otherwise a placeholder tree of the same shape (`index.html`,
//! `_app/version.json`, one file under `_app/immutable/`) with a `cargo::warning`. The same shape
//! lets the embed tests exercise the same paths and cache rules either way.
//!
//! A release profile with no SPA build fails instead, so a placeholder UI cannot ship.

#![allow(
    clippy::expect_used,
    reason = "a build script has no caller to return an error to; aborting the build is its \
              error path"
)]
#![allow(
    clippy::panic,
    reason = "a panic is how a build script refuses to produce a release binary with the \
              placeholder SPA"
)]
#![allow(
    clippy::print_stdout,
    reason = "stdout is the only channel Cargo reads build-script directives on"
)]

use std::path::{Path, PathBuf};

/// Where the `SvelteKit` build lands, relative to this crate.
const WEB_BUILD: &str = "../web/build";

/// The staging directory inside `OUT_DIR` that the `rust-embed` derive points at.
const STAGED: &str = "spa";

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest.join(WEB_BUILD);
    let staged =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join(STAGED);

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed={WEB_BUILD}");

    // Staging is a mirror, not an accumulation: a file deleted from the SPA build must disappear
    // from the binary too.
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).expect("OUT_DIR is writable");

    if source.is_dir() {
        let copied = copy_tree(&source, &staged);
        println!("cargo::rerun-if-changed={}", source.display());
        if copied == 0 {
            println!(
                "cargo::warning=apps/web/build/ exists but is empty; the binary will serve no SPA"
            );
        }
    } else if is_release() {
        // A release binary is a deployable artefact; a placeholder SPA inside one is a defect that
        // only shows up in front of a user. Fail the build instead.
        panic!(
            "apps/web/build/ is absent, so this release binary would embed the SPA placeholder \
             instead of the real UI.\nBuild the SPA first: `just release` (or `just build` for a \
             debug binary), which runs `bun --filter web build` before cargo.\nIf you meant to \
             build without a UI, use a debug profile — the placeholder is a test affordance, not a \
             shippable one."
        );
    } else {
        write_placeholder(&staged);
        println!(
            "cargo::warning=apps/web/build/ is absent, so the embedded SPA is a placeholder. Run \
             `just build` (bun --filter web build, then cargo build -p server) for the real one."
        );
    }
}

/// Whether cargo is building with a `release`-derived profile.
///
/// Cargo sets `PROFILE` to `release` for the release profile and any profile inheriting from it,
/// and to `debug` otherwise, including for `cargo test`.
fn is_release() -> bool {
    std::env::var_os("PROFILE").is_some_and(|profile| profile == "release")
}

/// Copies `from` into `to` recursively, returning how many files were copied.
fn copy_tree(from: &Path, to: &Path) -> usize {
    let mut copied = 0;
    let entries = match std::fs::read_dir(from) {
        Ok(entries) => entries,
        Err(error) => {
            println!("cargo::warning=cannot read {}: {error}", from.display());
            return 0;
        }
    };
    for entry in entries.flatten() {
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            std::fs::create_dir_all(&target).expect("OUT_DIR is writable");
            copied += copy_tree(&source, &target);
        } else {
            // Watch every file, not just the directory: a directory's mtime does not change when a
            // file inside it is rewritten in place.
            println!("cargo::rerun-if-changed={}", source.display());
            std::fs::copy(&source, &target).expect("OUT_DIR is writable");
            copied += 1;
        }
    }
    copied
}

/// Writes a placeholder tree shaped like a real `SvelteKit` `adapter-static` build.
fn write_placeholder(staged: &Path) {
    let immutable = staged.join("_app").join("immutable").join("entry");
    std::fs::create_dir_all(&immutable).expect("OUT_DIR is writable");

    std::fs::write(
        staged.join("index.html"),
        "<!doctype html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\">\
         <title>Kessel</title></head>\n<body>\n<p>The Kessel SPA was not built into this binary. \
         Run `just build`.</p>\n<script type=\"module\" src=\"./_app/immutable/entry/placeholder.js\">\
         </script>\n</body>\n</html>\n",
    )
    .expect("OUT_DIR is writable");
    std::fs::write(
        staged.join("_app").join("version.json"),
        "{\"version\":\"placeholder\"}\n",
    )
    .expect("OUT_DIR is writable");
    std::fs::write(
        immutable.join("placeholder.js"),
        "// placeholder: apps/web/build/ was absent when this binary was compiled\nexport {};\n",
    )
    .expect("OUT_DIR is writable");
}
