//! A bare path redirects using the ingress prefix.
//!
//! Under Home Assistant ingress the Supervisor strips the prefix before proxying and passes it in
//! `X-Ingress-Path`, so this handler sees `""` or `/`. The SPA's asset URLs (hash routing's runtime
//! `base` and `relativize-index.js`'s `./_app/…`) resolve against the document's directory, so the
//! shell must be served at a path ending in `/` or the SPA does not boot.
//!
//! Hence a bare or empty path is a 308 to `{X-Ingress-Path}/`. Never `Location: /`, which escapes
//! the prefix to the Home Assistant root. A relative `Location: ./` is not safe either: the browser
//! resolves it against `…/<token>`, dropping the token.
//!
//! The Location carries no fragment, so the browser re-applies the `#/route` it preserved across
//! the redirect and the hash router reads it.
//!
//! These tests cover the status, the `Location` with and without `X-Ingress-Path`, the absence of a
//! fragment, and that a path already ending in `/` is not redirected. They do not show that a real
//! Supervisor forwards a bare token path as an empty request path rather than as `/`.

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

use axum::body::Body;
use axum::http::{Request, StatusCode, Uri};
use server::state::AppState;

/// A sample ingress prefix. Not a real token and not a secret.
const INGRESS_PREFIX: &str = "/api/hassio_ingress/deadbeefdeadbeef";

/// A `GET` for `path` (which may be empty), optionally carrying the ingress prefix header.
fn get(path: &str, ingress: Option<&str>) -> Request<Body> {
    let uri = Uri::builder()
        .path_and_query(path)
        .build()
        .expect("a well-formed path");
    let mut builder = Request::builder().uri(uri);
    if let Some(prefix) = ingress {
        builder = builder.header(server::spa::INGRESS_PATH_HEADER, prefix);
    }
    builder.body(Body::empty()).expect("a well-formed request")
}

/// Sends `path` (optionally behind the ingress prefix) and returns the status and `Location`.
async fn probe(path: &str, ingress: Option<&str>) -> (StatusCode, Option<String>) {
    let (status, headers) = common::send_headers_only(
        server::api::app(AppState::new(common::bus())),
        get(path, ingress),
    )
    .await;
    let location = headers
        .get("location")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    (status, location)
}

#[tokio::test]
async fn bare_path_redirects_using_the_ingress_prefix() {
    // 1. The hot path: a bare token path behind ingress arrives with an empty request path.
    let (status, location) = probe("", Some(INGRESS_PREFIX)).await;
    assert_eq!(
        status,
        StatusCode::PERMANENT_REDIRECT,
        "a bare path must 308 so the shell is served from a directory URL"
    );
    let location = location.expect("a redirect carries a Location");
    assert_eq!(
        location,
        format!("{INGRESS_PREFIX}/"),
        "the Location must reproduce the full ingress prefix"
    );
    assert_ne!(
        location, "/",
        "`Location: /` escapes the ingress prefix and lands at the Home Assistant root — a blank \
         page"
    );
    assert!(
        !location.contains('#'),
        "the Location must carry no fragment, so the browser re-applies the `#/route` it preserved"
    );

    // 2. A path that does not end in `/` gets the same treatment, prefix included.
    let (status, location) = probe("/inspector", Some(INGRESS_PREFIX)).await;
    assert_eq!(
        status,
        StatusCode::PERMANENT_REDIRECT,
        "the shell is served only from a path ending in `/`"
    );
    assert_eq!(
        location.as_deref(),
        Some(format!("{INGRESS_PREFIX}/inspector/").as_str()),
        "the redirect keeps the requested path and stays inside the prefix"
    );

    // 3. On loopback there is no header, the mount point *is* the origin root, and `/` is both the
    // only correct target and not a prefix escape.
    let (status, location) = probe("", None).await;
    assert_eq!(
        status,
        StatusCode::PERMANENT_REDIRECT,
        "an empty path is still not a directory URL on loopback"
    );
    assert_eq!(
        location.as_deref(),
        Some("/"),
        "with no ingress prefix the origin root is the mount point"
    );

    // 4. A path that already ends in `/` is served, not redirected: a 308 here would loop forever.
    let (status, _location) = probe("/", Some(INGRESS_PREFIX)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a directory URL is served, not redirected — a 308 here would be an infinite loop"
    );

    // 5. An asset is served whatever its path looks like: it is a file, not a directory.
    let (status, _location) = probe("/_app/version.json", Some(INGRESS_PREFIX)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an embedded asset must never be redirected — the SPA fetches it by exact path"
    );
}
