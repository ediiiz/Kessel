//! `Session::notifications()` and `Session::ro_values()` expose the appliance values received
//! during the handshake. The fake device replays `fixtures/aes/hob-session.json`; no appliance is
//! contacted.
//!
//! `HandshakeOutcome::notifications` is only a count, so these tests check that the messages
//! themselves come out and that they are the same messages the count counted.

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

use std::time::Duration;

use common::{client_session, corpus, fake_wire};

#[tokio::test]
async fn notifications_are_reachable_from_the_session() {
    let corpus = corpus();
    let (sink, stream, device) = fake_wire();
    device.replay_rx(&corpus);

    let mut session = client_session(&corpus, sink, stream);
    let outcome = tokio::time::timeout(Duration::from_secs(5), session.run_handshake())
        .await
        .expect("the handshake must complete, not hang")
        .expect("the handshake completes without error");

    let notifications = session.notifications();

    // 1. The messages themselves come out, not just a count.
    assert!(
        !notifications.is_empty(),
        "the NOTIFY the corpus contains must be reachable as a MESSAGE, not only as the count in \
         HandshakeOutcome. An empty slice here is what an accessor looks like when it is wired to \
         nothing — and every existing assertion about notifications (a count >= 1) still passes in \
         that state, which is why this test exists"
    );

    // 2. And they are the same messages the count counted. A borrow of some *other* collection would
    //    satisfy assertion 1 and fail here.
    assert_eq!(
        notifications.len(),
        outcome.notifications,
        "the accessor must expose exactly what HandshakeOutcome counted"
    );

    // 3. The corpus's NOTIFY is the unsolicited `/ci/registeredDevices`. Naming the resource shows a
    //    real decoded message came through rather than a default-constructed one.
    let resources: Vec<&str> = notifications
        .iter()
        .filter_map(|message| message.resource.as_deref())
        .collect();
    assert!(
        resources.contains(&"/ci/registeredDevices"),
        "the corpus's unsolicited NOTIFY must be identifiable by resource; got {resources:?}"
    );

    // Keep the device (and thus the client's sink target) alive until the assertions are done.
    drop(device);
}

/// The appliance's `/ro` value snapshot (the post-CONNECT `/ro/allMandatoryValues` and
/// `/ro/allDescriptionChanges` RESPONSEs) survives the handshake with its `data` intact. An appliance
/// in standby may push no NOTIFY at all, so this snapshot is its only source of initial state.
#[tokio::test]
async fn ro_value_responses_are_retained_after_the_handshake() {
    let corpus = corpus();
    let (sink, stream, device) = fake_wire();
    device.replay_rx(&corpus);

    let mut session = client_session(&corpus, sink, stream);
    let outcome = tokio::time::timeout(Duration::from_secs(5), session.run_handshake())
        .await
        .expect("the handshake must complete, not hang")
        .expect("the handshake completes without error");
    assert_eq!(
        outcome.state,
        hc_proto::State::Connected,
        "the snapshot is a post-CONNECT thing, so the handshake has to have got there"
    );

    let retained = session.ro_values();
    assert!(
        !retained.is_empty(),
        "the /ro RESPONSEs the handshake loop went out of its way to read must be RETAINED. An empty \
         slice here means their `code` was checked and their bodies dropped, so no \
         caller could learn a value from an appliance that pushes no NOTIFY"
    );

    // Addressed by resource, so this cannot pass on some other retained message.
    let resources: Vec<&str> = retained
        .iter()
        .filter_map(|message| message.resource.as_deref())
        .collect();
    assert!(
        resources
            .iter()
            .any(|resource| resource.starts_with("/ro/")),
        "every retained message must be a /ro resource — that is the whole filter; got {resources:?}"
    );

    // And the BODY is what was being thrown away, so a body-less retention would be no fix at all.
    assert!(
        retained
            .iter()
            .any(|message| message.data.as_ref().is_some_and(|items| !items.is_empty())),
        "at least one retained /ro RESPONSE must carry a non-empty `data` — the body is the entire \
         point, and keeping the envelope without it would satisfy every assertion above"
    );

    // Nothing but /ro is retained: a `/ci/info` body carries device identifiers and must still be
    // dropped. The corpus contains /ci responses, so this is not vacuous.
    assert!(
        resources
            .iter()
            .all(|resource| resource.starts_with("/ro/")),
        "only /ro responses may be retained; a /ci or /ei body must still be dropped: {resources:?}"
    );

    drop(device);
}
