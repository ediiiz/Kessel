//! Every workspace member opts in to the one lint policy.
//!
//! The policy is declared once in the root `Cargo.toml` `[workspace.lints]` plus `clippy.toml`, and
//! each member opts in with `lints.workspace = true`. Nothing in Cargo enforces the opt-in: a crate
//! that omits it compiles and silently escapes the budget. This test is that enforcement.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::fs;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

/// The workspace root, derived from this crate's manifest directory (`apps/server`).
fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .expect("apps/server is two levels below the workspace root")
        .to_path_buf()
}

fn read_table(path: &Path) -> Table {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
        .parse::<Table>()
        .unwrap_or_else(|e| panic!("{} is not valid TOML: {e}", path.display()))
}

/// Every directory under `crates/` and `apps/` that contains a `Cargo.toml`.
fn member_manifests(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for group in ["crates", "apps"] {
        let dir = root.join(group);
        let entries =
            fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
        for entry in entries {
            let manifest = entry
                .expect("readable directory entry")
                .path()
                .join("Cargo.toml");
            if manifest.is_file() {
                found.push(manifest);
            }
        }
    }
    found.sort();
    found
}

/// The level of one lint entry, whether it is written as a bare string (`unwrap_used = "warn"`)
/// or as a table (`pedantic = { level = "warn", priority = -1 }`).
///
/// Checking *presence* is not enough: `unwrap_used = "allow"` would satisfy a `contains_key`
/// check while gutting the budget.
fn lint_level<'t>(table: &'t Table, key: &str) -> Option<&'t str> {
    let entry = table.get(key)?;
    entry.as_str().or_else(|| {
        entry
            .as_table()
            .and_then(|t| t.get("level"))
            .and_then(Value::as_str)
    })
}

/// The three lint groups, at `warn` and at `priority = -1`.
///
/// Without the priority Cargo cannot resolve the specific overrides against the group they
/// contradict, and the two deliberate `"allow"`s (`module_name_repetitions`,
/// `multiple_crate_versions`) would not take effect.
fn assert_clippy_groups(clippy: &Table) {
    for group in ["pedantic", "nursery", "cargo"] {
        assert_eq!(
            lint_level(clippy, group),
            Some("warn"),
            "clippy group `{group}` must be warned at the workspace root"
        );
        let priority = clippy
            .get(group)
            .and_then(Value::as_table)
            .and_then(|t| t.get("priority"))
            .and_then(Value::as_integer);
        assert_eq!(
            priority,
            Some(-1),
            "clippy group `{group}` needs `priority = -1` so the specific overrides win"
        );
    }
}

/// Every named clippy lint in the budget, at its exact level.
///
/// `multiple_crate_versions` is absent from this table: it is the one deliberate deviation, and
/// tightening it back to `warn` should not require editing this test.
fn assert_clippy_lint_levels(clippy: &Table) {
    for (lint, expected) in [
        // complexity / size caps
        ("cognitive_complexity", "warn"),
        ("too_many_lines", "warn"),
        ("too_many_arguments", "warn"),
        ("type_complexity", "warn"),
        ("excessive_nesting", "warn"),
        ("large_enum_variant", "warn"),
        ("large_stack_frames", "warn"),
        ("struct_excessive_bools", "warn"),
        ("fn_params_excessive_bools", "warn"),
        ("module_name_repetitions", "allow"),
        // restriction picks
        ("unwrap_used", "warn"),
        ("expect_used", "warn"),
        ("panic", "warn"),
        ("todo", "warn"),
        ("unimplemented", "warn"),
        ("indexing_slicing", "warn"),
        ("dbg_macro", "warn"),
        ("print_stdout", "warn"),
        ("print_stderr", "warn"),
        ("clone_on_ref_ptr", "warn"),
        ("mem_forget", "warn"),
        ("allow_attributes_without_reason", "warn"),
        ("missing_assert_message", "warn"),
    ] {
        assert_eq!(
            lint_level(clippy, lint),
            Some(expected),
            "clippy lint `{lint}` is part of the lint budget and must be `{expected}` in \
             [workspace.lints.clippy] — presence alone is not the gate, the level is"
        );
    }
}

