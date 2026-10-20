//! The transport half of the update pipeline, end to end through a real `hc_transport::Session`:
//!
//! 1. a notification routed in steady state is delivered to the installed sink;
//! 2. it is not also retained. The sink is the drain, which bounds the memory of a connection held
//!    open for weeks.
//!
//! The RX half is a script and the sink writes into a `Vec`; no socket is opened.

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

use common::{RecordedWire, notify_values, steady_state_session};
use hc_proto::IncomingMessage;
use hc_transport::NotificationSink;

/// The synthetic connection's `sID` and the appliance's `edMsgID`: per-connection counters, not
/// identifiers.
const S_ID: u64 = 4_242;
const ED_MSG_ID: u64 = 77;

/// Records how many batches arrived and how many messages in total.
///
/// It stores counts, never messages, because an `IncomingMessage` carries appliance body data.
#[derive(Default)]
struct Collector {
    batches: usize,
    messages: usize,
    uids: Vec<u64>,
}

impl NotificationSink for Collector {
    fn accept(&mut self, notifications: &[IncomingMessage]) {
        self.batches += 1;
        self.messages += notifications.len();
        self.uids.extend(uids_of(notifications));
    }
}

/// The `uid` of every item in every message — what a real consumer decodes.
fn uids_of(notifications: &[IncomingMessage]) -> Vec<u64> {
    notifications
        .iter()
        .filter_map(|message| message.data.as_deref())
        .flatten()
        .filter_map(|item| item.get("uid").and_then(serde_json::Value::as_u64))
        .collect()
}

#[tokio::test]
async fn steady_state_delivers_each_notification_to_the_sink_and_retains_none() {
    let wire = RecordedWire::default();
    let mut session = steady_state_session(
        &wire,
        S_ID,
        ED_MSG_ID,
        vec![
            notify_values(S_ID, 5_001, 41, 0),
            notify_values(S_ID, 5_002, 42, 1),
        ],
    );

    let mut collector = Collector::default();
    // Ends with `TransportError::Closed` when the script runs out — the natural end of a run.
    let ended = session.run_steady_state_with(Some(&mut collector)).await;
    assert!(
        ended.is_err(),
        "the scripted stream ends, so steady state must report the connection over: {ended:?}"
    );

    // 1. Delivery.
    assert_eq!(
        collector.messages, 2,
        "both notifications reached the sink; a sink wired to nothing sees zero"
    );
    assert_eq!(
        collector.uids,
        vec![41, 42],
        "and they carry the items the appliance sent, in arrival order"
    );

    // 2. The sink is also the drain, so nothing accumulates.
    assert!(
        session.notifications().is_empty(),
        "what was handed to the sink must NOT also be retained: a connection held open for weeks \
         would otherwise accumulate every NOTIFY any appliance ever sent"
    );
}

#[tokio::test]
async fn a_session_run_without_a_sink_still_accumulates_for_its_reader() {
    // Without a sink, notifications still accumulate for `notify_stream()`. The ignored live write
    // test in `apps/server` reads its value back that way.
    let wire = RecordedWire::default();
    let mut session = steady_state_session(
        &wire,
        S_ID,
        ED_MSG_ID,
        vec![notify_values(S_ID, 5_001, 41, 0)],
    );

    let ended = session.run_steady_state().await;
    assert!(ended.is_err(), "the script runs out: {ended:?}");
    assert_eq!(
        session.notifications().len(),
        1,
        "with no sink installed the buffer accumulates, which is what a bounded-pump reader reads"
    );
}
