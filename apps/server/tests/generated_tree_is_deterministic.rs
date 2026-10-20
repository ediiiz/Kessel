//! Two properties of the `ts-rs` generator that `types_are_in_sync` depends on but cannot itself
//! prove: its output is byte-stable across runs, and the barrel's export order is lexicographic.
//!
//! `types_are_in_sync` compares one run against the committed tree, so a nondeterministic emit loop
//! (for example `HashMap`-ordered) would make it flap red and green at random. Only comparing two
//! runs to each other catches that. A timestamp in the barrel header fails the determinism test but
//! not the ordering test; an unsorted emit list fails both.

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
use std::path::Path;

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

/// Runs the generator into a fresh temporary directory.
fn generate() -> (Vec<String>, BTreeMap<String, String>) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let written = server::types::export_api_types(dir.path()).expect("the generator runs");
    let files = read_ts_files(dir.path());
    (written, files)
}

#[test]
fn generator_output_is_byte_deterministic() {
    let (first_written, first) = generate();
    let (second_written, second) = generate();
    assert_eq!(
        first_written, second_written,
        "the generator reported a different file list on a second run"
    );
    assert_eq!(
        first, second,
        "two generator runs produced different bytes — `types_are_in_sync` would flap red/green at \
         random, which is worse than no gate (packages/api-types/README.md)"
    );
}

#[test]
fn generated_barrel_is_lexicographic_and_covers_every_type() {
    let (_written, generated) = generate();
    let barrel = generated
        .get("index.ts")
        .expect("the generator writes a barrel");

    let exported: Vec<&str> = barrel
        .lines()
        .filter_map(|line| line.strip_prefix("export type * from './"))
        .filter_map(|rest| rest.strip_suffix(".js';"))
        .collect();
    assert!(
        !exported.is_empty(),
        "the barrel re-exports nothing; it was:\n{barrel}"
    );

    let mut sorted = exported.clone();
    sorted.sort_unstable();
    assert_eq!(
        exported, sorted,
        "the barrel's export order must be lexicographic by file name, so the generated tree is deterministic"
    );

    let mut type_files: Vec<String> = generated
        .keys()
        .filter(|name| name.as_str() != "index.ts")
        .map(|name| name.trim_end_matches(".ts").to_owned())
        .collect();
    type_files.sort();
    let exported: Vec<String> = exported.iter().map(|name| (*name).to_owned()).collect();
    assert_eq!(
        exported, type_files,
        "every generated type file must be re-exported by the barrel, and nothing else — a \
         forgotten export is invisible to every other gate in this repo"
    );
}
