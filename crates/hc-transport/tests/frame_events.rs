//! Per-frame trace events at the codec boundary, consumed by the frame inspector. A consumer
//! classifies an event as a frame iff its fields contain `direction`.
//!
//! The capture mirrors `hc_core::trace::TraceBusLayer`'s rendering (see `common::CaptureLayer`), so
//! the strings asserted here are the strings an SSE `TraceFrame.fields` would carry. Everything is
//! in-process; no appliance is contacted.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed asserts; library code may not"
    )
)]

mod common;

use hc_proto::{Action, OutgoingMessage, State, Version};
use hc_transport::aes_ws::AesEncoder;
use hc_transport::trace::{DIRECTION_RX, DIRECTION_TX, FRAME_TARGET, TRANSPORT_AES};
use hc_transport::{FrameEncoder as _, WsFrame};

/// A `GET /ci/services`, the request whose RESPONSE carries the 403 the inspector helps diagnose.
fn a_get() -> OutgoingMessage {
    OutgoingMessage::bare(
        2_879_624_297,
        3_544_218_773,
        "/ci/services",
        Version::Int(1),
        Action::Get,
    )
}

/// An encode at the AES codec boundary emits an event carrying `direction`, the field name that
/// defines a frame event.
#[test]
fn frame_events_carry_a_direction_field() {
    let corpus = common::corpus();
    let (captured, _guard) = common::capture_traces();

    let mut encoder = AesEncoder::new(corpus.keys(), common::test_pad());
    let _frame = encoder.encode(&a_get()).expect("the GET seals");

    let frames = captured.frames();
    assert!(
        !frames.is_empty(),
        "the AES codec boundary emitted no event carrying a `direction` field, so the server's classifier \
         (apps/server/src/types.rs, FRAME_FIELD = \"direction\") would class nothing as a frame and \
         the frame inspector renders an empty list. {} event(s) were captured in total: {:?}",
        captured.events().len(),
        captured.events(),
    );

    // The server and web consumers already use the `rx`/`tx` vocabulary.
    assert_eq!(
        frames.first().and_then(|frame| frame.field("direction")),
        Some("tx"),
        "an encode is a frame leaving the client, so `direction` must be `tx` — the vocabulary the \
         merged SSE test and the merged viewer test both already use"
    );
}

/// Both directions produce frame events: diagnosing a request/response pair needs the RESPONSE
/// too. Driven through the real session loop over the golden corpus, so the counts are those of a
/// real handshake.
#[tokio::test]
async fn frame_events_are_emitted_for_both_directions() {
    let corpus = common::corpus();
    let (sink, stream, device) = common::fake_wire();
    let mut session = common::client_session(&corpus, sink, stream);
    device.replay_rx(&corpus);

    let (captured, guard) = common::capture_traces();
    let outcome = session
        .run_handshake()
        .await
        .expect("the golden corpus handshake completes");
    drop(guard);
    assert_eq!(outcome.state, State::Connected);

    let frames = captured.frames();
    let rx = frames
        .iter()
        .filter(|frame| frame.field("direction") == Some(DIRECTION_RX))
        .count();
    let tx = frames
        .iter()
        .filter(|frame| frame.field("direction") == Some(DIRECTION_TX))
        .count();

    let corpus_rx = corpus.frames.iter().filter(|frame| frame.is_rx()).count();
    assert_eq!(
        rx, corpus_rx,
        "every decoded frame must reach the inspector: the corpus replays {corpus_rx} rx frames \
         but {rx} carried `direction={DIRECTION_RX}`. Captured frame events: {frames:?}"
    );
    assert!(
        tx > 0,
        "the client sent {} frames but none carried `direction={DIRECTION_TX}`",
        corpus.frames.len() - corpus_rx
    );

    // Every frame event is one frame: no direction is emitted twice, and the two values partition
    // the set.
    assert_eq!(
        rx + tx,
        frames.len(),
        "a frame event carried a `direction` outside the {DIRECTION_RX}/{DIRECTION_TX} vocabulary"
    );

    // The rest of the contract, on a real decoded frame rather than a synthetic one.
    let inbound = frames
        .iter()
        .find(|frame| frame.field("direction") == Some(DIRECTION_RX))
        .expect("an rx frame event");
    assert_eq!(inbound.target, FRAME_TARGET);
    assert_eq!(inbound.field("transport"), Some(TRANSPORT_AES));
    assert_eq!(inbound.field("frame_kind"), Some("binary"));
    assert!(
        inbound.field("msg_id").is_some(),
        "msgID is one of the five mandatory wire fields and is what \
         correlates a RESPONSE with its request (§5.3), so a frame event must carry it: {inbound:?}"
    );
    assert!(
        inbound.field("resource").is_some(),
        "a frame event must name the resource it routes to: {inbound:?}"
    );
}

