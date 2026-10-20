//! End to end: the simulator on loopback, driven by Kessel's real TLS-PSK client and by a raw
//! WebSocket client that reads the frames the simulator sends.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "tests may panic"
)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use hc_crypto::Secret;
use hc_profile::{Access, EntityKind, Profile, ProtocolType, ValueType};
use hc_proto::{DeviceIdentity, IncomingMessage, State};
use hc_transport::NotificationSink;
use hc_transport::tls_psk::{self, PskConfig};
use kessel_sim::{Options, SharedAppliance, Simulator};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_openssl::SslStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

type RawWs = WebSocketStream<SslStream<TcpStream>>;

const WAIT: Duration = Duration::from_secs(10);

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/hc-profile/tests/fixtures/dishwasher-tls.zip")
}

async fn start() -> (SocketAddr, SharedAppliance, Profile) {
    let profile = Profile::from_zip_path(&fixture()).unwrap();
    let options = Options {
        port: 0,
        tick: Duration::from_millis(200),
        ..Options::default()
    };
    let simulator = Simulator::bind(&profile, &options).await.unwrap();
    let addr = simulator.local_addr().unwrap();
    assert!(addr.ip().is_loopback(), "the simulator binds loopback");
    let appliance = simulator.appliance();
    tokio::spawn(simulator.run());
    (addr, appliance, profile)
}

fn psk_config(profile: &Profile, identity: &[u8]) -> PskConfig {
    let psk = kessel_sim::tls::decode_psk(profile.credentials().psk64.expose_secret()).unwrap();
    PskConfig::new(identity.to_vec(), Secret::new(psk))
}

fn kessel_identity() -> DeviceIdentity {
    DeviceIdentity {
        name: "hcpy".to_owned(),
        id: "0badcafe".to_owned(),
    }
}

#[derive(Default)]
struct Collect(Vec<IncomingMessage>);

impl NotificationSink for Collect {
    fn accept(&mut self, notifications: &[IncomingMessage]) {
        self.0.extend_from_slice(notifications);
    }
}

#[tokio::test]
async fn kessel_client_reaches_connected_and_receives_values() {
    let (addr, appliance, profile) = start().await;
    let config = psk_config(&profile, b"");
    let mut session = tls_psk::connect("127.0.0.1", addr.port(), &config, kessel_identity())
        .await
        .unwrap();
    let outcome = timeout(WAIT, session.run_handshake())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        outcome.state,
        State::Connected,
        "handshake reaches CONNECTED"
    );
    assert!(!outcome.saw_403, "no 403 during the handshake");

    let mandatory = session
        .ro_values()
        .iter()
        .find(|message| message.resource.as_deref() == Some("/ro/allMandatoryValues"))
        .expect("the client kept /ro/allMandatoryValues");
    assert_eq!(mandatory.code, None, "the bulk read succeeded");
    let items = mandatory.data.as_ref().unwrap();
    assert_eq!(
        items.len(),
        appliance.lock().mandatory_values().len(),
        "every value-carrying entity is reported"
    );
    assert!(items.len() > 50, "the dishwasher has many values");

    // Steady state: the periodic NOTIFY reaches the client's notification sink.
    let mut sink = Collect::default();
    let _still_running = timeout(
        Duration::from_millis(900),
        session.run_steady_state_with(Some(&mut sink)),
    )
    .await;
    assert!(
        sink.0.iter().any(|message| {
            message.resource.as_deref() == Some("/ro/values") && message.data.is_some()
        }),
        "a live /ro/values NOTIFY arrived"
    );
}

#[tokio::test]
async fn the_second_psk_identity_is_accepted_too() {
    let (addr, _appliance, profile) = start().await;
    let config = psk_config(&profile, b"HCCOM_Local_App");
    let mut session = tls_psk::connect("127.0.0.1", addr.port(), &config, kessel_identity())
        .await
        .unwrap();
    let outcome = timeout(WAIT, session.run_handshake())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        outcome.state,
        State::Connected,
        "handshake reaches CONNECTED"
    );
}

