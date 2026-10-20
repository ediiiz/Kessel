//! A write returns the appliance's verdict: it waits for the RESPONSE carrying its `msgID`.

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
use hc_core::acks::WriteAcks;
use hc_core::bridge::take_guarded_session_acked;
use hc_core::write::{WriteCommand, WriteError};
use serde_json::json;

const S_ID: u64 = 1;
const ED_MSG_ID: u64 = 1;
/// The `msgID` the first write spends.
const WRITE_MSG_ID: u64 = ED_MSG_ID + 1;

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

fn response(msg_id: u64, code: Option<i64>) -> hc_proto::IncomingMessage {
    let code = code
        .map(|code| format!(r#","code":{code}"#))
        .unwrap_or_default();
    let frame = format!(
        r#"{{"sID":{S_ID},"msgID":{msg_id},"resource":"/ro/values","version":1,"action":"RESPONSE"{code}}}"#
    );
    hc_proto::parse(frame.as_bytes()).unwrap()
}

/// Writes once, answering with `answer` once the frame is on the wire.
async fn write_answered_by(
    answer: impl FnOnce(&WriteAcks) + Send + 'static,
) -> Result<(), WriteError> {
    let wire = RecordedWire::default();
    let mut session = seeded_session(&wire, S_ID, ED_MSG_ID);
    let acks = WriteAcks::new();
    let mut writer = take_guarded_session_acked(&mut session, acks.clone()).unwrap();
    let reader = tokio::spawn({
        let wire = wire.clone();
        async move {
            while wire.frames().is_empty() {
                tokio::task::yield_now().await;
            }
            answer(&acks);
        }
    });
    let verdict = writer.write(&feature_write()).await;
    reader.await.unwrap();
    verdict
}

#[tokio::test]
async fn a_write_the_appliance_refuses_returns_its_reason() {
    let verdict = write_answered_by(|acks| {
        assert!(acks.resolve(&response(WRITE_MSG_ID, Some(400))));
    })
    .await;
    let error = verdict.expect_err("a refused write is an error");
    assert!(error.to_string().contains("400"), "{error}");
}

#[tokio::test]
async fn a_write_the_appliance_takes_succeeds() {
    let verdict = write_answered_by(|acks| {
        assert!(acks.resolve(&response(WRITE_MSG_ID, None)));
    })
    .await;
    assert!(verdict.is_ok(), "{verdict:?}");
}

#[tokio::test]
async fn a_response_to_another_request_is_not_the_answer() {
    let verdict = write_answered_by(|acks| {
        assert!(!acks.resolve(&response(WRITE_MSG_ID + 7, Some(400))));
        assert!(acks.resolve(&response(WRITE_MSG_ID, None)));
    })
    .await;
    assert!(verdict.is_ok(), "{verdict:?}");
}

#[tokio::test]
async fn a_write_fails_when_the_connection_ends_before_the_answer() {
    let verdict = write_answered_by(WriteAcks::close).await;
    assert!(
        verdict.is_err(),
        "no answer is not an acceptance when the connection is gone"
    );
}
