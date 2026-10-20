//! A send handle's reachability is enforced, because a `Sender` can transmit without passing the
//! write guard.
//!
//! A `Sender` is cloneable (concurrent sends need it), and the guard receives a `Sender` rather than
//! constructing one, so the enforceable property is about call sites: how many places in the tree can
//! obtain a handle at all, and whether each one seals it. `Session::into_guarded` consumes the session
//! and hands the handle to a sealing closure, so the `Session`-shaped bypass is closed by types.
//!
//! What types cannot close, and why these sweeps exist:
//!
//! * `Sender` is `Clone`, so a caller holding a sanctioned handle can keep a clone.
//! * `into_sender` still yields a role-blind handle for the concurrent-send case.
//! * `crate::transport::FrameSink` is a public trait, so whoever constructs a sink can write frames
//!   with no `Session` at all. Production holds only a `GuardedWriter`, never a raw sink or an
//!   unguarded `Session`.
//!
//! # A conservative text sweep
//!
//! Needles cover both the method-call (`.`) and fully-qualified (`::`) spellings. A call whose method
//! name is not adjacent to `.` or `::` (a function-pointer binding, a macro-assembled name) evades
//! them; the sweeps are a review trigger, and the load-bearing guarantees are the take-once flag and
//! `GuardedWriter`'s private transport.
//!
//! The pattern is matched against file text, so a mention in a comment counts as a call site. Prose
//! in this file therefore never writes a swept method name directly after `.` or `::`. Over-reporting
//! is cleared by an allowlist entry, which is the right direction to be wrong in.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed asserts; library code may not"
    )
)]

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The workspace root (`crates/hc-transport/../..`).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Directory names never worth walking: build output, gitignored third-party checkouts, real
/// secrets, the JS trees, and nested worktrees (see
/// [`the_source_walk_does_not_descend_into_nested_worktrees`]).
const SKIPPED_DIRS: [&str; 9] = [
    ".git",
    "target",
    "node_modules",
    "reference",
    "spike",
    "secrets",
    "build",
    ".svelte-kit",
    "worktrees",
];

/// `path` relative to `root`, always slash-separated so an allowlist entry reads the same on every
/// platform.
fn relative_slash_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Every `*.rs` file in the workspace, as workspace-relative slash-separated paths.
fn rust_files() -> Vec<String> {
    let root = workspace_root();
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
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
                out.push(relative_slash_path(&root, &path));
            }
        }
    }
    out.sort();
    out
}

/// The only places a send handle may be taken beside the guard. Adding an entry asserts that the
/// handle it yields cannot reach an appliance without passing `hc_core::write::GuardedWriter::write`.
///
/// * `concurrency.rs` takes a handle via `into_sender` and clones it across 64 tasks to exercise
///   serialised sends; the sink is a fake wire.
/// * `hc-core/tests/msg_id_allocator.rs` uses the route in
///   `a_session_yields_its_send_handle_only_once` to assert it refuses; it obtains no handle.
const SENDER_CALL_SITE_ALLOWLIST: [&str; 2] = [
    "crates/hc-core/tests/msg_id_allocator.rs",
    "crates/hc-transport/tests/concurrency.rs",
];

/// The only places the sealing routes may be called from. A sealing closure is free to return the
/// raw `Sender` it is given, so these routes are a second way to obtain a bare handle.
///
/// * `crates/hc-core/src/bridge.rs`: the sanctioned sealers, which move the `Sender` into a private
///   field of a `GuardedWriter` and keep no clone.
/// * `crates/hc-transport/src/session.rs`: the consuming route delegating to the borrowing one.
/// * `crates/hc-transport/tests/into_guarded.rs`: deliberately returns the raw handle to prove the
///   route is not a stub; its sink is an in-process counter.
const SEAL_CALL_SITE_ALLOWLIST: [&str; 3] = [
    "crates/hc-core/src/bridge.rs",
    "crates/hc-transport/src/session.rs",
    "crates/hc-transport/tests/into_guarded.rs",
];

