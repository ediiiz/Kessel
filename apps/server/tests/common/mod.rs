//! Shared harness for the `apps/server` integration tests.
//!
//! Tests drive the real router in-process through `tower`'s `oneshot`, so nothing here opens a
//! socket. The one test that needs a real listener (`loopback_bind.rs`) binds port 0 and does not
//! use this module.

#![allow(
    dead_code,
    reason = "one harness serves several test binaries; each uses a subset, and Cargo compiles \
              this module separately into every one of them"
)]
#![allow(
    clippy::redundant_pub_crate,
    reason = "`redundant_pub_crate` (nursery) wants `pub` here while rustc's `unreachable_pub` \
              wants `pub(crate)`; `pub(crate)` is the honest visibility for a test harness"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use hc_core::trace::TraceBus;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

/// The workspace root, from `apps/server`.
pub(crate) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// The scrubbed hob profile fixture: synthetic identifiers under a synthetic psk/iv.
pub(crate) fn hob_zip_path() -> PathBuf {
    workspace_root()
        .join("crates")
        .join("hc-profile")
        .join("tests")
        .join("fixtures")
        .join("hob-aes.zip")
}

/// Its bytes, as the onboarding route receives them.
pub(crate) fn hob_zip_bytes() -> Vec<u8> {
    std::fs::read(hob_zip_path()).expect("the scrubbed hob profile fixture is readable")
}

/// One of the four scrubbed profile fixtures, by member name: `"hob-aes.zip"`, `"oven-tls.zip"`,
/// `"dishwasher-tls.zip"`, `"fridge-tls.zip"`.
///
/// All carry synthetic identifiers under a synthetic psk/iv, so a test that reads them touches no
/// real key material and no real appliance.
pub(crate) fn fixture_zip_bytes(name: &str) -> Vec<u8> {
    let path = workspace_root()
        .join("crates")
        .join("hc-profile")
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read(&path)
        .unwrap_or_else(|error| panic!("the scrubbed fixture {name} is readable: {error}"))
}

/// The `haId` a scrubbed fixture's manifest declares — a **synthetic** identifier.
///
/// Read out of the archive so it stays correct if the fixtures are re-scrubbed. Used only to assert
/// that a response body does **not** contain it: appliance identifiers are never exposed.
pub(crate) fn fixture_ha_id(name: &str) -> String {
    use std::io::Read as _;

    let bytes = fixture_zip_bytes(name);
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("the fixture is a ZIP");
    let manifest_name = archive
        .file_names()
        .find(|member| member.to_ascii_lowercase().ends_with(".json"))
        .expect("the fixture has a JSON manifest")
        .to_owned();
    let mut body = Vec::new();
    archive
        .by_name(&manifest_name)
        .expect("the manifest member is readable")
        .read_to_end(&mut body)
        .expect("the manifest member inflates");
    let manifest: serde_json::Value =
        serde_json::from_slice(&body).expect("the manifest is valid JSON");
    manifest
        .get("haId")
        .and_then(serde_json::Value::as_str)
        .expect("the manifest declares a haId")
        .to_owned()
}

/// A trace bus with the production ring-buffer capacity.
pub(crate) fn bus() -> Arc<TraceBus> {
    Arc::new(TraceBus::new(server::observability::TRACE_BUFFER_CAPACITY))
}

/// A response, flattened for assertions.
pub(crate) struct Reply {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    /// The body as text, lossily. Convenient for JSON and prose; **wrong for a binary asset.**
    ///
    /// `from_utf8_lossy` replaces every byte that is not valid UTF-8 with U+FFFD, so a `.woff2` read
    /// through here comes back full of `EF BF BD` and compares equal to nothing. Use [`bytes`] when
    /// the assertion is about bytes. This cost nothing until the SPA gained its first binary asset,
    /// at which point `spa_embed.rs` started failing against a perfectly correct server.
    pub(crate) body: String,
    /// The body exactly as it arrived.
    pub(crate) bytes: Vec<u8>,
}

impl Reply {
    /// Parses the body as JSON.
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("body is not JSON ({e}): {}", self.body))
    }

    /// One header as a string, if present.
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    }
}

/// Sends one request through the router and collects the whole response.
pub(crate) async fn send(app: axum::Router, request: Request<Body>) -> Reply {
    let response = app
        .oneshot(request)
        .await
        .expect("the router is infallible by construction");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("the response body is fully readable")
        .to_bytes();
    Reply {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
        bytes: bytes.to_vec(),
    }
}

/// Sends a request and returns the status and headers **without touching the body**.
///
/// The SSE route's body never ends, so collecting it would hang. Headers arrive before the first
/// event, which is exactly what the no-buffering assertion needs.
pub(crate) async fn send_headers_only(
    app: axum::Router,
    request: Request<Body>,
) -> (StatusCode, HeaderMap) {
    let response = app
        .oneshot(request)
        .await
        .expect("the router is infallible by construction");
    (response.status(), response.headers().clone())
}

/// Reads an SSE response body until `min_events` `event:`/`data:` records have arrived or `timeout`
/// elapses, whichever comes first.
///
/// Bounded because an SSE body never ends, so an unbounded read would hang instead of failing. The
/// collected text is returned either way, so the assertion decides what "not enough" means.
pub(crate) async fn sse_body(
    app: axum::Router,
    uri: &str,
    min_records: usize,
    timeout: std::time::Duration,
) -> (StatusCode, HeaderMap, String) {
    use futures_util::StreamExt as _;

    let request = Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("a well-formed request");
    let response = app
        .oneshot(request)
        .await
        .expect("the router is infallible by construction");
    let status = response.status();
    let headers = response.headers().clone();

    let mut stream = response.into_body().into_data_stream();
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + timeout;
    while text.matches("data:").count() < min_records {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(chunk))) => text.push_str(&String::from_utf8_lossy(&chunk)),
            // End of stream, a body error, or the deadline: stop and let the assertions speak.
            Ok(Some(Err(_)) | None) | Err(_) => break,
        }
    }
    (status, headers, text)
}

/// A `GET` with no headers — the shape a read request from the SPA has.
pub(crate) async fn get(app: axum::Router, uri: &str) -> Reply {
    let request = Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("a well-formed request");
    send(app, request).await
}
