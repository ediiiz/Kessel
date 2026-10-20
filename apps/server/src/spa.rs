//! Serving the `SvelteKit` SPA out of the binary, ingress-survivable.
//!
//! The bytes are embedded with `rust-embed` from `$OUT_DIR/spa/`, which `build.rs` stages from
//! `apps/web/build/`; the `embed-assets` feature keeps that true in debug builds too. Nothing is
//! read from disk at run time.
//!
//! The web build already emits hash routing and relative `./_app/…` URLs, so this module serves
//! the staged bytes verbatim, with no path rewriting.
//!
//! `_app/immutable/**` is content-hashed and cached for a year. `index.html` and
//! `_app/version.json` are never cached: `version.json` is how `SvelteKit` notices a new
//! deployment.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

/// The staged SPA, compiled into the binary. See `build.rs`.
#[derive(Debug, Embed)]
#[folder = "$OUT_DIR/spa"]
pub struct Assets;

/// Whether the assets are in the binary rather than read from disk at run time.
///
/// `false` would mean the `embed-assets` feature was switched off, which turns
/// `spa_assets_are_embedded` into a claim about a directory instead of about the binary.
pub const ASSETS_ARE_EMBEDDED: bool = cfg!(feature = "embed-assets");

/// The SPA shell every non-asset path resolves to.
pub const SHELL: &str = "index.html";

/// The header the Home Assistant Supervisor passes the (stripped) ingress prefix in.
pub const INGRESS_PATH_HEADER: &str = "x-ingress-path";

/// Every embedded asset path, sorted — the binary's manifest of itself.
#[must_use]
pub fn asset_names() -> Vec<String> {
    let mut names: Vec<String> = Assets::iter().map(|name| name.to_string()).collect();
    names.sort();
    names
}

/// `Cache-Control` for one asset path.
///
/// Content-hashed assets are immutable; the shell and the version marker never are.
#[must_use]
pub fn cache_control(path: &str) -> &'static str {
    if path.starts_with("_app/immutable/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    }
}

/// The `Location` a bare or non-directory path must be redirected to, or `None` to serve it.
///
/// The shell is served only from a path ending in `/`, because the SPA's relative asset URLs
/// resolve against the document's directory. A bare path is redirected to
/// `{X-Ingress-Path}{path}/`; the prefix comes from the header because the Supervisor strips it
/// before proxying.
///
/// Behind ingress the target is never `/`, which would land at the Home Assistant root. With no
/// header (loopback) the origin root is the mount point, so `/` is correct there. The value has no
/// fragment, so the browser keeps the `#/route` across the redirect.
#[must_use]
pub fn directory_redirect(path: &str, ingress_prefix: Option<&str>) -> Option<String> {
    if path.ends_with('/') {
        return None;
    }
    let prefix = ingress_prefix.unwrap_or_default().trim_end_matches('/');
    Some(format!("{prefix}{path}/"))
}

/// Serves one embedded asset, or `None` if the binary does not carry it.
#[must_use]
pub fn serve(path: &str) -> Option<Response> {
    let asset = Assets::get(path)?;
    let mime = asset.metadata.mimetype();
    let mut response = (StatusCode::OK, asset.data.into_owned()).into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(mime) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control(path)),
    );
    Some(response)
}

#[cfg(test)]
mod tests {
    use super::{
        ASSETS_ARE_EMBEDDED, Assets, SHELL, asset_names, cache_control, directory_redirect,
    };

    #[test]
    fn a_directory_path_is_never_redirected() {
        assert_eq!(
            directory_redirect("/", Some("/api/hassio_ingress/deadbeefdeadbeef")),
            None,
            "a path ending in `/` is already a directory URL; redirecting it would loop forever"
        );
        assert_eq!(directory_redirect("/deep/route/", None), None);
    }

    #[test]
    fn a_bare_path_redirects_inside_the_ingress_prefix_and_never_to_the_root() {
        let prefix = "/api/hassio_ingress/deadbeefdeadbeef";
        assert_eq!(
            directory_redirect("", Some(prefix)).as_deref(),
            Some("/api/hassio_ingress/deadbeefdeadbeef/"),
            "the redirect must reproduce the full prefix"
        );
        assert_ne!(
            directory_redirect("", Some(prefix)).as_deref(),
            Some("/"),
            "`Location: /` escapes the prefix and lands at the Home Assistant root"
        );
        // A prefix that already carries a trailing slash must not produce a doubled one.
        assert_eq!(
            directory_redirect("", Some("/api/hassio_ingress/tok/")).as_deref(),
            Some("/api/hassio_ingress/tok/")
        );
        assert_eq!(
            directory_redirect("/inspector", Some(prefix)).as_deref(),
            Some("/api/hassio_ingress/deadbeefdeadbeef/inspector/")
        );
    }

    #[test]
    fn on_loopback_the_origin_root_is_the_mount_point() {
        assert_eq!(
            directory_redirect("", None).as_deref(),
            Some("/"),
            "with no ingress prefix `/` is the only correct target, not a prefix escape"
        );
    }

    #[test]
    fn the_shell_is_embedded() {
        #[expect(
            clippy::assertions_on_constants,
            reason = "asserting the compile-time flag turns a switched-off `embed-assets` \
                      feature into a failing test"
        )]
        {
            assert!(
                ASSETS_ARE_EMBEDDED,
                "the `embed-assets` feature is off, so assets would be read from disk at run time"
            );
        }
        assert!(
            Assets::get(SHELL).is_some(),
            "the binary must carry {SHELL}; embedded assets are {:?}",
            asset_names()
        );
    }

    #[test]
    fn only_content_hashed_assets_are_cacheable() {
        assert_eq!(
            cache_control("_app/immutable/entry/start.abc123.js"),
            "public, max-age=31536000, immutable",
            "content-hashed assets are immutable"
        );
        for volatile in [SHELL, "_app/version.json"] {
            assert_eq!(
                cache_control(volatile),
                "no-store",
                "{volatile} must never be cached — version.json is how SvelteKit notices a new \
                 deployment"
            );
        }
    }
}