/// The `[workspace.lints]` table carries the policy, not an empty shell that would make
/// every member's opt-in vacuous.
fn assert_lint_policy_is_declared(workspace: &Table) {
    let lints = workspace
        .get("lints")
        .and_then(Value::as_table)
        .expect("root Cargo.toml declares [workspace.lints]");

    let clippy = lints
        .get("clippy")
        .and_then(Value::as_table)
        .expect("[workspace.lints.clippy] is declared");
    assert_clippy_groups(clippy);
    assert_clippy_lint_levels(clippy);

    let rust = lints
        .get("rust")
        .and_then(Value::as_table)
        .expect("[workspace.lints.rust] is declared");
    assert_eq!(
        lint_level(rust, "unsafe_code"),
        Some("forbid"),
        "unsafe_code is forbidden workspace-wide; only hc-transport::tls_psk may ever relax it"
    );
    for lint in ["missing_debug_implementations", "unreachable_pub"] {
        assert_eq!(
            lint_level(rust, lint),
            Some("warn"),
            "rust lint `{lint}` is part of the lint budget and must be `warn`"
        );
    }
}

/// The numeric budget in `clippy.toml` is exact, not softened over time.
fn assert_numeric_budget(root: &Path) {
    let clippy_toml = read_table(&root.join("clippy.toml"));
    for (key, expected) in [
        ("cognitive-complexity-threshold", 15_i64),
        ("too-many-lines-threshold", 80),
        ("too-many-arguments-threshold", 5),
        ("type-complexity-threshold", 200),
        ("excessive-nesting-threshold", 4),
        ("enum-variant-size-threshold", 200),
        ("max-struct-bools", 3),
        ("max-fn-params-bools", 3),
    ] {
        assert_eq!(
            clippy_toml.get(key).and_then(Value::as_integer),
            Some(expected),
            "clippy.toml `{key}` must be {expected} exactly"
        );
    }
    // A bool, so it falls outside the loop above; false while the API is pre-1.0.
    assert_eq!(
        clippy_toml
            .get("avoid-breaking-exported-api")
            .and_then(Value::as_bool),
        Some(false),
        "clippy.toml `avoid-breaking-exported-api` must be false: pre-1.0 we \
         want the pedantic/nursery API-shape lints to fire on public items, not be suppressed"
    );
}

/// `cargo fmt --check` with no `rustfmt.toml` would gate on rustfmt's defaults rather than the
/// house style, and pass just as loudly.
fn assert_rustfmt_config_exists(root: &Path) {
    let rustfmt = root.join("rustfmt.toml");
    assert!(
        rustfmt.is_file(),
        "rustfmt.toml must exist at the workspace root: without it `cargo fmt --check` silently \
         gates on rustfmt defaults instead of the repository's formatting policy"
    );
    let parsed = read_table(&rustfmt);
    assert_eq!(
        parsed.get("max_width").and_then(Value::as_integer),
        Some(100),
        "rustfmt.toml `max_width` must be 100, the width every other budget is written against"
    );
}

/// Every crate on disk opts in, and is a workspace member. This is the part that actually rots.
fn assert_every_member_opts_in(root: &Path, workspace: &Table) {
    let manifests = member_manifests(root);
    assert!(
        manifests.len() >= 7,
        "expected at least the six hc-* crates plus apps/server, found {}: {manifests:?}",
        manifests.len()
    );

    let members: Vec<&str> = workspace
        .get("members")
        .and_then(Value::as_array)
        .expect("[workspace] declares members")
        .iter()
        .filter_map(Value::as_str)
        .collect();

    let mut missing_lints = Vec::new();
    let mut missing_membership = Vec::new();
    for manifest in &manifests {
        let parsed = read_table(manifest);
        let opted_in = parsed
            .get("lints")
            .and_then(Value::as_table)
            .and_then(|t| t.get("workspace"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !opted_in {
            missing_lints.push(manifest.display().to_string());
        }

        let rel = manifest
            .parent()
            .and_then(|p| p.strip_prefix(root).ok())
            .expect("member path is under the workspace root")
            .to_string_lossy()
            .replace('\\', "/");
        if !members.contains(&rel.as_str()) {
            missing_membership.push(rel);
        }
    }

    assert!(
        missing_lints.is_empty(),
        "these workspace members do not declare `lints.workspace = true` and have silently \
         escaped the lint budget: {missing_lints:?}"
    );
    assert!(
        missing_membership.is_empty(),
        "these directories have a Cargo.toml but are not in [workspace.members], so \
         `cargo clippy --workspace` never sees them: {missing_membership:?}"
    );
}

#[test]
fn clippy_gate_is_wired() {
    let root = workspace_root();
    let root_manifest = read_table(&root.join("Cargo.toml"));
    let workspace = root_manifest
        .get("workspace")
        .and_then(Value::as_table)
        .expect("root Cargo.toml has a [workspace] table");

    assert_lint_policy_is_declared(workspace);
    assert_numeric_budget(&root);
    assert_rustfmt_config_exists(&root);
    assert_every_member_opts_in(&root, workspace);
}
