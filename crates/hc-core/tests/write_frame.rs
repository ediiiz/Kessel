//! A [`WriteCommand`] reaching the wire as a byte-exact frame: `POST /ro/values` for feature
//! writes, `/ro/selectedProgram` for program starts, and a refusal for anything else.
//!
//! Every assertion is on bytes, not on a parsed value. Appliance firmware parses JSON objects
//! positionally (`uid` must precede `value`), and `serde_json::Map`'s `PartialEq` ignores key order
//! even with `preserve_order`. The sink is a `Vec`; no appliance is contacted.

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

use common::{RecordedWire, recording_write_path, seeded_session};
use hc_core::bridge::{WriteFrameError, take_guarded_session, to_outgoing};
use hc_core::write::{DeviceRole, StartRequest, WriteCommand, start_command};
use hc_proto::Version;
use serde_json::json;

/// The `sID` and first write `msgID` this file writes under.
const S_ID: u64 = 1;
const MSG_ID: u64 = 2;

/// The appliance `edMsgID` that leaves the allocator handing out [`MSG_ID`] next: the seeded
/// handshake's `GET /ci/services` spends `edMsgID` itself, so the first write is one past it.
const ED_MSG_ID: u64 = MSG_ID - 1;

/// The expected `/ro/values` frame, byte for byte.
const EXPECTED_FRAME: &str = concat!(
    r#"{"sID":1,"msgID":2,"resource":"/ro/values","version":1,"action":"POST","#,
    r#""data":[{"uid":256,"value":7}]}"#
);

/// A captured frame as text, for an order-sensitive comparison.
fn frame_text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("a write frame is UTF-8 JSON")
}

/// A generic feature write — the shape `build_write_command` produces for an incoming `…/set`.
fn feature_write() -> WriteCommand {
    WriteCommand {
        resource: "/ro/values".to_owned(),
        target_uid: Some(256),
        field: None,
        value: json!(7),
        validate: true,
        options: Vec::new(),
    }
}

#[tokio::test]
async fn write_command_serialises_through_to_outgoing() {
    // 1. The frame builder itself.
    let message = to_outgoing(&feature_write(), S_ID, MSG_ID, Version::Int(1))
        .expect("a /ro/values feature write is representable");
    let bytes = message.to_wire_bytes().expect("the frame serialises");
    assert_eq!(
        frame_text(&bytes),
        EXPECTED_FRAME,
        "the write frame must be byte-exact: field order, compact separators, and `uid` before \
         `value` inside the item (message-model.md §5.5)"
    );

    // 2. The same frame through the whole write path: guard, adapter, real `hc_transport::Sender`,
    //    sink.
    let (mut writer, wire) = recording_write_path(S_ID, ED_MSG_ID);
    writer
        .write(&feature_write())
        .await
        .expect("no gate remains that could refuse a /ro/values write");
    let frames = wire.frames();
    assert_eq!(frames.len(), 1, "exactly one frame reaches the wire");
    assert_eq!(
        frame_text(frames.first().expect("the one frame")),
        EXPECTED_FRAME,
        "the bytes at the sink are the bytes asserted above — the adapter adds nothing"
    );
}

/// The cooktop program start `start_command` produces, a `/ro/selectedProgram` write. The frame
/// layer's behaviour is independent of the role.
fn program_start() -> WriteCommand {
    start_command(
        &DeviceRole::Cooktop,
        &StartRequest {
            program_uid: 0x1234,
            start_option_uid: 0,
            relative_time: 0,
        },
    )
    .expect("the program-start command is constructible")
}

