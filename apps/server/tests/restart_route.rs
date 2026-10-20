//! `POST /api/restart`.
//!
//! Every setting is `restart_required: true`, and the Supervisor restarts an add-on that asks
//! (`POST /addons/self/restart`), so the button belongs beside the message that says it is needed.
//!
//! The route is tested: that it is gated like every other mutation, that it answers before it acts,
//! and that it reports which mechanism it will use. The mechanism itself (killing the process or
//! asking a Supervisor) is not, so the decision is a pure function this file drives.
//!
//! Touches no device: `hassio::restart_self` is never reached because `SUPERVISOR_TOKEN` is unset.

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
use axum::http::{Request, StatusCode};
use server::auth::ApiToken;
use server::state::AppState;

const TOKEN: &str = "test-token-for-the-restart-route";

fn restart_request(token: Option<&str>) -> Request<Body> {
    let builder = Request::builder().method("POST").uri("/api/restart");
    let builder = match token {
        Some(token) => builder.header(server::auth::TOKEN_HEADER, token),
        None => builder,
    };
    builder.body(Body::empty()).expect("a well-formed request")
}

/// The restart is a mutation: ungated, anything that can reach the port could bounce the daemon and
/// every appliance connection it holds.
#[tokio::test]
async fn restarting_requires_the_token() {
    let state = AppState::new(common::bus()).with_token(ApiToken::from_string(TOKEN));

    let refused = common::send(server::api::app(state), restart_request(None)).await;

    assert_eq!(
        refused.status,
        StatusCode::UNAUTHORIZED,
        "an unauthenticated restart must be refused: {}",
        refused.body
    );
}

/// The answer arrives before the restart and says which mechanism is about to be used.
///
/// 202, not 200: the work has been accepted and has not happened yet.
#[tokio::test]
async fn a_restart_is_accepted_and_names_its_mechanism() {
    let state = AppState::new(common::bus()).with_token(ApiToken::from_string(TOKEN));

    let accepted = common::send(server::api::app(state), restart_request(Some(TOKEN))).await;

    assert_eq!(
        accepted.status,
        StatusCode::ACCEPTED,
        "the route must answer before it acts: {}",
        accepted.body
    );
    assert!(
        accepted.body.contains("\"mechanism\":\"process_exit\""),
        "with no SUPERVISOR_TOKEN this is not an add-on, so the only mechanism left is exiting — and \
         the UI has to say so, because outside the add-on nothing may restart the process: {}",
        accepted.body
    );
}

/// The mechanism depends only on whether there is a Supervisor to ask.
#[test]
fn the_mechanism_follows_the_supervisor_token() {
    use server::api::RestartMechanism;

    assert_eq!(
        server::api::restart_mechanism(|_| None),
        RestartMechanism::ProcessExit,
        "outside an add-on there is no Supervisor to ask"
    );
    assert_eq!(
        server::api::restart_mechanism(|key| {
            (key == server::hassio::TOKEN_ENV).then(|| "supervisor-token".to_owned())
        }),
        RestartMechanism::Supervisor,
        "under the add-on the Supervisor restarts the container properly, rather than the process \
         exiting and hoping something notices"
    );
}
