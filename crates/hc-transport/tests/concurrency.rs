//! Concurrent sends serialise so the TX chain stays intact.
//!
//! 64 tasks send at once through one [`Sender`]; a mirror [`Opener`] (appliance role) decrypts every
//! frame in arrival order, which only succeeds if frames hit the wire in the order the TX chain
//! advanced, i.e. the send lock spans encode + write. No appliance is contacted.

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

use hc_crypto::{Opener, Role};
use hc_proto::{Action, OutgoingMessage, Version};
use hc_transport::WsFrame;

use common::{client_session, corpus, fake_wire};

const SENDS: u64 = 64;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn n_concurrent_sends_serialize_and_chain_stays_intact() {
    let corpus = corpus();
    let keys = corpus.keys();
    let (sink, stream, mut device) = fake_wire();
    let session = client_session(&corpus, sink, stream);
    // `into_sender` consumes the session; the `Sender` keeps the TX half alive through the shared
    // `Arc`, so the drain below still sees every frame. It is `Option` because it shares the
    // take-once flag with the sealing routes; the test clones the one handle N times.
    let sender = session
        .into_sender()
        .expect("a fresh session yields its one send handle");

    // Fire 64 sends concurrently.
    let mut set = tokio::task::JoinSet::new();
    for id in 0..SENDS {
        let sender = sender.clone();
        set.spawn(async move {
            let message = OutgoingMessage::bare(1, id, "/ci/info", Version::Int(1), Action::Get);
            sender.send(&message).await
        });
    }

    let mut completed = 0_u64;
    while let Some(joined) = set.join_next().await {
        joined
            .expect("the send task joins")
            .expect("each concurrent send succeeds");
        completed += 1;
    }
    assert_eq!(completed, SENDS, "all concurrent sends complete");

    // The mirror opens the client's TX direction; a continuous chain decrypts only if the frames
    // arrived in exactly their seal order.
    let mut mirror = Opener::new(keys, Role::Appliance.open_direction());
    for index in 0..SENDS {
        let frame = device.recv().await.expect("a sealed frame arrives");
        let WsFrame::Binary(bytes) = frame else {
            panic!("every AES send is a binary frame, got {frame:?}");
        };
        mirror
            .open(&bytes)
            .unwrap_or_else(|error| panic!("frame {index} must decrypt in order: {error}"));
    }

    // The session was consumed by `into_sender`; the TX half survives behind the shared `Arc`, held
    // by `sender`, which is why the drain above completed. Dropping the handle now closes the wire.
    drop(sender);
    drop(device);
}
