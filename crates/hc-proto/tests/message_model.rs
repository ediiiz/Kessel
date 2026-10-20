//! Wire message model: serialisation, lenient parsing and response codes.

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

use std::collections::BTreeSet;

use common::corpus;
use hc_proto::{
    Action, DeviceIdentity, DeviceType, OutgoingMessage, ResponseCode, Version, apply_updates,
    parse,
};
use tracing_test::traced_test;

/// Compact `,`/`:` separators, `data` always a list on send, and a parsed frame re-serialises to the
/// exact captured bytes.
#[test]
fn message_roundtrips_compact_json() {
    let corpus = corpus();

    // Frame 2: GET /ci/services, version 1, no data — the simplest send.
    let services = corpus.frame(2);
    let built = OutgoingMessage::bare(
        services.s_id,
        services.msg_id,
        "/ci/services",
        Version::Int(1),
        Action::Get,
    );
    let wire = built.to_wire_bytes().unwrap();
    assert_eq!(
        wire,
        services.plaintext(),
        "GET /ci/services must serialise byte-for-byte to the captured frame 2 plaintext"
    );

    let text = String::from_utf8(wire).unwrap();
    assert!(
        !text.contains(", ") && !text.contains(": "),
        "serialisation must be compact (no space after ',' or ':'): {text}"
    );
    assert!(
        !text.contains("\"data\""),
        "a data-less GET must omit the data key entirely, not send data:null: {text}"
    );

    // Frame 1: RESPONSE /ei/initialValues with data — data must serialise as a one-element list.
    let response = corpus.frame(1);
    let identity = DeviceIdentity {
        name: corpus.client.device_name.clone(),
        id: corpus.client.device_id.clone(),
    };
    let built_response = OutgoingMessage::initial_values_response(
        response.s_id,
        response.msg_id,
        Version::Int(2),
        DeviceType::Application,
        &identity,
    );
    let response_wire = built_response.to_wire_bytes().unwrap();
    assert_eq!(
        response_wire,
        response.plaintext(),
        "RESPONSE /ei/initialValues must serialise byte-for-byte to captured frame 1"
    );
    let response_text = String::from_utf8(response_wire).unwrap();
    assert!(
        response_text.contains("\"data\":[{"),
        "data must be a JSON list even for a single object: {response_text}"
    );

    // Round-trip: a parsed inbound frame re-built as an outbound one reproduces the bytes.
    let reparsed = parse(&services.plaintext()).unwrap();
    assert_eq!(reparsed.action, Action::Get);
    assert_eq!(reparsed.resource.as_deref(), Some("/ci/services"));
    assert_eq!(reparsed.version.and_then(|v| v.as_int()), Some(1));
    assert!(reparsed.data.is_none(), "GET /ci/services carries no data");
    assert!(
        reparsed.code.is_none(),
        "no frame in the corpus carries a code"
    );
}

/// Appliances sometimes quote `sID`/`msgID`/`version`; the parser strips the quotes, parses a float
/// and truncates (`"4.2"` → `4`).
#[test]
fn coerces_string_sid_msgid_version() {
    // Synthetic: every numeric field is a quoted string, and version is a non-integral float.
    let quoted = br#"{"sID":"2879624297","msgID":"3544218773","resource":"/ci/services","version":"4.2","action":"GET"}"#;
    let parsed = parse(quoted).unwrap();
    assert_eq!(
        parsed.s_id,
        Some(2_879_624_297),
        "quoted sID must coerce to its int"
    );
    assert_eq!(
        parsed.msg_id,
        Some(3_544_218_773),
        "quoted msgID must coerce to its int"
    );
    assert_eq!(
        parsed.version.as_ref().and_then(Version::as_int),
        Some(4),
        "quoted float version must truncate to 4 (message-model.md §3.1)"
    );

    // The int form still parses — the coercion is a superset, not a replacement.
    let ints = br#"{"sID":2879624297,"msgID":3544218773,"resource":"/ci/services","version":2,"action":"GET"}"#;
    let parsed_ints = parse(ints).unwrap();
    assert_eq!(parsed_ints.s_id, Some(2_879_624_297));
    assert_eq!(parsed_ints.version.and_then(|v| v.as_int()), Some(2));
}

/// On a decode failure, the parser replaces the two-byte sequence `]"` with `]` and re-parses once.
#[test]
fn malformed_object_field_workaround() {
    // A stray quote after a closing bracket — invalid JSON as-is, valid after `]"` -> `]`.
    let malformed =
        br#"{"sID":2879624297,"msgID":1,"resource":"/ro/values","version":1,"action":"NOTIFY","data":[{"uid":1,"value":[1,2]"}]}"#;
    // Confirm the fixture really is malformed, so the test proves the repair, not lenient parsing.
    assert!(
        serde_json::from_slice::<serde_json::Value>(malformed).is_err(),
        "the fixture must fail a plain parse, or the workaround is untested"
    );

    let parsed = parse(malformed).expect("the `]\"` -> `]` repair must recover this frame");
    assert_eq!(parsed.action, Action::Notify);
    assert_eq!(parsed.resource.as_deref(), Some("/ro/values"));
    let data = parsed.data.expect("data present after repair");
    assert_eq!(data.len(), 1, "one update item survives the repair");
    assert_eq!(
        data.first()
            .and_then(|item| item.get("uid"))
            .and_then(serde_json::Value::as_i64),
        Some(1)
    );
}