/// The walk must not descend into `worktrees/`. A nested checkout's copy of an allowlisted file
/// matches no allowlist entry by path, so it would turn both sweeps red.
///
/// The test can only discriminate where a nested worktree exists; elsewhere it logs a SKIP for that
/// half rather than passing silently.
#[test]
fn the_source_walk_does_not_descend_into_nested_worktrees() {
    let files = rust_files();
    let leaked: Vec<&String> = files
        .iter()
        .filter(|file| file.starts_with("worktrees/"))
        .collect();
    // A sample, not the whole list, so the panic message stays readable.
    let sample: Vec<&&String> = leaked.iter().take(3).collect();
    let count = leaked.len();

    assert!(
        leaked.is_empty(),
        "the source walk descended into nested task worktrees: {count} files, e.g. {sample:?}\n\n\
         A file checked out under `worktrees/<task-id>/` is a second copy of a file whose real path is \
         already reviewed and allowlisted, but its path matches no allowlist entry — so both sweeps in \
         this file go red on the primary clone for as long as any worktree is live. Add the directory \
         to SKIPPED_DIRS rather than adding worktree paths to an allowlist."
    );

    // The other half: was there anything to skip? Only the primary clone can answer yes.
    let worktrees = workspace_root().join("worktrees");
    let nested_rust_file_exists = std::fs::read_dir(&worktrees).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            entry
                .path()
                .join("crates/hc-transport/tests/sender_reachability.rs")
                .is_file()
        })
    });
    if !nested_rust_file_exists {
        // Not `eprintln!`: libtest captures the print family, so a skip announced from a passing
        // test would be invisible without `--nocapture`. Writing to the `stderr` handle bypasses that.
        let mut err = std::io::stderr();
        let _ = writeln!(
            err,
            "SKIP the_source_walk_does_not_descend_into_nested_worktrees (discrimination half): no \
             nested worktree with Rust sources under {}. The assertion above ran and passed, but it \
             had nothing to skip, so it proves nothing here. It discriminates on the primary clone.",
            worktrees.display()
        );
        let _ = err.flush();
    }
}

#[test]
fn sender_is_not_reachable_outside_the_guard() {
    let files = rust_files();

    // Non-vacuity 1: the walk found the tree, rather than passing by looking at nothing.
    assert!(
        files.len() > 40,
        "the source walk found only {} .rs files, which cannot be this workspace — the sweep below \
         would pass by looking at nothing. Files: {files:?}",
        files.len()
    );
    assert!(
        files.contains(&"crates/hc-transport/src/session.rs".to_owned()),
        "the walk did not reach the file that DEFINES `sender()`, so it cannot be trusted to find \
         its callers"
    );

    // Built at runtime from parts so this file does not itself contain any pattern and needs no
    // self-exemption. `into_sender` is the sanctioned route but still yields a handle that can
    // transmit, so its call sites are swept too. The `::` forms catch fully-qualified calls.
    let needles = [
        format!(".{}(", "sender"),
        format!("::{}(", "sender"),
        format!(".into_{}(", "sender"),
        format!("::into_{}(", "sender"),
    ];

    let mut call_sites = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(workspace_root().join(file)) else {
            continue;
        };
        if needles.iter().any(|needle| text.contains(needle)) {
            call_sites.push(file.clone());
        }
    }

    // Non-vacuity 2: the pattern matches something, or a rename would make the sweep pass forever.
    assert!(
        !call_sites.is_empty(),
        "no call site of `sender()` was found anywhere. Either the method was renamed — in which \
         case this gate is now watching a pattern that cannot occur — or the sweep is broken. \
         Either way this test is no longer enforcing anything."
    );

    let unexpected: Vec<&String> = call_sites
        .iter()
        .filter(|file| !SENDER_CALL_SITE_ALLOWLIST.contains(&file.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "a route to a bare send handle is used outside the allowlist, in: {unexpected:?}\n\n\
         `sender()` returns a cloneable send handle that can transmit WITHOUT passing \
         `hc_core::write::GuardedWriter::write` — the single write path. That path refuses nothing, \
         so what a bare handle costs is not a bypassed gate but a SECOND route to an appliance, \
         which breaks the guarantee that every write goes through one place. A new call site is not \
         automatically wrong, but it must be \
         reviewed against that: if the handle it yields is sealed into a `GuardedWriter` (as \
         `hc_core::bridge::guarded_session_writer` does) add the file to \
         SENDER_CALL_SITE_ALLOWLIST in this test. If it is not, the write must be routed through \
         the guard instead. See this file's module docs."
    );

    // The allowlist must not rot: an entry that no longer calls the route is a stale exemption.
    for allowed in SENDER_CALL_SITE_ALLOWLIST {
        assert!(
            call_sites.iter().any(|file| file == allowed),
            "`{allowed}` is on SENDER_CALL_SITE_ALLOWLIST but no longer calls `sender()`. Remove \
             the entry rather than leaving a standing exemption behind."
        );
    }
}

