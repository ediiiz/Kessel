//! One `msgID` allocator per connection.
//!
//! The client's outbound counter starts at the `edMsgID` the appliance supplies in
//! `/ei/initialValues` and increments per outbound message, and responses are correlated by
//! `msgID`. A writer holding its own copy of the counter would re-use ids the session already
//! spent, landing on live correlation slots.
//!
//! These tests assert on the `msgID` of every frame that reaches the sink, from both the session
//! and the writer. The sink is a `Vec`, the RX half is a script, and the ids are synthetic.

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

use common::{RecordedWire, SERVER_MSG_ID, handshaking_session};
use hc_core::bridge::take_guarded_session;
use hc_core::write::WriteCommand;
use hc_proto::{OutgoingMessage, Version};
use serde_json::json;

/// Synthetic per-connection numbers. `ED_MSG_ID` is deliberately far from zero so that an
/// independently-seeded counter is obvious rather than coincidentally compatible.
const S_ID: u64 = 4_242;
const ED_MSG_ID: u64 = 100;

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

/// One connection, one sequence: the writer is created before the handshake runs, then both paths
/// emit and no id appears twice. Taking the handle first is what distinguishes a copied counter
/// from a shared one, and matches a long-lived per-device writer.
#[tokio::test]
async fn msg_id_allocator_is_shared_between_session_and_writer() {
    let wire = RecordedWire::default();
    let mut session = handshaking_session(&wire, S_ID, ED_MSG_ID);

    // Path 1's handle, taken first.
    let mut writer =
        take_guarded_session(&mut session).expect("a fresh session yields its one send handle");

    // Path 2: the session's own handshake frames. It ends in `Closed` once the script runs out; the
    // frames it emitted are already on the sink.
    let outcome = session.run_handshake().await;
    assert!(
        outcome.is_err(),
        "the two-message script cannot reach CONNECTED; the point is the frames it emitted first"
    );
    let after_handshake = wire.msg_ids();
    assert!(
        after_handshake.len() > 3,
        "the handshake must have emitted several frames for this test to mean anything, got {after_handshake:?}"
    );

    // Path 1 emits, after the session has spent its ids.
    for _ in 0..2_u8 {
        writer
            .write(&feature_write())
            .await
            .expect("no gate remains that could refuse a /ro/values write");
    }

    let all = wire.msg_ids();
    let write_ids = all
        .get(after_handshake.len()..)
        .expect("the write frames follow the handshake frames")
        .to_vec();
    assert_eq!(write_ids.len(), 2, "exactly two write frames were emitted");

    // (1) No id is re-used across the two paths. The response to `/ei/initialValues` echoes the
    // appliance's msgID and spends no client id, so it is excluded.
    let mut client_ids: Vec<u64> = all
        .iter()
        .copied()
        .filter(|id| *id != SERVER_MSG_ID)
        .collect();
    let emitted = client_ids.clone();
    client_ids.sort_unstable();
    client_ids.dedup();
    assert_eq!(
        client_ids.len(),
        emitted.len(),
        "a msgID was emitted twice on one connection, which lands on a live correlation slot. \
         Emitted, in order: {emitted:?}"
    );

    // (2) The writer continued the connection's sequence rather than starting a second one.
    let highest_handshake_id = after_handshake
        .iter()
        .copied()
        .filter(|id| *id != SERVER_MSG_ID)
        .max()
        .expect("the handshake spent at least one client id");
    assert_eq!(
        write_ids,
        vec![highest_handshake_id + 1, highest_handshake_id + 2],
        "the writer must continue the connection's one sequence. The handshake's client ids were \
         {after_handshake:?} (appliance id {SERVER_MSG_ID} excluded); the writer emitted {write_ids:?}"
    );

    // (3) The sequence started at the appliance's `edMsgID`, not at zero.
    assert_eq!(
        after_handshake
            .iter()
            .copied()
            .filter(|id| *id != SERVER_MSG_ID)
            .min(),
        Some(ED_MSG_ID),
        "the connection's first client id must be the appliance's edMsgID"
    );
}

/// A session hands out its send handle once, so a caller holding the session cannot build a second
/// write path. This covers the sealing routes and `into_sender` after a sealing route, checked by
/// bytes at the sink.
#[tokio::test]
async fn a_session_yields_its_send_handle_only_once() {
    let wire = RecordedWire::default();
    let mut session = handshaking_session(&wire, S_ID, ED_MSG_ID);

    let first = take_guarded_session(&mut session);
    assert!(first.is_some(), "the first take yields the handle");

    // A second attempt gets nothing.
    assert!(
        take_guarded_session(&mut session).is_none(),
        "a session must not yield a second send handle: two writers on one connection are two write \
         paths, and the second one would not be the one production sealed"
    );
    assert!(
        take_guarded_session(&mut session).is_none(),
        "changing the role does not buy a second handle either"
    );

    // Nothing was emitted by any of that.
    assert_eq!(
        wire.byte_count(),
        0,
        "taking a handle emits nothing; only a write does"
    );
    drop(first);

    // PHASE 3: `into_sender` is a third route to a handle and consumes the session, so it can only
    // run after a sealing route. If it still hands one out, the connection has two send handles and
    // one of them bypasses `hc_core::write::GuardedWriter::write`.
    let raw = session.into_sender();
    if let Some(handle) = raw {
        // Not reachable while the invariant holds. If it ever is, put the bytes on the sink so the
        // assertion below reports the real damage rather than merely a `Some`.
        let message = OutgoingMessage::bare(
            S_ID,
            ED_MSG_ID,
            "/ci/info",
            Version::Int(1),
            hc_proto::Action::Get,
        );
        let sent = handle.send(&message).await;
        panic!(
            "a RAW send handle must not be obtainable after the sealing route already took this \
             session's one handle. `into_sender` handed one out; the send returned {sent:?}, putting \
             {} byte(s) on the wire past the guard.",
            wire.byte_count()
        );
    }
    assert_eq!(
        wire.byte_count(),
        0,
        "and nothing reached the sink on any of the three refused routes"
    );
}