/// An item for an unknown `uid` is dropped and logged at debug, never fatal.
#[traced_test]
#[test]
fn unknown_uid_is_ignored_and_logged() {
    let known: BTreeSet<u64> = [3, 5].into_iter().collect();
    let items = vec![
        serde_json::json!({"uid": 3, "value": true}),
        serde_json::json!({"uid": 999, "value": 0}),
        serde_json::json!({"uid": 5, "value": false}),
    ];

    let applied = apply_updates(&known, &items);

    let uids: Vec<u64> = applied.iter().map(|update| update.uid).collect();
    assert_eq!(
        uids,
        vec![3, 5],
        "the unknown uid 999 must be dropped, not applied"
    );
    assert!(
        logs_contain("unknown uid"),
        "the dropped uid must be logged at debug (message-model.md §3.4)"
    );
}

/// Any non-null `code` is an error (so `code:0` is one); an unknown code survives as `Other`, and
/// 412/414 map to the same variant.
#[test]
fn code_non_null_becomes_typed_error() {
    let cases = [
        (400, ResponseCode::BadRequest),
        (403, ResponseCode::Forbidden),
        (413, ResponseCode::RequestEntityTooLong),
        (412, ResponseCode::RequestUriTooLong), // 412 and 414 merge...
        (414, ResponseCode::RequestUriTooLong), // ...to the same variant.
        (429, ResponseCode::TooManyRequests),
        (500, ResponseCode::InternalServerError),
        (512, ResponseCode::UnknownUid),
        (541, ResponseCode::ProcessStateNotCompliant),
        (544, ResponseCode::NotInLocalWifi),
        (0, ResponseCode::Other(0)), // non-null: 0 IS an error, classified Other.
        (777, ResponseCode::Other(777)), // unknown code retains its value.
    ];
    for (code, expected) in cases {
        let frame = format!(
            r#"{{"sID":1,"msgID":2,"resource":"/ci/info","version":2,"action":"RESPONSE","code":{code}}}"#
        );
        let parsed = parse(frame.as_bytes()).unwrap();
        assert_eq!(
            parsed.response_error(),
            Some((i64::from(code), expected)),
            "code {code} must classify to {expected} and stay an error"
        );
    }

    // `code:null` and an absent `code` are both "no error" — not an `Other(0)`.
    let nulled =
        br#"{"sID":1,"msgID":2,"resource":"/ci/info","version":2,"action":"RESPONSE","code":null}"#;
    assert!(
        parse(nulled).unwrap().response_error().is_none(),
        "code:null is not an error"
    );
    let absent = br#"{"sID":1,"msgID":2,"resource":"/ci/info","version":2,"action":"RESPONSE"}"#;
    assert!(
        parse(absent).unwrap().response_error().is_none(),
        "absent code is not an error"
    );
}

/// An update item's `access`/`available`/`min`/`max`/`stepSize` are kept alongside its value.
///
/// Sparse: `None` means the item said nothing about that field, so a value-only update does not
/// read as clearing the description.
#[test]
fn an_update_item_carries_its_description_not_only_its_value() {
    let known: BTreeSet<u64> = [2006, 5].into_iter().collect();
    let items = vec![
        serde_json::json!({
            "uid": 2006,
            "value": -18,
            "access": "readWrite",
            "available": true,
            "min": -24,
            "max": -16,
            "stepSize": 1
        }),
        // A plain value update. Every description field is absent, and absent is not zero.
        serde_json::json!({"uid": 5, "value": false}),
    ];

    let applied = apply_updates(&known, &items);

    let described = applied.first().expect("the described item is applied");
    assert_eq!(described.uid, 2006);
    assert_eq!(described.value, Some(serde_json::json!(-18)));
    assert_eq!(described.description.access.as_deref(), Some("readWrite"));
    assert_eq!(described.description.available, Some(true));
    assert_eq!(described.description.min, Some(-24));
    assert_eq!(described.description.max, Some(-16));
    assert_eq!(described.description.step, Some(1));
    assert!(
        described.description.is_some(),
        "an item that revises the description must say so"
    );

    let plain = applied.get(1).expect("the plain item is applied");
    assert!(
        !plain.description.is_some(),
        "a value-only item revises no description, and must not be read as clearing one"
    );
    assert_eq!(plain.description.min, None);
    assert_eq!(plain.description.access, None);
}

#[test]
fn an_item_without_a_value_is_not_a_reported_null() {
    let known: BTreeSet<u64> = [7, 8].into_iter().collect();
    let items = vec![
        serde_json::json!({"uid": 7, "available": false}),
        serde_json::json!({"uid": 8, "value": null}),
    ];

    let applied = apply_updates(&known, &items);

    let revised = applied
        .first()
        .expect("the description-only item is applied");
    assert_eq!(revised.value, None, "absent is not null");
    assert_eq!(revised.description.available, Some(false));
    let nulled = applied.get(1).expect("the null item is applied");
    assert_eq!(nulled.value, Some(serde_json::Value::Null));
}
