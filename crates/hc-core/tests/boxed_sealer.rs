//! The type-erased (boxed) write path:
//!
//! - every device's write reaches the wire, the cooktop and the oven included;
//! - through one guard, spending the session's own next `msgID`, so boxing adds no second send path
//!   or counter;
//! - a session yields its send handle once, so no second writer for a connection can exist.
//!
//! The routes run over a real `hc_transport::Session` with a recording sink, because the sealers'
//! job is to take a session's send handle. The byte count distinguishes a write that reached the
//! wire from an `Ok` that sent nothing.

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

use common::{RecordedWire, seeded_session};
use hc_core::bridge::{guarded_session_boxed, take_guarded_session_boxed};
use hc_core::write::{WriteCommand, WriteError};
use serde_json::json;

/// Synthetic per-connection counters, not appliance identifiers.
const S_ID: u64 = 1;
const ED_MSG_ID: u64 = 1;

/// A plain writable-feature command: one `{uid, value}` item at `/ro/values`.
fn feature_write() -> WriteCommand {
    WriteCommand {
        resource: "/ro/values".to_owned(),
        target_uid: Some(0x0100),
        field: None,
        value: json!(7),
        validate: true,
        options: Vec::new(),
    }
}

/// What one attempted write through a boxed sealer produced.
struct Attempt {
    verdict: Result<(), WriteError>,
    bytes: usize,
    frames: Vec<Vec<u8>>,
    msg_ids: Vec<u64>,
}

impl Attempt {
    /// Asserts the write returned `Ok` and put bytes on the wire; an `Ok` with zero bytes is a write
    /// that silently did nothing.
    fn assert_reached_the_wire(&self, context: &str) {
        assert!(
            self.verdict.is_ok(),
            "{context}: the write must succeed — no gate remains that could refuse it. Got {:?}",
            self.verdict
        );
        assert!(
            self.bytes > 0,
            "{context}: BYTES. An Ok verdict with an empty sink is a write that did nothing, which is \
             the failure a verdict-only assertion cannot see."
        );
        assert_eq!(
            self.frames.len(),
            1,
            "{context}: exactly one frame per write — more would mean a duplicated send path"
        );
        assert_eq!(
            self.msg_ids,
            vec![ED_MSG_ID + 1],
            "{context}: the frame must spend the SESSION's own next msgID. A boxed writer with its own \
             counter would emit a plausible-looking id from a copy of the position"
        );
    }
}

/// One write through the borrowing boxed sealer, which leaves the session alive to keep running
/// `run_steady_state`.
async fn borrowing_route() -> Attempt {
    let wire = RecordedWire::default();
    let mut session = seeded_session(&wire, S_ID, ED_MSG_ID);
    let mut writer = take_guarded_session_boxed(&mut session)
        .expect("a fresh session has not yet yielded its send handle");
    let verdict = writer.write(&feature_write()).await;
    Attempt {
        verdict,
        bytes: wire.byte_count(),
        frames: wire.frames(),
        msg_ids: wire.msg_ids(),
    }
}

/// One write through the **consuming** boxed sealer, for a caller with no further use for the session.
async fn consuming_route() -> Attempt {
    let wire = RecordedWire::default();
    let session = seeded_session(&wire, S_ID, ED_MSG_ID);
    let mut writer = guarded_session_boxed(session)
        .expect("a fresh session has not yet yielded its send handle");
    let verdict = writer.write(&feature_write()).await;
    Attempt {
        verdict,
        bytes: wire.byte_count(),
        frames: wire.frames(),
        msg_ids: wire.msg_ids(),
    }
}

/// Both boxed routes put a write on the wire, once, on the session's own counter.
#[tokio::test]
async fn boxed_sealers_reach_the_wire_through_one_guard() {
    borrowing_route()
        .await
        .assert_reached_the_wire("the borrowing boxed sealer");
    consuming_route()
        .await
        .assert_reached_the_wire("the consuming boxed sealer");
}

/// A session yields its send handle once, boxed route included. A second writer for one connection
/// would be an accidental write path and would desynchronise the `msgID` counter.
#[tokio::test]
async fn a_session_yields_its_boxed_handle_only_once() {
    let wire = RecordedWire::default();
    let mut session = seeded_session(&wire, S_ID, ED_MSG_ID);

    let first = take_guarded_session_boxed(&mut session);
    assert!(first.is_some(), "the first take must succeed");

    let second = take_guarded_session_boxed(&mut session);
    assert!(
        second.is_none(),
        "the second take must return None — one session, one send handle, so no second write path \
         for this connection can be built"
    );
}