/// A program start reaches the wire as a program body (`program` before `options`), through
/// `to_outgoing` and through a real `Sender`. This is the write that can turn on a cooktop or oven.
#[tokio::test]
async fn a_program_start_reaches_the_wire() {
    // 1. Through `to_outgoing`: the frame builds, and it is the program body rather than a retargeted
    //    feature write.
    let frame = to_outgoing(&program_start(), S_ID, MSG_ID, Version::Int(1))
        .expect("a /ro/selectedProgram command builds a program-start frame");
    let wire = String::from_utf8(frame.to_wire_bytes().expect("the frame serialises"))
        .expect("utf-8 wire bytes");
    assert!(
        wire.contains(r#""resource":"/ro/selectedProgram""#),
        "the resource must be preserved, never retargeted to /ro/values: {wire}"
    );
    assert!(
        wire.contains(r#""data":[{"program":4660,"options":"#),
        "the body must be the program shape with `program` first (as the Python references \
         send it; Go's inverse order is an encoding/json map-sort artefact): {wire}"
    );

    // 2. Through the real `Sender` path, so the bytes production emits are the bytes asserted above.
    let (mut writer, sink) = recording_write_path(S_ID, ED_MSG_ID);
    writer
        .write(&program_start())
        .await
        .expect("nothing refuses a program start any more");
    let frames = sink.frames();
    assert_eq!(frames.len(), 1, "exactly one frame reached the sink");
    let emitted = String::from_utf8(frames.first().expect("one frame").clone()).expect("utf-8");
    assert!(
        emitted.contains(r#""data":[{"program":4660,"options":[]}]"#),
        "the sink's bytes carry the program body, options first-class and empty here: {emitted}"
    );
    assert_eq!(
        sink.msg_ids(),
        vec![ED_MSG_ID + 1],
        "and it spends the session's own next msgID, like any other write"
    );
}

/// A resource with no known write frame is refused.
///
/// `/ro/allMandatoryValues` is a real resource, but only ever read with `GET`, so it is the
/// realistic mistake: a resource seen on the wire reached for as a write target. Widening the set
/// of writable resources should require moving this needle deliberately.
#[tokio::test]
async fn an_unevidenced_resource_is_still_refused() {
    let command = WriteCommand {
        resource: "/ro/allMandatoryValues".to_owned(),
        target_uid: Some(1),
        field: None,
        value: serde_json::json!(1),
        validate: true,
        options: Vec::new(),
    };
    let refused = to_outgoing(&command, S_ID, MSG_ID, Version::Int(1));
    assert!(
        matches!(refused, Err(WriteFrameError::UnsupportedResource(ref resource))
            if resource == "/ro/allMandatoryValues"),
        "default-deny survives: this crate has no attested WRITE frame for /ro/allMandatoryValues, \
         and guessing a frame shape is not an honest output: {refused:?}"
    );

    let (mut writer, sink) = recording_write_path(S_ID, ED_MSG_ID);
    let result = writer.write(&command).await;
    assert!(
        result.is_err(),
        "and it fails on the real path too: {result:?}"
    );
    assert_eq!(
        sink.byte_count(),
        0,
        "ZERO bytes for a resource with no evidenced frame"
    );
}

/// The `/ro/values` frame is byte-identical through every construction route:
///
/// 1. `to_outgoing` directly, the frame builder;
/// 2. `hc_core::bridge::guarded_session`, which consumes the session (`recording_write_path`);
/// 3. `hc_core::bridge::take_guarded_session`, which borrows it.
///
/// Asserted on bytes, because key order matters to the firmware and a `Value` comparison ignores it.
#[tokio::test]
async fn write_frames_still_serialise_byte_exactly() {
    // (1) The builder.
    let direct = to_outgoing(&feature_write(), S_ID, MSG_ID, Version::Int(1))
        .expect("a /ro/values feature write is representable")
        .to_wire_bytes()
        .expect("the frame serialises");
    assert_eq!(
        frame_text(&direct),
        EXPECTED_FRAME,
        "to_outgoing's bytes moved: field order, compact separators, or `uid` before `value` inside \
         the item (message-model.md §5.5)"
    );

    // (2) The consuming route.
    let (mut consuming, consuming_wire) = recording_write_path(S_ID, ED_MSG_ID);
    consuming
        .write(&feature_write())
        .await
        .expect("no gate remains that could refuse a /ro/values write");
    assert_eq!(
        consuming_wire
            .frames()
            .iter()
            .map(|bytes| frame_text(bytes))
            .collect::<Vec<_>>(),
        vec![EXPECTED_FRAME.to_owned()],
        "the consuming route (guarded_session) must put the expected bytes on the wire"
    );

    // (3) The borrowing, take-once route, used by a long-lived per-device writer.
    let borrowing_wire = RecordedWire::default();
    let mut session = seeded_session(&borrowing_wire, S_ID, ED_MSG_ID);
    let mut borrowing =
        take_guarded_session(&mut session).expect("a fresh session yields its one send handle");
    borrowing
        .write(&feature_write())
        .await
        .expect("no gate remains that could refuse a /ro/values write");
    assert_eq!(
        borrowing_wire
            .frames()
            .iter()
            .map(|bytes| frame_text(bytes))
            .collect::<Vec<_>>(),
        vec![EXPECTED_FRAME.to_owned()],
        "the borrowing route (take_guarded_session) must put the SAME bytes on the \
         wire — the two construction paths may not diverge"
    );

    // Non-vacuity inside the test: the two routes agree with the builder, byte for byte.
    assert_eq!(
        consuming_wire.frames(),
        borrowing_wire.frames(),
        "both sealing routes must emit identical bytes"
    );
    assert_eq!(
        consuming_wire.frames().first().map(Vec::as_slice),
        Some(direct.as_slice()),
        "and both must agree with to_outgoing"
    );
}
