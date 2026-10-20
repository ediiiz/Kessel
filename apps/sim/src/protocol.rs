//! Frame shapes, copied from the recorded appliance session.
//!
//! Key order is `sID, msgID, resource, version, action, data` (or `code` in place of `data`),
//! because some firmware and parsers read positionally.

use serde_json::{Map, Value, json};

/// The service table the simulator advertises, as the recorded hob does: `ci` 2 makes the client
/// run `/ci/authentication` + `/ci/info`, and `ni` makes it ask for `/ni/info`.
pub const SERVICES: [(&str, i64); 4] = [("ro", 1), ("ei", 2), ("ci", 2), ("ni", 1)];

/// The `version` the simulator uses for a resource it originates: the service table's entry for
/// the two letters after the leading `/`, defaulting to 1.
#[must_use]
pub fn version_for(resource: &str) -> i64 {
    let key = resource.get(1..3).unwrap_or_default();
    SERVICES
        .iter()
        .find(|(service, _)| *service == key)
        .map_or(1, |(_, version)| *version)
}

/// A request from the client, loosely parsed.
#[derive(Debug, Clone)]
pub struct Request {
    /// The client's `msgID`, echoed verbatim in the RESPONSE.
    pub msg_id: Value,
    /// The resource path.
    pub resource: String,
    /// The client's `version`, echoed verbatim in the RESPONSE.
    pub version: Value,
    /// The verb.
    pub action: String,
    /// The `data` list; a single object is wrapped, anything else is empty.
    pub data: Vec<Value>,
}

impl Request {
    /// Parses one text frame. `None` when it is not a JSON object with a resource and an action.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        let object = value.as_object()?;
        let data = match object.get("data") {
            Some(Value::Array(items)) => items.clone(),
            Some(item @ Value::Object(_)) => vec![item.clone()],
            _ => Vec::new(),
        };
        Some(Self {
            msg_id: object.get("msgID").cloned().unwrap_or(Value::Null),
            resource: object.get("resource")?.as_str()?.to_owned(),
            version: object.get("version").cloned().unwrap_or_else(|| json!(1)),
            action: object.get("action")?.as_str()?.to_owned(),
            data,
        })
    }
}

fn envelope(
    s_id: u64,
    msg_id: Value,
    resource: &str,
    version: Value,
    action: &str,
) -> Map<String, Value> {
    let mut frame = Map::new();
    frame.insert("sID".to_owned(), json!(s_id));
    frame.insert("msgID".to_owned(), msg_id);
    frame.insert("resource".to_owned(), json!(resource));
    frame.insert("version".to_owned(), version);
    frame.insert("action".to_owned(), json!(action));
    frame
}

/// A successful RESPONSE to `request`; `data` is omitted when `None`.
#[must_use]
pub fn response(s_id: u64, request: &Request, data: Option<Vec<Value>>) -> Value {
    let mut frame = envelope(
        s_id,
        request.msg_id.clone(),
        &request.resource,
        request.version.clone(),
        "RESPONSE",
    );
    if let Some(data) = data {
        frame.insert("data".to_owned(), Value::Array(data));
    }
    Value::Object(frame)
}

/// An error RESPONSE: the same envelope with a top-level `code` and no `data`.
#[must_use]
pub fn error_response(s_id: u64, request: &Request, code: i64) -> Value {
    let mut frame = envelope(
        s_id,
        request.msg_id.clone(),
        &request.resource,
        request.version.clone(),
        "RESPONSE",
    );
    frame.insert("code".to_owned(), json!(code));
    Value::Object(frame)
}

/// A device-initiated message (`POST /ei/initialValues`, `NOTIFY …`).
#[must_use]
pub fn device_message(
    s_id: u64,
    msg_id: u64,
    resource: &str,
    action: &str,
    data: Vec<Value>,
) -> Value {
    let mut frame = envelope(
        s_id,
        json!(msg_id),
        resource,
        json!(version_for(resource)),
        action,
    );
    frame.insert("data".to_owned(), Value::Array(data));
    Value::Object(frame)
}

/// The `/ci/services` RESPONSE body.
#[must_use]
pub fn services() -> Vec<Value> {
    SERVICES
        .iter()
        .map(|(service, version)| json!({"service": service, "version": version}))
        .collect()
}

/// Static, synthetic facts about the simulated appliance. Nothing here comes from a real device.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// The appliance type from the profile, e.g. `Dishwasher`.
    pub device_type: String,
    /// The brand from the profile, e.g. `BOSCH`.
    pub brand: String,
}

