//! The committed `packages/api-types/src/generated/` tree is byte-identical to what
//! `server::types::export_api_types` emits now.
//!
//! The barrel (`index.ts`) is generator output and part of the comparison, so a forgotten export
//! fails here instead of surfacing as a missing import in the SPA. Determinism and export order are
//! pinned by `generated_tree_is_deterministic.rs`.
//!
//! Regenerate with `cargo run -p server --example export-types`.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The committed generated tree (`apps/server/../../packages/api-types/src/generated`).
fn committed_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("packages")
        .join("api-types")
        .join("src")
        .join("generated")
}

/// Every `*.ts` file in `dir`, keyed by file name.
fn read_ts_files(dir: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{} unreadable: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.extension().is_some_and(|ext| ext == "ts") {
            let name = path
                .file_name()
                .expect("a file name")
                .to_str()
                .expect("utf-8 file name")
                .to_owned();
            files.insert(name, std::fs::read_to_string(&path).expect("readable"));
        }
    }
    files
}

/// Runs the generator into a fresh temporary directory and reads back what it wrote.
fn generate() -> (Vec<String>, BTreeMap<String, String>) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let written = server::types::export_api_types(dir.path()).expect("the generator runs");
    let files = read_ts_files(dir.path());
    (written, files)
}

#[test]
fn types_are_in_sync() {
    let (written, generated) = generate();
    let committed = read_ts_files(&committed_dir());

    assert!(
        !generated.is_empty(),
        "the generator emitted nothing — there is no API contract to keep in sync"
    );

    let generated_names: Vec<&String> = generated.keys().collect();
    let committed_names: Vec<&String> = committed.keys().collect();
    assert_eq!(
        generated_names, committed_names,
        "the committed generated tree has a different file set than a fresh generator run; \
         run `just types` and commit its output"
    );

    for (name, body) in &generated {
        assert_eq!(
            committed.get(name),
            Some(body),
            "`{name}` differs from a fresh generator run — run `just types` and commit it"
        );
    }

    // The barrel must be one of the files the generator reports writing: a hand-written barrel
    // would be outside this comparison entirely.
    assert!(
        written.iter().any(|file| file == "index.ts"),
        "the generator did not write `index.ts`; a hand-written barrel is not policed by any gate \
         in this repo. Emit it from the generator. Wrote: {written:?}"
    );
}
