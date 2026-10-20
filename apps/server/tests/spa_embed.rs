//! The SPA is served from the binary, with no `apps/web/build/` needed on disk.
//!
//! "Embedded" means three things together:
//!
//! 1. every asset the binary carries is served with 200 and the byte-identical content of the
//!    compiled-in copy (`spa::Assets::get`);
//! 2. `spa::ASSETS_ARE_EMBEDDED` is true, i.e. the `embed-assets` feature
//!    (`rust-embed/debug-embed`) is on. Without it `rust-embed` reads the staged folder from disk in a debug build, and every
//!    other assertion here would still pass;
//! 3. an embedded path is served, and anything else gets `index.html` with status 200, not 404,
//!    with the right cache headers.
//!
//! `build.rs` stages a placeholder tree of the same shape when `apps/web/build/` is absent, so no
//! assertion here is conditional. When the real build is present, the staged copy is also compared
//! to it byte for byte.

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

use axum::http::StatusCode;
use server::spa;
use server::state::AppState;

#[tokio::test]
async fn spa_assets_are_embedded() {
    assert_embedding_is_compiled_in();

    let names = spa::asset_names();
    assert!(
        names.iter().any(|name| name == spa::SHELL),
        "the binary must carry the SPA shell; it carries {names:?}"
    );
    assert!(
        names.iter().any(|name| name.starts_with("_app/")),
        "the binary must carry the SvelteKit `_app/` tree; it carries {names:?}"
    );

    assert_every_asset_is_served_from_the_binary(&names).await;
    assert_shell_and_fallback().await;
    assert_staged_copy_matches_a_real_build();
}

/// The assertion the other three depend on: the bytes are in the binary, not on disk.
fn assert_embedding_is_compiled_in() {
    #[expect(
        clippy::assertions_on_constants,
        reason = "the value IS a compile-time configuration flag; asserting it is how a \
                  switched-off `embed-assets` feature becomes a failing test rather than a binary \
                  that silently reads its assets from disk"
    )]
    {
        assert!(
            spa::ASSETS_ARE_EMBEDDED,
            "the `embed-assets` feature is off, so `rust-embed` would read the staged folder from \
             disk at run time and every other assertion here would be about a directory"
        );
    }
}

/// Every embedded asset is served, byte-identically, with the Cache-Control its path implies.
async fn assert_every_asset_is_served_from_the_binary(names: &[String]) {
    for name in names {
        let reply = common::get(server::api::app(state()), &format!("/{name}")).await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "embedded asset `{name}` must be served: {}",
            reply.body
        );
        let embedded = spa::Assets::get(name).expect("the name came from the embed");
        // `reply.bytes`, not `reply.body`: the text view is lossy, and the SPA now embeds fonts.
        // Comparing the lossy string turned every non-UTF-8 byte into U+FFFD and failed here while
        // the served bytes were in fact identical.
        assert_eq!(
            reply.bytes,
            embedded.data.as_ref(),
            "`{name}` was not served byte-identically from the embedded copy"
        );
        assert_eq!(
            reply.header("cache-control").as_deref(),
            Some(spa::cache_control(name)),
            "`{name}` must carry the Cache-Control its path implies"
        );
    }
}

/// The shell at the mount point, and the SPA fallback for everything else.
async fn assert_shell_and_fallback() {
    let shell = common::get(server::api::app(state()), "/").await;
    assert_eq!(
        shell.status,
        StatusCode::OK,
        "the mount point serves the SPA"
    );
    assert!(
        shell
            .header("content-type")
            .is_some_and(|value| value.starts_with("text/html")),
        "the shell is HTML; headers were {:?}",
        shell.headers
    );
    assert_eq!(
        shell.header("cache-control").as_deref(),
        Some("no-store"),
        "the shell must never be cached, or a redeploy serves a stale entry point"
    );

    let unknown = common::get(server::api::app(state()), "/not/an/asset/").await;
    assert_eq!(
        unknown.status,
        StatusCode::OK,
        "an unknown path must fall back to the shell with 200, not 404, so client-side routes load: {}",
        unknown.body
    );
    assert_eq!(
        unknown.body, shell.body,
        "the fallback must be the shell itself"
    );
}

/// When `apps/web/build/` exists, the embedded copy is that build; otherwise a loud notice.
fn assert_staged_copy_matches_a_real_build() {
    let on_disk = common::workspace_root()
        .join("apps")
        .join("web")
        .join("build")
        .join("index.html");
    match std::fs::read(&on_disk) {
        Ok(bytes) => {
            let embedded = spa::Assets::get(spa::SHELL).expect("the shell is embedded");
            assert_eq!(
                embedded.data.as_ref(),
                bytes.as_slice(),
                "the embedded shell is not the one `bun --filter web build` produced — build.rs \
                 staged something else"
            );
        }
        Err(_) => announce(
            "NOTE spa_assets_are_embedded: apps/web/build/ is absent, so the embedded tree is \
             build.rs's placeholder. The embedding mechanism is still fully asserted; only the \
             'staged copy equals the real build' comparison was skipped. Run `just build` to \
             exercise it.",
        ),
    }
}

/// A state with no devices — the SPA does not depend on the registry.
fn state() -> AppState {
    AppState::new(common::bus())
}

/// Announces a skipped comparison on the process's real stderr.
///
/// Not `eprintln!`: libtest captures that, so a notice from a passing test would be invisible
/// without `--nocapture`.
fn announce(message: &str) {
    use std::io::Write as _;
    let mut stderr = std::io::stderr();
    let _ = writeln!(stderr, "{message}");
    let _ = stderr.flush();
}
