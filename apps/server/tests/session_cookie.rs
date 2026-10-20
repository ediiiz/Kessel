//! The SPA authenticates with a session cookie instead of carrying the operator token.
//!
//! `Origin` and `Sec-Fetch-Site` cannot be forged by a web page but can by any other client, and the
//! daemon binds `0.0.0.0` with writes that reach a cooktop and an oven, so the gate cannot key on
//! them. Instead `POST /api/session` exchanges the token for a `HttpOnly; SameSite=Strict` cookie,
//! and the gate accepts either that cookie or the `X-Kessel-Token` header. External API clients keep
//! the header.
//!
//! This is stricter than holding the token in the page: nothing in the page can read an `HttpOnly`
//! cookie, and a session is revocable (`DELETE /api/session`) and dies with the process.
//! `SameSite=Strict` provides the CSRF protection the custom header did (see `src/auth.rs`).
//!
//! Touches no device: every test drives the router in-process through `tower`'s `oneshot`.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use server::auth::{ApiToken, SESSION_COOKIE, TOKEN_HEADER};
use server::state::AppState;

mod common;

/// The token these tests authorise the exchange with.
const TEST_TOKEN: &str = "test-token-not-a-real-one";

fn state() -> AppState {
    AppState::new(common::bus()).with_token(ApiToken::from_string(TEST_TOKEN))
}

/// `PUT /api/config` with a body the config route accepts — the mutating request under test.
///
/// It needs no onboarded device and no live session, so a failure here is about the gate alone.
fn mutating_request() -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri("/api/config")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .expect("a well-formed request")
}

/// Exchanges the token for a session, returning the cookie value the daemon set.
async fn open_session(state: &AppState) -> String {
    let request = Request::builder()
        .method("POST")
        .uri("/api/session")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::empty())
        .expect("a well-formed request");
    let reply = common::send(server::api::app(state.clone()), request).await;
    assert_eq!(
        reply.status,
        StatusCode::NO_CONTENT,
        "the exchange succeeds when the token is right: {}",
        reply.body
    );
    let cookie = reply
        .header("set-cookie")
        .expect("the exchange sets a session cookie");
    cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_owned())
        .expect("the cookie carries a value")
}

/// A mutating request authenticated by the cookie alone, with no token header.
#[tokio::test]
async fn a_session_cookie_authorises_a_mutating_request() {
    let state = state();
    let session = open_session(&state).await;

    let request = {
        let (mut parts, body) = mutating_request().into_parts();
        parts.headers.insert(
            "cookie",
            format!("{SESSION_COOKIE}={session}")
                .parse()
                .expect("a valid header value"),
        );
        Request::from_parts(parts, body)
    };
    let reply = common::send(server::api::app(state), request).await;

    assert_eq!(
        reply.status,
        StatusCode::OK,
        "a valid session cookie must authorise a mutating request without the token header: {}",
        reply.body
    );
}

/// The cookie attributes are asserted individually, so dropping one cannot pass as a formatting
/// change.
#[tokio::test]
async fn the_session_cookie_is_httponly_and_samesite_strict() {
    let state = state();
    let request = Request::builder()
        .method("POST")
        .uri("/api/session")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::empty())
        .expect("a well-formed request");

    let reply = common::send(server::api::app(state), request).await;
    let cookie = reply.header("set-cookie").expect("a cookie is set");

    assert!(
        cookie.contains("HttpOnly"),
        "without HttpOnly any script in the origin can read the session: {cookie}"
    );
    assert!(
        cookie.contains("SameSite=Strict"),
        "SameSite=Strict is what replaces the custom header's CSRF property (auth.rs module docs): {cookie}"
    );
    assert!(
        cookie.contains("Path=/"),
        "the cookie must cover the whole app, ingress prefix included: {cookie}"
    );
    assert!(
        !cookie.contains(TEST_TOKEN),
        "the operator token must never appear in a cookie; only a session secret does"
    );
}

