//! The removed write and read gates stay removed.
//!
//! A sweep over the source text of `crates/**` and `apps/**`, with the needles assembled from parts
//! so this file cannot match itself, and a non-vacuity check so a broken walk cannot pass silently.
//! Re-introducing a gate deliberately means changing this test first.

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

mod common;

/// Directories the walk does not descend into.
///
/// `worktrees` is skipped because walking it flags the primary clone's files at their nested paths.
const SKIPPED_DIRS: [&str; 5] = ["target", "node_modules", ".git", "worktrees", "build"];

/// Every `.rs` file under `root`, excluding [`SKIPPED_DIRS`] and this test file itself.
fn rust_sources(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !SKIPPED_DIRS.contains(&name.as_ref()) {
                rust_sources(&path, out);
            }
        } else if name.ends_with(".rs") && name != "no_gates_remain.rs" {
            out.push(path);
        }
    }
}

/// How many lines of `text` name `needle`.
///
/// Comments count too: the removed gates are not named anywhere in the tree, code or prose, so a
/// mention of one is a sign that it is coming back.
fn lines_naming(text: &str, needle: &str) -> usize {
    text.lines()
        .filter(|line| names_the_gate(line, needle))
        .count()
}

/// Whether `line` names `needle` as a whole identifier, so a longer identifier such as the
/// `LiveWriter` alias does not match.
fn names_the_gate(line: &str, needle: &str) -> bool {
    let mut rest = line;
    while let Some(at) = rest.find(needle) {
        let after = rest[at + needle.len()..].chars().next();
        if !after.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            return true;
        }
        rest = &rest[at + needle.len()..];
    }
    false
}

#[test]
fn the_removed_gates_stay_removed() {
    // Assembled from parts so this file's own text cannot satisfy the sweep it runs.
    let live_env = format!("KESSEL{}LIVE", "_");
    let write_env = format!("{live_env}{}WRITE", "_");
    let needles = [
        write_env,
        format!("Live{}", "Write"),
        format!("Live{}", "Reads"),
        format!("fn {}authorize", ""),
        format!("is{}ever{}writable", "_", "_"),
    ];

    let root = common::workspace_root();
    let mut files = Vec::new();
    rust_sources(&root.join("crates"), &mut files);
    rust_sources(&root.join("apps"), &mut files);

    // Non-vacuity: the walk must actually have found the tree.
    assert!(
        files.len() > 80,
        "the source walk found only {} files, which means it is broken rather than that the tree is \
         small — every assertion below would pass vacuously",
        files.len()
    );

    let mut offenders: Vec<String> = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
        for needle in &needles {
            let hits = lines_naming(&text, needle);
            if hits > 0 {
                offenders.push(format!("{relative}: {needle} ({hits} line(s))"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "the write and read gates were removed on purpose and must not come back unasked, not even \
         as a name in a comment. Offenders: {offenders:#?}"
    );
}

/// The needles are capable of matching — otherwise the absence above proves nothing.
#[test]
fn the_sweep_can_detect_a_reintroduced_gate() {
    let write_env = format!("KESSEL{}LIVE{}WRITE", "_", "_");
    let planted = format!("    let gate = std::env::var(\"{write_env}\");");
    assert!(
        planted.contains(&write_env),
        "the needle must match a line that re-introduces the gate, or the sweep is decorative"
    );
    assert!(
        names_the_gate(&planted, &write_env),
        "the whole-identifier matcher must catch the planted line"
    );
    // A longer identifier that merely starts with a needle is not a match.
    assert!(
        !names_the_gate(
            "pub type LiveWriter = GuardedWriter<BoxedTransport>;",
            "LiveWrite"
        ),
        "LiveWriter must not be mistaken for LiveWrite, or the sweep flags a legitimate type and the \
         obvious fix is to weaken it"
    );
    // A comment naming a needle is flagged as well: the gates are not named anywhere.
    let comment = format!("// gated behind {write_env}");
    assert_eq!(
        lines_naming(&comment, &write_env),
        1,
        "a comment that names the gate must be caught too, or the env var can creep back as prose \
         before it creeps back as code"
    );
}
