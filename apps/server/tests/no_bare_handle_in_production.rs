//! Nothing under `apps/**` can reach a bare send handle or a raw frame sink.
//!
//! Production holds a live `Session` (the device task needs it to run `run_steady_state`), but the
//! only thing it does with the write half is hand it to an `hc_core::bridge` sealer, which wraps it
//! in a `GuardedWriter` and keeps no clone. The send handle is take-once, so the session has
//! already spent it on the guard.
//!
//! This is a text sweep: a name not written next to `.` or `::` (a function-pointer binding, a
//! macro-assembled name) evades it. It is a review trigger; the real guarantees are the take-once
//! flag and `GuardedWriter`'s private transport. `FrameSink` is a public trait, so whoever
//! constructs a sink can still write unguarded frames; this sweep makes that discipline checkable.

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

/// The workspace root (`apps/server/../..`).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the workspace root resolves")
}

/// Directory names never worth walking under `apps/`: build output and the JS trees.
const SKIPPED_DIRS: [&str; 5] = ["target", "node_modules", "build", ".svelte-kit", ".git"];

/// Every `*.rs` file under `apps/`, as workspace-relative slash-separated paths.
///
/// Rooted at `apps/` so it never sweeps `crates/**`, where these names are supposed to appear.
fn app_rust_files() -> Vec<String> {
    let root = workspace_root();
    let mut out = Vec::new();
    let mut stack = vec![root.join("apps")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() && !SKIPPED_DIRS.contains(&name.as_str()) {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let relative = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push(relative);
            }
        }
    }
    out.sort();
    out
}

/// Files exempt from the sweep. Empty: an entry would be a claim that a file names a bare-handle
/// route and is safe anyway.
const ALLOWLIST: [&str; 0] = [];

#[test]
fn no_file_under_apps_names_a_bare_send_handle_or_sink() {
    let files = app_rust_files();

    // Non-vacuity: a typo in the root would otherwise make this pass by looking at nothing.
    assert!(
        files.len() > 15,
        "the walk under apps/ found only {} .rs files, which cannot be this workspace: {files:?}",
        files.len()
    );
    assert!(
        files.contains(&"apps/server/src/live_transport.rs".to_owned()),
        "the walk must reach the file that holds the live Session, or it proves nothing: {files:?}"
    );

    // Every needle is assembled at runtime from parts, so this file does not contain one and the
    // sweep can cover it too without a self-exemption. Do not spell the qualified form out anywhere
    // in this file, comments included, or the sweep flags itself.
    //
    // The raw-handle route has no legitimate use anywhere under `apps/`, tests included, so it is
    // swept over every file, in both the dotted and the fully-qualified spelling.
    let handle = "sender";
    let handle_needles = [format!(".into_{handle}("), format!("::into_{handle}(")];

    // The raw-sink needles are swept over `src/` only: a test may implement the trait for an
    // in-process recording sink (`tests/write_registry.rs` does). They name the capability
    // (implementing the trait, calling its write method), not the type: the device task is generic
    // over the transport halves and must write the trait as a bound, which grants nothing.
    let write_method = format!("{}_frame", "send");
    let sink_needles = [write_method, format!("impl {} for", "FrameSink")];

    let mut offenders = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(workspace_root().join(file)) else {
            continue;
        };
        let is_production = file.contains("/src/");
        let hit = handle_needles.iter().any(|needle| text.contains(needle))
            || (is_production && sink_needles.iter().any(|needle| text.contains(needle)));
        if hit {
            offenders.push(file.clone());
        }
    }

    let unexpected: Vec<&String> = offenders
        .iter()
        .filter(|file| !ALLOWLIST.contains(&file.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "a route to a bare send handle or a raw frame sink is named under apps/, in: \
         {unexpected:?}\n\n\
         Production may hold a live `Session` — the device task needs one to run `run_steady_state` — \
         but the only thing it may do with the write half is hand it to an `hc_core::bridge` sealer, \
         which puts it inside a `GuardedWriter`. `into_sender` yields the handle RAW, and a `FrameSink` \
         can write frames to an appliance without going through the single write path at all. The \
         guard refuses nothing, so a second route is precisely the harm — there is no gate for it to \
         bypass, only exclusivity for it to break. If a new use \
         is genuinely sealed, add the \
         file to this test's ALLOWLIST with its reason."
    );
}