/// Every route to a send handle **consumes** the `Session`, so a caller cannot retain the session and
/// mint a second handle from it later. `Sender` is still `Clone`, which is why the call-site sweep
/// above stays.
#[test]
fn the_sanctioned_route_to_a_send_handle_consumes_the_session() {
    let session_rs = std::fs::read_to_string(
        workspace_root()
            .join("crates")
            .join("hc-transport")
            .join("src")
            .join("session.rs"),
    )
    .expect("session.rs is readable");

    // Assembled at runtime, like the sweep's needles, so this file matches neither pattern itself.
    let consuming_decl = format!("pub fn into_{}(self)", "sender");
    let borrowing_decl = format!("pub fn {}(&self)", "sender");

    assert!(
        session_rs.contains(&consuming_decl),
        "`Session` must offer a route to a send handle that CONSUMES the session, so a caller cannot \
         retain the session and mint a second handle later. Expected a `{consuming_decl}` \
         declaration in crates/hc-transport/src/session.rs."
    );

    // No borrowing route to a send handle may exist.
    assert!(
        !session_rs.contains(&borrowing_decl),
        "`{borrowing_decl}` is back. A borrowing route to a send handle lets a caller keep the \
         session and mint further handles from it at any later point, which is the bypass this test \
         exists to close; `Session::into_guarded` (which consumes the session and hands the handle \
         to a sealing closure) is the replacement."
    );
}

/// The sealing routes' call sites are enumerated too, because a sealing closure can return the raw
/// handle it was given. Same text sweep and non-vacuity checks as
/// `sender_is_not_reachable_outside_the_guard`; see [`SEAL_CALL_SITE_ALLOWLIST`].
#[test]
fn the_sealing_routes_are_called_only_where_they_seal() {
    let files = rust_files();
    assert!(
        files.len() > 40,
        "the source walk found only {} .rs files, which cannot be this workspace",
        files.len()
    );

    // Built at runtime from parts, as above; the `::` forms catch fully-qualified calls.
    let needles = [
        format!(".into_{}(", "guarded"),
        format!("::into_{}(", "guarded"),
        format!(".take_{}(", "guarded"),
        format!("::take_{}(", "guarded"),
    ];

    let mut call_sites = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(workspace_root().join(file)) else {
            continue;
        };
        if needles.iter().any(|needle| text.contains(needle)) {
            call_sites.push(file.clone());
        }
    }

    assert!(
        !call_sites.is_empty(),
        "no call site of the sealing routes was found anywhere. Either they were renamed — in which \
         case this gate is watching a pattern that cannot occur — or the sweep is broken."
    );

    let unexpected: Vec<&String> = call_sites
        .iter()
        .filter(|file| !SEAL_CALL_SITE_ALLOWLIST.contains(&file.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "a sealing route is called outside the allowlist, in: {unexpected:?}\n\n\
         The closure passed to `Session::take_guarded`/`into_guarded` receives a `Sender` that can \
         transmit WITHOUT passing `hc_core::write::GuardedWriter::write` — the single write path, \
         which refuses nothing but is the only route there is meant to be. A new call site is not \
         automatically wrong, but the closure must seal the handle (as \
         `hc_core::bridge::guarded_session` does) rather than return it. If it does, add the file to \
         SEAL_CALL_SITE_ALLOWLIST. Prefer calling one of the four \
         `hc_core::bridge` sealers instead of the raw route."
    );

    for allowed in SEAL_CALL_SITE_ALLOWLIST {
        assert!(
            call_sites.iter().any(|file| file == allowed),
            "`{allowed}` is on SEAL_CALL_SITE_ALLOWLIST but no longer calls a sealing route. Remove \
             the entry rather than leaving a standing exemption behind."
        );
    }
}
