//! The write choke point:
//!
//! 1. every role's write reaches the wire, the cooktop and the oven included;
//! 2. there is only one way to get there: `SessionWriter::new` is `pub(crate)`, its construction
//!    sites are fixed, and `boxed_session_writer` is private. An accidental second write path is the
//!    way bytes could reach an appliance nobody aimed at.
//!
//! Byte counts catch an `Ok` verdict that emitted nothing.

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

use std::path::PathBuf;

use common::{RecordedWire, RecordingTransport, recording_write_path, seeded_session};
use hc_core::bridge::take_guarded_session;
use hc_core::write::{
    DeviceRole, GuardedWriter, StartRequest, WriteCommand, WriteError, start_command,
};
use serde_json::json;

/// The synthetic `sID` and appliance `edMsgID` the wired-path cases run under. Both are
/// per-connection counters, not appliance identifiers.
const S_ID: u64 = 1;
const ED_MSG_ID: u64 = 1;

fn hob_start() -> WriteCommand {
    // A fully-formed hob program start: the most dangerous command the mapping can produce.
    start_command(
        &DeviceRole::Cooktop,
        &StartRequest {
            program_uid: 0x1234,
            start_option_uid: 0,
            relative_time: 0,
        },
    )
    .expect("the hob start command is constructible — that is exactly why the guard must exist")
}

fn oven_write() -> WriteCommand {
    // A plain feature write, used for the oven and the fridge.
    WriteCommand {
        resource: "/ro/values".to_owned(),
        target_uid: Some(42),
        field: None,
        value: json!(180),
        validate: true,
        options: Vec::new(),
    }
}

/// Every role's write reaches the transport exactly once, the cooktop and the oven included.
#[tokio::test]
async fn every_role_reaches_the_transport_through_the_choke_point() {
    for (role, command) in [
        (DeviceRole::Cooktop, hob_start()),
        (DeviceRole::Oven, oven_write()),
        (DeviceRole::Dishwasher, dishwasher_start()),
        (DeviceRole::Fridge, oven_write()),
    ] {
        let transport = RecordingTransport::new();
        let mut writer = GuardedWriter::new(transport.clone());
        let result = writer.write(&command).await;
        assert!(
            result.is_ok(),
            "{role:?}: no gate remains that could refuse this write: {result:?}"
        );
        assert_eq!(
            transport.count(),
            1,
            "{role:?}: the command must reach the transport exactly once. Zero would mean an Ok \
             verdict that emitted nothing; more than one would mean a duplicated send path"
        );
    }
}

fn dishwasher_start() -> WriteCommand {
    start_command(
        &DeviceRole::Dishwasher,
        &StartRequest {
            program_uid: 0x1000,
            start_option_uid: 0x022E,
            relative_time: 0,
        },
    )
    .expect("dishwasher start is constructible")
}

/// A guarded write fails only if the transport fails. (`check_access` refuses separately, based on
/// the appliance's own access declaration.)
#[tokio::test]
async fn the_only_remaining_failure_is_the_transport() {
    struct Failing;
    #[async_trait::async_trait]
    impl hc_core::write::WriteTransport for Failing {
        async fn transmit(&mut self, _command: &WriteCommand) -> Result<(), String> {
            Err("the socket is gone".to_owned())
        }
    }

    let mut writer = GuardedWriter::new(Failing);
    let result = writer.write(&hob_start()).await;
    assert!(
        matches!(result, Err(WriteError::Transport(ref reason)) if reason.contains("socket")),
        "a failing transport is the only failure a write can now report: {result:?}"
    );
}

/// `crates/hc-core/src/bridge.rs`, read as text — see
/// [`session_writer_cannot_be_constructed_outside_the_guard`] for why this is a source assertion.
fn bridge_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("bridge.rs");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// `SessionWriter::new` is sealed, so the guarded constructors in `bridge.rs` are the only way to
/// obtain a writer over a real `hc_transport::Sender`.
///
/// A source assertion, because the property is visibility: an integration test cannot call a
/// `pub(crate)` function, and a compile-fail harness is not a workspace dependency.
#[test]
fn session_writer_cannot_be_constructed_outside_the_guard() {
    let source = bridge_source();
    assert!(
        source
            .contains("pub(crate) const fn new(sender: Sender<Si, E>, session: WriteSessionState)"),
        "SessionWriter::new must be pub(crate): an integrator who can build a SessionWriter can \
         hold a write path that never passed the device allowlist"
    );
    assert!(
        !source.contains("pub const fn new(sender: Sender<Si, E>"),
        "the sealed constructor must not also exist as a `pub` one"
    );
    // The two sites are `guarded_session_writer`, which moves the writer straight into a guard, and
    // the private `boxed_session_writer`, whose privacy the next assertion pins.
    assert_eq!(
        source.matches("SessionWriter::new(").count(),
        2,
        "the only construction sites are guarded_session_writer and the private boxed_session_writer"
    );
    assert!(
        source.contains("fn boxed_session_writer<Si, E>(")
            && !source.contains("pub fn boxed_session_writer"),
        "boxed_session_writer must stay private: it yields a bare BoxedTransport, which is a write \
         path with no role check — exactly what SessionWriter::new is pub(crate) to prevent. \
         Publishing it would re-open that hole one level up."
    );
}

/// A write puts exactly one frame on a real `FrameSink`, spending the session's own next `msgID`,
/// through both construction routes: `guarded_session` consumes the session, and
/// `take_guarded_session` borrows it so it can keep running `run_steady_state`.
#[tokio::test]
async fn both_seal_routes_put_bytes_on_the_wire() {
    // A `/ro/values` feature write; program starts are covered in `write_frame.rs`.
    for command in [oven_write()] {
        // Route A: the consuming seal.
        let (mut writer, wire) = recording_write_path(S_ID, ED_MSG_ID);
        writer
            .write(&command)
            .await
            .expect("no gate remains that could refuse this");
        assert!(
            wire.byte_count() > 0,
            "the consuming route must emit bytes — an Ok with an empty sink is a write that did nothing"
        );
        assert_eq!(
            wire.msg_ids(),
            vec![ED_MSG_ID + 1],
            "and spend the session's own next msgID (`message-model.md` §5.1)"
        );

        // Route B: the borrowing, take-once seal used by a long-lived per-device writer.
        let borrowed_wire = RecordedWire::default();
        let mut session = seeded_session(&borrowed_wire, S_ID, ED_MSG_ID);
        let mut borrowed =
            take_guarded_session(&mut session).expect("a fresh session yields its one send handle");
        borrowed
            .write(&command)
            .await
            .expect("no gate remains that could refuse this");
        assert_eq!(
            borrowed_wire.frames().len(),
            1,
            "the borrowing route must emit exactly one frame"
        );
        assert_eq!(borrowed_wire.msg_ids(), vec![ED_MSG_ID + 1]);
    }
}