#[tokio::test]
async fn a_wrong_psk_fails_the_tls_handshake() {
    let (addr, _appliance, _profile) = start().await;
    let config = PskConfig::new(Vec::new(), Secret::new(vec![0_u8; 32]));
    assert!(
        tls_psk::tls_connect_stream("127.0.0.1", addr.port(), &config)
            .await
            .is_err(),
        "a different PSK cannot connect"
    );
}

async fn raw_client(addr: SocketAddr, profile: &Profile) -> (RawWs, u64) {
    let config = psk_config(profile, b"");
    let tls = tls_psk::tls_connect_stream("127.0.0.1", addr.port(), &config)
        .await
        .unwrap();
    let url = format!("ws://127.0.0.1:{}/homeconnect", addr.port());
    let (mut ws, _response) = tokio_tungstenite::client_async(url, tls).await.unwrap();
    let opening = next_json(&mut ws).await;
    assert_eq!(
        opening["resource"],
        json!("/ei/initialValues"),
        "opens with initialValues"
    );
    assert_eq!(opening["action"], json!("POST"), "initialValues is a POST");
    assert!(opening["data"][0]["edMsgID"].is_u64(), "carries an edMsgID");
    (ws, opening["sID"].as_u64().unwrap())
}

async fn next_json(ws: &mut RawWs) -> Value {
    loop {
        let message = timeout(WAIT, ws.next()).await.unwrap().unwrap().unwrap();
        if let Message::Text(text) = message {
            return serde_json::from_str(text.as_str()).unwrap();
        }
    }
}

async fn request(
    ws: &mut RawWs,
    (s_id, msg_id): (u64, u64),
    action: &str,
    resource: &str,
    data: Value,
) {
    let mut frame = json!({
        "sID": s_id, "msgID": msg_id, "resource": resource, "version": 1, "action": action
    });
    if !data.is_null() {
        frame["data"] = data;
    }
    ws.send(Message::text(frame.to_string())).await.unwrap();
}

/// Reads until the RESPONSE to `msg_id`, collecting every other frame seen on the way.
async fn response_to(ws: &mut RawWs, msg_id: u64, seen: &mut Vec<Value>) -> Value {
    loop {
        let frame = next_json(ws).await;
        if frame["action"] == json!("RESPONSE") && frame["msgID"] == json!(msg_id) {
            return frame;
        }
        seen.push(frame);
    }
}

/// Reads until a NOTIFY on `resource` whose items include `uid`.
async fn notify_for(ws: &mut RawWs, resource: &str, uid: u16, seen: &mut Vec<Value>) -> Value {
    if let Some(found) = seen.iter().find(|frame| carries(frame, resource, uid)) {
        return found.clone();
    }
    loop {
        let frame = next_json(ws).await;
        if carries(&frame, resource, uid) {
            return frame;
        }
        seen.push(frame);
    }
}

fn carries(frame: &Value, resource: &str, uid: u16) -> bool {
    frame["action"] == json!("NOTIFY")
        && frame["resource"] == json!(resource)
        && frame["data"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["uid"] == json!(uid)))
}

