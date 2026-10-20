//! The consuming route to a guarded writer.
//!
//! `Session::take_guarded` borrows, for a long-lived writer that keeps running `run_steady_state`.
//! `Session::into_guarded` takes `self`, so no `Session` or `Sender` survives the call except what
//! the sealing closure returns. A use-after-move cannot be a test (it would not compile), so the
//! declaration is asserted as source text, alongside a check that the sealed handle really works.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed asserts; library code may not"
    )
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use hc_proto::{
    DeviceIdentity, Handshake, HandshakeConfig, IncomingMessage, OutgoingMessage, RandomNonce,
};
use hc_transport::codec::{FrameDecoder, FrameEncoder};
use hc_transport::session::{Endpoints, Session};
use hc_transport::transport::{FrameSink, FrameStream};
use hc_transport::{TransportError, WsFrame};

/// `crates/hc-transport/src/session.rs`, read as text.
fn session_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("session.rs");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// A sink that counts frames, so the sealed handle can be shown to actually work.
#[derive(Clone, Default)]
struct CountingSink {
    frames: Arc<AtomicUsize>,
}

#[async_trait]
impl FrameSink for CountingSink {
    async fn send_frame(&mut self, _frame: WsFrame) -> Result<(), TransportError> {
        self.frames.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}

struct DeadStream;

#[async_trait]
impl FrameStream for DeadStream {
    async fn next_frame(&mut self) -> Result<WsFrame, TransportError> {
        Err(TransportError::Closed)
    }
}

struct PlainEncoder;

impl FrameEncoder for PlainEncoder {
    fn encode(&mut self, message: &OutgoingMessage) -> Result<WsFrame, TransportError> {
        Ok(WsFrame::Binary(message.to_wire_bytes()?))
    }
}

struct DeadDecoder;

impl FrameDecoder for DeadDecoder {
    fn decode(&mut self, _frame: WsFrame) -> Result<IncomingMessage, TransportError> {
        Err(TransportError::Closed)
    }
}

fn session(sink: CountingSink) -> Session<CountingSink, DeadStream, PlainEncoder, DeadDecoder> {
    Session::new(
        Endpoints::new(sink, DeadStream, PlainEncoder, DeadDecoder),
        Handshake::new(HandshakeConfig {
            // Synthetic: not a real `deviceName`/`deviceID`.
            identity: DeviceIdentity {
                name: "kessel-test".to_owned(),
                id: "00000000".to_owned(),
            },
        }),
        Box::new(RandomNonce),
    )
}

/// Assembled at runtime so this file does not itself contain the spellings it asserts about.
fn declarations() -> (String, String) {
    (
        format!("pub fn into_{}<W>(\n        self,", "guarded"),
        format!("pub fn into_{}<W>(\n        &mut self,", "guarded"),
    )
}

#[test]
fn into_guarded_consumes_the_session() {
    let source = session_source();
    let (consuming, borrowing) = declarations();

    assert!(
        source.contains(&consuming),
        "`Session` must offer a CONSUMING route to a guarded writer, so that a caller with no further \
         use for the session cannot retain it and keep the `handle_taken` flag as the only thing \
         standing between them and a second handle. Expected `{consuming}` in \
         crates/hc-transport/src/session.rs; found neither it nor a borrowing twin worth reporting. \
         `Session::take_guarded` (which borrows, for the long-lived-writer case) is not a substitute — \
         it is the weaker of the two."
    );
    assert!(
        !source.contains(&borrowing),
        "the consuming route must not also exist in a borrowing spelling under the same name"
    );

    // Non-vacuity: the assertion above is watching a file that really is this crate's session module,
    // and not an empty string from a mis-joined path.
    assert!(
        source.contains("pub struct Session<Si, St, E, D>"),
        "the source read is not crates/hc-transport/src/session.rs"
    );
    assert!(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src").is_dir(),
        "the manifest dir does not look like a crate root"
    );
}

/// The consuming route is not a stub: the handle it seals really transmits, and the session's own
/// `sID`/allocator/version reach the closure.
#[tokio::test]
async fn into_guarded_yields_a_working_handle() {
    let sink = CountingSink::default();
    let connection = session(sink.clone());

    let (sender, s_id, msg_ids, version) = connection
        .into_guarded(|sender, s_id, msg_ids, version| (sender, s_id, msg_ids, version))
        .expect("a fresh session yields its one handle");

    // A fresh FSM has seen no `/ei/initialValues`, so `sID` is 0 and the allocator sits at 0 (the
    // counter restarts at the appliance's `edMsgID` when it arrives), and an empty services table
    // resolves every resource to the default version of 1.
    assert_eq!(s_id, 0, "no sID has been assigned yet");
    assert_eq!(msg_ids.peek(), 0, "the allocator has not been repositioned");
    assert_eq!(
        version.as_int(),
        Some(1),
        "an empty /ci/services table means version 1 (message-model.md §5.2 step 1)"
    );

    let message = OutgoingMessage::bare(
        s_id,
        msg_ids.take(),
        "/ci/info",
        version,
        hc_proto::Action::Get,
    );
    sender.send(&message).await.expect("the handle transmits");
    assert_eq!(
        sink.frames.load(Ordering::SeqCst),
        1,
        "the sealed handle must reach the sink — otherwise the assertion above proves nothing about a \
         real route"
    );
}