/// The refusal that matters: a guessed or stale cookie is not a credential.
#[tokio::test]
async fn an_unknown_session_cookie_is_refused() {
    let state = state();
    // A well-formed value of the right shape — 64 hex characters — that was never issued.
    let forged = "f".repeat(64);

    let request = {
        let (mut parts, body) = mutating_request().into_parts();
        parts.headers.insert(
            "cookie",
            format!("{SESSION_COOKIE}={forged}")
                .parse()
                .expect("a valid header value"),
        );
        Request::from_parts(parts, body)
    };
    let reply = common::send(server::api::app(state), request).await;

    assert_eq!(
        reply.status,
        StatusCode::UNAUTHORIZED,
        "a session that was never issued must not authorise anything: {}",
        reply.body
    );
}

/// No session, no cookie, no header — still refused. The gate did not become optional.
#[tokio::test]
async fn a_mutating_request_with_no_credential_is_still_refused() {
    let reply = common::send(server::api::app(state()), mutating_request()).await;

    assert_eq!(
        reply.status,
        StatusCode::UNAUTHORIZED,
        "the gate must still refuse an unauthenticated mutation: {}",
        reply.body
    );
}

/// The header path is untouched: an external API client keeps working.
#[tokio::test]
async fn the_token_header_still_authorises_on_its_own() {
    let request = {
        let (mut parts, body) = mutating_request().into_parts();
        parts.headers.insert(
            TOKEN_HEADER,
            TEST_TOKEN.parse().expect("a valid header value"),
        );
        Request::from_parts(parts, body)
    };
    let reply = common::send(server::api::app(state()), request).await;

    assert_eq!(
        reply.status,
        StatusCode::OK,
        "the token header must keep working with no session involved: {}",
        reply.body
    );
}

/// A session cannot be minted without the token; otherwise the cookie would be an authentication
/// bypass.
#[tokio::test]
async fn a_session_cannot_be_opened_without_the_token() {
    let request = Request::builder()
        .method("POST")
        .uri("/api/session")
        .body(Body::empty())
        .expect("a well-formed request");

    let reply = common::send(server::api::app(state()), request).await;

    assert_eq!(
        reply.status,
        StatusCode::UNAUTHORIZED,
        "the exchange is a mutating route and must present the operator token: {}",
        reply.body
    );
    assert!(
        reply.header("set-cookie").is_none(),
        "a refused exchange must not set a cookie"
    );
}

/// `DELETE` ends the session, and the cookie stops working immediately afterwards.
#[tokio::test]
async fn closing_a_session_revokes_it() {
    let state = state();
    let session = open_session(&state).await;
    let cookie_header = format!("{SESSION_COOKIE}={session}");

    let close = Request::builder()
        .method("DELETE")
        .uri("/api/session")
        .header("cookie", &cookie_header)
        .body(Body::empty())
        .expect("a well-formed request");
    let reply = common::send(server::api::app(state.clone()), close).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT, "logout succeeds");
    assert!(
        reply
            .header("set-cookie")
            .is_some_and(|cookie| cookie.contains("Max-Age=0")),
        "logout must also clear the cookie in the browser"
    );

    let after = {
        let (mut parts, body) = mutating_request().into_parts();
        parts.headers.insert(
            "cookie",
            cookie_header.parse().expect("a valid header value"),
        );
        Request::from_parts(parts, body)
    };
    let reply = common::send(server::api::app(state), after).await;
    assert_eq!(
        reply.status,
        StatusCode::UNAUTHORIZED,
        "a revoked session must not authorise anything: {}",
        reply.body
    );
}

/// `GET /api/session` is what the SPA asks on load to decide whether to show the token field.
#[tokio::test]
async fn the_session_route_reports_whether_this_caller_is_authenticated() {
    let state = state();

    let anonymous = Request::builder()
        .uri("/api/session")
        .body(Body::empty())
        .expect("a well-formed request");
    let reply = common::send(server::api::app(state.clone()), anonymous).await;
    assert_eq!(
        reply.json(),
        serde_json::json!({ "authenticated": false, "auth_required": true }),
        "no cookie means not authenticated"
    );

    let session = open_session(&state).await;
    let held = Request::builder()
        .uri("/api/session")
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .body(Body::empty())
        .expect("a well-formed request");
    let reply = common::send(server::api::app(state), held).await;
    assert_eq!(
        reply.json(),
        serde_json::json!({ "authenticated": true, "auth_required": true }),
        "a live cookie reports authenticated"
    );
}