fn item_value(frame: &Value, uid: u16) -> Value {
    frame["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["uid"] == json!(uid))
        .unwrap()["value"]
        .clone()
}

fn find_setting(profile: &Profile, wanted: impl Fn(&hc_profile::Entity) -> bool) -> u16 {
    profile
        .model()
        .entities()
        .values()
        .find(|entity| {
            entity.kind == EntityKind::Setting
                && entity.access == Some(Access::ReadWrite)
                && entity.available != Some(false)
                && wanted(entity)
        })
        .expect("the dishwasher has such a setting")
        .uid
}

#[tokio::test]
async fn a_valid_write_is_acknowledged_and_notified() {
    let (addr, appliance, profile) = start().await;
    let (mut ws, s_id) = raw_client(addr, &profile).await;
    let uid = find_setting(&profile, |entity| {
        entity.protocol_type() == Some(ProtocolType::Boolean)
    });
    let before = appliance.lock().value(uid).cloned().unwrap();
    let after = json!(!before.as_bool().unwrap());

    let mut seen = Vec::new();
    request(
        &mut ws,
        (s_id, 100),
        "POST",
        "/ro/values",
        json!([{"uid": uid, "value": after}]),
    )
    .await;
    let response = response_to(&mut ws, 100, &mut seen).await;
    assert_eq!(
        response["sID"],
        json!(s_id),
        "the RESPONSE carries the session id"
    );
    assert!(
        response.get("code").is_none(),
        "an accepted write has no code: {response}"
    );
    assert!(
        response.get("data").is_none(),
        "an accepted write has no data: {response}"
    );

    let notify = notify_for(&mut ws, "/ro/values", uid, &mut seen).await;
    assert_eq!(
        item_value(&notify, uid),
        after,
        "the NOTIFY carries the new value"
    );
    assert_eq!(
        appliance.lock().value(uid),
        Some(&after),
        "the state changed"
    );
}

#[tokio::test]
async fn invalid_writes_get_code_400_and_change_nothing() {
    let (addr, appliance, profile) = start().await;
    let (mut ws, s_id) = raw_client(addr, &profile).await;
    let enum_uid = find_setting(&profile, |entity| {
        matches!(entity.value_type, ValueType::Enumeration { .. })
    });
    let read_only = profile
        .model()
        .entities()
        .values()
        .find(|entity| entity.kind == EntityKind::Status && entity.access == Some(Access::Read))
        .unwrap()
        .uid;
    let before = appliance.lock().mandatory_values();

    let cases = [
        json!([{"uid": enum_uid, "value": 9999}]),
        json!([{"uid": enum_uid, "value": "not an ordinal"}]),
        json!([{"uid": read_only, "value": 1}]),
        json!([{"uid": 65000, "value": 1}]),
        json!([]),
    ];
    let mut seen = Vec::new();
    for (msg_id, data) in (200_u64..).zip(cases) {
        request(&mut ws, (s_id, msg_id), "POST", "/ro/values", data).await;
        let response = response_to(&mut ws, msg_id, &mut seen).await;
        assert_eq!(response["code"], json!(400), "refused with 400: {response}");
        assert_eq!(
            response["resource"],
            json!("/ro/values"),
            "echoes the resource"
        );
    }
    assert_eq!(
        appliance.lock().mandatory_values(),
        before,
        "no value changed"
    );
}

#[tokio::test]
async fn writes_sometimes_revise_another_options_description() {
    let (addr, _appliance, profile) = start().await;
    let (mut ws, s_id) = raw_client(addr, &profile).await;
    let uid = find_setting(&profile, |entity| {
        entity.protocol_type() == Some(ProtocolType::Boolean)
    });
    let mut seen = Vec::new();
    for (msg_id, value) in (300_u64..310).zip([true, false].into_iter().cycle()) {
        request(
            &mut ws,
            (s_id, msg_id),
            "POST",
            "/ro/values",
            json!([{"uid": uid, "value": value}]),
        )
        .await;
        let response = response_to(&mut ws, msg_id, &mut seen).await;
        assert!(response.get("code").is_none(), "accepted: {response}");
    }
    // Let the last NOTIFYs arrive.
    request(
        &mut ws,
        (s_id, 399),
        "GET",
        "/ro/allDescriptionChanges",
        Value::Null,
    )
    .await;
    let bulk = response_to(&mut ws, 399, &mut seen).await;

    let change = seen
        .iter()
        .find(|frame| frame["resource"] == json!("/ro/descriptionChange"))
        .expect("ten writes produce at least one description change");
    let item = &change["data"][0];
    let changed = u16::try_from(item["uid"].as_u64().unwrap()).unwrap();
    assert_ne!(changed, uid, "the change is about a different feature");
    let entity = profile.model().entity(changed).unwrap();
    assert_eq!(entity.kind, EntityKind::Option, "only options are revised");
    assert!(
        item.get("access").is_some() || item.get("available").is_some(),
        "the item revises access or availability: {item}"
    );
    assert!(
        bulk["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["uid"] == item["uid"]),
        "the revision is reported by /ro/allDescriptionChanges"
    );

    // Kessel's decoder reads the item as a description revision.
    let known = std::iter::once(u64::from(changed)).collect();
    let updates = hc_proto::apply_updates(&known, std::slice::from_ref(item));
    assert!(
        updates[0].description.is_some(),
        "decoded as a description change"
    );
}

#[tokio::test]
async fn programs_start_and_stop() {
    let (addr, appliance, profile) = start().await;
    let (mut ws, s_id) = raw_client(addr, &profile).await;
    let model = profile.model();
    let program = *model.programs().keys().nth(1).unwrap();
    let uid_of = |kind| {
        model
            .entities()
            .values()
            .find(|entity| entity.kind == kind)
            .unwrap()
            .uid
    };
    let active = uid_of(EntityKind::ActiveProgram);
    let selected = uid_of(EntityKind::SelectedProgram);

    let mut seen = Vec::new();
    let body = json!([{"program": program, "options": []}]);
    request(&mut ws, (s_id, 400), "POST", "/ro/selectedProgram", body).await;
    let response = response_to(&mut ws, 400, &mut seen).await;
    assert!(
        response.get("code").is_none(),
        "program start accepted: {response}"
    );
    let notify = notify_for(&mut ws, "/ro/values", active, &mut seen).await;
    assert_eq!(
        item_value(&notify, active),
        json!(program),
        "active program notified"
    );
    assert_eq!(
        item_value(&notify, selected),
        json!(program),
        "selected program notified"
    );

    request(
        &mut ws,
        (s_id, 401),
        "POST",
        "/ro/selectedProgram",
        json!([{"program": 1, "options": []}]),
    )
    .await;
    let response = response_to(&mut ws, 401, &mut seen).await;
    assert_eq!(
        response["code"],
        json!(400),
        "an undeclared program is refused"
    );

    request(
        &mut ws,
        (s_id, 402),
        "DELETE",
        "/ro/activeProgram",
        Value::Null,
    )
    .await;
    let response = response_to(&mut ws, 402, &mut seen).await;
    assert!(
        response.get("code").is_none(),
        "program stop accepted: {response}"
    );
    assert_eq!(
        appliance.lock().value(active),
        Some(&json!(0)),
        "no active program"
    );
    assert_eq!(
        appliance.lock().value(selected),
        Some(&json!(program)),
        "selection kept"
    );
}

#[tokio::test]
async fn unknown_paths_are_refused() {
    let (addr, _appliance, profile) = start().await;
    let config = psk_config(&profile, b"");
    let tls = tls_psk::tls_connect_stream("127.0.0.1", addr.port(), &config)
        .await
        .unwrap();
    let url = format!("ws://127.0.0.1:{}/elsewhere", addr.port());
    assert!(
        tokio_tungstenite::client_async(url, tls).await.is_err(),
        "only /homeconnect upgrades"
    );
}

#[test]
fn the_seed_makes_description_changes_reproducible() {
    let profile = Profile::from_zip_path(&fixture()).unwrap();
    let uid = find_setting(&profile, |entity| {
        entity.protocol_type() == Some(ProtocolType::Boolean)
    });
    let run = |seed| {
        let mut appliance = kessel_sim::Appliance::new(
            profile.model().clone(),
            seed,
            kessel_sim::AccessCase::Upper,
        );
        (0..20)
            .map(|_| appliance.maybe_redescribe(uid))
            .collect::<Vec<_>>()
    };
    assert_eq!(run(7), run(7), "the same seed gives the same changes");
    assert_ne!(run(7), run(8), "a different seed gives different changes");
    assert!(
        run(7).iter().flatten().all(|item| {
            item.get("access")
                .is_none_or(|word| word == "READ" || word == "READWRITE")
        }),
        "access flips between READ and READWRITE"
    );
}
