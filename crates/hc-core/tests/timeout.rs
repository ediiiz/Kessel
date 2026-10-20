//! Steady-state request timeout. `Sender::send` bypasses the handshake correlator, so a
//! post-CONNECT request has no deadline of its own; the supervisor enforces one.
//!
//! Runs on tokio's paused clock: a never-answered request is a response that arrives far past the
//! deadline, so a missing timeout fails by assertion rather than by hanging.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::time::Duration;

use hc_core::supervisor::{SupervisorError, await_response};

#[tokio::test(start_paused = true)]
async fn steady_state_request_times_out_without_wedging_device() {
    let deadline = Duration::from_millis(hc_proto::RESPONSE_TIMEOUT_MS);

    // An hour away is never in practice. The paused clock auto-advances, so without a timeout the
    // late value comes through instead of hanging.
    let never_answered = async {
        tokio::time::sleep(Duration::from_hours(1)).await;
        42_i32
    };
    let result = await_response(never_answered, deadline).await;
    assert!(
        matches!(result, Err(SupervisorError::RequestTimeout(_))),
        "a request unanswered past its deadline must time out, not wait forever: {result:?}"
    );

    // Not wedged: the device task can still issue and complete the next request immediately.
    let prompt = await_response(async { 7_i32 }, deadline).await;
    assert_eq!(
        prompt.expect("the device task is not wedged by the earlier timeout"),
        7,
        "a promptly-answered request after a timeout still resolves"
    );
}
