//! Write frames: `POST /ro/values` feature writes and `POST /ro/selectedProgram` program starts.
//!
//! The captured corpus is a read-only session with no write frame, so these tests pin the frame
//! shape rather than replaying a capture. They are pure serialisation tests; nothing here does I/O.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use hc_proto::{
    Action, OutgoingMessage, ProtoError, Version, WriteItem, WriteValue, parse, service_key,
};
use serde_json::{Number, Value, json};

/// Three representative `/ro/values` write items: a number, a string and a boolean.
fn documented_items() -> Vec<WriteItem> {
    vec![
        WriteItem {
            uid: 539,
            value: WriteValue::Number(Number::from(2)),
        },
        WriteItem {
            uid: 520,
            value: WriteValue::Str("2023-07-07T15:01:21".to_owned()),
        },
        WriteItem {
            uid: 547,
            value: WriteValue::Bool(false),
        },
    ]
}

/// A feature write is `POST /ro/values`.
#[test]
fn write_frame_uses_the_documented_resource_and_verb() {
    let message = OutgoingMessage::values_write(1, 2, Version::Int(1), documented_items())
        .expect("a three-item write is accepted");

    assert_eq!(
        message.resource, "/ro/values",
        "the feature-write resource is /ro/values in all three references"
    );
    assert_eq!(
        message.action,
        Action::Post,
        "the feature-write verb is POST — a GET here would be a read of the same resource"
    );
    assert_eq!(
        service_key(&message.resource),
        Some("ro"),
        "the service key that resolves `version` is `ro` (message-model.md §5.2)"
    );

    // And on the wire, in the documented field order.
    let wire = String::from_utf8(message.to_wire_bytes().unwrap()).unwrap();
    assert!(
        wire.contains(r#""resource":"/ro/values","version":1,"action":"POST","data":["#),
        "resource, version, action then data, in that order (message-model.md §1): {wire}"
    );
    assert!(
        !wire.contains(r#""code""#),
        "an outbound write carries no code — code is response-only (message-model.md §1): {wire}"
    );
}
/// Firmware parses JSON objects positionally, so `uid` must precede `value`. Asserted on the byte
/// string, because `serde_json::Value` equality ignores key order.
#[test]
fn write_body_serializes_positionally() {
    let single = OutgoingMessage::values_write(
        1,
        2,
        Version::Int(1),
        vec![WriteItem {
            uid: 256,
            value: WriteValue::Number(Number::from(7)),
        }],
    )
    .expect("a one-item write is accepted");
    let wire = String::from_utf8(single.to_wire_bytes().unwrap()).unwrap();
    assert_eq!(
        wire,
        r#"{"sID":1,"msgID":2,"resource":"/ro/values","version":1,"action":"POST","data":[{"uid":256,"value":7}]}"#,
        "the write frame must be byte-exact: five mandatory fields in order, then data, and \
         uid BEFORE value inside the item"
    );

    // Spelled out, so a failure says which rule broke rather than just "bytes differ".
    assert!(
        wire.contains(r#""data":[{"uid":256,"value":7}]"#),
        "the item must emit uid first, then value: {wire}"
    );
    assert!(
        !wire.contains(r#""value":7,"uid""#),
        "value must never precede uid — that is the positional-parser hazard: {wire}"
    );
    assert!(
        !wire.contains(", ") && !wire.contains(": "),
        "compact separators, no space after ',' or ':' (message-model.md §2.1): {wire}"
    );

    // Numbers, strings and booleans keep their JSON type, still uid-first, and a multi-item write
    // stays one list.
    let many = OutgoingMessage::values_write(1, 2, Version::Int(1), documented_items())
        .expect("a three-item write is accepted");
    let many_wire = String::from_utf8(many.to_wire_bytes().unwrap()).unwrap();
    assert!(
        many_wire.ends_with(
            r#""data":[{"uid":539,"value":2},{"uid":520,"value":"2023-07-07T15:01:21"},{"uid":547,"value":false}]}"#
        ),
        "every item is uid-then-value, the numbers/strings/booleans keep their JSON type, and the \
         items stay in the order given: {many_wire}"
    );
}
/// A write frame parses back with its routing metadata and items intact, and re-emitting it
/// reproduces the original bytes exactly.
#[test]
fn write_body_round_trips_through_outgoing_message() {
    let built = OutgoingMessage::values_write(
        2_879_624_297,
        3_544_218_780,
        Version::Int(1),
        documented_items(),
    )
    .expect("a three-item write is accepted");
    let bytes = built.to_wire_bytes().unwrap();

    // Parse it back through the crate's real inbound path.
    let parsed = parse(&bytes).unwrap();
    assert_eq!(parsed.action, Action::Post);
    assert_eq!(parsed.resource.as_deref(), Some("/ro/values"));
    assert_eq!(parsed.s_id, Some(2_879_624_297));
    assert_eq!(parsed.msg_id, Some(3_544_218_780));
    assert_eq!(parsed.version.as_ref().and_then(Version::as_int), Some(1));
    assert!(
        parsed.code.is_none(),
        "no code on an outbound write (message-model.md §4)"
    );

    let items = parsed.data.expect("data is a list of write items");
    assert_eq!(items.len(), 3, "data is a list, one entry per item");
    let uids: Vec<i64> = items
        .iter()
        .filter_map(|item| item.get("uid").and_then(Value::as_i64))
        .collect();
    assert_eq!(uids, vec![539, 520, 547], "item order is preserved");
    let value_at = |index: usize| {
        items
            .get(index)
            .and_then(|item| item.get("value"))
            .expect("every item carries a value")
    };
    assert_eq!(value_at(0).as_i64(), Some(2));
    assert_eq!(value_at(1).as_str(), Some("2023-07-07T15:01:21"));
    assert_eq!(value_at(2).as_bool(), Some(false));

    // Re-emitting the same message reproduces the bytes: the framing path is deterministic.
    let rebuilt = OutgoingMessage::values_write(
        parsed.s_id.unwrap(),
        parsed.msg_id.unwrap(),
        parsed.version.unwrap(),
        documented_items(),
    )
    .expect("a three-item write is accepted");
    assert_eq!(
        rebuilt.to_wire_bytes().unwrap(),
        bytes,
        "a write frame round-trips through the existing framing path byte-for-byte"
    );

    // `expects_response` is currently `GET`-only, so a POST registers no correlation slot. This pins
    // current behaviour, not a protocol rule.
    assert!(
        !built.expects_response(),
        "documenting today's behaviour: POST does not register a correlation slot"
    );
}
/// A `null`, array or object `value` is refused with a typed error. Write values are scalars; no
/// encoding for other shapes is known, so refusing is the fail-safe choice.
#[test]
fn write_body_rejects_a_value_it_cannot_represent() {
    for (label, unrepresentable) in [
        ("null", Value::Null),
        ("array", json!([1, 2])),
        ("object", json!({"uid": 1, "value": 2})),
    ] {
        let refused = WriteValue::from_json(&unrepresentable);
        assert!(
            matches!(refused, Err(ProtoError::UnrepresentableWriteValue(kind)) if kind == label),
            "a {label} value must be refused as a typed \
             ProtoError::UnrepresentableWriteValue({label}), not accepted: {refused:?}"
        );
        // And no partial frame escapes: the item cannot be built, so nothing reaches the wire.
        assert!(
            WriteItem::from_json(539, &unrepresentable).is_err(),
            "a {label} value must not yield a WriteItem"
        );
    }

    // The scalars that *are* attested still convert, and keep their JSON type.
    assert_eq!(
        WriteValue::from_json(&json!(2)).unwrap(),
        WriteValue::Number(Number::from(2)),
        "an integer value converts (reference/hcpy/README.md:443)"
    );
    assert_eq!(
        WriteValue::from_json(&json!("2023-07-07T15:01:21")).unwrap(),
        WriteValue::Str("2023-07-07T15:01:21".to_owned()),
        "a string value converts (reference/hcpy/README.md:452)"
    );
    assert_eq!(
        WriteValue::from_json(&json!(false)).unwrap(),
        WriteValue::Bool(false),
        "a boolean value converts (reference/hcpy/README.md:458)"
    );
    // A non-integral number keeps its JSON form on the wire rather than being truncated to an int.
    let fractional = OutgoingMessage::values_write(
        1,
        2,
        Version::Int(1),
        vec![WriteItem::from_json(548, &json!(1.5)).unwrap()],
    )
    .expect("a one-item write is accepted");
    assert!(
        String::from_utf8(fractional.to_wire_bytes().unwrap())
            .unwrap()
            .ends_with(r#""data":[{"uid":548,"value":1.5}]}"#),
        "a non-integral number keeps its JSON form, and is not truncated"
    );

    // `WriteValue::Number` holds a `serde_json::Number`, which cannot be NaN or infinite; an `f64`
    // field would silently serialise those as `null`.
    assert!(
        Number::from_f64(f64::NAN).is_none() && Number::from_f64(f64::INFINITY).is_none(),
        "a non-finite number cannot even be constructed, so it cannot be silently nulled"
    );
}
/// An empty feature write is a typed error, not a frame. `"data":[]` is not a known frame shape
/// and an empty write asks for nothing, so refusing it costs nothing.
#[test]
fn write_body_refuses_an_empty_item_list() {
    let refused = OutgoingMessage::values_write(1, 2, Version::Int(1), Vec::new());

    assert!(
        matches!(refused, Err(ProtoError::EmptyWriteItemList)),
        "an empty item list must be refused as a typed ProtoError::EmptyWriteItemList, not turned \
         into a frame: {refused:?}"
    );

    // Nothing reaches `to_wire_bytes`, so `"data":[]` cannot be emitted.
    assert!(
        refused.is_err(),
        "no OutgoingMessage exists to serialise, so `\"data\":[]` cannot be emitted"
    );

    // One item is still enough: the refusal is about emptiness only.
    let single = OutgoingMessage::values_write(
        1,
        2,
        Version::Int(1),
        vec![WriteItem {
            uid: 539,
            value: WriteValue::Number(Number::from(2)),
        }],
    )
    .expect("a one-item write is still accepted");
    assert!(
        String::from_utf8(single.to_wire_bytes().unwrap())
            .unwrap()
            .ends_with(r#""data":[{"uid":539,"value":2}]}"#),
        "a one-item write is unaffected by the empty-list refusal"
    );

    // And a three-item write, so the guard cannot be a length check pointing the wrong way.
    assert!(
        OutgoingMessage::values_write(1, 2, Version::Int(1), documented_items()).is_ok(),
        "a multi-item write is accepted"
    );
}

// =================================================================================================
// Program start: `POST /ro/selectedProgram` carries `{"program":<uid>,"options":[{"uid":…,…}]}`.
// =================================================================================================

/// `program` is serialised before `options`, because firmware parses objects positionally.
/// Asserted on the byte string, since `serde_json::Value` equality ignores key order.
#[test]
fn program_start_body_serializes_program_before_options() {
    let frame = OutgoingMessage::program_start(
        1,
        2,
        Version::Int(1),
        0x1234,
        vec![WriteItem {
            uid: 558,
            value: WriteValue::Number(Number::from(0)),
        }],
    );
    let wire = String::from_utf8(frame.to_wire_bytes().unwrap()).unwrap();

    assert_eq!(
        wire,
        r#"{"sID":1,"msgID":2,"resource":"/ro/selectedProgram","version":1,"action":"POST","data":[{"program":4660,"options":[{"uid":558,"value":0}]}]}"#,
        "the program-start frame must be byte-exact: the five mandatory fields in order, then data, \
         then `program` BEFORE `options`, and `uid` before `value` inside each option"
    );

    // Spelled out so a failure names the rule that broke.
    assert!(
        wire.contains(r#""data":[{"program":4660,"options":"#),
        "`program` must precede `options` — hcpy's order, not Go's map-sorted one: {wire}"
    );
    assert!(
        !wire.contains(r#""options":[{"uid":558,"value":0}],"program""#),
        "`options` must never precede `program`: that is Go's encoding/json artefact, and this frame \
         starts appliances: {wire}"
    );
}

/// The uid-first rule applies inside the nested options list too.
#[test]
fn program_start_options_keep_uid_first() {
    let frame = OutgoingMessage::program_start(
        7,
        9,
        Version::Int(1),
        1,
        vec![
            WriteItem {
                uid: 551,
                value: WriteValue::Number(Number::from(1800)),
            },
            WriteItem {
                uid: 558,
                value: WriteValue::Bool(true),
            },
        ],
    );
    let wire = String::from_utf8(frame.to_wire_bytes().unwrap()).unwrap();

    assert!(
        wire.contains(r#""options":[{"uid":551,"value":1800},{"uid":558,"value":true}]"#),
        "every option keeps uid first, in the order given: {wire}"
    );
    assert!(
        !wire.contains(r#""value":1800"#) || !wire.contains(r#""value":1800,"uid""#),
        "no option may emit value before uid: {wire}"
    );
}

/// A program start with no options emits `"options":[]`, keeping the key rather than omitting it.
/// Unlike an empty feature write, starting a program without options is a normal request.
#[test]
fn a_program_start_with_no_options_emits_an_empty_list() {
    let frame = OutgoingMessage::program_start(1, 2, Version::Int(1), 8, Vec::new());
    let wire = String::from_utf8(frame.to_wire_bytes().unwrap()).unwrap();
    assert_eq!(
        wire,
        r#"{"sID":1,"msgID":2,"resource":"/ro/selectedProgram","version":1,"action":"POST","data":[{"program":8,"options":[]}]}"#,
        "an optionless program start keeps the `options` key with an empty list: {wire}"
    );
}
