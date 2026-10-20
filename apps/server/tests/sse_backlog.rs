//! `a_replayed_frame_says_so`: the retained backlog is labelled as history.
//!
//! `crate::sse::frames` replays `TraceBus`'s ring buffer before the live feed. The inspector does
//! not care, but a page that reacts to events would otherwise act on every connection transition
//! since startup (for example, scheduling a needless resync on every mount), so each frame says
//! whether it is replayed.
//!
//! Touches no device: a bus and three synthetic records.

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

use futures_util::StreamExt as _;
use hc_core::trace::{TraceLevel, TraceRecord};

/// One connection transition, the kind `hc_core::supervisor::emit_state` emits.
fn transition(state: &str) -> TraceRecord {
    TraceRecord::new(TraceLevel::Info, "hc_core::supervisor", "connection state")
        .with_field("device", "dishwasher")
        .with_field("state", state)
}

#[tokio::test]
async fn a_replayed_frame_says_so() {
    let bus = common::bus();

    // Two transitions already in the ring buffer when the browser subscribes: history.
    bus.emit(transition("connecting"));
    bus.emit(transition("connected"));

    let mut frames = Box::pin(server::sse::frames(&bus));

    for expected in ["connecting", "connected"] {
        let frame = frames.next().await.expect("the backlog is replayed first");
        assert_eq!(
            frame.fields.get("state").map(String::as_str),
            Some(expected)
        );
        assert!(
            frame.replayed,
            "a frame from the retained buffer is history, and a reader that acts on events has to \
             be able to tell: {frame:?}"
        );
    }

    // And one that happens while the browser is watching: not history.
    bus.emit(transition("reconnecting"));
    let frame = frames
        .next()
        .await
        .expect("the live feed follows the backlog");
    assert_eq!(
        frame.fields.get("state").map(String::as_str),
        Some("reconnecting")
    );
    assert!(
        !frame.replayed,
        "an event emitted after the subscription is live, and is the one worth reacting to: {frame:?}"
    );
}
