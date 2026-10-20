//! Regenerates the committed TypeScript bindings in `packages/api-types/src/generated/` and the
//! label catalogue's `data/labels/schema.json`.
//!
//! `tests/types_are_in_sync.rs` and `tests/feature_labels.rs` compare both against a fresh run. This is an
//! example rather than a binary (it would ship) or an ignored test (it would run with the live
//! suite and write to the repository).

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a developer tool run from a terminal; no tracing subscriber is installed"
)]

fn main() {
    // Derived the same way `tests/types_are_in_sync.rs` derives it, so the two cannot drift.
    let generated = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("packages")
        .join("api-types")
        .join("src")
        .join("generated");

    let schema = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join("labels")
        .join("schema.json");
    if let Err(error) = std::fs::write(&schema, server::labels::catalogue_schema()) {
        eprintln!("writing {}: {error}", schema.display());
        std::process::exit(1);
    }
    println!("wrote {}", schema.display());

    match server::types::export_api_types(&generated) {
        Ok(files) => {
            println!("wrote {} files to {}", files.len(), generated.display());
            for file in files {
                println!("  {file}");
            }
            println!("\nreview the diff and commit it: the sync gate compares byte-for-byte");
        }
        Err(error) => {
            eprintln!("the generator failed: {error}");
            std::process::exit(1);
        }
    }
}