impl DeviceInfo {
    /// The `/ci/info` body, in the recorded key order, with synthetic identifiers.
    #[must_use]
    pub fn ci_info(&self) -> Vec<Value> {
        vec![json!({
            "deviceID": "SIM-000000000000",
            "eNumber": "SIM000000/00",
            "brand": self.brand,
            "vib": "SIM000000",
            "mac": "00-00-00-00-00-00",
            "haVersion": "1.0",
            "swVersion": "0.0.0.0",
            "hwVersion": "0.0.0.0",
            "deviceType": self.device_type,
            "deviceInfo": "",
            "customerIndex": "00",
            "serialNumber": "000000000000000000",
            "fdString": "0000",
            "wifiInterfaces": 1
        })]
    }

    /// The `/iz/info` body. The advertised services do not include `iz`, but a client that asks
    /// anyway gets an answer.
    #[must_use]
    pub fn iz_info(&self) -> Vec<Value> {
        vec![json!({
            "deviceID": "SIM-000000000000",
            "eNumber": "SIM000000/00",
            "brand": self.brand,
            "vib": "SIM000000",
            "mac": "00-00-00-00-00-00",
            "haVersion": "1.0",
            "swVersion": "0.0.0.0",
            "hwVersion": "0.0.0.0",
            "deviceType": self.device_type,
            "serialNumber": "000000000000000000"
        })]
    }
}

/// The `/ni/info` body: one loopback "Wi-Fi" interface.
#[must_use]
pub fn ni_info() -> Vec<Value> {
    vec![json!({
        "interfaceID": 0,
        "type": "WiFi",
        "ssid": "kessel-sim",
        "rssi": -50,
        "primary": true,
        "status": "CONNECTED",
        "configured": true,
        "euiAddress": "00-00-00-00-00-00",
        "ipV4": {
            "ipAddress": "127.0.0.1",
            "prefixSize": 8,
            "gateway": "127.0.0.1",
            "dnsServer": "127.0.0.1"
        }
    })]
}

/// The `/ci/registeredDevices` body: a synthetic phone app that is offline, and the connected
/// client under the name it gave in its `/ei/initialValues` RESPONSE.
#[must_use]
pub fn registered_devices(client_name: &str, client_id: &str) -> Vec<Value> {
    vec![
        json!({
            "endDeviceID": 0,
            "deviceType": "Application",
            "deviceName": "SIM-PHONE",
            "deviceID": "0000000000000000",
            "connected": false,
            "protected": false
        }),
        json!({
            "endDeviceID": 1,
            "deviceType": "Application",
            "deviceName": client_name,
            "deviceID": client_id,
            "connected": true,
            "protected": false
        }),
    ]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "asserts may panic")]

    use super::*;

    #[test]
    fn response_keeps_the_recorded_key_order_and_echoes_the_request() {
        let request = Request::parse(
            r#"{"sID":7,"msgID":42,"resource":"/ci/services","version":1,"action":"GET"}"#,
        )
        .unwrap();
        let frame = response(7, &request, Some(services()));
        let text = serde_json::to_string(&frame).unwrap();
        assert!(
            text.starts_with(
                r#"{"sID":7,"msgID":42,"resource":"/ci/services","version":1,"action":"RESPONSE","data":[{"service":"ro","version":1}"#
            ),
            "unexpected frame: {text}"
        );
    }

    #[test]
    fn error_response_carries_a_top_level_code_and_no_data() {
        let request = Request::parse(
            r#"{"sID":7,"msgID":9,"resource":"/ro/values","version":1,"action":"POST","data":[{"uid":1,"value":2}]}"#,
        )
        .unwrap();
        let text = serde_json::to_string(&error_response(7, &request, 400)).unwrap();
        assert_eq!(
            text,
            r#"{"sID":7,"msgID":9,"resource":"/ro/values","version":1,"action":"RESPONSE","code":400}"#,
            "error shape"
        );
    }

    #[test]
    fn device_messages_take_the_service_version() {
        let frame = device_message(
            1,
            2,
            "/ei/initialValues",
            "POST",
            vec![json!({"edMsgID": 3})],
        );
        assert_eq!(frame.get("version"), Some(&json!(2)), "ei is version 2");
        let frame = device_message(1, 2, "/ro/values", "NOTIFY", Vec::new());
        assert_eq!(frame.get("version"), Some(&json!(1)), "ro is version 1");
    }
}