// ---- `fields` contract pins ---------------------------------------------------------------------

/// `chain` is an ordinal that starts at zero on a fresh codec, so a reconnect is visible in the
/// inspector. It must never be chain material: the rolling HMAC and CBC IV are key-derived.
#[test]
fn the_chain_field_is_an_ordinal_not_chain_material() {
    let corpus = common::corpus();
    let (captured, guard) = common::capture_traces();

    let mut encoder = AesEncoder::new(corpus.keys(), common::test_pad());
    for _ in 0..3_u8 {
        let _frame = encoder.encode(&a_get()).expect("the GET seals");
    }
    drop(guard);

    let chains: Vec<String> = captured
        .frames()
        .iter()
        .map(|frame| frame.field("chain").unwrap_or("<absent>").to_owned())
        .collect();
    assert_eq!(
        chains,
        vec!["0".to_owned(), "1".to_owned(), "2".to_owned()],
        "`chain` must be how many frames this direction's chain has already advanced, so a \
         reconnect's fresh codec starts at 0"
    );

    // The codec really does hold the material being withheld, so its absence below is meaningful.
    let hmac = encoder.last_hmac();
    assert_ne!(
        hmac,
        [0_u8; hc_crypto::MAC_LEN],
        "the TX chain must have advanced, or the sweep below proves nothing"
    );
    let rendered = hex::encode(hmac);
    for frame in captured.frames() {
        for text in frame.as_swept_strings() {
            assert!(
                !text.contains(&rendered),
                "the rolling HMAC reached a frame event: it is key-derived and must never leave the \
                 crate"
            );
        }
    }
}

/// An absent optional field is an absent key, not an empty one, so a reader can tell "no `code`"
/// from "`code: 0`", a distinction that matters to the protocol.
#[test]
fn absent_optional_fields_are_absent_keys_not_empty_ones() {
    let corpus = common::corpus();
    let (captured, guard) = common::capture_traces();
    let mut encoder = AesEncoder::new(corpus.keys(), common::test_pad());
    let _frame = encoder.encode(&a_get()).expect("the GET seals");
    drop(guard);

    let frames = captured.frames();
    let frame = frames.first().expect("one frame event");
    assert_eq!(
        frame.field("code"),
        None,
        "a GET carries no `code` (message-model.md §1: `code` is response-only), so the key must be \
         absent rather than empty: {frame:?}"
    );
    // ... while the fields it does carry are all present and non-empty.
    for key in [
        "direction",
        "transport",
        "frame_kind",
        "wire_bytes",
        "wire_head_hex",
        "action",
        "resource",
        "msg_id",
        "chain",
    ] {
        let value = frame.field(key);
        assert!(
            value.is_some_and(|text| !text.is_empty()),
            "`{key}` must be present and non-empty on every AES frame event — the server's frame \
             classifier depends on it: {frame:?}"
        );
    }
}

/// The capture mirrors `hc_core::trace::TraceBusLayer`'s rendering: a `&str` field arrives verbatim,
/// a numeric one as its `Debug`, and `message` is split out of `fields`.
#[test]
fn capture_renders_fields_like_the_trace_bus() {
    let (captured, guard) = common::capture_traces();
    tracing::trace!(target: FRAME_TARGET, direction = "rx", wire_bytes = 48_usize, "frame");
    drop(guard);

    let events = captured.events();
    let event = events.first().expect("the event was captured");
    assert_eq!(event.message, "frame", "`message` is split out of `fields`");
    assert!(!event.fields.contains_key("message"));
    assert_eq!(event.field("direction"), Some("rx"), "a `&str` is verbatim");
    assert_eq!(
        event.field("wire_bytes"),
        Some("48"),
        "a numeric field is rendered through `Debug`, exactly as hc-core's FieldVisitor does"
    );
}

/// A control frame never reaches a codec, so it never appears as a frame event: the session ponging
/// a ping is not protocol traffic.
#[tokio::test]
async fn control_frames_are_not_frame_events() {
    let corpus = common::corpus();
    let (sink, stream, device) = common::fake_wire();
    let mut session = common::client_session(&corpus, sink, stream);

    let (captured, guard) = common::capture_traces();
    device.send(WsFrame::Ping(Vec::new()));
    device.send(WsFrame::Close);
    let _closed = session.run_handshake().await;
    drop(guard);

    assert!(
        captured.frames().is_empty(),
        "a ping/close exchange produced frame events: {:?}",
        captured.frames()
    );
}
